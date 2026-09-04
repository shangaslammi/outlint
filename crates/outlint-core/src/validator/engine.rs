//! The validation walk: level admission, scope binding, and diagnostics.

use std::collections::BTreeMap;

use crate::typed_value::{ParseFailure, TypedValue};
use crate::{
    ByteOffset, CaptureName, Cardinality, Constraint, ConstraintIndex, ConstraintPath, Document,
    DocumentFrontmatter, FrontmatterAnchor, FrontmatterLocation, HeaderLevel, Heading,
    HeadingLocation, Matcher, OutlineProvenance, RuleIndex, RuleOutcome, Schema, SchemaNode,
    ScopePath, Section, SectionRule, TextRange, UpperBound,
};

use super::constraints::EvalCtx;
use super::diagnostic::{
    Diagnostic, DiagnosticId, DiagnosticLocation, DiagnosticTarget, FrontmatterBlock,
    FrontmatterLineRange, HeaderPath, InvolvedHeader,
};
use super::prepare::{PreparedRule, ValidationPlan};

/// Validates one parsed document against a schema and its prepared plan.
pub(super) fn validate_document(
    schema: &Schema,
    document: &Document,
    plan: &ValidationPlan,
) -> Vec<Diagnostic> {
    Validator::new(schema, document).run(plan)
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
                        // A missing `h1` belongs to the document root's scope,
                        // whose virtual parent has no header path.
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
            // Already the section's complete ancestor chain. Do not rebuild it
            // from the diagnostic attribution path, which is intentionally
            // empty under the sugar's single-`h1` document voice.
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
                // Populated by the capture-extraction lane; nothing is
                // evaluated here.
                captures: BTreeMap::new(),
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
    /// rules do not participate in the order pairing, while unmatched headers
    /// are unconstrained by ordering. Each violated pair is one `ordered`
    /// diagnostic, so that a
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
pub(super) struct BoundScope<'d> {
    pub(super) occurrences: Vec<BoundSection<'d>>,
}

#[derive(Debug)]
pub(super) struct BoundSection<'d> {
    pub(super) rule_index: usize,
    pub(super) section: &'d Section,
    path: HeaderPath,
    pub(super) child: BoundScope<'d>,
    /// What each capture this section's rule declares evaluated to.
    ///
    /// Empty here and populated by the lane that extracts capture values:
    /// nothing is extracted, parsed, or compared yet. It is stored on the
    /// bound section rather than recomputed per check because §3.8 ordering,
    /// value locators, and dependency suppression all read the same result
    /// and must all read the *same* one.
    #[allow(dead_code)]
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
#[allow(dead_code)]
pub(super) enum BoundValueState {
    /// The source parsed to a value of the declared type.
    Valid(TypedValue),
    /// A primary `invalid-value` reason exists, whether or not its diagnostic
    /// is later filtered.
    Invalid(ParseFailure),
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
        Constraint::Ordered(_) | Constraint::OrderedLocators(_) => DiagnosticId::Ordered,
    }
}
