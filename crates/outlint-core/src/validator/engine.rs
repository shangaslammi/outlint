//! The validation walk: level admission, scope binding, and diagnostics.

use std::collections::BTreeMap;

use crate::typed_value::{
    parse_header, BoundComponent, ParseFailure, ResolvedYamlKind, TypedValue, ValueType,
};
use crate::{
    ByteOffset, CaptureName, CapturePath, Cardinality, ChildScope, Constraint, ConstraintIndex,
    ConstraintPath, DeclaredScope, Document, DocumentFrontmatter, DocumentShape, ExtrasMode,
    FrontmatterAnchor, FrontmatterLocation, GuardIndex, GuardPath, HeaderLevel, Heading,
    HeadingLocation, Matcher, OrderEntryPath, OrderIndex, RuleIndex, RulePath, Schema, SchemaNode,
    ScopeMode, ScopePath, Section, SectionRule, TextRange, TitleSlot, UpperBound,
};

use crate::locator::QueryLimitExceeded;

use super::constraints::{EvalCtx, Truth};
use super::diagnostic::{
    Diagnostic, DiagnosticId, DiagnosticLocation, DiagnosticTarget, FrontmatterBlock,
    FrontmatterLineRange, HeaderPath, InvolvedHeader, ValidationOperationalError,
};
use super::frontmatter_values::{self, CaptureFailure, CaptureProblem, FrontmatterValues};
use super::prepare::{PreparedRule, ValidationPlan};
use super::value_order;

/// Validates one parsed document against a schema and its prepared plan.
///
/// # Errors
///
/// Returns the operational failure of §4.6 when a frontmatter query cannot be
/// evaluated within this implementation's resource limit. The walk stops
/// there and its diagnostics are discarded: §11.5 admits a complete verdict or
/// none, and a set built up to the point of failure would read as the former.
pub(super) fn validate_document(
    schema: &Schema,
    document: &Document,
    plan: &ValidationPlan,
) -> Result<Vec<Diagnostic>, ValidationOperationalError> {
    let mut validator = Validator::new(schema, document);
    validator
        .run(plan)
        .map_err(|limit| ValidationOperationalError::new(limit.to_string()))
}

#[cfg(test)]
pub(super) fn validation_work_count(
    schema: &Schema,
    document: &Document,
    plan: &ValidationPlan,
) -> Result<WorkCounter, ValidationOperationalError> {
    let mut validator = Validator::new(schema, document);
    validator
        .run(plan)
        .map_err(|limit| ValidationOperationalError::new(limit.to_string()))?;
    Ok(validator.work)
}

#[cfg(test)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct WorkCounter {
    pub(super) guard_matcher_evaluations: usize,
    pub(super) accepting_matcher_evaluations: usize,
    pub(super) extras_classifications: usize,
}

struct Validator<'a> {
    schema: &'a Schema,
    document: &'a Document,
    diagnostics: Vec<Diagnostic>,
    #[cfg(test)]
    work: WorkCounter,
}

struct BindScopeInput<'a, 'd> {
    sections: &'a [PathedSection<'d>],
    scope: &'a DeclaredScope,
    prepared_rules: &'a [PreparedRule],
    prepared_guards: &'a [super::prepare::PreparedMatcher],
    schema_scope: &'a ScopePath,
    parent: Option<&'d Heading>,
    parent_path: &'a HeaderPath,
}

struct ValueOrderCheck<'a, 'd> {
    rules: &'a [SectionRule],
    occurrences: &'a [BoundSection<'d>],
    schema_scope: &'a ScopePath,
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
            #[cfg(test)]
            work: WorkCounter::default(),
        }
    }

    fn run(&mut self, plan: &ValidationPlan) -> Result<Vec<Diagnostic>, QueryLimitExceeded> {
        self.validate_frontmatter(plan.frontmatter.as_ref());
        // §8 evaluates the declared frontmatter captures once, straight after
        // the block's own checks and before the outline walk. §2.3 keeps this
        // independent of `frontmatter.schema`: a JSON Schema failure leaves a
        // valid resolved mapping, so the captures are still read.
        let values = self.evaluate_frontmatter_captures();
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
        let root_level = match &self.schema.document {
            DocumentShape::Outline(_) => 0,
            DocumentShape::Title(TitleSlot::Forbidden { .. }) => 1,
            DocumentShape::Title(
                TitleSlot::Spelled { .. } | TitleSlot::ImpliedBySections { .. },
            ) => u8::from(!has_h1),
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
        match &self.schema.document {
            DocumentShape::Outline(scope) => {
                self.validate_outline_root(&top, scope, plan, &values)?
            }
            DocumentShape::Title(title) => {
                self.validate_sugar_root(&top, has_h1, title, plan, &values)?
            }
        }
        Ok(std::mem::take(&mut self.diagnostics))
    }

    /// Binds the general form's outline scope: `h1` rules on the virtual root.
    ///
    /// Ordinary exhaustive scope semantics apply, and top-level constraints
    /// attach here, targeting the document since the virtual root has no
    /// header to name.
    fn validate_outline_root(
        &mut self,
        top: &[PathedSection<'a>],
        scope: &'a DeclaredScope,
        plan: &ValidationPlan,
        frontmatter: &FrontmatterValues<'a>,
    ) -> Result<(), QueryLimitExceeded> {
        let schema = self.schema;
        let admitted = admitted_at_root(top, HeaderLevel::H1, schema.options.allow_skipped_levels);
        let root_scope = ScopePath(Vec::new());
        let root_path = HeaderPath::default();
        let root = self.bind_scope(BindScopeInput {
            sections: &admitted,
            scope,
            prepared_rules: &plan.rules,
            prepared_guards: &plan.guards,
            schema_scope: &root_scope,
            parent: None,
            parent_path: &root_path,
        });
        self.validate_constraints(
            EvalCtx {
                current: &root,
                current_rules: &scope.rules,
                root: &root,
                root_rules: &scope.rules,
                frontmatter,
                queries: &plan.queries,
                match_case: schema.options.match_case,
            },
            &scope.constraints,
            &root_scope,
            None,
            &root_path,
        )
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
        title: &'a crate::TitleSlot,
        plan: &ValidationPlan,
        frontmatter: &FrontmatterValues<'a>,
    ) -> Result<(), QueryLimitExceeded> {
        let schema = self.schema;
        let (matcher, title_children) = match title {
            TitleSlot::Forbidden { children } => {
                // The headless scope: the virtual root stands in at level 1
                // and `sections` binds the document's top-level `h2`s.
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
                let admitted =
                    admitted_at_root(top, HeaderLevel::H2, schema.options.allow_skipped_levels);
                return self.bind_sugar_sections(&admitted, children, frontmatter, plan, None);
            }
            TitleSlot::Spelled { matcher, children } => (matcher, children),
            TitleSlot::ImpliedBySections { children } => (&Matcher::Any, children),
        };
        if !has_h1 {
            self.emit(
                Diagnostic {
                    id: DiagnosticId::MissingTitle,
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
            let admitted =
                admitted_at_root(top, HeaderLevel::H2, schema.options.allow_skipped_levels);
            return self.bind_sugar_sections(&admitted, title_children, frontmatter, plan, None);
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
                if plan
                    .title
                    .as_ref()
                    .is_some_and(|matcher| !matcher.matches(&pathed.section.heading.text))
                {
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
            let mut children = child_sections(
                occurrence.section,
                &occurrence.path,
                schema.options.allow_skipped_levels,
            );
            if index == 0 && !admitted_strays.is_empty() {
                let mut merged = std::mem::take(&mut admitted_strays);
                merged.extend(children);
                children = merged;
            }
            let owner = attribute.then_some((&occurrence.section.heading, &occurrence.path));
            self.bind_sugar_sections(&children, title_children, frontmatter, plan, owner)?;
        }
        Ok(())
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
        child_scope: &'a ChildScope,
        frontmatter: &FrontmatterValues<'a>,
        plan: &ValidationPlan,
        owner: Option<(&'a Heading, &HeaderPath)>,
    ) -> Result<(), QueryLimitExceeded> {
        let scope = ScopePath(Vec::new());
        let (parent, path) = match owner {
            Some((heading, path)) => (Some(heading), path.clone()),
            None => (None, HeaderPath::default()),
        };
        let bound = self.bind_child_scope(
            sections,
            child_scope,
            &plan.rules,
            &plan.guards,
            &scope,
            parent,
            &path,
        );
        self.validate_constraints(
            EvalCtx {
                current: &bound,
                current_rules: child_scope.rules(),
                // `$.` refs in a sugar schema resolve against the `sections`
                // scope, as they always have — here, this instance of it.
                root: &bound,
                root_rules: child_scope.rules(),
                frontmatter,
                queries: &plan.queries,
                match_case: self.schema.options.match_case,
            },
            child_scope.constraints(),
            &scope,
            parent,
            &path,
        )
    }

    /// Evaluates every declared frontmatter capture once (§2.3).
    ///
    /// The retained states are the record §6.3 makes dependent checks read:
    /// the diagnostics below may be filtered, and filtering one must not
    /// change what `fm.<name>` concluded about the value it names.
    fn evaluate_frontmatter_captures(&mut self) -> FrontmatterValues<'a> {
        let (values, problems) = frontmatter_values::evaluate(
            self.schema.frontmatter.captures(),
            &self.document.frontmatter,
            self.schema.frontmatter.is_required(),
        );
        for problem in problems {
            self.emit_capture_problem(&values, problem);
        }
        values
    }

    /// Places one capture primary at the entry it is about (§6.1, §6.2).
    fn emit_capture_problem(&mut self, values: &FrontmatterValues<'_>, problem: CaptureProblem) {
        let CaptureProblem {
            name,
            pointer,
            anchor,
            reason,
        } = problem;
        let (id, message) = match reason {
            CaptureFailure::Invalid {
                value_type,
                source,
                failure,
            } => (
                DiagnosticId::InvalidValue,
                format!(
                    "frontmatter capture `{name}` is not a valid `{}`: {}",
                    value_type.as_str(),
                    value_failure_reason(value_type, &source, &failure)
                ),
            ),
            CaptureFailure::Missing => (
                DiagnosticId::MissingValue,
                format!("frontmatter capture `{name}` is required but selects no value"),
            ),
        };
        let node = SchemaNode::FrontmatterCapture(name);
        // No reference: §6.2 attributes a capture diagnostic to its
        // declaration, and the schema node above already names that
        // declaration exactly.
        if let Some(diagnostic) =
            values.entry_diagnostic(id, pointer, &anchor, node, message, Vec::new())
        {
            self.emit(diagnostic, None, false);
        }
    }

    fn validate_frontmatter(&mut self, validator: Option<&jsonschema::Validator>) {
        let required = self.schema.frontmatter.is_required();
        let forbidden = self.schema.frontmatter.is_forbidden();
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
            scope,
            prepared_rules,
            prepared_guards,
            schema_scope,
            parent,
            parent_path,
        } = input;
        let rules = &scope.rules;
        let allow_skipped = self.schema.options.allow_skipped_levels;
        let mut retained = Vec::new();
        for pathed in sections {
            let guarded = self.first_matching_guard(prepared_guards, &pathed.section.heading.text);
            if let Some(guard_index) = guarded {
                self.emit_present(
                    DiagnosticId::NotAllowed,
                    pathed.path.clone(),
                    &pathed.section.heading,
                    Some(SchemaNode::Guard(GuardPath {
                        scope: schema_scope.clone(),
                        index: GuardIndex(guard_index),
                    })),
                    "a prohibition guard rejects this section",
                );
            } else {
                retained.push(pathed);
            }
        }
        let columns = rules.len();
        let mut matrix = Vec::new();
        for pathed in &retained {
            for rule in prepared_rules {
                #[cfg(test)]
                {
                    self.work.accepting_matcher_evaluations =
                        self.work.accepting_matcher_evaluations.saturating_add(1);
                }
                matrix.push(rule.matcher.matches(&pathed.section.heading.text));
            }
        }
        if scope.extras == ExtrasMode::Anywhere {
            let retained_rows = (0..retained.len())
                .map(|heading_index| {
                    #[cfg(test)]
                    {
                        self.work.extras_classifications =
                            self.work.extras_classifications.saturating_add(1);
                    }
                    let start = heading_index.saturating_mul(columns);
                    let end = heading_index.saturating_add(1).saturating_mul(columns);
                    matrix
                        .get(start..end)
                        .is_some_and(|row| row.iter().any(|value| *value))
                })
                .collect::<Vec<_>>();
            let mut row = 0usize;
            retained.retain(|_| {
                let keep = retained_rows.get(row).copied().unwrap_or(false);
                row = row.saturating_add(1);
                keep
            });
            matrix = matrix
                .chunks(columns.max(1))
                .zip(retained_rows)
                .filter(|(_, keep)| *keep)
                .flat_map(|(row, _)| row.iter().copied())
                .collect();
        }
        let assignment = match scope.mode {
            ScopeMode::Ordered => super::sequence::assign(rules, &matrix, retained.len()),
            ScopeMode::Unordered => {
                let mut assigned = vec![None; retained.len()];
                let mut counts = vec![0; rules.len()];
                for (heading_index, slot) in assigned.iter_mut().enumerate() {
                    if let Some(rule_index) = (0..rules.len()).find(|rule_index| {
                        matrix
                            .get(
                                heading_index
                                    .saturating_mul(columns)
                                    .saturating_add(*rule_index),
                            )
                            .copied()
                            .unwrap_or(false)
                    }) {
                        *slot = Some(rule_index);
                        if let Some(count) = counts.get_mut(rule_index) {
                            *count += 1;
                        }
                    }
                }
                super::sequence::Assignment {
                    rules: assigned,
                    counts,
                    accepted: true,
                    recovery_cost: super::sequence::RecoveryCost {
                        unassigned: 0,
                        wildcard: 0,
                    },
                }
            }
        };
        let mut occurrences = Vec::new();
        for (heading_index, pathed) in retained.iter().enumerate() {
            let section = pathed.section;
            let path = pathed.path.clone();
            let Some(rule_index) = assignment.rules.get(heading_index).copied().flatten() else {
                let row_matches = matrix.get(
                    heading_index.saturating_mul(columns)
                        ..heading_index.saturating_add(1).saturating_mul(columns),
                );
                let misplaced = scope.mode == ScopeMode::Ordered
                    && row_matches.is_some_and(|row| row.iter().any(|value| *value));
                self.emit_present(
                    if misplaced {
                        DiagnosticId::MisplacedSection
                    } else {
                        DiagnosticId::UnexpectedSection
                    },
                    path,
                    &section.heading,
                    scope_owner_node(self.schema, schema_scope),
                    if misplaced {
                        "the section matches a rule but cannot occupy its ordered phase"
                    } else {
                        "the section matches no accepting rule"
                    },
                );
                continue;
            };
            let (Some(rule), Some(prepared_rule)) =
                (rules.get(rule_index), prepared_rules.get(rule_index))
            else {
                continue;
            };
            // §8 parses a matched header's declared captures before visiting
            // its children, and §3.3 makes that the moment every declared
            // capture is parsed — the one place a value is read, so that
            // ordering, locators, and dependency suppression all read the
            // same result rather than each recomputing it.
            let captures = self.bind_rule_captures(
                rule,
                prepared_rule,
                section,
                &path,
                &rule_path(schema_scope, rule_index),
            );
            let child_refs = child_sections(section, &path, allow_skipped);
            let mut child_scope_path = schema_scope.clone();
            child_scope_path.0.push(RuleIndex(rule_index));
            let child = self.bind_child_scope(
                &child_refs,
                &rule.children,
                &prepared_rule.sections,
                &prepared_rule.guards,
                &child_scope_path,
                Some(&section.heading),
                &path,
            );
            occurrences.push(BoundSection {
                rule_index,
                section,
                path,
                child,
                captures,
            });
        }

        for (rule_index, rule) in rules.iter().enumerate() {
            let cardinality = rule.cardinality;
            let count = assignment
                .counts
                .get(rule_index)
                .copied()
                .unwrap_or_default();
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
        // §8 runs the typed order entries after cardinality and after the
        // scope's own rule order, and §3.8 makes this mechanism "independent
        // of the across-rule ordering in §3.7": it runs whether or not the
        // scope is ordered, and it keeps every occurrence the cardinality
        // check just complained about.
        self.validate_value_order(ValueOrderCheck {
            rules,
            occurrences: &occurrences,
            schema_scope,
        });
        // §4.4's runtime singularity, taken from the raw match counts. It is
        // recorded here, beside the cardinality check that has just read the
        // same counts, precisely so that a locator descent never has to ask
        // whether a `too-many-sections` diagnostic survived §6.3 filtering.
        let singular = assignment.counts.iter().map(|count| *count <= 1).collect();
        BoundScope {
            occurrences,
            singular,
        }
    }

    fn first_matching_guard(
        &mut self,
        guards: &[super::prepare::PreparedMatcher],
        heading: &str,
    ) -> Option<usize> {
        for (index, guard) in guards.iter().enumerate() {
            #[cfg(test)]
            {
                self.work.guard_matcher_evaluations =
                    self.work.guard_matcher_evaluations.saturating_add(1);
            }
            if guard.matches(heading) {
                return Some(index);
            }
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn bind_child_scope<'d>(
        &mut self,
        sections: &[PathedSection<'d>],
        child_scope: &ChildScope,
        prepared_rules: &[PreparedRule],
        prepared_guards: &[super::prepare::PreparedMatcher],
        schema_scope: &ScopePath,
        parent: Option<&'d Heading>,
        parent_path: &HeaderPath,
    ) -> BoundScope<'d> {
        match child_scope {
            ChildScope::Declared(scope) => self.bind_scope(BindScopeInput {
                sections,
                scope,
                prepared_rules,
                prepared_guards,
                schema_scope,
                parent,
                parent_path,
            }),
            ChildScope::Omitted => BoundScope {
                occurrences: Vec::new(),
                singular: Vec::new(),
            },
            ChildScope::GuardsOnly(_) => {
                for pathed in sections {
                    if let Some(index) =
                        self.first_matching_guard(prepared_guards, &pathed.section.heading.text)
                    {
                        self.emit_present(
                            DiagnosticId::NotAllowed,
                            pathed.path.clone(),
                            &pathed.section.heading,
                            Some(SchemaNode::Guard(GuardPath {
                                scope: schema_scope.clone(),
                                index: GuardIndex(index),
                            })),
                            "a prohibition guard rejects this section",
                        );
                    }
                }
                BoundScope {
                    occurrences: Vec::new(),
                    singular: Vec::new(),
                }
            }
        }
    }

    /// Parses every capture the matched rule declares (§3.3, §2.4).
    ///
    /// One pass over the declarations, reading each named group out of one
    /// match of the matcher input. Each substring is parsed into an owned
    /// value immediately, so no regex result outlives this call, and the
    /// outcome — value or failure — is retained for every declaration whether
    /// or not anything reads it. §6.3 puts dependency suppression before
    /// `outlint-disable` filtering, so the retained state is the record a
    /// dependent check consults; whether the diagnostic below survives
    /// filtering says nothing about it.
    fn bind_rule_captures(
        &mut self,
        rule: &SectionRule,
        prepared_rule: &PreparedRule,
        section: &Section,
        path: &HeaderPath,
        rule_path: &RulePath,
    ) -> BTreeMap<CaptureName, BoundValueState> {
        if rule.captures.is_empty() {
            return BTreeMap::new();
        }
        let groups = prepared_rule.matcher.named_groups(&section.heading.text);
        let mut bound = BTreeMap::new();
        for (name, declaration) in &rule.captures {
            let value_type = declaration.value_type();
            let state = match groups.get(name.as_str()) {
                Some(source) => match parse_header(value_type, source) {
                    Ok(value) => BoundValueState::Valid(value),
                    Err(failure) => {
                        // §6.2: `invalid-value` from a rule capture targets
                        // the header whose capture is invalid and is
                        // "attributed to that capture declaration".
                        let message = rule_capture_message(name, value_type, source, &failure);
                        self.emit_present(
                            DiagnosticId::InvalidValue,
                            path.clone(),
                            &section.heading,
                            Some(SchemaNode::Capture(CapturePath {
                                rule: rule_path.clone(),
                                name: name.clone(),
                            })),
                            &message,
                        );
                        BoundValueState::Invalid
                    }
                },
                // Unreachable by construction: §2.2 admits only
                // mandatory-participation groups, and the loader refuses a
                // declaration whose group is enclosed by an alternation or a
                // zero-minimum repetition. Were that ever to change, nothing
                // was looked at here, so nothing is concluded — which every
                // dependent check reads as a reason to stand down rather than
                // as a value.
                None => BoundValueState::Unevaluated,
            };
            bound.insert(name.clone(), state);
        }
        bound
    }

    /// Reports every adjacent pair that violates a rule's `order` (§3.8).
    ///
    /// Which pairs those are is [`value_order`]'s answer; this places the
    /// diagnostic §6.2 asks for: targeted and anchored at the pair's second
    /// header, listing exactly the first and second headers in that order,
    /// and attributed to the order entry rather than to the rule. Anchoring
    /// at the second header is also what makes the pair's own
    /// `outlint-disable` line the one that hides it.
    fn validate_value_order(&mut self, check: ValueOrderCheck<'_, '_>) {
        let ValueOrderCheck {
            rules,
            occurrences,
            schema_scope,
        } = check;
        for violation in value_order::violations(rules, occurrences) {
            let involved = [violation.first, violation.second]
                .into_iter()
                .map(|occurrence| InvolvedHeader {
                    path: occurrence.path.clone(),
                    location: heading_location(&occurrence.section.heading.location),
                })
                .collect();
            self.emit(
                Diagnostic {
                    id: DiagnosticId::OrderViolation,
                    target: DiagnosticTarget::Header(violation.second.path.clone()),
                    location: heading_location(&violation.second.section.heading.location),
                    schema_node: Some(SchemaNode::OrderEntry(OrderEntryPath {
                        rule: rule_path(schema_scope, violation.rule_index),
                        order_index: OrderIndex(violation.order_index),
                    })),
                    involved_headers: involved,
                    references: Vec::new(),
                    message: value_order::violation_message(&violation),
                },
                Some(&violation.second.section.heading),
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
        if count < cardinality.min() as usize {
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
                        cardinality.min()
                    ),
                },
                None,
                false,
            );
        }
        let UpperBound::Bounded(max) = cardinality.max() else {
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
    ) -> Result<(), QueryLimitExceeded> {
        for (index, constraint) in constraints.iter().enumerate() {
            let node = ConstraintPath {
                scope: schema_scope.clone(),
                index: ConstraintIndex(index),
            };
            let evaluation = eval.constraint_evaluation(constraint, &node)?;
            // The operands' own primaries stand whatever the constraint
            // concluded, and they are emitted before that conclusion is
            // acted on: §4.6 has one node both produce `invalid-value` and
            // suppress its constraint, so neither replaces the other.
            for pending in evaluation.pending {
                self.emit(pending, None, false);
            }
            match evaluation.truth {
                // §5.3: a suppressed constraint "produces no constraint
                // diagnostic". Its operands' primaries have already gone out.
                Truth::Satisfied | Truth::Suppressed => continue,
                Truth::Unsatisfied => {}
            }
            let id = constraint_id(constraint);
            let involved = evaluation
                .occurrences
                .into_iter()
                .map(|occurrence| InvolvedHeader {
                    path: occurrence.path.clone(),
                    location: heading_location(&occurrence.section.heading.location),
                })
                .collect();
            self.emit(
                Diagnostic {
                    id,
                    // The scope the constraint is attached to. The virtual
                    // document root has no header path; the sugar's single-h1
                    // voice likewise attributes its sections scope to the
                    // document (§6.2).
                    target: match parent {
                        Some(_) => DiagnosticTarget::Header(parent_path.clone()),
                        None => DiagnosticTarget::Document,
                    },
                    location: parent
                        .map_or_else(root_location, |heading| heading_location(&heading.location)),
                    schema_node: Some(SchemaNode::Constraint(node)),
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
                    current_rules: rule.children.rules(),
                    root: eval.root,
                    root_rules: eval.root_rules,
                    frontmatter: eval.frontmatter,
                    queries: eval.queries,
                    match_case: eval.match_case,
                },
                rule.children.constraints(),
                &child_schema_scope,
                Some(&occurrence.section.heading),
                &occurrence.path,
            )?;
        }
        Ok(())
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
pub(super) struct BoundScope<'d> {
    pub(super) occurrences: Vec<BoundSection<'d>>,
    /// Whether each rule of this scope, in rule-list order, matched at most
    /// one header here.
    ///
    /// §4.4 makes an unnarrowed non-terminal locator step depend on exactly
    /// this: a statically singular rule that matched several headers in a
    /// cardinality-violating concrete scope suppresses every constraint
    /// evaluation descending through it. The fact is computed from raw match
    /// counts as the scope is bound, which is what puts it before the §6.3
    /// filtering of the `too-many-sections` diagnostic that reports the same
    /// counts.
    singular: Vec<bool>,
}

impl BoundScope<'_> {
    /// Whether the rule at `rule_index` matched at most one header here.
    ///
    /// A rule index this scope does not have cannot have matched anything, so
    /// it is singular; treating the unknown as plural would suppress a
    /// descent that never depended on anything.
    pub(super) fn is_singular(&self, rule_index: usize) -> bool {
        self.singular.get(rule_index).copied().unwrap_or(true)
    }
}

#[derive(Debug)]
pub(super) struct BoundSection<'d> {
    pub(super) rule_index: usize,
    pub(super) section: &'d Section,
    path: HeaderPath,
    pub(super) child: BoundScope<'d>,
    /// What each capture this section's rule declares evaluated to.
    ///
    /// Empty for a rule that declares none. It is stored on the bound section
    /// rather than recomputed per check because §3.8 ordering, value
    /// locators, and dependency suppression all read the same result and must
    /// all read the *same* one.
    pub(super) captures: BTreeMap<CaptureName, BoundValueState>,
}

/// What one capture evaluated to for one bound section.
///
/// This is the validator's own record, deliberately independent of the
/// diagnostics it produces. §6.3 decides dependency suppression "before these
/// comments filter diagnostics", so a dependent check must ask this state
/// whether its input held — never whether an `invalid-value` diagnostic
/// survived `outlint-disable`. Deriving suppression from emitted diagnostics
/// would make hiding a diagnostic re-enable the check that depended on it,
/// which §6.3 forbids in as many words.
#[derive(Debug)]
pub(super) enum BoundValueState {
    /// The source parsed to a value of the declared type.
    Valid(TypedValue),
    /// The source failed the type's lexical, kind, calendar, SemVer, or bound
    /// requirement, so a primary `invalid-value` reason exists whether or not
    /// its diagnostic is later filtered.
    ///
    /// The reason itself is not retained: it is spent on that diagnostic at
    /// the moment of failure, and what every dependent check asks of this
    /// state is only whether a value came out of it.
    Invalid,
    /// The capture was evaluated and selected no usable value. For a
    /// `required: true` frontmatter capture this is what `missing-value`
    /// reports; for an optional one it is ordinary, valid absence.
    Absent,
    /// The capture was never evaluated because its prerequisite source was
    /// itself absent or invalid — an absent frontmatter block, say, or a
    /// header that did not match. Distinct from [`Self::Absent`]: nothing was
    /// looked at, so nothing can be concluded about the value.
    Unevaluated,
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

/// One bound section's bindable children.
///
/// §3.1: "A skipping subtree under the default of §1.5 is in no scope, so
/// §3.2 through §3.8 never see it." A child more than one level below its
/// parent is therefore dropped here rather than matched and then excused,
/// which is what makes §1.5's "takes part in no rule" true of every rule
/// mechanism at once: it matches none, counts toward no cardinality,
/// satisfies no constraint locator, exports no capture, and joins no order
/// sequence. Nor does anything below it, because the subtree is never
/// entered — and dropping it is also what keeps a closed scope from calling
/// it `unexpected-section`, which would be that scope having an opinion
/// about a header §3.1 says it never sees.
///
/// §1.5 still reports it: the skipped-level walk covers the whole document
/// and consults no scope, and its own recursion is what keeps a well-nested
/// descendant from being blamed for its ancestor's misplacement.
///
/// With `allow_skipped_levels` there is nothing to drop — "the skip is
/// admitted: the header becomes an ordinary member of the enclosing scope
/// and is matched against that scope's rules like any sibling".
///
/// The virtual root has the same rule with its own stand-in level; see
/// [`admitted_at_root`], which differs only in having no parent header to
/// take the level from.
fn child_sections<'d>(
    section: &'d Section,
    path: &HeaderPath,
    allow_skipped: bool,
) -> Vec<PathedSection<'d>> {
    let child_level = section.heading.level as u8 + 1;
    section
        .children
        .iter()
        .filter(|child| {
            let level = child.heading.level as u8;
            level == child_level || (allow_skipped && level > child_level)
        })
        .map(|child| PathedSection {
            section: child,
            path: appended_path(path, &child.heading.diagnostic_text),
        })
        .collect()
}

pub(super) fn root_location() -> DiagnosticLocation {
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

fn scope_owner_node(schema: &Schema, scope: &ScopePath) -> Option<SchemaNode> {
    scope.0.split_last().map_or_else(
        || schema.is_sugar().then_some(SchemaNode::Title),
        |(index, parent)| {
            Some(SchemaNode::Rule(RulePath {
                scope: ScopePath(parent.to_vec()),
                index: *index,
            }))
        },
    )
}

fn matcher_label(matcher: &Matcher) -> String {
    match matcher {
        Matcher::Exact(text) => text.0.clone(),
        Matcher::Glob(pattern) => pattern.0.clone(),
        Matcher::Regex(pattern) => format!("/{}/", pattern.0),
        Matcher::Any => "*".into(),
    }
}

/// The `invalid-value` wording for a rule capture (§6.2).
///
/// §6.2 requires the message to "identify the expected type and the
/// responsible capture", so both open it; the reason follows.
fn rule_capture_message(
    name: &CaptureName,
    value_type: ValueType,
    source: &str,
    failure: &ParseFailure,
) -> String {
    format!(
        "capture `{name}` is not a valid `{}`: {}",
        value_type.as_str(),
        value_failure_reason(value_type, source, failure)
    )
}

/// Why one source failed its declared type, in §2.4's terms.
///
/// The reason is a fact about the value and is the same whichever source
/// supplied it; only the sentence around it says where the value came from.
/// `source` is the exact spelling that was read, and is unused by the kind
/// failure, which is about the shape of the node rather than its text.
fn value_failure_reason(value_type: ValueType, source: &str, failure: &ParseFailure) -> String {
    match failure {
        ParseFailure::KindMismatch { expected, actual } => {
            // §2.4: "diagnostics SHOULD suggest quoting this common mistake"
            // — the unquoted `version: 2.2` that reads as a YAML float where
            // a string-kinded type was declared.
            let hint = if *expected == ResolvedYamlKind::String
                && matches!(actual, ResolvedYamlKind::Integer | ResolvedYamlKind::Float)
            {
                ", so quote it to make it a YAML string"
            } else {
                ""
            };
            format!(
                "the value is a YAML {} where a `{}` needs a YAML {}{hint}",
                yaml_kind_name(*actual),
                value_type.as_str(),
                yaml_kind_name(*expected)
            )
        }
        ParseFailure::Lexical => format!(
            "`{source}` does not have the form a `{}` is written in",
            value_type.as_str()
        ),
        ParseFailure::BoundOverflow { component } => match component {
            BoundComponent::Int => {
                format!("`{source}` is outside the signed 64-bit range an `int` allows")
            }
            BoundComponent::SemverMajor => {
                format!("the major identifier of `{source}` is outside the unsigned 64-bit range")
            }
            BoundComponent::SemverMinor => {
                format!("the minor identifier of `{source}` is outside the unsigned 64-bit range")
            }
            BoundComponent::SemverPatch => {
                format!("the patch identifier of `{source}` is outside the unsigned 64-bit range")
            }
            BoundComponent::SemverPrerelease { index } => format!(
                "pre-release identifier {} of `{source}` is outside the unsigned 64-bit range",
                index + 1
            ),
            BoundComponent::DottedComponent { index } => format!(
                "component {} of `{source}` is outside the unsigned 32-bit range",
                index + 1
            ),
        },
        ParseFailure::InvalidDate => {
            format!("`{source}` names no day in the proleptic Gregorian calendar")
        }
        // §2.4: a build-metadata failure "MUST identify that suffix as the
        // reason", so the suffix is named rather than described.
        ParseFailure::BuildMetadata { suffix } => format!(
            "`{source}` carries the build metadata `{suffix}`, which a `semver` does not admit"
        ),
    }
}

/// The §1.6 YAML kind's name as a diagnostic spells it.
fn yaml_kind_name(kind: ResolvedYamlKind) -> &'static str {
    match kind {
        ResolvedYamlKind::Null => "null",
        ResolvedYamlKind::Boolean => "boolean",
        ResolvedYamlKind::Integer => "integer",
        ResolvedYamlKind::Float => "finite decimal",
        ResolvedYamlKind::String => "string",
        ResolvedYamlKind::Sequence => "sequence",
        ResolvedYamlKind::Mapping => "mapping",
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
