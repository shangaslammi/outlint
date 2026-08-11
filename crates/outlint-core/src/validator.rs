//! Pure validation of a parsed Markdown outline against a normalized schema.
//!
//! Validation is deliberately separate from parsing and IO: callers can load
//! and parse fixture text once, then pass only values to [`validate`].

use crate::loader::{preloaded_json_schema_registry, NoExternalRetrieve};
use crate::matcher::{compile_anchored_pattern, compile_glob_pattern};
use crate::{
    ByteOffset, Cardinality, Constraint, ConstraintIndex, ConstraintPath, Document,
    DocumentFrontmatter, FrontmatterAnchor, FrontmatterLocation, FrontmatterPolicy, FrontmatterRef,
    FrontmatterSchema, HeaderLevel, Heading, HeadingLocation, Matcher, Proposition, RefAnchor,
    RuleIndex, RuleOutcome, RuleRef, Schema, SchemaNode, ScopePath, Section, SectionRule,
    TextRange, UpperBound,
};

/// A stable identifier from the diagnostic vocabulary in specification §6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    /// holds more than one `h1`.
    TooManySections,
    /// A header is not reachable from the document's spine.
    DetachedSection,
    /// The schema declares a title but the document has none.
    MissingTitle,
    /// Reserved for a required frontmatter block that is absent.
    MissingFrontmatter,
    /// Reserved for a present frontmatter block forbidden by the schema.
    ForbiddenFrontmatter,
    /// Reserved for a frontmatter block that is not a YAML mapping.
    InvalidFrontmatter,
    /// Reserved for a failure from delegated JSON Schema validation.
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
            Self::DetachedSection => "detached-section",
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
    /// Produces the portable representation used by the conformance corpus.
    pub fn display(&self) -> String {
        self.0.join(" > ")
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

/// A schema compiled once for validating any number of documents.
pub struct PreparedValidator {
    schema: Schema,
    plan: ValidationPlan,
}

impl PreparedValidator {
    /// Compiles matchers and the immutable JSON Schema resource registry.
    pub fn new(schema: &Schema) -> Result<Self, PrepareValidationError> {
        Ok(Self {
            schema: schema.clone(),
            plan: ValidationPlan::new(schema)?,
        })
    }

    /// Validates one parsed document without recompiling schema state.
    ///
    /// Frontmatter validation is included; `fm.` propositions remain deferred
    /// and therefore evaluate to false.
    pub fn validate(&self, document: &Document) -> Vec<Diagnostic> {
        Validator::new(&self.schema, document).run(&self.plan)
    }
}

/// Prepares and validates one document.
///
/// Use [`PreparedValidator`] directly when validating multiple documents.
pub fn validate(
    schema: &Schema,
    document: &Document,
) -> Result<Vec<Diagnostic>, PrepareValidationError> {
    PreparedValidator::new(schema).map(|prepared| prepared.validate(document))
}

struct ValidationPlan {
    title: Option<PreparedMatcher>,
    sections: Vec<PreparedRule>,
    frontmatter: Option<jsonschema::Validator>,
}

impl ValidationPlan {
    fn new(schema: &Schema) -> Result<Self, PrepareValidationError> {
        Ok(Self {
            title: schema
                .title
                .as_ref()
                .map(|matcher| PreparedMatcher::new(matcher, schema.options.match_case))
                .transpose()?,
            sections: prepare_rules(&schema.sections, schema.options.match_case)?,
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
        let spine = Spine::of(&self.document.sections);
        self.validate_title(&spine.reachable, plan.title.as_ref());
        self.validate_single_spine(&spine);
        if !self.schema.options.allow_skipped_levels {
            // Structural and schema-independent, like the reachability rule
            // itself: a skipped level inside a detached subtree is still a
            // skipped level, and reporting it does not enroll the subtree in
            // any rule.
            self.validate_skipped_levels(&self.document.sections, None, &HeaderPath::default());
        }

        // The root scope is the `h2` children of the document's `h1`, or every
        // `h2` in the document when there is no `h1`: exactly the reachable
        // `h2`s. Each one keeps its own ancestor chain, so two same-named root
        // sections under different ancestors stay distinct. The scope is flat
        // across `h1`s, which only matters on a document that already breaks
        // the one-`h1` bound.
        let root_sections = spine
            .reachable
            .into_iter()
            .filter(|pathed| pathed.section.heading.level == HeaderLevel::H2)
            .collect::<Vec<_>>();
        let root_schema_scope = ScopePath(Vec::new());
        let root_path = HeaderPath::default();
        let root = self.bind_scope(BindScopeInput {
            sections: &root_sections,
            rules: &self.schema.sections,
            prepared_rules: &plan.sections,
            strict: false,
            schema_scope: &root_schema_scope,
            parent: None,
            parent_path: &root_path,
        });
        self.validate_constraints(
            EvalCtx {
                current: &root,
                current_rules: &self.schema.sections,
                root: &root,
                root_rules: &self.schema.sections,
            },
            &self.schema.constraints,
            &root_schema_scope,
            None,
            &root_path,
        );
        self.diagnostics
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

    fn validate_title(
        &mut self,
        sections: &[PathedSection<'_>],
        prepared: Option<&PreparedMatcher>,
    ) {
        let Some(matcher) = &self.schema.title else {
            return;
        };
        let Some(prepared) = prepared else {
            return;
        };
        // The title is the document's `h1`, always: `sections` describes `h2`.
        let titles = sections
            .iter()
            .filter(|pathed| pathed.section.heading.level == HeaderLevel::H1)
            .collect::<Vec<_>>();
        if titles.is_empty() {
            self.emit(
                Diagnostic {
                    id: DiagnosticId::MissingTitle,
                    // The title sits above the root scope, so it has no parent.
                    target: DiagnosticTarget::MissingHeader {
                        parent: HeaderPath::default(),
                        matcher: matcher_label(matcher),
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
            return;
        }
        for title in &titles {
            if !prepared.matches(&title.section.heading.text) {
                self.emit(
                    Diagnostic {
                        id: DiagnosticId::NotAllowed,
                        // The offending title is present; only its text is wrong.
                        target: DiagnosticTarget::Header(title.path.clone()),
                        location: heading_location(&title.section.heading.location),
                        schema_node: Some(SchemaNode::Title),
                        involved_headers: Vec::new(),
                        references: Vec::new(),
                        message: "the title does not match the schema title matcher".into(),
                    },
                    Some(&title.section.heading),
                    true,
                );
            }
        }
        // A surplus title is a surplus `h1`, so `validate_single_spine`
        // reports it for every schema, titled or not.
    }

    /// Enforces the single top-level spine: at most one `h1`, and every header
    /// reachable from it.
    ///
    /// A schema describes the totality of the document, so no header is
    /// implicitly outside it. The two diagnostics together say so, and neither
    /// implies the other:
    ///
    /// - `# A` / `# B` / `## X` leaves every header reachable, yet forks the
    ///   spine in two.
    /// - `## X` / `# A` / `## Y` has one `h1`, yet `X` hangs outside it.
    ///
    /// A document with no `h1` at all conforms; its spine is its own `h2`s.
    fn validate_single_spine(&mut self, spine: &Spine<'_>) {
        let mut h1s = spine
            .reachable
            .iter()
            .filter(|pathed| pathed.section.heading.level == HeaderLevel::H1)
            .skip(1);
        // One diagnostic per document, anchored on the second `h1` in document
        // order: that is where the spine forks, and further surplus `h1`s name
        // the same fork and say nothing new.
        if let Some(excess) = h1s.next() {
            // The `h1` is the title when the schema declares one, and only then
            // is there a schema node to blame; the spine bound is structural.
            let (schema_node, message) = if self.schema.title.is_some() {
                (
                    Some(SchemaNode::Title),
                    "the document has more than one title",
                )
            } else {
                (None, "the document has more than one h1 header")
            };
            self.emit(
                Diagnostic {
                    id: DiagnosticId::TooManySections,
                    // Anchored on the surplus header, which is a real header.
                    target: DiagnosticTarget::Header(excess.path.clone()),
                    location: heading_location(&excess.section.heading.location),
                    schema_node,
                    involved_headers: Vec::new(),
                    references: Vec::new(),
                    message: message.to_owned(),
                },
                Some(&excess.section.heading),
                true,
            );
        }

        // Reported once per detached subtree *root* rather than once per
        // detached header: a header below a detached one is misplaced only as
        // a consequence of its ancestor, and moving that ancestor onto the
        // spine takes the whole subtree with it. Detached *siblings* are
        // independent misplacements, each with its own location, its own fix,
        // and its own inline suppression anchor, so each is reported — unlike
        // a forked spine, where one fork point explains every surplus `h1`.
        for section in &spine.detached {
            self.emit(
                Diagnostic {
                    id: DiagnosticId::DetachedSection,
                    // The detached section is a real header, and its path —
                    // one segment, no ancestor — is what shows it detached.
                    target: DiagnosticTarget::Header(section.path.clone()),
                    location: heading_location(&section.section.heading.location),
                    // Structural, like a skipped level: nothing in the schema
                    // asked for this header to sit where it does.
                    schema_node: None,
                    involved_headers: Vec::new(),
                    references: Vec::new(),
                    message: "the header is not reachable from the document's spine".into(),
                },
                Some(&section.section.heading),
                true,
            );
        }
    }

    fn validate_skipped_levels(
        &mut self,
        sections: &[Section],
        parent: Option<&Heading>,
        parent_path: &HeaderPath,
    ) {
        for section in sections {
            let path = appended_path(parent_path, &section.heading.diagnostic_text);
            if parent.is_some_and(|parent| section.heading.level as u8 > parent.level as u8 + 1) {
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
            self.validate_skipped_levels(&section.children, Some(&section.heading), &path);
        }
    }

    fn bind_scope<'d>(&mut self, input: BindScopeInput<'_, 'd>) -> BoundScope<'d> {
        let BindScopeInput {
            sections,
            rules,
            prepared_rules,
            strict,
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
        BoundScope { occurrences }
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

/// The document split into what the schema describes and what hangs outside it.
///
/// Every header must be reachable from the document's spine. A document has at
/// most one `h1`; if one exists it is the spine and the root scope is its `h2`
/// children, otherwise the root scope is the document's own `h2`s.
///
/// Reachable is the `h1` and everything below it — the whole subtree, not just
/// the root scope, so an `h3` sitting directly under the `h1` is reachable even
/// though it is in no scope. When there is no `h1`, reachable is the `h2`s and
/// everything below them. Anything else is detached and takes part in no rule
/// matching, no cardinality count and no constraint: the schema describes the
/// totality of the document, so a header hanging outside it is not silently
/// pooled in.
#[derive(Debug)]
struct Spine<'d> {
    /// Every reachable header, flattened in document order with its complete
    /// ancestor chain.
    reachable: Vec<PathedSection<'d>>,
    /// The root of each detached subtree, in document order. Descendants are
    /// deliberately absent: they are detached only by consequence.
    detached: Vec<PathedSection<'d>>,
}

impl<'d> Spine<'d> {
    /// Splits a document's top-level section forest along reachability.
    ///
    /// A heading nests under the nearest preceding heading of a lower level, so
    /// the whole question is decided at the top level of the forest. An `h1`
    /// has no lower level to nest under and is therefore always top-level;
    /// conversely, every header that follows an `h1` is inside some `h1`'s
    /// subtree. So when the document has an `h1`, the unreachable headers are
    /// exactly the top-level ones that are not `h1`s — which are exactly those
    /// preceding the first `h1`. With no `h1`, the same argument one level
    /// down leaves the top-level headers below `h2` — those preceding the first
    /// `h2`, plus any in a document with no `h2` at all.
    fn of(sections: &'d [Section]) -> Self {
        let has_h1 = sections
            .iter()
            .any(|section| section.heading.level == HeaderLevel::H1);
        let spine_level = if has_h1 {
            HeaderLevel::H1
        } else {
            HeaderLevel::H2
        };
        let mut spine = Self {
            reachable: Vec::new(),
            detached: Vec::new(),
        };
        for section in sections {
            if section.heading.level == spine_level {
                collect_sections(
                    std::slice::from_ref(section),
                    &HeaderPath::default(),
                    &mut spine.reachable,
                );
            } else {
                spine.detached.push(PathedSection {
                    section,
                    path: appended_path(&HeaderPath::default(), &section.heading.diagnostic_text),
                });
            }
        }
        spine
    }
}

/// Flattens the section forest in document order, giving every section the full
/// ancestor chain that names it.
fn collect_sections<'a>(
    sections: &'a [Section],
    parent_path: &HeaderPath,
    output: &mut Vec<PathedSection<'a>>,
) {
    for section in sections {
        let path = appended_path(parent_path, &section.heading.diagnostic_text);
        // Pre-order, so the flattened order stays document order.
        output.push(PathedSection {
            section,
            path: path.clone(),
        });
        collect_sections(&section.children, &path, output);
    }
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

fn frontmatter_satisfied(_reference: &FrontmatterRef) -> bool {
    false
}

#[derive(Clone, Copy)]
struct EvalCtx<'s, 'd> {
    current: &'s BoundScope<'d>,
    current_rules: &'s [SectionRule],
    root: &'s BoundScope<'d>,
    root_rules: &'s [SectionRule],
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
            Proposition::Frontmatter(reference) => frontmatter_satisfied(reference),
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
        schema.title = Some(Matcher::Regex(RegexPattern("(".into())));
        let error = PreparedValidator::new(&schema)
            .err()
            .expect("malformed regex must fail preparation");
        assert!(error.message.contains("cannot compile matcher"));
    }

    #[test]
    fn diagnostics_retain_normative_document_and_schema_anchors() {
        let loaded = load_schema("version: 1\nsections:\n  - match: Item\n    repeat: 2..2\n")
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
        // distinguish them only because the enclosing `h1` is kept. Two `h1`
        // headers also break the single top-level spine, so the shape that
        // makes the paths differ is itself reported.
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

    fn spine_diagnostics(schema: &str, markdown: &str) -> Vec<Diagnostic> {
        let loaded = load_schema(schema).expect("test schema is valid");
        let document = parse_markdown(markdown, MarkdownOptions::default());
        validate(&loaded.schema, &document)
            .expect("schema prepares")
            .into_iter()
            .filter(|diagnostic| diagnostic.id == DiagnosticId::TooManySections)
            .collect()
    }

    fn detached_diagnostics(schema: &str, markdown: &str) -> Vec<Diagnostic> {
        let loaded = load_schema(schema).expect("test schema is valid");
        let document = parse_markdown(markdown, MarkdownOptions::default());
        validate(&loaded.schema, &document)
            .expect("schema prepares")
            .into_iter()
            .filter(|diagnostic| diagnostic.id == DiagnosticId::DetachedSection)
            .collect()
    }

    #[test]
    fn surplus_h1_headers_are_reported_once_on_the_second_one() {
        let schema = "version: 1\nsections:\n  - match: Overview\n    repeat: 0..n\n";

        // One `h1` above any number of root sections is the intended shape.
        assert!(spine_diagnostics(schema, "# One\n## Overview\n## Overview\n").is_empty());
        // No `h1` at all is equally fine: the root scope is the whole document.
        assert!(spine_diagnostics(schema, "## Overview\n").is_empty());

        let two = spine_diagnostics(schema, "# One\n## Overview\n# Two\n## Overview\n");
        assert_eq!(two.len(), 1);
        let diagnostic = two.first().expect("one diagnostic was asserted");
        // Anchored on the second `h1`, the point at which the spine forks.
        assert_eq!(
            diagnostic.target,
            DiagnosticTarget::Header(HeaderPath(vec!["Two".into()]))
        );
        assert_eq!(diagnostic.location.line, 3);
        // Nothing in the schema names `h1`, so there is no node to blame.
        assert_eq!(diagnostic.schema_node, None);

        // Surplus beyond the second header says nothing new.
        let three = spine_diagnostics(schema, "# One\n# Two\n# Three\n## Overview\n");
        assert_eq!(three.len(), 1);
        assert_eq!(
            three[0].target,
            DiagnosticTarget::Header(HeaderPath(vec!["Two".into()]))
        );
    }

    #[test]
    fn h2_headers_outside_the_documents_h1_are_detached() {
        let schema = "version: 1\nsections:\n  - match: Overview\n    repeat: 0..n\n";

        // Bounding the `h1` count is not enough on its own: this document has
        // exactly one `h1`, yet the leading `h2` precedes it with an empty
        // ancestor chain while the trailing one sits under it.
        let detached = detached_diagnostics(schema, "## Overview\n# Part One\n## Overview\n");
        assert!(spine_diagnostics(schema, "## Overview\n# Part One\n## Overview\n").is_empty());
        assert_eq!(detached.len(), 1);
        let diagnostic = detached.first().expect("one diagnostic was asserted");
        assert_eq!(
            diagnostic.target,
            DiagnosticTarget::Header(HeaderPath(vec!["Overview".into()]))
        );
        assert_eq!(diagnostic.location.line, 1);
        // The `h1` is structural, so nothing in the schema is to blame — not
        // even when the schema names it with `title:`.
        assert_eq!(diagnostic.schema_node, None);
        assert_eq!(
            detached_diagnostics(
                "version: 1\ntitle: Part One\nsections:\n  - match: Overview\n    repeat: 0..n\n",
                "## Overview\n# Part One\n",
            )[0]
            .schema_node,
            None
        );

        // Every `h2` under the one `h1` conforms, and so does a document that
        // has no `h1` at all: its root scope is the whole document.
        assert!(detached_diagnostics(schema, "# Part One\n## Overview\n## Overview\n").is_empty());
        assert!(detached_diagnostics(schema, "## Overview\n## Overview\n").is_empty());

        // Each detached header is its own misplacement, so each is reported.
        let two = detached_diagnostics(schema, "## A\n## B\n# Part One\n## Overview\n");
        assert_eq!(
            two.iter()
                .map(|diagnostic| diagnostic.target.clone())
                .collect::<Vec<_>>(),
            [
                DiagnosticTarget::Header(HeaderPath(vec!["A".into()])),
                DiagnosticTarget::Header(HeaderPath(vec!["B".into()])),
            ]
        );

        // A detached header carries its own inline suppression, and the file
        // suppression covers them all.
        assert!(detached_diagnostics(
            schema,
            "<!-- outlint-disable detached-section -->\n## Overview\n# Part One\n",
        )
        .is_empty());
        assert!(detached_diagnostics(
            schema,
            "<!-- outlint-disable-file detached-section -->\n## A\n## B\n# Part One\n",
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
    fn a_detached_header_takes_part_in_no_rule_matching_or_counting() {
        // The out-of-scope `h2` neither satisfies the rule it would match nor
        // withdraws the requirement: the root scope is the `h1`'s children,
        // and none of them is a `Detached`.
        assert_eq!(
            ids_and_targets(
                "version: 1\nsections:\n  - match: Detached\n    required: true\n",
                "## Detached\n# Title\n## Attached\n",
            ),
            [
                (
                    DiagnosticId::DetachedSection,
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
                DiagnosticId::DetachedSection,
                DiagnosticTarget::Header(HeaderPath(vec!["Overview".into()])),
            )]
        );
    }

    #[test]
    fn a_detached_subtree_is_reported_once_at_its_root() {
        // A header that should not be there cannot meaningfully be missing a
        // child, so nothing below the detached root is validated.
        assert_eq!(
            ids_and_targets(
                "version: 1\nsections:\n  - match: X\n    repeat: 0..n\n    strict: true\n    sections:\n      - match: Deep\n        required: true\n",
                "## X\n### Surprise\n# Title\n",
            ),
            [(
                DiagnosticId::DetachedSection,
                DiagnosticTarget::Header(HeaderPath(vec!["X".into()])),
            )]
        );

        // Detached *siblings* are independent misplacements with separate
        // fixes, so they stay one diagnostic each.
        assert_eq!(
            ids_and_targets(
                "version: 1\nsections:\n  - match: \"*\"\n    repeat: 0..n\n",
                "## A\n### Under A\n## B\n# Title\n",
            ),
            [
                (
                    DiagnosticId::DetachedSection,
                    DiagnosticTarget::Header(HeaderPath(vec!["A".into()])),
                ),
                (
                    DiagnosticId::DetachedSection,
                    DiagnosticTarget::Header(HeaderPath(vec!["B".into()])),
                ),
            ]
        );
    }

    #[test]
    fn headers_deeper_than_h2_with_nothing_above_them_are_detached() {
        let schema = "version: 1\nsections:\n  - match: Sec\n    repeat: 0..n\n";

        // `skipped-level` cannot catch these: it compares a header with its
        // parent, and an orphan has none.
        assert_eq!(
            ids_and_targets(schema, "### Orphan\n# Title\n## Sec\n"),
            [(
                DiagnosticId::DetachedSection,
                DiagnosticTarget::Header(HeaderPath(vec!["Orphan".into()])),
            )]
        );

        // With no `h1`, the root scope is the document's `h2`s, so a header
        // above the first of them is outside the schema just the same.
        assert_eq!(
            ids_and_targets(schema, "### Orphan\n## Sec\n"),
            [(
                DiagnosticId::DetachedSection,
                DiagnosticTarget::Header(HeaderPath(vec!["Orphan".into()])),
            )]
        );

        // A document with no spine at all is entirely unreachable.
        assert_eq!(
            ids_and_targets(schema, "### One\n#### Two\n### Three\n"),
            [
                (
                    DiagnosticId::DetachedSection,
                    DiagnosticTarget::Header(HeaderPath(vec!["One".into()])),
                ),
                (
                    DiagnosticId::DetachedSection,
                    DiagnosticTarget::Header(HeaderPath(vec!["Three".into()])),
                ),
            ]
        );
    }

    #[test]
    fn reachability_leaves_unmatched_headers_to_strict_alone() {
        // Structural reachability is not a second gate on rule matching: a
        // reachable header that matches no rule is the business of `strict`,
        // which stays opt-in.
        let open = "version: 1\nsections:\n  - match: Known\n    repeat: 0..n\n";
        assert_eq!(
            ids_and_targets(open, "# Title\n## Known\n## Unmatched\n### Child\n"),
            []
        );
        assert_eq!(ids_and_targets(open, "## Known\n## Unmatched\n"), []);

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
    fn a_declared_title_still_owns_the_surplus_title_diagnostic() {
        let titled = spine_diagnostics(
            "version: 1\ntitle: Project\nsections:\n  - match: Item\n    repeat: 0..n\n",
            "# Project\n# Project\n## Item\n",
        );
        assert_eq!(titled.len(), 1);
        let diagnostic = titled.first().expect("one diagnostic was asserted");
        assert_eq!(diagnostic.schema_node, Some(SchemaNode::Title));
        assert_eq!(diagnostic.message, "the document has more than one title");

        // Without `title:` the identical document gets the structural wording,
        // because nothing in the schema names the `h1`.
        let untitled = spine_diagnostics(
            "version: 1\nsections:\n  - match: Item\n    repeat: 0..n\n",
            "# Project\n# Project\n## Item\n",
        );
        assert_eq!(untitled.len(), 1);
        assert_eq!(
            untitled[0].message,
            "the document has more than one h1 header"
        );
    }

    #[test]
    fn a_surplus_header_carries_its_own_inline_suppression() {
        assert!(spine_diagnostics(
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
        let mut schema = load_schema("version: 1\nsections: []\n")
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
        let schema = load_schema("version: 1\nfrontmatter: { allow: false }\nsections: []\n")
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
        let mut schema = load_schema("version: 1\nsections: []\n")
            .expect("test schema is valid")
            .schema;
        schema.frontmatter = FrontmatterPolicy::Optional {
            schema: Some(json_schema.clone()),
        };
        let absent = parse_markdown("# Title\n", MarkdownOptions::default());
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
}
