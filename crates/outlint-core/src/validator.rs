//! Pure validation of a parsed Markdown outline against a normalized schema.
//!
//! Validation is deliberately separate from parsing and IO: callers can load
//! and parse fixture text once, then pass only values to [`validate`].

use crate::loader::{
    json_schema_reference_budget_message, json_schema_reference_count, parse_frontmatter_scalar,
    preloaded_json_schema_registry, NoExternalRetrieve, MAX_JSON_SCHEMA_REFERENCES,
};
use crate::matcher::{compile_anchored_pattern, compile_glob_pattern};
use crate::{
    ByteOffset, Cardinality, Constraint, ConstraintIndex, ConstraintPath, Document,
    DocumentFrontmatter, FrontmatterAnchor, FrontmatterLocation, FrontmatterPolicy, FrontmatterRef,
    FrontmatterScalar, FrontmatterSchema, HeaderLevel, Heading, HeadingLocation, Matcher,
    OutlineProvenance, Proposition, RefAnchor, RuleIndex, RuleOutcome, RuleRef, Schema, SchemaNode,
    ScopePath, Section, SectionRule, TextRange, UpperBound,
};
use std::{error::Error, fmt};

/// A stable identifier from the diagnostic vocabulary in specification §6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DiagnosticId {
    /// A heading is more than one level below its nearest parent.
    SkippedLevel,
    /// A present heading is denied by its first matching rule or title matcher.
    NotAllowed,
    /// A heading has no matching rule in a strict scope.
    UnexpectedSection,
    /// No heading matched a rule whose minimum is nonzero.
    MissingSection,
    /// Some headings matched a rule, but fewer than its minimum.
    TooFewSections,
    /// More headings matched a rule than its finite maximum, or the document
    /// holds more than one `h1` under a sugar schema.
    TooManySections,
    /// The schema declares a title but the document has none.
    MissingTitle,
    /// A required frontmatter block is absent.
    MissingFrontmatter,
    /// A present frontmatter block is forbidden by the schema.
    ForbiddenFrontmatter,
    /// A frontmatter block is not a valid YAML mapping.
    InvalidFrontmatter,
    /// A frontmatter value fails its JSON Schema.
    FrontmatterSchema,
    /// An `one_of` constraint does not have exactly one satisfied ref.
    OneOf,
    /// An `any_of` constraint has no satisfied ref.
    AnyOf,
    /// An `at_most_one` constraint has more than one satisfied ref.
    AtMostOne,
    /// An `all_or_none` constraint has some but not all refs satisfied.
    AllOrNone,
    /// A `requires` condition is satisfied without every consequence.
    Requires,
    /// A `conflicts` condition and at least one exclusion are both satisfied.
    Conflicts,
    /// Concrete occurrences violate an `ordered` constraint.
    Ordered,
}

impl fmt::Display for DiagnosticId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl DiagnosticId {
    /// Returns the public, suppression-compatible spelling of this id.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SkippedLevel => "skipped-level",
            Self::NotAllowed => "not-allowed",
            Self::UnexpectedSection => "unexpected-section",
            Self::MissingSection => "missing-section",
            Self::TooFewSections => "too-few-sections",
            Self::TooManySections => "too-many-sections",
            Self::MissingTitle => "missing-title",
            Self::MissingFrontmatter => "missing-frontmatter",
            Self::ForbiddenFrontmatter => "forbidden-frontmatter",
            Self::InvalidFrontmatter => "invalid-frontmatter",
            Self::FrontmatterSchema => "frontmatter-schema",
            Self::OneOf => "one_of",
            Self::AnyOf => "any_of",
            Self::AtMostOne => "at_most_one",
            Self::AllOrNone => "all_or_none",
            Self::Requires => "requires",
            Self::Conflicts => "conflicts",
            Self::Ordered => "ordered",
        }
    }
}

/// A path of case-preserving visible heading texts.
///
/// A header path is always the complete document-tree ancestor chain, from the
/// document's topmost enclosing heading down to the header itself. It does not
/// begin at the root scope: an enclosing `h1`, which is the title when the
/// document has one, is part of the path. Two same-named sections under
/// different ancestors therefore have different paths.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct HeaderPath(pub Vec<String>);

impl HeaderPath {
    /// Returns the heading texts in ancestor-to-descendant order.
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }
}

impl fmt::Display for HeaderPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, heading) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str(" > ")?;
            }
            formatter.write_str(heading)?;
        }
        Ok(())
    }
}

/// A source anchor in the Markdown document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiagnosticLocation {
    /// The source line to highlight.
    pub range: TextRange,
    /// One-based line number.
    pub line: u64,
    /// One-based byte column.
    pub column: u64,
}

/// A concrete header relevant to a constraint violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvolvedHeader {
    /// The concrete header's document path.
    pub path: HeaderPath,
    /// The concrete header's source anchor.
    pub location: DiagnosticLocation,
}

/// A normalized constraint reference retained for diagnostic presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticReference {
    /// A rule reference paired with its resolved target matcher.
    Rule {
        /// The normalized relative or schema-root-anchored reference.
        reference: RuleRef,
        /// Matcher of the rule targeted by `reference`.
        matcher: Matcher,
    },
    /// A document-level frontmatter proposition.
    Frontmatter(FrontmatterRef),
}

/// What a diagnostic is about.
///
/// The four cases carry text of different provenance, and conflating them in
/// one [`HeaderPath`] silently mixes document text with schema text. Only
/// [`Self::Header`] names text that occurs in the document; the matcher label
/// in [`Self::MissingHeader`] comes from the schema and may occur nowhere in
/// the document; [`Self::Document`] and [`Self::Frontmatter`] name no header
/// at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticTarget {
    /// A header that exists in the document, named by its document path.
    Header(HeaderPath),
    /// A section the schema requires but the document does not contain.
    MissingHeader {
        /// Document path of the header whose scope should have contained it.
        ///
        /// Empty when no header encloses the missing section: either it belongs
        /// to the root scope, or it is the title, which sits *above* the root
        /// scope rather than in it.
        parent: HeaderPath,
        /// Label of the unsatisfied schema matcher: exact text, a glob, a
        /// slash-delimited regex, or `*`. This is schema text, not a heading.
        matcher: String,
    },
    /// The document as a whole, when no single header can name the violation.
    ///
    /// This is the root scope, which is attached to the schema root rather than
    /// to any rule: a constraint on it has no parent header to point at, and
    /// the enclosing `h1` is not one, since a document need not have an `h1`.
    Document,
    /// A frontmatter block, or a value inside one. Has no header path.
    Frontmatter {
        /// The offending block, absent only when the document has none.
        block: Option<FrontmatterBlock>,
    },
}

/// The frontmatter block a diagnostic is about, and the value within it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontmatterBlock {
    /// One-based inclusive line range of the complete frontmatter block.
    pub line_range: FrontmatterLineRange,
    /// JSON Pointer of a value rejected by JSON Schema, when applicable.
    pub json_pointer: Option<String>,
}

/// One validation violation, with both document and schema-side anchors.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Diagnostic {
    /// Stable diagnostic category.
    pub id: DiagnosticId,
    /// What the diagnostic is about: a header, a missing one, or frontmatter.
    pub target: DiagnosticTarget,
    /// Primary Markdown source anchor.
    pub location: DiagnosticLocation,
    /// Structural schema node responsible for the diagnostic, when one exists.
    pub schema_node: Option<SchemaNode>,
    /// Concrete headers participating in a constraint violation.
    pub involved_headers: Vec<InvolvedHeader>,
    /// Normalized references participating in a constraint violation.
    pub references: Vec<DiagnosticReference>,
    /// Human-readable context; callers should key behavior on [`Self::id`].
    pub message: String,
}

/// One-based inclusive line range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrontmatterLineRange {
    /// First line covered by the range.
    pub start_line: u64,
    /// Last line covered by the range.
    pub end_line: u64,
}

/// Failure to prepare a reusable validator from a semantic schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareValidationError {
    /// Human-readable compilation failure.
    pub message: String,
}

impl fmt::Display for PrepareValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for PrepareValidationError {}

/// A schema compiled once for validating any number of documents.
pub struct PreparedValidator {
    schema: Schema,
    plan: ValidationPlan,
}

impl PreparedValidator {
    /// Compiles matchers and the immutable JSON Schema resource registry.
    ///
    /// Callers should pass a [`Schema`] produced by the loader. Preparation is
    /// not a substitute for the loader's semantic checks when a schema has
    /// been assembled manually from its public fields.
    ///
    /// # Errors
    ///
    /// Returns an error if a matcher or frontmatter JSON Schema cannot be compiled.
    /// A schema returned by the loader has already passed equivalent checks,
    /// but preparation retains a defensive failure path rather than assuming
    /// every caller obtained the value from that boundary.
    pub fn new(schema: &Schema) -> Result<Self, PrepareValidationError> {
        Ok(Self {
            schema: schema.clone(),
            plan: ValidationPlan::new(schema)?,
        })
    }

    /// Validates one parsed document without recompiling schema state.
    ///
    /// Frontmatter validation is included, and `fm.` propositions in
    /// constraints evaluate against the document's frontmatter (§4.6).
    ///
    /// Diagnostic order is deterministic for a given schema and document but
    /// follows the validation walk and is not a contract of this crate: a
    /// refactor may reorder it between releases. Callers that promise an
    /// output order must sort on diagnostic content, as the CLI does with a
    /// documented total key.
    pub fn validate(&self, document: &Document) -> Vec<Diagnostic> {
        Validator::new(&self.schema, document).run(&self.plan)
    }
}

/// Prepares and validates one document.
///
/// Use [`PreparedValidator`] directly when validating multiple documents.
/// Diagnostic order is deterministic but not a contract; see
/// [`PreparedValidator::validate`].
///
/// # Example
///
/// ```
/// use outlint_core::{load_schema, parse_markdown, validate, MarkdownOptions};
///
/// let loaded = load_schema("version: 1\ntitle: '*'\nsections: []\n")?;
/// let document = parse_markdown("# Guide\n", MarkdownOptions::default());
/// let diagnostics = validate(&loaded.schema, &document)
///     .expect("loaded schema matchers compile");
///
/// assert!(diagnostics.is_empty());
/// # Ok::<(), outlint_core::InvalidSchema>(())
/// ```
pub fn validate(
    schema: &Schema,
    document: &Document,
) -> Result<Vec<Diagnostic>, PrepareValidationError> {
    PreparedValidator::new(schema).map(|prepared| prepared.validate(document))
}

struct ValidationPlan {
    outline: Vec<PreparedRule>,
    frontmatter: Option<jsonschema::Validator>,
}

impl ValidationPlan {
    fn new(schema: &Schema) -> Result<Self, PrepareValidationError> {
        Ok(Self {
            outline: prepare_rules(&schema.outline, schema.options.match_case)?,
            frontmatter: frontmatter_schema(&schema.frontmatter)
                .map(compile_frontmatter_schema)
                .transpose()?,
        })
    }
}

fn frontmatter_schema(policy: &FrontmatterPolicy) -> Option<&FrontmatterSchema> {
    match policy {
        FrontmatterPolicy::Optional { schema }
        | FrontmatterPolicy::Required { schema }
        | FrontmatterPolicy::Forbidden { schema } => schema.as_ref(),
    }
}

fn compile_frontmatter_schema(
    schema: &FrontmatterSchema,
) -> Result<jsonschema::Validator, PrepareValidationError> {
    // This is the second place a frontmatter schema graph is compiled, and compiling a
    // reference chain costs a stack frame per link, so the budget is charged
    // here too rather than trusted to have been charged upstream. Today the
    // loader is the only constructor of a `FrontmatterSchema` and refuses the
    // same graphs, but a compile that overruns the stack aborts the process
    // instead of returning, which is not a failure a later caller can recover
    // from — so the check belongs at the call, not at the one path into it.
    let references = std::iter::once(&schema.root)
        .chain(schema.resources.values())
        .fold(0usize, |total, document| {
            total.saturating_add(json_schema_reference_count(document))
        });
    if references > MAX_JSON_SCHEMA_REFERENCES {
        return Err(PrepareValidationError {
            message: json_schema_reference_budget_message(),
        });
    }
    let mut registry = preloaded_json_schema_registry()
        .add(schema.root_uri.as_str(), &schema.root)
        .map_err(|error| PrepareValidationError {
            message: format!("cannot register frontmatter JSON Schema root: {error}"),
        })?;
    for (uri, resource) in &schema.resources {
        registry =
            registry
                .add(uri.as_str(), resource)
                .map_err(|error| PrepareValidationError {
                    message: format!("cannot register frontmatter JSON Schema resource: {error}"),
                })?;
    }
    let registry = registry.prepare().map_err(|error| PrepareValidationError {
        message: format!("cannot prepare frontmatter JSON Schema registry: {error}"),
    })?;
    jsonschema::draft202012::options()
        .with_registry(&registry)
        .with_base_uri(schema.root_uri.clone())
        .with_retriever(NoExternalRetrieve)
        .build(&schema.root)
        .map_err(|error| PrepareValidationError {
            message: format!("cannot compile frontmatter JSON Schema: {error}"),
        })
}

#[derive(Debug)]
struct PreparedRule {
    matcher: PreparedMatcher,
    sections: Vec<PreparedRule>,
}

fn prepare_rules(
    rules: &[SectionRule],
    match_case: bool,
) -> Result<Vec<PreparedRule>, PrepareValidationError> {
    rules
        .iter()
        .map(|rule| {
            Ok(PreparedRule {
                matcher: PreparedMatcher::new(&rule.matcher, match_case)?,
                sections: prepare_rules(&rule.sections, match_case)?,
            })
        })
        .collect()
}

#[derive(Debug)]
enum PreparedMatcher {
    Exact { text: String, match_case: bool },
    Pattern(regex::Regex),
    Any,
}

impl PreparedMatcher {
    fn new(matcher: &Matcher, match_case: bool) -> Result<Self, PrepareValidationError> {
        Ok(match matcher {
            Matcher::Exact(exact) => Self::Exact {
                text: exact.0.clone(),
                match_case,
            },
            Matcher::Glob(glob) => Self::Pattern(
                compile_glob_pattern(&glob.0, match_case).map_err(prepare_matcher_error)?,
            ),
            Matcher::Regex(pattern) => {
                Self::Pattern(compile_pattern(&pattern.0, match_case, false)?)
            }
            Matcher::Any => Self::Any,
        })
    }

    fn matches(&self, text: &str) -> bool {
        match self {
            Self::Exact {
                text: expected,
                match_case: true,
            } => expected == text,
            Self::Exact {
                text: expected,
                match_case: false,
            } => crate::case_fold::simple_eq(expected, text),
            Self::Pattern(regex) => regex.is_match(text),
            Self::Any => true,
        }
    }
}

fn compile_pattern(
    body: &str,
    match_case: bool,
    dot_matches_new_line: bool,
) -> Result<regex::Regex, PrepareValidationError> {
    compile_anchored_pattern(body, match_case, dot_matches_new_line).map_err(prepare_matcher_error)
}

fn prepare_matcher_error(error: regex::Error) -> PrepareValidationError {
    PrepareValidationError {
        message: format!("cannot compile matcher: {error}"),
    }
}

struct Validator<'a> {
    schema: &'a Schema,
    document: &'a Document,
    diagnostics: Vec<Diagnostic>,
}

struct BindScopeInput<'a, 'd> {
    sections: &'a [PathedSection<'d>],
    rules: &'a [SectionRule],
    prepared_rules: &'a [PreparedRule],
    strict: bool,
    ordered: bool,
    schema_scope: &'a ScopePath,
    parent: Option<&'d Heading>,
    parent_path: &'a HeaderPath,
}

struct OrderCheck<'a, 'd> {
    rules: &'a [SectionRule],
    occurrences: &'a [BoundSection<'d>],
    schema_scope: &'a ScopePath,
    parent: Option<&'d Heading>,
    parent_path: &'a HeaderPath,
}

struct CardinalityCheck<'a, 'd> {
    cardinality: Cardinality,
    count: usize,
    rule: &'a SectionRule,
    rule_index: usize,
    occurrences: &'a [BoundSection<'d>],
    schema_scope: &'a ScopePath,
    parent: Option<&'d Heading>,
    parent_path: &'a HeaderPath,
}

impl<'a> Validator<'a> {
    fn new(schema: &'a Schema, document: &'a Document) -> Self {
        Self {
            schema,
            document,
            diagnostics: Vec::new(),
        }
    }

    fn run(mut self, plan: &ValidationPlan) -> Vec<Diagnostic> {
        self.validate_frontmatter(plan.frontmatter.as_ref());
        let document = self.document;
        let top = top_level_sections(&document.sections);
        let has_h1 = top
            .iter()
            .any(|pathed| pathed.section.heading.level == HeaderLevel::H1);
        // The document root is a virtual level-0 header enclosing the whole
        // document: outline rules describe its `h1` children the way nested
        // rules describe any header's children. When a sugar schema meets a
        // document with no `h1`, the root stands in at level 1 — the
        // `sections` scope then binds the document's own top-level `h2`s
        // (alongside the missing-title the absent `h1` earns) — and
        // `title: null` declares that shape outright, whatever the document
        // contains.
        let root_level = match self.schema.outline_provenance {
            OutlineProvenance::Outline => 0,
            OutlineProvenance::NoTitle => 1,
            OutlineProvenance::Title | OutlineProvenance::BareSections => u8::from(!has_h1),
        };
        if !self.schema.options.allow_skipped_levels {
            // Structural and schema-independent: the walk covers the whole
            // document, including subtrees the root never admits into any
            // scope, and reporting a skipped level does not enroll a header
            // in any rule. A top-level header deeper than the root's child
            // level skips against the virtual root itself — the shape the
            // retired `detached-section` diagnostic used to name.
            self.validate_skipped_levels(&document.sections, root_level, &HeaderPath::default());
        }
        let frontmatter = match &document.frontmatter {
            DocumentFrontmatter::Mapping { value, .. } => Some(value),
            DocumentFrontmatter::Absent | DocumentFrontmatter::Invalid { .. } => None,
        };
        match self.schema.outline_provenance {
            OutlineProvenance::Outline => self.validate_outline_root(&top, plan, frontmatter),
            OutlineProvenance::Title
            | OutlineProvenance::BareSections
            | OutlineProvenance::NoTitle => {
                self.validate_sugar_root(&top, has_h1, plan, frontmatter)
            }
        }
        self.diagnostics
    }

    /// Binds the general form's outline scope: `h1` rules on the virtual root.
    ///
    /// Ordinary scope semantics apply — the outline scope is open unless a
    /// rule closes its own, an unmatched `h1` is nobody's business, and
    /// top-level constraints attach here, targeting the document since the
    /// virtual root has no header to name.
    fn validate_outline_root(
        &mut self,
        top: &[PathedSection<'a>],
        plan: &ValidationPlan,
        frontmatter: Option<&'a serde_json::Map<String, serde_json::Value>>,
    ) {
        let schema = self.schema;
        let admitted = admitted_at_root(top, HeaderLevel::H1, schema.options.allow_skipped_levels);
        let root_scope = ScopePath(Vec::new());
        let root_path = HeaderPath::default();
        let root = self.bind_scope(BindScopeInput {
            sections: &admitted,
            rules: &schema.outline,
            prepared_rules: &plan.outline,
            strict: false,
            ordered: schema.options.ordered_sections,
            schema_scope: &root_scope,
            parent: None,
            parent_path: &root_path,
        });
        self.validate_constraints(
            EvalCtx {
                current: &root,
                current_rules: &schema.outline,
                root: &root,
                root_rules: &schema.outline,
                frontmatter,
                match_case: schema.options.match_case,
            },
            &schema.constraints,
            &root_scope,
            None,
            &root_path,
        );
    }

    /// Binds a sugar schema's synthesized `h1` rule with its legacy voice.
    ///
    /// The sugar desugars to one required `h1` rule, but its diagnostics
    /// predate the outline form and keep their spellings: the rule's absence
    /// is `missing-title` anchored at [`SchemaNode::Title`], a mismatched
    /// `h1` is `not-allowed` there rather than an ignored header, a surplus
    /// `h1` is one `too-many-sections` on the second, and a lone `h1`'s
    /// `sections` scope keeps reporting as the legacy root — cardinality
    /// misses name no parent header and constraints target the document.
    /// Each `h1` still binds its own child scope, so two same-named sections
    /// under different `h1`s are budgeted per parent exactly as in every
    /// nested scope — and when more than one `h1` binds, each instance's
    /// diagnostics carry the owning `h1`'s path so the failing subtrees stay
    /// apart.
    fn validate_sugar_root(
        &mut self,
        top: &[PathedSection<'a>],
        has_h1: bool,
        plan: &ValidationPlan,
        frontmatter: Option<&'a serde_json::Map<String, serde_json::Value>>,
    ) {
        let schema = self.schema;
        let provenance = schema.outline_provenance;
        let (Some(rule), Some(prepared)) = (schema.outline.first(), plan.outline.first()) else {
            return;
        };

        if provenance == OutlineProvenance::NoTitle || !has_h1 {
            // The headless scope: the virtual root stands in at level 1 and
            // the `sections` rules bind the document's top-level `h2`s.
            // One scope instance, so the legacy document voice is unambiguous.
            // Bare `sections:` implies `title: "*"`, so a headless document
            // is missing its title there exactly as under a spelled title.
            if provenance != OutlineProvenance::NoTitle {
                self.emit(
                    Diagnostic {
                        id: DiagnosticId::MissingTitle,
                        // The title sits above the root scope, so it has no
                        // parent.
                        target: DiagnosticTarget::MissingHeader {
                            parent: HeaderPath::default(),
                            matcher: matcher_label(&rule.matcher),
                        },
                        location: root_location(),
                        schema_node: Some(SchemaNode::Title),
                        involved_headers: Vec::new(),
                        references: Vec::new(),
                        message: "the document has no required title".into(),
                    },
                    None,
                    false,
                );
            }
            if provenance == OutlineProvenance::NoTitle {
                // `title: null` desugars to a denied `h1` rule: a present
                // `h1` is rejected wholesale, its subtree validated no
                // further, like any header a deny rule matches.
                for pathed in top {
                    if pathed.section.heading.level == HeaderLevel::H1 {
                        self.emit_present(
                            DiagnosticId::NotAllowed,
                            pathed.path.clone(),
                            &pathed.section.heading,
                            Some(SchemaNode::Title),
                            "the schema declares a document with no title",
                        );
                    }
                }
            }
            let admitted =
                admitted_at_root(top, HeaderLevel::H2, schema.options.allow_skipped_levels);
            self.bind_sugar_sections(&admitted, rule, prepared, frontmatter, None);
            return;
        }

        // The titled scope: every `h1` occupies the synthesized rule, a
        // mismatch reporting as a wrong title rather than dropping the header
        // — its children are still the document's real structure and are
        // still validated. Deeper top-level headers join only when skipped
        // levels are allowed, and never as the title: the title rule is an
        // `h1` rule, so only `h1`s occupy it or count against its one-title
        // bound. An admitted deeper header instead binds into the `sections`
        // scope — the title rule's child scope — like any skipped child of a
        // bound header.
        let admitted = admitted_at_root(top, HeaderLevel::H1, schema.options.allow_skipped_levels);
        let mut occurrences = Vec::new();
        let mut admitted_strays = Vec::new();
        for pathed in &admitted {
            if pathed.section.heading.level == HeaderLevel::H1 {
                // Only a spelled title matcher can miss: the bare-sections
                // any-text matcher accepts every `h1`.
                if !prepared.matcher.matches(&pathed.section.heading.text) {
                    self.emit_present(
                        DiagnosticId::NotAllowed,
                        pathed.path.clone(),
                        &pathed.section.heading,
                        Some(SchemaNode::Title),
                        "the title does not match the schema title matcher",
                    );
                }
                occurrences.push(pathed);
            } else {
                // A top-level header deeper than `h1` can only precede the
                // document's first `h1` — any later one nests under an `h1`
                // in the parse tree — so joining the first instance below
                // keeps document order.
                admitted_strays.push(PathedSection {
                    section: pathed.section,
                    path: pathed.path.clone(),
                });
            }
        }
        // One diagnostic per document, anchored on the second occurrence in
        // document order: that is where the bound breaks, and further surplus
        // says nothing new. Every `h1` is the title — spelled or implied —
        // so the title node takes the blame either way.
        if let Some(excess) = occurrences.get(1) {
            self.emit(
                Diagnostic {
                    id: DiagnosticId::TooManySections,
                    target: DiagnosticTarget::Header(excess.path.clone()),
                    location: heading_location(&excess.section.heading.location),
                    schema_node: Some(SchemaNode::Title),
                    involved_headers: Vec::new(),
                    references: Vec::new(),
                    message: "the document has more than one title".to_owned(),
                },
                Some(&excess.section.heading),
                true,
            );
        }
        // The instance voice depends on how many `h1`s bound the rule. A
        // single-`h1` sugar document reads as "the document" — the legacy
        // root voice, with cardinality misses naming no parent and
        // constraints targeting the document — and that voice is a corpus
        // compatibility gate. With more than one `h1` the same voice would
        // collapse two failing subtrees into byte-identical diagnostics, so
        // each occurrence's diagnostics then carry the owning `h1`: the full
        // path saying which subtree failed.
        let attribute = occurrences.len() > 1;
        for (index, occurrence) in occurrences.iter().enumerate() {
            let mut children = child_sections(occurrence.section, &occurrence.path);
            if index == 0 && !admitted_strays.is_empty() {
                let mut merged = std::mem::take(&mut admitted_strays);
                merged.extend(children);
                children = merged;
            }
            let owner = attribute.then_some((&occurrence.section.heading, &occurrence.path));
            self.bind_sugar_sections(&children, rule, prepared, frontmatter, owner);
        }
    }

    /// Binds one instance of a sugar schema's `sections` scope.
    ///
    /// With no `owner`, the scope reports as the legacy root: no parent
    /// header for cardinality misses, the document as constraint target, and
    /// the empty schema scope — which is the public address of the `sections`
    /// list. With an `owner` — one `h1` of several, where the legacy voice
    /// would repeat itself verbatim per subtree — cardinality misses name the
    /// owning `h1` as their parent, and constraints target and anchor on it.
    /// The schema scope stays the empty path either way: which instance bound
    /// the rules does not move where the rules live. Section paths always
    /// carry their real ancestor chain, enclosing `h1` included.
    fn bind_sugar_sections(
        &mut self,
        sections: &[PathedSection<'a>],
        rule: &'a SectionRule,
        prepared: &PreparedRule,
        frontmatter: Option<&'a serde_json::Map<String, serde_json::Value>>,
        owner: Option<(&'a Heading, &HeaderPath)>,
    ) {
        let scope = ScopePath(Vec::new());
        let (parent, path) = match owner {
            Some((heading, path)) => (Some(heading), path.clone()),
            None => (None, HeaderPath::default()),
        };
        let bound = self.bind_scope(BindScopeInput {
            sections,
            rules: &rule.sections,
            prepared_rules: &prepared.sections,
            strict: rule.strict,
            ordered: rule.ordered,
            schema_scope: &scope,
            parent,
            parent_path: &path,
        });
        self.validate_constraints(
            EvalCtx {
                current: &bound,
                current_rules: &rule.sections,
                // `$.` refs in a sugar schema resolve against the `sections`
                // scope, as they always have — here, this instance of it.
                root: &bound,
                root_rules: &rule.sections,
                frontmatter,
                match_case: self.schema.options.match_case,
            },
            &rule.constraints,
            &scope,
            parent,
            &path,
        );
    }

    fn validate_frontmatter(&mut self, validator: Option<&jsonschema::Validator>) {
        let required = matches!(self.schema.frontmatter, FrontmatterPolicy::Required { .. });
        let forbidden = matches!(self.schema.frontmatter, FrontmatterPolicy::Forbidden { .. });
        match &self.document.frontmatter {
            DocumentFrontmatter::Absent => {
                if required {
                    self.emit_frontmatter(
                        DiagnosticId::MissingFrontmatter,
                        None,
                        "the document is missing required frontmatter".into(),
                        None,
                    );
                }
            }
            DocumentFrontmatter::Invalid { location, message } => {
                if forbidden {
                    self.emit_frontmatter(
                        DiagnosticId::ForbiddenFrontmatter,
                        Some(*location),
                        "frontmatter is forbidden by the schema".into(),
                        None,
                    );
                }
                self.emit_frontmatter(
                    DiagnosticId::InvalidFrontmatter,
                    Some(*location),
                    message.clone(),
                    None,
                );
            }
            DocumentFrontmatter::Mapping {
                value,
                location,
                anchors,
            } => {
                if forbidden {
                    self.emit_frontmatter(
                        DiagnosticId::ForbiddenFrontmatter,
                        Some(*location),
                        "frontmatter is forbidden by the schema".into(),
                        None,
                    );
                }
                let Some(validator) = validator else {
                    return;
                };
                // jsonschema's serde_json backend accepts `&Value`; keep the
                // public document model narrower and wrap it only at this boundary.
                let instance = serde_json::Value::Object(value.clone());
                let mut errors = validator
                    .iter_errors(&instance)
                    .map(|error| (error.instance_path().as_str().to_owned(), error.to_string()))
                    .collect::<Vec<_>>();
                errors.sort();
                for (pointer, message) in errors {
                    // The root pointer names the mapping, whose extent is the
                    // block; only a pointer into it can name a narrower anchor.
                    let anchor = anchors.get(&pointer);
                    self.emit_frontmatter_at(
                        DiagnosticId::FrontmatterSchema,
                        Some(*location),
                        anchor,
                        message,
                        Some(pointer),
                    );
                }
            }
        }
    }

    /// Emits a diagnostic about a frontmatter block as a whole.
    fn emit_frontmatter(
        &mut self,
        id: DiagnosticId,
        location: Option<FrontmatterLocation>,
        message: String,
        json_pointer: Option<String>,
    ) {
        self.emit_frontmatter_at(id, location, None, message, json_pointer);
    }

    /// Emits a frontmatter diagnostic anchored at `anchor`, when one is known.
    ///
    /// The range stays the block's: the diagnostic concerns the block, and only
    /// the point a reader is sent to narrows to the offending entry.
    fn emit_frontmatter_at(
        &mut self,
        id: DiagnosticId,
        location: Option<FrontmatterLocation>,
        anchor: Option<FrontmatterAnchor>,
        message: String,
        json_pointer: Option<String>,
    ) {
        let diagnostic_location =
            location.map_or_else(root_location, |location| DiagnosticLocation {
                range: location.range,
                line: anchor.map_or(location.start_line, |anchor| anchor.line),
                column: anchor.map_or(1, |anchor| anchor.column),
            });
        let block = location.map(|location| FrontmatterBlock {
            line_range: FrontmatterLineRange {
                start_line: location.start_line,
                end_line: location.end_line,
            },
            json_pointer,
        });
        let schema_node = if id == DiagnosticId::FrontmatterSchema {
            Some(SchemaNode::FrontmatterSchemaDocument)
        } else {
            Some(SchemaNode::Frontmatter)
        };
        self.emit(
            Diagnostic {
                id,
                target: DiagnosticTarget::Frontmatter { block },
                location: diagnostic_location,
                schema_node,
                involved_headers: Vec::new(),
                references: Vec::new(),
                message,
            },
            None,
            false,
        );
    }

    /// Reports every heading more than one level below its parent.
    ///
    /// `parent_level` is the enclosing header's level, or the virtual document
    /// root's stand-in level for the top of the forest: 0 in general, 1 when a
    /// sugar or `title: null` schema binds a headless document's `h2`s
    /// directly. A top-level header deeper than `parent_level + 1` therefore
    /// skips against the document root itself — the shape the retired
    /// `detached-section` diagnostic used to name — and, exactly like that
    /// predecessor, it takes part in no rule unless `allow_skipped_levels`
    /// admits it into the root's scope.
    fn validate_skipped_levels(
        &mut self,
        sections: &[Section],
        parent_level: u8,
        parent_path: &HeaderPath,
    ) {
        for section in sections {
            let path = appended_path(parent_path, &section.heading.diagnostic_text);
            if section.heading.level as u8 > parent_level + 1 {
                self.emit(
                    Diagnostic {
                        id: DiagnosticId::SkippedLevel,
                        target: DiagnosticTarget::Header(path.clone()),
                        location: heading_location(&section.heading.location),
                        schema_node: None,
                        involved_headers: Vec::new(),
                        references: Vec::new(),
                        message: "the heading skips a level below its parent".into(),
                    },
                    Some(&section.heading),
                    true,
                );
            }
            self.validate_skipped_levels(&section.children, section.heading.level as u8, &path);
        }
    }

    fn bind_scope<'d>(&mut self, input: BindScopeInput<'_, 'd>) -> BoundScope<'d> {
        let BindScopeInput {
            sections,
            rules,
            prepared_rules,
            strict,
            ordered,
            schema_scope,
            parent,
            parent_path,
        } = input;
        let mut counts = vec![0_usize; rules.len()];
        let mut occurrences = Vec::new();
        for pathed in sections {
            let section = pathed.section;
            // Already the section's complete ancestor chain: a scope is not
            // necessarily rooted at `parent_path` (the root scope is flat).
            let path = pathed.path.clone();
            let matched = rules
                .iter()
                .zip(prepared_rules)
                .enumerate()
                .find(|(_, (_, prepared))| prepared.matcher.matches(&section.heading.text));
            let Some((rule_index, (rule, prepared_rule))) = matched else {
                if strict {
                    let schema_node = schema_scope.0.split_last().map(|(index, parent_scope)| {
                        SchemaNode::Rule(crate::RulePath {
                            scope: ScopePath(parent_scope.to_vec()),
                            index: *index,
                        })
                    });
                    self.emit_present(
                        DiagnosticId::UnexpectedSection,
                        path,
                        &section.heading,
                        schema_node,
                        "the section is not permitted in this closed scope",
                    );
                }
                continue;
            };
            let node = SchemaNode::Rule(rule_path(schema_scope, rule_index));
            if matches!(rule.outcome, RuleOutcome::Deny) {
                self.emit_present(
                    DiagnosticId::NotAllowed,
                    path,
                    &section.heading,
                    Some(node),
                    "the first matching rule denies this section",
                );
                continue;
            }

            if let Some(count) = counts.get_mut(rule_index) {
                *count += 1;
            }
            let child_refs = child_sections(section, &path);
            let mut child_scope_path = schema_scope.clone();
            child_scope_path.0.push(RuleIndex(rule_index));
            let child = self.bind_scope(BindScopeInput {
                sections: &child_refs,
                rules: &rule.sections,
                prepared_rules: &prepared_rule.sections,
                strict: rule.strict,
                ordered: rule.ordered,
                schema_scope: &child_scope_path,
                parent: Some(&section.heading),
                parent_path: &path,
            });
            occurrences.push(BoundSection {
                rule_index,
                section,
                path,
                child,
            });
        }

        for (rule_index, rule) in rules.iter().enumerate() {
            let RuleOutcome::Allow(cardinality) = rule.outcome else {
                continue;
            };
            let count = counts.get(rule_index).copied().unwrap_or_default();
            self.validate_cardinality(CardinalityCheck {
                cardinality,
                count,
                rule,
                rule_index,
                occurrences: &occurrences,
                schema_scope,
                parent,
                parent_path,
            });
        }
        if ordered {
            self.validate_order(OrderCheck {
                rules,
                occurrences: &occurrences,
                schema_scope,
                parent,
                parent_path,
            });
        }
        BoundScope { occurrences }
    }

    /// Checks an ordered scope: every header an earlier accepting rule
    /// matched must precede every header a later one matched (§3.7).
    ///
    /// The check is §5.1's `last(A) < first(B)` over adjacent pairs of the
    /// scope's accepting rules that matched anything, in list order. Denied
    /// rules and unmatched headers match nothing that counts, so they float
    /// freely. Each violated pair is one `ordered` diagnostic, so that a
    /// misplaced section is named by the neighbours it broke rather than by
    /// the whole scope at once.
    fn validate_order(&mut self, check: OrderCheck<'_, '_>) {
        let OrderCheck {
            rules,
            occurrences,
            schema_scope,
            parent,
            parent_path,
        } = check;
        let present = rules
            .iter()
            .enumerate()
            .filter(|(_, rule)| matches!(rule.outcome, RuleOutcome::Allow(_)))
            .map(|(rule_index, rule)| {
                let matched = occurrences
                    .iter()
                    .filter(|occurrence| occurrence.rule_index == rule_index)
                    .collect::<Vec<_>>();
                (rule, matched)
            })
            .filter(|(_, matched)| !matched.is_empty())
            .collect::<Vec<_>>();
        let schema_node = schema_scope.0.split_last().map_or_else(
            || {
                (self.schema.outline_provenance != OutlineProvenance::Outline)
                    .then_some(SchemaNode::Title)
            },
            |(index, parent_scope)| {
                Some(SchemaNode::Rule(crate::RulePath {
                    scope: ScopePath(parent_scope.to_vec()),
                    index: *index,
                }))
            },
        );
        for pair in present.windows(2) {
            let [(earlier, earlier_matched), (later, later_matched)] = pair else {
                continue;
            };
            let position =
                |occurrence: &&BoundSection<'_>| occurrence.section.heading.location.range.start.0;
            let last_earlier = earlier_matched.iter().map(position).max();
            let first_later = later_matched.iter().map(position).min();
            if matches!((last_earlier, first_later), (Some(last), Some(first)) if last < first) {
                continue;
            }
            let mut involved = earlier_matched
                .iter()
                .chain(later_matched.iter())
                .map(|occurrence| InvolvedHeader {
                    path: occurrence.path.clone(),
                    location: heading_location(&occurrence.section.heading.location),
                })
                .collect::<Vec<_>>();
            involved.sort_by_key(|header| (header.location.line, header.location.column));
            self.emit(
                Diagnostic {
                    id: DiagnosticId::Ordered,
                    target: match parent {
                        Some(_) => DiagnosticTarget::Header(parent_path.clone()),
                        None => DiagnosticTarget::Document,
                    },
                    location: parent
                        .map_or_else(root_location, |heading| heading_location(&heading.location)),
                    schema_node: schema_node.clone(),
                    involved_headers: involved,
                    references: Vec::new(),
                    message: format!(
                        "sections are out of the declared order: `{}` must precede `{}`",
                        matcher_label(&earlier.matcher),
                        matcher_label(&later.matcher)
                    ),
                },
                parent,
                true,
            );
        }
    }

    fn validate_cardinality(&mut self, check: CardinalityCheck<'_, '_>) {
        let CardinalityCheck {
            cardinality,
            count,
            rule,
            rule_index,
            occurrences,
            schema_scope,
            parent,
            parent_path,
        } = check;
        let schema_node = Some(SchemaNode::Rule(rule_path(schema_scope, rule_index)));
        if count < cardinality.min as usize {
            let id = if count == 0 {
                DiagnosticId::MissingSection
            } else {
                DiagnosticId::TooFewSections
            };
            self.emit(
                Diagnostic {
                    id,
                    // No concrete header represents the unmet cardinality —
                    // matching headers may exist, just too few of them — so the
                    // last segment can only be the rule's matcher label.
                    target: DiagnosticTarget::MissingHeader {
                        parent: parent_path.clone(),
                        matcher: matcher_label(&rule.matcher),
                    },
                    location: parent
                        .map_or_else(root_location, |heading| heading_location(&heading.location)),
                    schema_node: schema_node.clone(),
                    involved_headers: Vec::new(),
                    references: Vec::new(),
                    message: format!(
                        "matched {count} sections, but at least {} are required",
                        cardinality.min
                    ),
                },
                None,
                false,
            );
        }
        let UpperBound::Bounded(max) = cardinality.max else {
            return;
        };
        if count <= max as usize {
            return;
        }
        let excess_index = max as usize;
        let Some(excess) = occurrences
            .iter()
            .filter(|occurrence| occurrence.rule_index == rule_index)
            .nth(excess_index)
        else {
            return;
        };
        self.emit(
            Diagnostic {
                id: DiagnosticId::TooManySections,
                target: DiagnosticTarget::Header(excess.path.clone()),
                location: heading_location(&excess.section.heading.location),
                schema_node,
                involved_headers: Vec::new(),
                references: Vec::new(),
                message: format!("more than {max} sections match this rule"),
            },
            Some(&excess.section.heading),
            true,
        );
    }

    fn validate_constraints<'d>(
        &mut self,
        eval: EvalCtx<'_, 'd>,
        constraints: &[Constraint],
        schema_scope: &ScopePath,
        parent: Option<&Heading>,
        parent_path: &HeaderPath,
    ) {
        for (index, constraint) in constraints.iter().enumerate() {
            if eval.constraint_satisfied(constraint) {
                continue;
            }
            let id = constraint_id(constraint);
            let involved = eval
                .constraint_occurrences(constraint)
                .into_iter()
                .map(|occurrence| InvolvedHeader {
                    path: occurrence.path.clone(),
                    location: heading_location(&occurrence.section.heading.location),
                })
                .collect();
            self.emit(
                Diagnostic {
                    id,
                    // The scope the constraint is attached to. At the document
                    // root that scope is flat and spans headers under different
                    // ancestors, so no single header can name it.
                    target: match parent {
                        Some(_) => DiagnosticTarget::Header(parent_path.clone()),
                        None => DiagnosticTarget::Document,
                    },
                    location: parent
                        .map_or_else(root_location, |heading| heading_location(&heading.location)),
                    schema_node: Some(SchemaNode::Constraint(ConstraintPath {
                        scope: schema_scope.clone(),
                        index: ConstraintIndex(index),
                    })),
                    involved_headers: involved,
                    references: eval.constraint_references(constraint),
                    message: format!("the `{}` constraint is not satisfied", id.as_str()),
                },
                parent,
                true,
            );
        }

        for occurrence in &eval.current.occurrences {
            let Some(rule) = eval.current_rules.get(occurrence.rule_index) else {
                continue;
            };
            let mut child_schema_scope = schema_scope.clone();
            child_schema_scope.0.push(RuleIndex(occurrence.rule_index));
            self.validate_constraints(
                EvalCtx {
                    current: &occurrence.child,
                    current_rules: &rule.sections,
                    root: eval.root,
                    root_rules: eval.root_rules,
                    frontmatter: eval.frontmatter,
                    match_case: eval.match_case,
                },
                &rule.constraints,
                &child_schema_scope,
                Some(&occurrence.section.heading),
                &occurrence.path,
            );
        }
    }

    /// Emits a diagnostic about a header that is present in the document.
    fn emit_present(
        &mut self,
        id: DiagnosticId,
        path: HeaderPath,
        heading: &Heading,
        schema_node: Option<SchemaNode>,
        message: &str,
    ) {
        self.emit(
            Diagnostic {
                id,
                target: DiagnosticTarget::Header(path),
                location: heading_location(&heading.location),
                schema_node,
                involved_headers: Vec::new(),
                references: Vec::new(),
                message: message.into(),
            },
            Some(heading),
            true,
        );
    }

    fn emit(&mut self, diagnostic: Diagnostic, anchor: Option<&Heading>, inline_allowed: bool) {
        let id = diagnostic.id.as_str();
        if self.document.file_suppressions.contains(id)
            || (inline_allowed && anchor.is_some_and(|heading| heading.suppressions.contains(id)))
        {
            return;
        }
        self.diagnostics.push(diagnostic);
    }
}

#[derive(Debug)]
struct BoundScope<'d> {
    occurrences: Vec<BoundSection<'d>>,
}

#[derive(Debug)]
struct BoundSection<'d> {
    rule_index: usize,
    section: &'d Section,
    path: HeaderPath,
    child: BoundScope<'d>,
}

/// A document section paired with its complete document-tree path.
#[derive(Debug)]
struct PathedSection<'d> {
    section: &'d Section,
    path: HeaderPath,
}

/// The document's top-level section forest — the virtual root's children —
/// each with the one-segment path that names it.
fn top_level_sections(sections: &[Section]) -> Vec<PathedSection<'_>> {
    sections
        .iter()
        .map(|section| PathedSection {
            section,
            path: appended_path(&HeaderPath::default(), &section.heading.diagnostic_text),
        })
        .collect()
}

/// The virtual root's bindable children.
///
/// `child_level` is the level the root's scope describes: `h1` for the outline
/// scope, `h2` when a sugar or `title: null` schema binds a headless
/// document's `sections` scope directly. A deeper top-level header skips
/// levels against the virtual root; it binds into the root's scope only when
/// `allow_skipped_levels` says so, and otherwise takes part in nothing — the
/// preserved half of the retired detached-section semantics, with the
/// skipped-level walk speaking about it. (Inside a *bound* scope, children
/// bind whatever their level, as they always have; the virtual root differs
/// because an unadmitted top-level subtree has no bound ancestor at all.)
fn admitted_at_root<'d>(
    top: &[PathedSection<'d>],
    child_level: HeaderLevel,
    allow_skipped: bool,
) -> Vec<PathedSection<'d>> {
    top.iter()
        .filter(|pathed| {
            let level = pathed.section.heading.level;
            level == child_level || (allow_skipped && level > child_level)
        })
        .map(|pathed| PathedSection {
            section: pathed.section,
            path: pathed.path.clone(),
        })
        .collect()
}

fn child_sections<'d>(section: &'d Section, path: &HeaderPath) -> Vec<PathedSection<'d>> {
    section
        .children
        .iter()
        .map(|child| PathedSection {
            section: child,
            path: appended_path(path, &child.heading.diagnostic_text),
        })
        .collect()
}

fn root_location() -> DiagnosticLocation {
    DiagnosticLocation {
        range: TextRange {
            start: ByteOffset(0),
            end: ByteOffset(0),
        },
        line: 1,
        column: 1,
    }
}

fn heading_location(location: &HeadingLocation) -> DiagnosticLocation {
    DiagnosticLocation {
        range: location.line_range,
        line: location.line,
        column: location.column,
    }
}

fn appended_path(parent: &HeaderPath, child: &str) -> HeaderPath {
    let mut path = parent.0.clone();
    path.push(child.to_owned());
    HeaderPath(path)
}

fn rule_path(scope: &ScopePath, index: usize) -> crate::RulePath {
    crate::RulePath {
        scope: scope.clone(),
        index: RuleIndex(index),
    }
}

fn matcher_label(matcher: &Matcher) -> String {
    match matcher {
        Matcher::Exact(text) => text.0.clone(),
        Matcher::Glob(pattern) => pattern.0.clone(),
        Matcher::Regex(pattern) => format!("/{}/", pattern.0),
        Matcher::Any => "*".into(),
    }
}

fn constraint_id(constraint: &Constraint) -> DiagnosticId {
    match constraint {
        Constraint::OneOf(_) => DiagnosticId::OneOf,
        Constraint::AnyOf(_) => DiagnosticId::AnyOf,
        Constraint::AtMostOne(_) => DiagnosticId::AtMostOne,
        Constraint::AllOrNone(_) => DiagnosticId::AllOrNone,
        Constraint::Requires { .. } => DiagnosticId::Requires,
        Constraint::Conflicts { .. } => DiagnosticId::Conflicts,
        Constraint::Ordered(_) => DiagnosticId::Ordered,
    }
}

/// Evaluates an `fm.` proposition against the document's frontmatter (§4.6).
///
/// The bare form is satisfied iff the addressed value exists and is not null —
/// mappings and sequences included. The `=` form additionally requires typed
/// scalar equality, so it is never satisfied by a mapping or sequence value.
fn frontmatter_satisfied(
    frontmatter: Option<&serde_json::Map<String, serde_json::Value>>,
    reference: &FrontmatterRef,
    match_case: bool,
) -> bool {
    let Some(value) = frontmatter.and_then(|mapping| mapping.get(&reference.path.first.0)) else {
        return false;
    };
    let mut value = value;
    for key in &reference.path.rest {
        let Some(next) = value.as_object().and_then(|mapping| mapping.get(&key.0)) else {
            return false;
        };
        value = next;
    }
    if value.is_null() {
        return false;
    }
    match &reference.equals {
        None => true,
        Some(expected) => frontmatter_scalar_equals(value, expected, match_case),
    }
}

/// Typed equality between a frontmatter value and a resolved ref literal.
///
/// Both sides went through the YAML 1.2 core-schema resolver
/// ([`parse_frontmatter_scalar`]): the document side when the frontmatter was
/// read, the literal when the schema was loaded. Equality requires the same
/// type and the same value — `1` matches neither `"1"` nor `1.0`. Document
/// numbers keep their source lexeme (arbitrary precision), so re-resolving
/// that lexeme yields the canonical form the literal already carries.
fn frontmatter_scalar_equals(
    value: &serde_json::Value,
    expected: &FrontmatterScalar,
    match_case: bool,
) -> bool {
    match (value, expected) {
        (serde_json::Value::Bool(actual), FrontmatterScalar::Boolean(expected)) => {
            actual == expected
        }
        (serde_json::Value::String(actual), FrontmatterScalar::String(expected)) => {
            if match_case {
                actual == expected
            } else {
                crate::case_fold::simple_eq(actual, expected)
            }
        }
        (
            serde_json::Value::Number(actual),
            FrontmatterScalar::Integer(_) | FrontmatterScalar::Float(_),
        ) => parse_frontmatter_scalar(&actual.to_string()) == *expected,
        // Null never reaches here (the bare form already rejected it), and a
        // mapping or sequence is unsatisfied by every `=` form.
        _ => false,
    }
}

#[derive(Clone, Copy)]
struct EvalCtx<'s, 'd> {
    current: &'s BoundScope<'d>,
    current_rules: &'s [SectionRule],
    root: &'s BoundScope<'d>,
    root_rules: &'s [SectionRule],
    /// The document's frontmatter mapping, when one parsed. `fm.` propositions
    /// address the document rather than a scope, so this is the same from
    /// every constraint node.
    frontmatter: Option<&'d serde_json::Map<String, serde_json::Value>>,
    match_case: bool,
}

impl<'s, 'd> EvalCtx<'s, 'd> {
    fn constraint_satisfied(self, constraint: &Constraint) -> bool {
        match constraint {
            Constraint::OneOf(refs) => {
                refs.iter()
                    .filter(|proposition| self.proposition_satisfied(proposition))
                    .count()
                    == 1
            }
            Constraint::AnyOf(refs) => refs
                .iter()
                .any(|proposition| self.proposition_satisfied(proposition)),
            Constraint::AtMostOne(refs) => {
                refs.iter()
                    .filter(|proposition| self.proposition_satisfied(proposition))
                    .count()
                    <= 1
            }
            Constraint::AllOrNone(refs) => {
                let values = refs
                    .iter()
                    .map(|proposition| self.proposition_satisfied(proposition))
                    .collect::<Vec<_>>();
                values.iter().all(|value| *value) || values.iter().all(|value| !*value)
            }
            Constraint::Requires {
                condition,
                consequences,
            } => {
                !self.proposition_satisfied(condition)
                    || consequences
                        .iter()
                        .all(|proposition| self.proposition_satisfied(proposition))
            }
            Constraint::Conflicts {
                condition,
                exclusions,
            } => {
                !self.proposition_satisfied(condition)
                    || exclusions
                        .iter()
                        .all(|proposition| !self.proposition_satisfied(proposition))
            }
            Constraint::Ordered(refs) => {
                let satisfied = refs
                    .iter()
                    .map(|reference| self.resolve_occurrences(reference))
                    .filter(|occurrences| !occurrences.is_empty())
                    .collect::<Vec<_>>();
                satisfied
                    .iter()
                    .zip(satisfied.iter().skip(1))
                    .all(|(left, right)| {
                        let last_left = left
                            .iter()
                            .map(|occurrence| occurrence.section.heading.location.range.start.0)
                            .max();
                        let first_right = right
                            .iter()
                            .map(|occurrence| occurrence.section.heading.location.range.start.0)
                            .min();
                        matches!((last_left, first_right), (Some(left), Some(right)) if left < right)
                    })
            }
        }
    }

    fn proposition_satisfied(self, proposition: &Proposition) -> bool {
        match proposition {
            Proposition::Rule(reference) => !self.resolve_occurrences(reference).is_empty(),
            Proposition::Frontmatter(reference) => {
                frontmatter_satisfied(self.frontmatter, reference, self.match_case)
            }
        }
    }

    fn resolve_occurrences(self, reference: &RuleRef) -> Vec<&'s BoundSection<'d>> {
        let (start_scope, start_rules) = match reference.anchor {
            RefAnchor::CurrentScope => (self.current, self.current_rules),
            RefAnchor::SchemaRoot => (self.root, self.root_rules),
        };
        let mut candidate_scopes = vec![(start_scope, start_rules)];
        let mut found = Vec::new();
        for (position, id) in reference.path.iter().enumerate() {
            found.clear();
            let mut next_scopes = Vec::new();
            for (candidate, candidate_rules) in std::mem::take(&mut candidate_scopes) {
                let Some((index, rule)) = candidate_rules
                    .iter()
                    .enumerate()
                    .find(|(_, rule)| rule.id.as_ref() == Some(id))
                else {
                    continue;
                };
                for occurrence in candidate
                    .occurrences
                    .iter()
                    .filter(|occurrence| occurrence.rule_index == index)
                {
                    found.push(occurrence);
                    next_scopes.push((&occurrence.child, &rule.sections[..]));
                }
            }
            if position < reference.path.rest.len() {
                candidate_scopes = next_scopes;
            }
        }
        found
    }

    fn constraint_occurrences(self, constraint: &Constraint) -> Vec<&'s BoundSection<'d>> {
        let mut occurrences = Vec::new();
        match constraint {
            Constraint::OneOf(refs)
            | Constraint::AnyOf(refs)
            | Constraint::AtMostOne(refs)
            | Constraint::AllOrNone(refs) => {
                for proposition in refs.iter() {
                    self.add_proposition_occurrences(proposition, &mut occurrences);
                }
            }
            Constraint::Requires {
                condition,
                consequences,
            } => {
                self.add_proposition_occurrences(condition, &mut occurrences);
                for proposition in consequences.iter() {
                    self.add_proposition_occurrences(proposition, &mut occurrences);
                }
            }
            Constraint::Conflicts {
                condition,
                exclusions,
            } => {
                self.add_proposition_occurrences(condition, &mut occurrences);
                for proposition in exclusions.iter() {
                    self.add_proposition_occurrences(proposition, &mut occurrences);
                }
            }
            Constraint::Ordered(refs) => {
                for reference in refs.iter() {
                    occurrences.extend(self.resolve_occurrences(reference));
                }
            }
        }
        occurrences.sort_by_key(|occurrence| occurrence.section.heading.location.range.start.0);
        occurrences.dedup_by_key(|occurrence| occurrence.section.heading.location.range.start.0);
        occurrences
    }

    fn constraint_references(self, constraint: &Constraint) -> Vec<DiagnosticReference> {
        let mut references = Vec::new();
        match constraint {
            Constraint::OneOf(items)
            | Constraint::AnyOf(items)
            | Constraint::AtMostOne(items)
            | Constraint::AllOrNone(items) => {
                references.extend(
                    items
                        .iter()
                        .filter_map(|proposition| self.diagnostic_reference(proposition)),
                );
            }
            Constraint::Requires {
                condition,
                consequences,
            } => {
                references.extend(
                    std::iter::once(condition)
                        .chain(consequences.iter())
                        .filter_map(|proposition| self.diagnostic_reference(proposition)),
                );
            }
            Constraint::Conflicts {
                condition,
                exclusions,
            } => {
                references.extend(
                    std::iter::once(condition)
                        .chain(exclusions.iter())
                        .filter_map(|proposition| self.diagnostic_reference(proposition)),
                );
            }
            Constraint::Ordered(items) => {
                references.extend(items.iter().filter_map(|reference| {
                    self.rule_for_ref(reference)
                        .map(|rule| DiagnosticReference::Rule {
                            reference: reference.clone(),
                            matcher: rule.matcher.clone(),
                        })
                }));
            }
        }
        references
    }

    fn diagnostic_reference(self, proposition: &Proposition) -> Option<DiagnosticReference> {
        match proposition {
            Proposition::Rule(reference) => {
                self.rule_for_ref(reference)
                    .map(|rule| DiagnosticReference::Rule {
                        reference: reference.clone(),
                        matcher: rule.matcher.clone(),
                    })
            }
            Proposition::Frontmatter(reference) => {
                Some(DiagnosticReference::Frontmatter(reference.clone()))
            }
        }
    }

    fn rule_for_ref(self, reference: &RuleRef) -> Option<&'s SectionRule> {
        let mut rules = match reference.anchor {
            RefAnchor::CurrentScope => self.current_rules,
            RefAnchor::SchemaRoot => self.root_rules,
        };
        let mut target = None;
        for id in reference.path.iter() {
            target = rules.iter().find(|rule| rule.id.as_ref() == Some(id));
            rules = &target?.sections;
        }
        target
    }

    fn add_proposition_occurrences(
        self,
        proposition: &Proposition,
        output: &mut Vec<&'s BoundSection<'d>>,
    ) {
        if let Proposition::Rule(reference) = proposition {
            output.extend(self.resolve_occurrences(reference));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        load_schema, parse_markdown, ExactText, GlobPattern, MarkdownOptions, RegexPattern,
    };

    fn matcher_matches(matcher: &Matcher, text: &str, match_case: bool) -> bool {
        PreparedMatcher::new(matcher, match_case)
            .expect("test matcher compiles")
            .matches(text)
    }

    #[test]
    fn every_matcher_form_is_fully_anchored() {
        assert!(matcher_matches(
            &Matcher::Exact(ExactText("cat".into())),
            "cat",
            true
        ));
        assert!(!matcher_matches(
            &Matcher::Exact(ExactText("cat".into())),
            "cats",
            true
        ));
        assert!(matcher_matches(
            &Matcher::Glob(GlobPattern("c*t".into())),
            "coat",
            true
        ));
        assert!(!matcher_matches(
            &Matcher::Glob(GlobPattern("c*t".into())),
            "a coat",
            true
        ));
        assert!(matcher_matches(
            &Matcher::Regex(RegexPattern("c.+t".into())),
            "coat",
            true
        ));
        assert!(!matcher_matches(
            &Matcher::Regex(RegexPattern("c.+t".into())),
            "a coat",
            true
        ));
    }

    #[test]
    fn glob_treats_every_non_star_character_literally() {
        let matcher = Matcher::Glob(GlobPattern("file[1].*".into()));
        assert!(matcher_matches(&matcher, "file[1].md", true));
        assert!(!matcher_matches(&matcher, "file1.md", true));
    }

    #[test]
    fn glob_star_matches_newlines_in_multiline_setext_text() {
        let matcher = Matcher::Glob(GlobPattern("first*last".into()));
        assert!(matcher_matches(&matcher, "first\nmiddle\nlast", true));
    }

    #[test]
    fn exact_matching_does_not_compile_input_as_a_regex() {
        let text = "x".repeat(1_000_000);
        let matcher = Matcher::Exact(ExactText(text.clone()));
        assert!(matcher_matches(&matcher, &text, true));
    }

    #[test]
    fn case_insensitive_matching_is_unicode_aware_for_all_forms() {
        let matchers = [
            Matcher::Exact(ExactText("ÉCOLE".into())),
            Matcher::Glob(GlobPattern("ÉCO*".into())),
            Matcher::Regex(RegexPattern("ÉCO.*".into())),
        ];
        for matcher in matchers {
            assert!(matcher_matches(&matcher, "école", false));
            assert!(!matcher_matches(&matcher, "école", true));
        }
        let simple_fold_matchers = [
            Matcher::Exact(ExactText("S".into())),
            Matcher::Glob(GlobPattern("S*".into())),
            Matcher::Regex(RegexPattern("S.*".into())),
        ];
        for matcher in simple_fold_matchers {
            assert!(matcher_matches(&matcher, "ſ", false));
        }

        let full_only_fold_matchers = [
            Matcher::Exact(ExactText("Straße".into())),
            Matcher::Glob(GlobPattern("Straße*".into())),
            Matcher::Regex(RegexPattern("Straße.*".into())),
        ];
        for matcher in full_only_fold_matchers {
            assert!(!matcher_matches(&matcher, "STRASSE", false));
        }
    }

    #[test]
    fn inline_regex_flags_compose_with_match_case() {
        let matcher = Matcher::Regex(RegexPattern("(?i:api)".into()));
        assert!(matcher_matches(&matcher, "API", true));
        assert!(matcher_matches(&matcher, "api", true));
    }

    #[test]
    fn malformed_manually_constructed_regex_fails_preparation() {
        let mut schema = load_schema("version: 1\nsections: []\n")
            .expect("test schema is valid")
            .schema;
        schema.outline[0].matcher = Matcher::Regex(RegexPattern("(".into()));
        let error = PreparedValidator::new(&schema)
            .err()
            .expect("malformed regex must fail preparation");
        assert!(error.message.contains("cannot compile matcher"));
    }

    #[test]
    fn diagnostics_retain_normative_document_and_schema_anchors() {
        let loaded =
            load_schema("version: 1\ntitle: null\nsections:\n  - match: Item\n    repeat: 2..2\n")
                .expect("test schema is valid");
        let document = parse_markdown("## Item\n## Item\n## Item\n", MarkdownOptions::default());
        let diagnostics = validate(&loaded.schema, &document).expect("schema prepares");

        assert_eq!(diagnostics.len(), 1);
        let diagnostic = diagnostics.first().expect("one diagnostic was asserted");
        assert_eq!(diagnostic.id, DiagnosticId::TooManySections);
        assert_eq!(diagnostic.location.line, 3);
        assert_eq!(
            diagnostic.target,
            DiagnosticTarget::Header(HeaderPath(vec!["Item".into()]))
        );
        assert_eq!(
            diagnostic.schema_node,
            Some(SchemaNode::Rule(crate::RulePath {
                scope: ScopePath(Vec::new()),
                index: RuleIndex(0),
            }))
        );
    }

    #[test]
    fn header_paths_carry_the_enclosing_h1() {
        let loaded = load_schema(
            "version: 1\nsections:\n  - match: Overview\n    repeat: 1..n\n    sections:\n      - match: Goals\n        required: true\n",
        )
        .expect("test schema is valid");
        let document = parse_markdown(
            "# Part One\n## Overview\n# Part Two\n## Overview\n",
            MarkdownOptions::default(),
        );
        let targets = validate(&loaded.schema, &document)
            .expect("schema prepares")
            .into_iter()
            .map(|diagnostic| diagnostic.target)
            .collect::<Vec<_>>();

        // Same rule, same matcher, two different enclosing headers: the paths
        // distinguish them because the enclosing `h1` is kept, and each `h1`
        // binds its own `sections` scope. Two `h1` headers also break the
        // sugar's one-title bound, so the shape that makes the paths differ
        // is itself reported.
        assert_eq!(
            targets,
            [
                DiagnosticTarget::Header(HeaderPath(vec!["Part Two".into()])),
                DiagnosticTarget::MissingHeader {
                    parent: HeaderPath(vec!["Part One".into(), "Overview".into()]),
                    matcher: "Goals".into(),
                },
                DiagnosticTarget::MissingHeader {
                    parent: HeaderPath(vec!["Part Two".into(), "Overview".into()]),
                    matcher: "Goals".into(),
                },
            ]
        );
    }

    fn surplus_diagnostics(schema: &str, markdown: &str) -> Vec<Diagnostic> {
        let loaded = load_schema(schema).expect("test schema is valid");
        let document = parse_markdown(markdown, MarkdownOptions::default());
        validate(&loaded.schema, &document)
            .expect("schema prepares")
            .into_iter()
            .filter(|diagnostic| diagnostic.id == DiagnosticId::TooManySections)
            .collect()
    }

    fn skipped_diagnostics(schema: &str, markdown: &str) -> Vec<Diagnostic> {
        let loaded = load_schema(schema).expect("test schema is valid");
        let document = parse_markdown(markdown, MarkdownOptions::default());
        validate(&loaded.schema, &document)
            .expect("schema prepares")
            .into_iter()
            .filter(|diagnostic| diagnostic.id == DiagnosticId::SkippedLevel)
            .collect()
    }

    #[test]
    fn surplus_h1_headers_are_reported_once_on_the_second_one() {
        let schema = "version: 1\nsections:\n  - match: Overview\n    repeat: 0..n\n";

        // One `h1` above any number of root sections is the intended shape.
        assert!(surplus_diagnostics(schema, "# One\n## Overview\n## Overview\n").is_empty());
        // No `h1` misses the implied title, but that is not a surplus: the
        // `sections` scope still binds the document's own top-level `h2`s.
        assert!(surplus_diagnostics(schema, "## Overview\n").is_empty());

        let two = surplus_diagnostics(schema, "# One\n## Overview\n# Two\n## Overview\n");
        assert_eq!(two.len(), 1);
        let diagnostic = two.first().expect("one diagnostic was asserted");
        // Anchored on the second `h1`, where the one-title bound breaks.
        assert_eq!(
            diagnostic.target,
            DiagnosticTarget::Header(HeaderPath(vec!["Two".into()]))
        );
        assert_eq!(diagnostic.location.line, 3);
        // Bare `sections:` implies `title: "*"`, so the implied title takes
        // the blame even though no `title:` key is spelled.
        assert_eq!(diagnostic.schema_node, Some(SchemaNode::Title));

        // Surplus beyond the second header says nothing new.
        let three = surplus_diagnostics(schema, "# One\n# Two\n# Three\n## Overview\n");
        assert_eq!(three.len(), 1);
        assert_eq!(
            three[0].target,
            DiagnosticTarget::Header(HeaderPath(vec!["Two".into()]))
        );
    }

    #[test]
    fn h2_headers_outside_the_documents_h1_skip_against_the_virtual_root() {
        let schema = "version: 1\nsections:\n  - match: Overview\n    repeat: 0..n\n";

        // Bounding the `h1` count is not enough on its own: this document has
        // exactly one `h1`, yet the leading `h2` precedes it with an empty
        // ancestor chain while the trailing one sits under it. The leading
        // one is a level skip against the virtual level-0 document root —
        // what `detached-section` used to name.
        let skipped = skipped_diagnostics(schema, "## Overview\n# Part One\n## Overview\n");
        assert!(surplus_diagnostics(schema, "## Overview\n# Part One\n## Overview\n").is_empty());
        assert_eq!(skipped.len(), 1);
        let diagnostic = skipped.first().expect("one diagnostic was asserted");
        assert_eq!(
            diagnostic.target,
            DiagnosticTarget::Header(HeaderPath(vec!["Overview".into()]))
        );
        assert_eq!(diagnostic.location.line, 1);
        // The skip is structural, so nothing in the schema is to blame — not
        // even when the schema names the `h1` with `title:`.
        assert_eq!(diagnostic.schema_node, None);
        assert_eq!(
            skipped_diagnostics(
                "version: 1\ntitle: Part One\nsections:\n  - match: Overview\n    repeat: 0..n\n",
                "## Overview\n# Part One\n",
            )[0]
            .schema_node,
            None
        );

        // Every `h2` under the one `h1` conforms, and so does a document that
        // has no `h1` at all: the virtual root then stands in at level 1.
        assert!(skipped_diagnostics(schema, "# Part One\n## Overview\n## Overview\n").is_empty());
        assert!(skipped_diagnostics(schema, "## Overview\n## Overview\n").is_empty());

        // Each stray top-level header is its own misplacement, so each is
        // reported.
        let two = skipped_diagnostics(schema, "## A\n## B\n# Part One\n## Overview\n");
        assert_eq!(
            two.iter()
                .map(|diagnostic| diagnostic.target.clone())
                .collect::<Vec<_>>(),
            [
                DiagnosticTarget::Header(HeaderPath(vec!["A".into()])),
                DiagnosticTarget::Header(HeaderPath(vec!["B".into()])),
            ]
        );

        // A stray header carries its own inline suppression, and the file
        // suppression covers them all.
        assert!(skipped_diagnostics(
            schema,
            "<!-- outlint-disable skipped-level -->\n## Overview\n# Part One\n",
        )
        .is_empty());
        assert!(skipped_diagnostics(
            schema,
            "<!-- outlint-disable-file skipped-level -->\n## A\n## B\n# Part One\n",
        )
        .is_empty());
    }

    fn ids_and_targets(schema: &str, markdown: &str) -> Vec<(DiagnosticId, DiagnosticTarget)> {
        let loaded = load_schema(schema).expect("test schema is valid");
        let document = parse_markdown(markdown, MarkdownOptions::default());
        validate(&loaded.schema, &document)
            .expect("schema prepares")
            .into_iter()
            .map(|diagnostic| (diagnostic.id, diagnostic.target))
            .collect()
    }

    #[test]
    fn an_unadmitted_top_level_header_takes_part_in_no_rule_matching_or_counting() {
        // The stray `h2` neither satisfies the rule it would match nor
        // withdraws the requirement: the `sections` scope binds the `h1`'s
        // children, and none of them is a `Detached`.
        assert_eq!(
            ids_and_targets(
                "version: 1\nsections:\n  - match: Detached\n    required: true\n",
                "## Detached\n# Title\n## Attached\n",
            ),
            [
                (
                    DiagnosticId::SkippedLevel,
                    DiagnosticTarget::Header(HeaderPath(vec!["Detached".into()])),
                ),
                (
                    DiagnosticId::MissingSection,
                    DiagnosticTarget::MissingHeader {
                        parent: HeaderPath::default(),
                        matcher: "Detached".into(),
                    },
                ),
            ]
        );

        // Nor does it count toward a maximum: one `Overview` is in scope, and
        // one is what the rule allows.
        assert_eq!(
            ids_and_targets(
                "version: 1\nsections:\n  - match: Overview\n    repeat: 0..1\n",
                "## Overview\n# Part One\n## Overview\n",
            ),
            [(
                DiagnosticId::SkippedLevel,
                DiagnosticTarget::Header(HeaderPath(vec!["Overview".into()])),
            )]
        );
    }

    #[test]
    fn an_unadmitted_subtree_is_reported_once_at_its_root() {
        // A header that should not be there cannot meaningfully be missing a
        // child, so nothing below the unadmitted root is bound; the skip walk
        // still descends, and finds `Surprise` one level under `X`, which is
        // no skip at all.
        assert_eq!(
            ids_and_targets(
                "version: 1\nsections:\n  - match: X\n    repeat: 0..n\n    strict: true\n    sections:\n      - match: Deep\n        required: true\n",
                "## X\n### Surprise\n# Title\n",
            ),
            [(
                DiagnosticId::SkippedLevel,
                DiagnosticTarget::Header(HeaderPath(vec!["X".into()])),
            )]
        );

        // Stray *siblings* are independent misplacements with separate
        // fixes, so they stay one diagnostic each.
        assert_eq!(
            ids_and_targets(
                "version: 1\nsections:\n  - match: \"*\"\n    repeat: 0..n\n",
                "## A\n### Under A\n## B\n# Title\n",
            ),
            [
                (
                    DiagnosticId::SkippedLevel,
                    DiagnosticTarget::Header(HeaderPath(vec!["A".into()])),
                ),
                (
                    DiagnosticId::SkippedLevel,
                    DiagnosticTarget::Header(HeaderPath(vec!["B".into()])),
                ),
            ]
        );
    }

    #[test]
    fn orphan_headers_skip_against_the_virtual_root() {
        let schema = "version: 1\nsections:\n  - match: Sec\n    repeat: 0..n\n";

        // An orphan has no parent header; the virtual document root is what
        // it skips against — level 0 when the document has an `h1`.
        assert_eq!(
            ids_and_targets(schema, "### Orphan\n# Title\n## Sec\n"),
            [(
                DiagnosticId::SkippedLevel,
                DiagnosticTarget::Header(HeaderPath(vec!["Orphan".into()])),
            )]
        );

        // With `title: null` the root stands in at level 1 and the `h2`s
        // bind directly, so a deeper orphan skips just the same.
        let headless = "version: 1\ntitle: null\nsections:\n  - match: Sec\n    repeat: 0..n\n";
        assert_eq!(
            ids_and_targets(headless, "### Orphan\n## Sec\n"),
            [(
                DiagnosticId::SkippedLevel,
                DiagnosticTarget::Header(HeaderPath(vec!["Orphan".into()])),
            )]
        );

        // A document of nothing but orphans reports each top-level one; the
        // `h4` one level under its `h3` parent is no skip of its own.
        assert_eq!(
            ids_and_targets(headless, "### One\n#### Two\n### Three\n"),
            [
                (
                    DiagnosticId::SkippedLevel,
                    DiagnosticTarget::Header(HeaderPath(vec!["One".into()])),
                ),
                (
                    DiagnosticId::SkippedLevel,
                    DiagnosticTarget::Header(HeaderPath(vec!["Three".into()])),
                ),
            ]
        );
    }

    #[test]
    fn level_admission_leaves_unmatched_headers_to_strict_alone() {
        // Structural admission is not a second gate on rule matching: a
        // bound scope's header that matches no rule is the business of
        // `strict`, which stays opt-in.
        let open = "version: 1\nsections:\n  - match: Known\n    repeat: 0..n\n";
        assert_eq!(
            ids_and_targets(open, "# Title\n## Known\n## Unmatched\n### Child\n"),
            []
        );
        let open_headless =
            "version: 1\ntitle: null\nsections:\n  - match: Known\n    repeat: 0..n\n";
        assert_eq!(
            ids_and_targets(open_headless, "## Known\n## Unmatched\n"),
            []
        );

        let closed =
            "version: 1\nsections:\n  - match: Known\n    repeat: 0..n\n    strict: true\n";
        assert_eq!(
            ids_and_targets(closed, "# Title\n## Known\n### Surprise\n"),
            [(
                DiagnosticId::UnexpectedSection,
                DiagnosticTarget::Header(HeaderPath(vec![
                    "Title".into(),
                    "Known".into(),
                    "Surprise".into(),
                ])),
            )]
        );
    }

    #[test]
    fn allow_skipped_levels_admits_top_level_headers_into_the_root_scope() {
        // General form, virtual root at level 0: an `h2` at the top skips a
        // level. With the option off it is reported and takes part in
        // nothing; with it on it binds into the outline scope like any
        // skipped child of a bound header, and can satisfy an h1 rule.
        let strict_levels = "version: 1\noutline:\n  - match: Stray\n    required: true\n";
        assert_eq!(
            ids_and_targets(strict_levels, "## Stray\n"),
            [
                (
                    DiagnosticId::SkippedLevel,
                    DiagnosticTarget::Header(HeaderPath(vec!["Stray".into()])),
                ),
                (
                    DiagnosticId::MissingSection,
                    DiagnosticTarget::MissingHeader {
                        parent: HeaderPath::default(),
                        matcher: "Stray".into(),
                    },
                ),
            ]
        );
        let lax_levels = "version: 1\noptions:\n  allow_skipped_levels: true\n\
                          outline:\n  - match: Stray\n    required: true\n";
        assert_eq!(ids_and_targets(lax_levels, "## Stray\n"), []);

        // Sugar's headless scope stands in at level 1, one level down: a
        // top-level `h3` is the skip there, and admission works the same.
        let sugar = "version: 1\ntitle: null\nsections:\n  - match: Deep\n    required: true\n";
        assert_eq!(
            ids_and_targets(sugar, "### Deep\n"),
            [
                (
                    DiagnosticId::SkippedLevel,
                    DiagnosticTarget::Header(HeaderPath(vec!["Deep".into()])),
                ),
                (
                    DiagnosticId::MissingSection,
                    DiagnosticTarget::MissingHeader {
                        parent: HeaderPath::default(),
                        matcher: "Deep".into(),
                    },
                ),
            ]
        );
        let lax_sugar = "version: 1\noptions:\n  allow_skipped_levels: true\n\
                         title: null\nsections:\n  - match: Deep\n    required: true\n";
        assert_eq!(ids_and_targets(lax_sugar, "### Deep\n"), []);
    }

    #[test]
    fn title_null_denies_h1_and_binds_top_level_h2s() {
        let schema =
            "version: 1\ntitle: null\nsections:\n  - match: Overview\n    required: true\n";

        // The declared shape: no h1, the sections scope is the document's
        // own top-level h2s.
        assert_eq!(ids_and_targets(schema, "## Overview\n"), []);
        assert_eq!(
            ids_and_targets(schema, "## Wrong\n"),
            [(
                DiagnosticId::MissingSection,
                DiagnosticTarget::MissingHeader {
                    parent: HeaderPath::default(),
                    matcher: "Overview".into(),
                },
            )]
        );

        // A present h1 is rejected wholesale at the title node, its subtree
        // validated no further — like any header a deny rule matches. The
        // top-level h2 before it still binds.
        let loaded = load_schema(schema).expect("test schema is valid");
        let document = parse_markdown(
            "## Overview\n# Surprise\n## Hidden\n",
            MarkdownOptions::default(),
        );
        let diagnostics = validate(&loaded.schema, &document).expect("schema prepares");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].id, DiagnosticId::NotAllowed);
        assert_eq!(
            diagnostics[0].target,
            DiagnosticTarget::Header(HeaderPath(vec!["Surprise".into()]))
        );
        assert_eq!(diagnostics[0].schema_node, Some(SchemaNode::Title));
        assert_eq!(
            diagnostics[0].message,
            "the schema declares a document with no title"
        );
    }

    #[test]
    fn bare_sections_implies_a_required_title() {
        // `sections:` without `title:` means `title: "*"`: exactly one `h1`,
        // any text. A document that loses its `# Title` no longer passes
        // silently.
        let bare = "version: 1\nsections:\n  - match: Overview\n    required: true\n";
        let loaded = load_schema(bare).expect("test schema is valid");
        let document = parse_markdown("## Overview\n", MarkdownOptions::default());
        let diagnostics = validate(&loaded.schema, &document).expect("schema prepares");
        assert_eq!(diagnostics.len(), 1);
        let diagnostic = diagnostics.first().expect("one diagnostic was asserted");
        assert_eq!(diagnostic.id, DiagnosticId::MissingTitle);
        assert_eq!(diagnostic.message, "the document has no required title");
        assert_eq!(diagnostic.location, root_location());
        assert_eq!(
            diagnostic.target,
            DiagnosticTarget::MissingHeader {
                parent: HeaderPath::default(),
                matcher: "*".into(),
            }
        );
        // With no `title:` key to blame, the title node anchors on the
        // `sections` entry — the spelling that implied the rule.
        assert_eq!(diagnostic.schema_node, Some(SchemaNode::Title));
        let anchor = loaded
            .locations
            .nodes
            .get(&SchemaNode::Title)
            .expect("bare sections records a title anchor");
        let spelled = &bare[anchor.range.start.0..anchor.range.end.0];
        assert_eq!(spelled, "- match: Overview\n    required: true\n");

        // A single `h1` — any text — satisfies the implied title, and the
        // same headless document under `title: null` is declared conformant.
        assert_eq!(ids_and_targets(bare, "# Anything\n## Overview\n"), []);
        let null = "version: 1\ntitle: null\nsections:\n  - match: Overview\n    required: true\n";
        assert_eq!(ids_and_targets(null, "## Overview\n"), []);

        // The strictness is sugar business: the general form has no title
        // slot, so a zero-`h1` document under `outline:` misses nothing.
        let general = "version: 1\noptions:\n  allow_skipped_levels: true\n\
                       outline:\n  - match: Part\n    repeat: \"0..n\"\n\
                       \x20   sections:\n      - match: Overview\n        required: true\n";
        assert_eq!(ids_and_targets(general, ""), []);
    }

    #[test]
    fn a_general_form_h1_that_matches_no_rule_is_an_open_scope_header() {
        // No bespoke wrong-title verdict in the general form: an unmatched h1
        // is simply not this schema's business unless a rule or `strict`
        // makes it so, and the required rule reports its own absence.
        let schema = "version: 1\noutline:\n  - match: \"Guide *\"\n    required: true\n";
        assert_eq!(
            ids_and_targets(schema, "# Handbook\n## Anything\n"),
            [(
                DiagnosticId::MissingSection,
                DiagnosticTarget::MissingHeader {
                    parent: HeaderPath::default(),
                    matcher: "Guide *".into(),
                },
            )]
        );
    }

    #[test]
    fn multi_h1_sugar_cardinality_misses_carry_the_owning_h1() {
        // Two failing `h1` subtrees under the legacy document voice would be
        // byte-identical; with more than one bound `h1` each instance's
        // diagnostics name their owner instead, so both parents appear.
        assert_eq!(
            ids_and_targets(
                "version: 1\ntitle: \"*\"\nsections:\n  - match: Overview\n    required: true\n",
                "# One\n# Two\n",
            ),
            [
                (
                    DiagnosticId::TooManySections,
                    DiagnosticTarget::Header(HeaderPath(vec!["Two".into()])),
                ),
                (
                    DiagnosticId::MissingSection,
                    DiagnosticTarget::MissingHeader {
                        parent: HeaderPath(vec!["One".into()]),
                        matcher: "Overview".into(),
                    },
                ),
                (
                    DiagnosticId::MissingSection,
                    DiagnosticTarget::MissingHeader {
                        parent: HeaderPath(vec!["Two".into()]),
                        matcher: "Overview".into(),
                    },
                ),
            ]
        );

        // A single bound `h1` keeps the exact legacy voice: no parent header
        // on the miss. That voice is pinned corpus-wide; this is the local
        // witness that the attribution switch is the occurrence count.
        assert_eq!(
            ids_and_targets(
                "version: 1\ntitle: \"*\"\nsections:\n  - match: Overview\n    required: true\n",
                "# One\n",
            ),
            [(
                DiagnosticId::MissingSection,
                DiagnosticTarget::MissingHeader {
                    parent: HeaderPath::default(),
                    matcher: "Overview".into(),
                },
            )]
        );
    }

    #[test]
    fn multi_h1_sugar_constraints_target_the_owning_h1() {
        let schema = "version: 1\nsections:\n  - id: a\n    match: A\n    required: false\n  \
                      - id: b\n    match: B\n    required: false\nconstraints:\n  - requires: { if: a, then: b }\n";

        // One `h1`: the legacy voice, the document as target.
        let single = load_schema(schema).expect("test schema is valid");
        let document = parse_markdown("# One\n## A\n", MarkdownOptions::default());
        let single_diagnostics = validate(&single.schema, &document).expect("schema prepares");
        assert_eq!(single_diagnostics.len(), 1);
        assert_eq!(single_diagnostics[0].id, DiagnosticId::Requires);
        assert_eq!(single_diagnostics[0].target, DiagnosticTarget::Document);
        assert_eq!(single_diagnostics[0].location.line, 1);

        // Two `h1`s, both violating: each violation targets and anchors on
        // its own `h1` header instead of naming the document twice.
        let document = parse_markdown("# One\n## A\n# Two\n## A\n", MarkdownOptions::default());
        let diagnostics = validate(&single.schema, &document).expect("schema prepares");
        let requires = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.id == DiagnosticId::Requires)
            .map(|diagnostic| (diagnostic.target.clone(), diagnostic.location.line))
            .collect::<Vec<_>>();
        assert_eq!(
            requires,
            [
                (DiagnosticTarget::Header(HeaderPath(vec!["One".into()])), 1),
                (DiagnosticTarget::Header(HeaderPath(vec!["Two".into()])), 3),
            ]
        );
    }

    #[test]
    fn an_admitted_top_level_h2_never_occupies_the_title_slot() {
        // MAJOR-2 ruling: only `h1`s count for the title. With skipped levels
        // allowed, a leading `h2` that the title matcher would accept used to
        // consume the one-title bound — every leading `h2` under `title: "*"`
        // — yielding a phantom surplus title plus a missing section. It now
        // binds into the `sections` scope instead, where `Overview` under the
        // real `h1` and the unmatched `Intro` are both ordinary open-scope
        // members.
        let schema = "version: 1\noptions:\n  allow_skipped_levels: true\ntitle: \"*\"\n\
                      sections:\n  - match: Overview\n    required: true\n";
        assert_eq!(
            ids_and_targets(schema, "## Intro\n# Doc\n## Overview\n"),
            []
        );
    }

    #[test]
    fn an_admitted_top_level_h2_binds_the_titled_documents_sections_scope() {
        // The stray is not merely excluded from the title slot — it joins the
        // `sections` scope and can satisfy its rules. This pins the ruled
        // behavior against both regressions at once: under the old counting,
        // `Intro` matches `*` and occupies the title slot (surplus title plus
        // two missing-`Intro` instances); were the stray dropped outright,
        // the required `Intro` rule would fire. Only binding into the
        // `sections` scope leaves the document clean.
        let schema = "version: 1\noptions:\n  allow_skipped_levels: true\ntitle: \"*\"\n\
                      sections:\n  - match: Intro\n    required: true\n";
        assert_eq!(ids_and_targets(schema, "## Intro\n# Doc\n"), []);
    }

    #[test]
    fn surplus_titles_blame_the_spelled_or_implied_title() {
        let titled = surplus_diagnostics(
            "version: 1\ntitle: Project\nsections:\n  - match: Item\n    repeat: 0..n\n",
            "# Project\n# Project\n## Item\n",
        );
        assert_eq!(titled.len(), 1);
        let diagnostic = titled.first().expect("one diagnostic was asserted");
        assert_eq!(diagnostic.schema_node, Some(SchemaNode::Title));
        assert_eq!(diagnostic.message, "the document has more than one title");

        // Without `title:` the identical document reads the same way: bare
        // `sections:` implies `title: "*"`, so the surplus `h1` is a surplus
        // title there too, blamed on the implied title node.
        let untitled = surplus_diagnostics(
            "version: 1\nsections:\n  - match: Item\n    repeat: 0..n\n",
            "# Project\n# Project\n## Item\n",
        );
        assert_eq!(untitled.len(), 1);
        assert_eq!(untitled[0].schema_node, Some(SchemaNode::Title));
        assert_eq!(untitled[0].message, "the document has more than one title");
    }

    #[test]
    fn a_surplus_header_carries_its_own_inline_suppression() {
        assert!(surplus_diagnostics(
            "version: 1\nsections:\n  - match: Overview\n    repeat: 0..n\n",
            "# One\n## Overview\n<!-- outlint-disable too-many-sections -->\n# Two\n",
        )
        .is_empty());
    }

    #[test]
    fn root_scope_violations_name_the_document_rather_than_a_header() {
        let loaded = load_schema(
            "version: 1\nsections:\n  - id: a\n    match: A\n    required: true\n  - id: b\n    match: B\n    required: true\nconstraints:\n  - all_or_none: [a, b]\n",
        )
        .expect("test schema is valid");
        let document = parse_markdown("# Part One\n## B\n", MarkdownOptions::default());
        let targets = validate(&loaded.schema, &document)
            .expect("schema prepares")
            .into_iter()
            .map(|diagnostic| diagnostic.target)
            .collect::<Vec<_>>();

        // The root scope is flat, so a constraint on it has no one header to
        // name; a missing root section still has its schema-side matcher label.
        assert_eq!(
            targets,
            [
                DiagnosticTarget::MissingHeader {
                    parent: HeaderPath::default(),
                    matcher: "A".into(),
                },
                DiagnosticTarget::Document,
            ]
        );
    }

    #[test]
    fn unexpected_section_points_to_the_rule_that_closed_its_scope() {
        let loaded = load_schema("version: 1\nsections:\n  - match: Parent\n    strict: true\n")
            .expect("test schema is valid");
        let document = parse_markdown("## Parent\n### Surprise\n", MarkdownOptions::default());
        let diagnostics = validate(&loaded.schema, &document).expect("schema prepares");

        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.id == DiagnosticId::UnexpectedSection)
            .expect("the strict child scope rejects Surprise");
        assert_eq!(
            diagnostic.schema_node,
            Some(SchemaNode::Rule(crate::RulePath {
                scope: ScopePath(Vec::new()),
                index: RuleIndex(0),
            }))
        );
    }

    #[test]
    fn validates_required_frontmatter_against_json_schema() {
        let mut schema = load_schema("version: 1\nfrontmatter: { required: true }\nsections: []\n")
            .expect("test schema is valid")
            .schema;
        let object = serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "required": ["status"],
            "properties": { "status": { "enum": ["draft", "final"] } }
        });
        schema.frontmatter = FrontmatterPolicy::Required {
            schema: Some(FrontmatterSchema {
                root_uri: "https://outlint.invalid/root.json".into(),
                root: object,
                resources: std::collections::BTreeMap::new(),
            }),
        };

        let absent = parse_markdown("# Title\n", MarkdownOptions::default());
        assert_eq!(
            validate(&schema, &absent).expect("schema prepares")[0].id,
            DiagnosticId::MissingFrontmatter
        );

        let invalid = parse_markdown(
            "---\nstatus: proposed\n---\n# Title\n",
            MarkdownOptions::default(),
        );
        let diagnostics = validate(&schema, &invalid).expect("schema prepares");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].id, DiagnosticId::FrontmatterSchema);
        let DiagnosticTarget::Frontmatter { block: Some(block) } = &diagnostics[0].target else {
            panic!("a frontmatter schema diagnostic targets a present block");
        };
        assert_eq!(block.json_pointer.as_deref(), Some("/status"));
        assert_eq!(
            (block.line_range.start_line, block.line_range.end_line),
            (1, 3)
        );
        assert_eq!(
            diagnostics[0].schema_node,
            Some(SchemaNode::FrontmatterSchemaDocument)
        );

        let valid = parse_markdown(
            "---\nstatus: final\n---\n# Title\n",
            MarkdownOptions::default(),
        );
        assert!(validate(&schema, &valid)
            .expect("schema prepares")
            .is_empty());
    }

    #[test]
    fn frontmatter_schema_messages_quote_document_number_spellings() {
        let mut schema = load_schema("version: 1\ntitle: null\nsections: []\n")
            .expect("test schema is valid")
            .schema;
        schema.frontmatter = FrontmatterPolicy::Optional {
            schema: Some(FrontmatterSchema {
                root_uri: "https://outlint.invalid/root.json".into(),
                root: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "whole": { "maximum": 1 },
                        "fraction": { "maximum": 1 },
                        "lower_exponent": { "maximum": 1 },
                        "upper_exponent": { "maximum": 1 }
                    }
                }),
                resources: std::collections::BTreeMap::new(),
            }),
        };
        let document = parse_markdown(
            "---\nwhole: 100.0\nfraction: 1.5\nlower_exponent: 1e2\nupper_exponent: 1E2\n---\n",
            MarkdownOptions::default(),
        );
        let messages = validate(&schema, &document)
            .expect("schema prepares")
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect::<Vec<_>>();

        assert_eq!(
            messages,
            [
                "1.5 is greater than the maximum of 1",
                "1e2 is greater than the maximum of 1",
                "1E2 is greater than the maximum of 1",
                "100.0 is greater than the maximum of 1",
            ]
        );
    }

    #[test]
    fn manually_constructed_frontmatter_schema_denies_remote_retrieval() {
        let remote_uri = "https://example.invalid/frontmatter.schema.json";
        let mut schema = load_schema("version: 1\nsections: []\n")
            .expect("test schema is valid")
            .schema;
        schema.frontmatter = FrontmatterPolicy::Optional {
            schema: Some(FrontmatterSchema {
                root_uri: "https://outlint.invalid/root.json".into(),
                root: serde_json::json!({"$ref": remote_uri}),
                resources: std::collections::BTreeMap::new(),
            }),
        };

        let error = match PreparedValidator::new(&schema) {
            Err(error) => error,
            Ok(_) => panic!("remote refs cannot be retrieved during preparation"),
        };
        assert!(
            error.message.contains(&format!(
                "JSON Schema resource `{remote_uri}` was not preloaded"
            )),
            "unexpected retrieval diagnostic: {}",
            error.message
        );
        assert!(
            !error.message.contains("Default retriever"),
            "unexpected retrieval diagnostic: {}",
            error.message
        );
    }

    #[test]
    fn reports_invalid_and_forbidden_frontmatter_without_schema_execution() {
        let schema =
            load_schema("version: 1\nfrontmatter: { allow: false }\ntitle: null\nsections: []\n")
                .expect("test schema is valid")
                .schema;
        let document = parse_markdown("---\n- item\n---\n", MarkdownOptions::default());
        let ids = validate(&schema, &document)
            .expect("schema prepares")
            .into_iter()
            .map(|diagnostic| diagnostic.id)
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                DiagnosticId::ForbiddenFrontmatter,
                DiagnosticId::InvalidFrontmatter
            ]
        );
    }

    #[test]
    fn optional_forbidden_and_file_suppression_apply_to_json_schema() {
        let json_schema = FrontmatterSchema {
            root_uri: "https://outlint.invalid/root.json".into(),
            root: serde_json::Value::Bool(false),
            resources: std::collections::BTreeMap::new(),
        };
        let mut schema = load_schema("version: 1\ntitle: null\nsections: []\n")
            .expect("test schema is valid")
            .schema;
        schema.frontmatter = FrontmatterPolicy::Optional {
            schema: Some(json_schema.clone()),
        };
        let absent = parse_markdown("## Title\n", MarkdownOptions::default());
        assert!(validate(&schema, &absent)
            .expect("schema prepares")
            .is_empty());

        let suppressed = parse_markdown(
            "---\nstatus: draft\n---\n<!-- outlint-disable-file frontmatter-schema -->\n",
            MarkdownOptions::default(),
        );
        assert!(validate(&schema, &suppressed)
            .expect("schema prepares")
            .is_empty());

        schema.frontmatter = FrontmatterPolicy::Forbidden {
            schema: Some(json_schema),
        };
        let present = parse_markdown("---\nstatus: draft\n---\n", MarkdownOptions::default());
        let ids = validate(&schema, &present)
            .expect("schema prepares")
            .into_iter()
            .map(|diagnostic| diagnostic.id)
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                DiagnosticId::ForbiddenFrontmatter,
                DiagnosticId::FrontmatterSchema
            ]
        );
    }

    #[test]
    fn preparing_refuses_a_reference_chain_longer_than_the_compiler_can_recurse_over() {
        // Preparing a validator compiles the linked graph a second time, and
        // compiling a reference re-enters the compiler at its target, so a
        // chain costs a stack frame per link here exactly as it does in the
        // loader -- while every link sits at the same JSON depth, which is why
        // no nesting bound sees it. An overrun aborts the process rather than
        // returning, so this path cannot rely on the loader having refused
        // first; it charges the budget itself. Both sides of the boundary are
        // pinned, since a bound that quietly drifted below what it promises
        // would refuse graphs the compiler handles comfortably.
        let document = parse_markdown("---\nstatus: draft\n---\n", MarkdownOptions::default());

        let mut schema = load_schema("version: 1\ntitle: null\nsections: []\n")
            .expect("test schema is valid")
            .schema;
        schema.frontmatter = FrontmatterPolicy::Optional {
            schema: Some(reference_chain_schema(MAX_JSON_SCHEMA_REFERENCES - 1)),
        };
        assert!(validate(&schema, &document)
            .expect("a graph spending the whole budget still prepares")
            .is_empty());

        schema.frontmatter = FrontmatterPolicy::Optional {
            schema: Some(reference_chain_schema(MAX_JSON_SCHEMA_REFERENCES)),
        };
        let error = validate(&schema, &document).expect_err("one reference more is refused");
        assert_eq!(error.message, json_schema_reference_budget_message());
    }

    /// Builds a graph whose root reference starts a chain of `links` hops
    /// ending at `true`, declaring `links + 1` references in all.
    fn reference_chain_schema(links: usize) -> FrontmatterSchema {
        let mut definitions = serde_json::Map::new();
        definitions.insert("end".into(), serde_json::Value::Bool(true));
        for index in 0..links {
            let target = if index + 1 == links {
                "#/$defs/end".to_owned()
            } else {
                format!("#/$defs/{}", index + 1)
            };
            definitions.insert(index.to_string(), serde_json::json!({ "$ref": target }));
        }
        FrontmatterSchema {
            root_uri: "https://outlint.invalid/root.json".into(),
            root: serde_json::json!({ "$ref": "#/$defs/0", "$defs": definitions }),
            resources: std::collections::BTreeMap::new(),
        }
    }

    /// Builds an `fm.` reference the way the loader normalizes one: the
    /// equality literal resolves through the shared core-schema resolver.
    fn fm_reference(path: &[&str], equals: Option<&str>) -> crate::FrontmatterRef {
        let mut keys = path.iter();
        crate::FrontmatterRef {
            path: crate::NonEmpty {
                first: crate::FrontmatterKey(
                    (*keys.next().expect("test paths are non-empty")).to_owned(),
                ),
                rest: keys
                    .map(|key| crate::FrontmatterKey((*key).to_owned()))
                    .collect(),
            },
            equals: equals.map(parse_frontmatter_scalar),
        }
    }

    /// Evaluates one `fm.` proposition against a Markdown document's parsed
    /// frontmatter, typed by the real reader.
    fn fm_satisfied(markdown: &str, path: &[&str], equals: Option<&str>, match_case: bool) -> bool {
        let document = parse_markdown(markdown, MarkdownOptions::default());
        let frontmatter = match &document.frontmatter {
            DocumentFrontmatter::Mapping { value, .. } => Some(value),
            DocumentFrontmatter::Absent | DocumentFrontmatter::Invalid { .. } => None,
        };
        frontmatter_satisfied(frontmatter, &fm_reference(path, equals), match_case)
    }

    #[test]
    fn bare_frontmatter_refs_are_presence_of_a_non_null_value() {
        let document = "---\npresent: 1\nempty: null\nnested:\n  inner: yes\n---\n";
        assert!(fm_satisfied(document, &["present"], None, false));
        // A null value does not satisfy the bare form, and neither does a key
        // the frontmatter lacks.
        assert!(!fm_satisfied(document, &["empty"], None, false));
        assert!(!fm_satisfied(document, &["absent"], None, false));
        // Nested steps address nested mappings.
        assert!(fm_satisfied(document, &["nested", "inner"], None, false));
        assert!(!fm_satisfied(document, &["nested", "missing"], None, false));
        // A step into a non-mapping is unsatisfied, whatever the value is.
        assert!(!fm_satisfied(document, &["present", "deeper"], None, false));
        // A document with no frontmatter at all satisfies nothing.
        assert!(!fm_satisfied("# Title\n", &["present"], None, false));
    }

    #[test]
    fn bare_refs_accept_collections_but_equality_refuses_them() {
        let document = "---\nitems:\n  - one\ntable:\n  key: value\n---\n";
        // The bare form is satisfied by any non-null value, collections
        // included; the `=` form compares scalars only.
        assert!(fm_satisfied(document, &["items"], None, false));
        assert!(fm_satisfied(document, &["table"], None, false));
        assert!(!fm_satisfied(document, &["items"], Some("one"), false));
        assert!(!fm_satisfied(document, &["table"], Some("value"), false));
        // Stepping through a sequence is unsatisfied: only mappings nest.
        assert!(!fm_satisfied(document, &["items", "one"], None, false));
    }

    #[test]
    fn equality_is_typed_by_the_core_schema_resolver() {
        let document = "---\ncount: 1\nspelled: \"1\"\ndraft: true\nquoted: \"true\"\n---\n";
        assert!(fm_satisfied(document, &["count"], Some("1"), false));
        assert!(fm_satisfied(document, &["draft"], Some("true"), false));
        // There is no quoting in the ref literal: the quotes are characters
        // of the string, which the value `"1"` does not contain.
        assert!(!fm_satisfied(document, &["spelled"], Some("\"1\""), false));
        // The spec's three negative examples: no cross-type coercion.
        assert!(!fm_satisfied(document, &["spelled"], Some("1"), false));
        assert!(!fm_satisfied(document, &["quoted"], Some("true"), false));
        assert!(!fm_satisfied(document, &["count"], Some("1.0"), false));
        // Both sides canonicalize before comparing: spelling is irrelevant
        // within a type.
        let spellings = "---\nhex: 0x10\nfloat: 12.5\n---\n";
        assert!(fm_satisfied(spellings, &["hex"], Some("16"), false));
        assert!(fm_satisfied(spellings, &["float"], Some("1.25e1"), false));
        assert!(!fm_satisfied(spellings, &["hex"], Some("16.0"), false));
        // `=null` can never hold: a null value already fails the bare form.
        assert!(!fm_satisfied(
            "---\nempty: null\n---\n",
            &["empty"],
            Some("null"),
            false
        ));
    }

    #[test]
    fn string_equality_follows_match_case_with_simple_folding() {
        let document = "---\nstatus: Deprecated\nfold: \u{17f}\n---\n";
        assert!(fm_satisfied(
            document,
            &["status"],
            Some("deprecated"),
            false
        ));
        assert!(!fm_satisfied(
            document,
            &["status"],
            Some("deprecated"),
            true
        ));
        assert!(fm_satisfied(
            document,
            &["status"],
            Some("Deprecated"),
            true
        ));
        // Unicode simple folding: `ſ` matches `S` only case-insensitively.
        assert!(fm_satisfied(document, &["fold"], Some("S"), false));
        assert!(!fm_satisfied(document, &["fold"], Some("S"), true));
    }

    #[test]
    fn deep_nesting_resolves_one_mapping_per_step() {
        let document = "---\na:\n  b:\n    c:\n      d: leaf\n---\n";
        assert!(fm_satisfied(document, &["a", "b", "c", "d"], None, false));
        assert!(fm_satisfied(
            document,
            &["a", "b", "c", "d"],
            Some("leaf"),
            false
        ));
        assert!(!fm_satisfied(
            document,
            &["a", "b", "c", "d", "e"],
            None,
            false
        ));
        assert!(!fm_satisfied(
            document,
            &["a", "b", "c"],
            Some("leaf"),
            false
        ));
    }

    #[test]
    fn frontmatter_constraints_fire_and_release_through_validation() {
        let loaded = load_schema(
            "version: 1\nsections:\n  - id: migration\n    match: Migration\n    \
             required: false\nconstraints:\n  - requires: { if: fm.status=deprecated, \
             then: migration }\n",
        )
        .expect("test schema is valid");

        let firing = parse_markdown(
            "---\nstatus: deprecated\n---\n# Doc\n",
            MarkdownOptions::default(),
        );
        let diagnostics = validate(&loaded.schema, &firing).expect("schema prepares");
        assert_eq!(diagnostics.len(), 1);
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic.id, DiagnosticId::Requires);
        // The constraint sits at the schema root, so the target is the
        // document; the frontmatter side is named among the references.
        assert_eq!(diagnostic.target, DiagnosticTarget::Document);
        assert_eq!(
            diagnostic.references[0],
            DiagnosticReference::Frontmatter(fm_reference(&["status"], Some("deprecated"))),
        );

        // Unsatisfied condition: nothing fires.
        let inert = parse_markdown(
            "---\nstatus: current\n---\n# Doc\n",
            MarkdownOptions::default(),
        );
        assert!(validate(&loaded.schema, &inert)
            .expect("schema prepares")
            .is_empty());

        // Satisfied consequence: nothing fires either.
        let satisfied = parse_markdown(
            "---\nstatus: deprecated\n---\n# Doc\n## Migration\n",
            MarkdownOptions::default(),
        );
        assert!(validate(&loaded.schema, &satisfied)
            .expect("schema prepares")
            .is_empty());
    }

    #[test]
    fn fm_refs_read_frontmatter_even_when_a_nested_rule_is_addressable_as_fm_x() {
        // A nested rule id `fm` with child `x` would make the rule path
        // `fm.x` spellable — but `fm.` refs resolve via the frontmatter slot,
        // never the rule forest, so the headers below cannot satisfy the
        // condition.
        let loaded = load_schema(
            "version: 1\nsections:\n  - id: outer\n    match: Outer\n    required: false\n    \
             sections:\n      - id: fm\n        match: FM\n        required: false\n        \
             sections:\n          - id: x\n            match: X\n            required: false\n    \
             constraints:\n      - requires: { if: fm.x, then: fm.present }\n",
        )
        .expect("only a top-level `fm` rule id is reserved");

        // Headers satisfy the rule path fm -> x in the constraint's scope; the
        // frontmatter key `x` is absent. Were the ref a rule ref, the
        // condition would hold and the unsatisfiable consequence would fire.
        let headers_only = parse_markdown(
            "# Doc\n## Outer\n### FM\n#### X\n",
            MarkdownOptions::default(),
        );
        assert!(validate(&loaded.schema, &headers_only)
            .expect("schema prepares")
            .is_empty());

        // The frontmatter key alone fires it, with no matching header in sight.
        let frontmatter_only = parse_markdown(
            "---\nx: 1\n---\n# Doc\n## Outer\n",
            MarkdownOptions::default(),
        );
        let diagnostics = validate(&loaded.schema, &frontmatter_only).expect("schema prepares");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].id, DiagnosticId::Requires);
    }

    fn ordered_diagnostics(schema: &str, markdown: &str) -> Vec<Diagnostic> {
        let loaded = load_schema(schema).expect("test schema is valid");
        let document = parse_markdown(markdown, MarkdownOptions::default());
        validate(&loaded.schema, &document)
            .expect("schema prepares")
            .into_iter()
            .filter(|diagnostic| diagnostic.id == DiagnosticId::Ordered)
            .collect()
    }

    #[test]
    fn a_scope_orders_its_rules_by_default() {
        // No constraint spelled: the `sections` list is the order. Under the
        // sugar's document voice the violation targets the document and is
        // attributed to the title node, since the scope has no rule of its
        // own; the message names the pair that broke.
        let schema =
            "version: 1\nsections:\n  - match: Overview\n  - match: Usage\n  - match: Notes\n";
        assert_eq!(
            ids_and_targets(schema, "# T\n## Overview\n## Usage\n## Notes\n"),
            []
        );
        let diagnostics = ordered_diagnostics(schema, "# T\n## Usage\n## Overview\n## Notes\n");
        assert_eq!(diagnostics.len(), 1);
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic.target, DiagnosticTarget::Document);
        assert_eq!(diagnostic.schema_node, Some(SchemaNode::Title));
        assert_eq!(diagnostic.location.line, 1);
        assert!(diagnostic.references.is_empty());
        assert_eq!(
            diagnostic.message,
            "sections are out of the declared order: `Overview` must precede `Usage`"
        );
        // Involved headers are the two rules' occurrences in document order.
        assert_eq!(
            diagnostic
                .involved_headers
                .iter()
                .map(|header| header.path.clone())
                .collect::<Vec<_>>(),
            [
                HeaderPath(vec!["T".into(), "Usage".into()]),
                HeaderPath(vec!["T".into(), "Overview".into()]),
            ]
        );
    }

    #[test]
    fn implicit_order_reports_each_broken_adjacent_pair() {
        // `last(A) < first(B)` over adjacent present rules, one diagnostic per
        // broken pair: a fully reversed list breaks every pair, while a
        // single displaced section breaks only the pairs around it.
        let schema = "version: 1\nsections:\n  - match: A\n  - match: B\n  - match: C\n";
        let reversed = ordered_diagnostics(schema, "# T\n## C\n## B\n## A\n");
        assert_eq!(
            reversed
                .iter()
                .map(|diagnostic| diagnostic.message.as_str())
                .collect::<Vec<_>>(),
            [
                "sections are out of the declared order: `A` must precede `B`",
                "sections are out of the declared order: `B` must precede `C`",
            ]
        );
        let displaced = ordered_diagnostics(schema, "# T\n## A\n## C\n## B\n");
        assert_eq!(displaced.len(), 1);
        assert_eq!(
            displaced[0].message,
            "sections are out of the declared order: `B` must precede `C`"
        );
    }

    #[test]
    fn implicit_order_ignores_unmatched_and_denied_headers_and_absent_rules() {
        // Unmatched headers in an open scope match no rule and float; a
        // denied rule matches nothing that counts; an absent optional rule is
        // simply not among the present pairs.
        let schema = "version: 1\nsections:\n  - match: A\n  - match: B\n    required: false\n  - match: C\n  - match: X\n    allow: false\n";
        assert_eq!(
            ids_and_targets(schema, "# T\n## Free\n## A\n## Free\n## C\n## Free\n"),
            []
        );
        assert_eq!(
            ids_and_targets(schema, "# T\n## X\n## A\n## C\n"),
            [(
                DiagnosticId::NotAllowed,
                DiagnosticTarget::Header(HeaderPath(vec!["T".into(), "X".into()])),
            )]
        );
    }

    #[test]
    fn implicit_order_compares_all_occurrences_of_repeated_rules() {
        // Repeats of one rule may sit together but not straddle the next
        // rule's occurrences: every A precedes every B.
        let schema = "version: 1\nsections:\n  - match: \"A *\"\n  - match: \"B *\"\n";
        assert_eq!(
            ids_and_targets(schema, "# T\n## A 1\n## A 2\n## B 1\n## B 2\n"),
            []
        );
        assert_eq!(
            ids_and_targets(schema, "# T\n## A 1\n## B 1\n## A 2\n"),
            [(DiagnosticId::Ordered, DiagnosticTarget::Document)]
        );
    }

    #[test]
    fn nested_and_outline_scopes_order_themselves_with_their_own_owners() {
        // A nested scope's violation targets the owning header and is
        // attributed to the owning rule; the general form's outline scope
        // targets the document and has no schema node, the root being
        // nobody's rule.
        let nested = "version: 1\nsections:\n  - match: Steps\n    sections:\n      - match: One\n      - match: Two\n";
        let diagnostics = ordered_diagnostics(nested, "# T\n## Steps\n### Two\n### One\n");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].target,
            DiagnosticTarget::Header(HeaderPath(vec!["T".into(), "Steps".into()]))
        );
        assert_eq!(
            diagnostics[0].schema_node,
            Some(SchemaNode::Rule(crate::RulePath {
                scope: ScopePath(Vec::new()),
                index: RuleIndex(0),
            }))
        );
        assert_eq!(diagnostics[0].location.line, 2);

        let outline = "version: 1\noutline:\n  - match: Intro\n  - match: Part\n";
        let diagnostics = ordered_diagnostics(outline, "# Part\n# Intro\n");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].target, DiagnosticTarget::Document);
        assert_eq!(diagnostics[0].schema_node, None);
    }

    #[test]
    fn the_option_sets_the_default_and_a_rule_overrides_it_for_its_scope() {
        let unordered = "version: 1\noptions:\n  ordered_sections: false\nsections:\n  - match: A\n  - match: B\n";
        assert_eq!(ids_and_targets(unordered, "# T\n## B\n## A\n"), []);

        // The rule's own `ordered` wins in both directions.
        let opted_in = "version: 1\noptions:\n  ordered_sections: false\nsections:\n  - match: S\n    ordered: true\n    sections:\n      - match: A\n      - match: B\n";
        assert_eq!(
            ids_and_targets(opted_in, "# T\n## S\n### B\n### A\n"),
            [(
                DiagnosticId::Ordered,
                DiagnosticTarget::Header(HeaderPath(vec!["T".into(), "S".into()])),
            )]
        );
        let opted_out = "version: 1\nsections:\n  - match: S\n    ordered: false\n    sections:\n      - match: A\n      - match: B\n";
        assert_eq!(ids_and_targets(opted_out, "# T\n## S\n### B\n### A\n"), []);
    }

    #[test]
    fn implicit_order_binds_per_instance_and_speaks_for_each_owner() {
        // Two h1s under the sugar bind two instances; each is compared on
        // its own and names its owning h1, since the document voice would
        // otherwise emit indistinguishable duplicates.
        let schema = "version: 1\nsections:\n  - match: A\n  - match: B\n";
        let diagnostics = ordered_diagnostics(schema, "# One\n## A\n## B\n# Two\n## B\n## A\n");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].target,
            DiagnosticTarget::Header(HeaderPath(vec!["Two".into()]))
        );
        assert_eq!(diagnostics[0].location.line, 4);
    }

    #[test]
    fn implicit_order_is_suppressible_at_the_owning_header() {
        let schema = "version: 1\nsections:\n  - match: S\n    sections:\n      - match: A\n      - match: B\n";
        assert_eq!(
            ids_and_targets(
                schema,
                "# T\n<!-- outlint-disable ordered -->\n## S\n### B\n### A\n"
            ),
            []
        );
    }
}
