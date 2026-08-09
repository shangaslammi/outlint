//! Pure validation of a parsed Markdown outline against a normalized schema.
//!
//! Validation is deliberately separate from parsing and IO: callers can load
//! and parse fixture text once, then pass only values to [`validate`].

use regex::RegexBuilder;

use crate::{
    AtLeastTwo, ByteOffset, Cardinality, Constraint, ConstraintIndex, ConstraintPath, Document,
    FrontmatterRef, Heading, HeadingLocation, Matcher, NonEmpty, Proposition, RefAnchor, RuleIndex,
    RuleOutcome, RuleRef, Schema, SchemaNode, ScopePath, Section, SectionRule, TextRange,
    UpperBound,
};

/// A stable identifier from the diagnostic vocabulary in specification §6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticId {
    SkippedLevel,
    NotAllowed,
    UnexpectedSection,
    MissingSection,
    TooFewSections,
    TooManySections,
    MissingTitle,
    MissingFrontmatter,
    ForbiddenFrontmatter,
    InvalidFrontmatter,
    FrontmatterSchema,
    OneOf,
    AnyOf,
    AtMostOne,
    AllOrNone,
    Requires,
    Conflicts,
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
    pub line: u32,
    /// One-based byte column.
    pub column: u32,
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
        reference: RuleRef,
        matcher: Matcher,
    },
    /// A document-level frontmatter proposition.
    Frontmatter(FrontmatterRef),
}

/// One validation violation, with both document and schema-side anchors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Stable diagnostic category.
    pub id: DiagnosticId,
    /// Path to the concrete or expected header involved.
    pub path: HeaderPath,
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

/// Validates an already parsed document against a normalized schema.
///
/// Frontmatter parsing is intentionally not simulated here. Until the document
/// model represents frontmatter, every [`Proposition::Frontmatter`] evaluates
/// to false. This still gives correct behavior for all constraints containing
/// only rule references and for implications guarded by absent frontmatter.
pub fn validate(schema: &Schema, document: &Document) -> Vec<Diagnostic> {
    Validator::new(schema, document).run()
}

/// Tests a normalized matcher using the schema's Unicode-aware case policy.
///
/// Invalid regex bodies in manually constructed schemas simply do not match;
/// normally the loader prevents such values from reaching this boundary.
pub fn matcher_matches(matcher: &Matcher, text: &str, match_case: bool) -> bool {
    match matcher {
        Matcher::Any => true,
        Matcher::Exact(exact) => regex_matches(&regex::escape(&exact.0), text, match_case),
        Matcher::Glob(glob) => {
            let body = glob
                .0
                .split('*')
                .map(regex::escape)
                .collect::<Vec<_>>()
                .join(".*");
            regex_matches(&body, text, match_case)
        }
        Matcher::Regex(pattern) => regex_matches(&pattern.0, text, match_case),
    }
}

fn regex_matches(body: &str, text: &str, match_case: bool) -> bool {
    let anchored = format!(r"\A(?:{body})\z");
    RegexBuilder::new(&anchored)
        .case_insensitive(!match_case)
        .build()
        .is_ok_and(|regex| regex.is_match(text))
}

struct Validator<'a> {
    schema: &'a Schema,
    document: &'a Document,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Validator<'a> {
    fn new(schema: &'a Schema, document: &'a Document) -> Self {
        Self {
            schema,
            document,
            diagnostics: Vec::new(),
        }
    }

    fn run(mut self) -> Vec<Diagnostic> {
        let mut all = Vec::new();
        collect_sections(&self.document.sections, &mut all);
        self.validate_title(&all);
        if !self.schema.options.allow_skipped_levels {
            self.validate_skipped_levels(&self.document.sections, None, &HeaderPath::default());
        }

        let root_sections = all
            .into_iter()
            .filter(|section| section.heading.level == self.schema.options.root_level)
            .collect::<Vec<_>>();
        let root = self.bind_scope(
            &root_sections,
            &self.schema.sections,
            false,
            &ScopePath(Vec::new()),
            None,
            &HeaderPath::default(),
        );
        self.validate_constraints(
            &root,
            &root,
            &self.schema.sections,
            &self.schema.constraints,
            &ScopePath(Vec::new()),
            None,
            &HeaderPath::default(),
        );
        self.diagnostics
    }

    fn validate_title(&mut self, sections: &[&Section]) {
        let Some(matcher) = &self.schema.title else {
            return;
        };
        let Some(title_level) = previous_level(self.schema.options.root_level) else {
            return;
        };
        let titles = sections
            .iter()
            .copied()
            .filter(|section| section.heading.level == title_level)
            .collect::<Vec<_>>();
        if titles.is_empty() {
            self.emit(
                Diagnostic {
                    id: DiagnosticId::MissingTitle,
                    path: HeaderPath(vec![matcher_label(matcher)]),
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
            if !matcher_matches(matcher, &title.heading.text, self.schema.options.match_case) {
                let path = HeaderPath(vec![title.heading.diagnostic_text.clone()]);
                self.emit(
                    Diagnostic {
                        id: DiagnosticId::NotAllowed,
                        path,
                        location: heading_location(&title.heading.location),
                        schema_node: Some(SchemaNode::Title),
                        involved_headers: Vec::new(),
                        references: Vec::new(),
                        message: "the title does not match the schema title matcher".into(),
                    },
                    Some(&title.heading),
                    true,
                );
            }
        }
        if let Some(excess) = titles.get(1) {
            let path = HeaderPath(vec![excess.heading.diagnostic_text.clone()]);
            self.emit(
                Diagnostic {
                    id: DiagnosticId::TooManySections,
                    path,
                    location: heading_location(&excess.heading.location),
                    schema_node: Some(SchemaNode::Title),
                    involved_headers: Vec::new(),
                    references: Vec::new(),
                    message: "the document has more than one title".into(),
                },
                Some(&excess.heading),
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
            if parent.is_some_and(|parent| {
                level_number(section.heading.level) > level_number(parent.level).saturating_add(1)
            }) {
                self.emit(
                    Diagnostic {
                        id: DiagnosticId::SkippedLevel,
                        path: path.clone(),
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

    #[allow(clippy::too_many_arguments)]
    fn bind_scope<'d>(
        &mut self,
        sections: &[&'d Section],
        rules: &[SectionRule],
        strict: bool,
        schema_scope: &ScopePath,
        parent: Option<&'d Heading>,
        parent_path: &HeaderPath,
    ) -> BoundScope<'d> {
        let mut counts = vec![0_u32; rules.len()];
        let mut occurrences = Vec::new();
        for section in sections {
            let path = appended_path(parent_path, &section.heading.diagnostic_text);
            let matched = rules.iter().enumerate().find(|(_, rule)| {
                matcher_matches(
                    &rule.matcher,
                    &section.heading.text,
                    self.schema.options.match_case,
                )
            });
            let Some((rule_index, rule)) = matched else {
                if strict {
                    self.emit_present(
                        DiagnosticId::UnexpectedSection,
                        path,
                        &section.heading,
                        None,
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
                *count = count.saturating_add(1);
            }
            let child_refs = section.children.iter().collect::<Vec<_>>();
            let mut child_scope_path = schema_scope.clone();
            child_scope_path.0.push(RuleIndex(rule_index));
            let child = self.bind_scope(
                &child_refs,
                &rule.sections,
                rule.strict,
                &child_scope_path,
                Some(&section.heading),
                &path,
            );
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
            self.validate_cardinality(
                cardinality,
                count,
                rule,
                rule_index,
                &occurrences,
                schema_scope,
                parent,
                parent_path,
            );
        }
        BoundScope { occurrences }
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_cardinality(
        &mut self,
        cardinality: Cardinality,
        count: u32,
        rule: &SectionRule,
        rule_index: usize,
        occurrences: &[BoundSection<'_>],
        schema_scope: &ScopePath,
        parent: Option<&Heading>,
        parent_path: &HeaderPath,
    ) {
        let schema_node = Some(SchemaNode::Rule(rule_path(schema_scope, rule_index)));
        if count < cardinality.min {
            let id = if count == 0 {
                DiagnosticId::MissingSection
            } else {
                DiagnosticId::TooFewSections
            };
            self.emit(
                Diagnostic {
                    id,
                    path: appended_path(parent_path, &matcher_label(&rule.matcher)),
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
        if count <= max {
            return;
        }
        let Some(excess_index) = usize::try_from(max).ok() else {
            return;
        };
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
                path: excess.path.clone(),
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

    #[allow(clippy::too_many_arguments)]
    fn validate_constraints<'d>(
        &mut self,
        scope: &'d BoundScope<'d>,
        root: &'d BoundScope<'d>,
        rules: &[SectionRule],
        constraints: &[Constraint],
        schema_scope: &ScopePath,
        parent: Option<&Heading>,
        parent_path: &HeaderPath,
    ) {
        for (index, constraint) in constraints.iter().enumerate() {
            if constraint_satisfied(constraint, scope, rules, root, &self.schema.sections) {
                continue;
            }
            let id = constraint_id(constraint);
            let involved =
                constraint_occurrences(constraint, scope, rules, root, &self.schema.sections)
                    .into_iter()
                    .map(|occurrence| InvolvedHeader {
                        path: occurrence.path.clone(),
                        location: heading_location(&occurrence.section.heading.location),
                    })
                    .collect();
            self.emit(
                Diagnostic {
                    id,
                    path: parent_path.clone(),
                    location: parent
                        .map_or_else(root_location, |heading| heading_location(&heading.location)),
                    schema_node: Some(SchemaNode::Constraint(ConstraintPath {
                        scope: schema_scope.clone(),
                        index: ConstraintIndex(index),
                    })),
                    involved_headers: involved,
                    references: constraint_references(constraint, rules, &self.schema.sections),
                    message: format!("the `{}` constraint is not satisfied", id.as_str()),
                },
                parent,
                true,
            );
        }

        for occurrence in &scope.occurrences {
            let Some(rule) = rules.get(occurrence.rule_index) else {
                continue;
            };
            let mut child_schema_scope = schema_scope.clone();
            child_schema_scope.0.push(RuleIndex(occurrence.rule_index));
            self.validate_constraints(
                &occurrence.child,
                root,
                &rule.sections,
                &rule.constraints,
                &child_schema_scope,
                Some(&occurrence.section.heading),
                &occurrence.path,
            );
        }
    }

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
                path,
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

fn collect_sections<'a>(sections: &'a [Section], output: &mut Vec<&'a Section>) {
    for section in sections {
        output.push(section);
        collect_sections(&section.children, output);
    }
}

fn previous_level(level: crate::HeaderLevel) -> Option<crate::HeaderLevel> {
    match level {
        crate::HeaderLevel::H1 => None,
        crate::HeaderLevel::H2 => Some(crate::HeaderLevel::H1),
        crate::HeaderLevel::H3 => Some(crate::HeaderLevel::H2),
        crate::HeaderLevel::H4 => Some(crate::HeaderLevel::H3),
        crate::HeaderLevel::H5 => Some(crate::HeaderLevel::H4),
        crate::HeaderLevel::H6 => Some(crate::HeaderLevel::H5),
    }
}

const fn level_number(level: crate::HeaderLevel) -> u8 {
    level as u8
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

fn constraint_satisfied<'d>(
    constraint: &Constraint,
    scope: &'d BoundScope<'d>,
    rules: &[SectionRule],
    root: &'d BoundScope<'d>,
    root_rules: &[SectionRule],
) -> bool {
    match constraint {
        Constraint::OneOf(refs) => {
            proposition_list(refs)
                .filter(|proposition| {
                    proposition_satisfied(proposition, scope, rules, root, root_rules)
                })
                .count()
                == 1
        }
        Constraint::AnyOf(refs) => proposition_list(refs)
            .any(|proposition| proposition_satisfied(proposition, scope, rules, root, root_rules)),
        Constraint::AtMostOne(refs) => {
            proposition_list(refs)
                .filter(|proposition| {
                    proposition_satisfied(proposition, scope, rules, root, root_rules)
                })
                .count()
                <= 1
        }
        Constraint::AllOrNone(refs) => {
            let values = proposition_list(refs)
                .map(|proposition| {
                    proposition_satisfied(proposition, scope, rules, root, root_rules)
                })
                .collect::<Vec<_>>();
            values.iter().all(|value| *value) || values.iter().all(|value| !*value)
        }
        Constraint::Requires {
            condition,
            consequences,
        } => {
            !proposition_satisfied(condition, scope, rules, root, root_rules)
                || non_empty_items(consequences).all(|proposition| {
                    proposition_satisfied(proposition, scope, rules, root, root_rules)
                })
        }
        Constraint::Conflicts {
            condition,
            exclusions,
        } => {
            !proposition_satisfied(condition, scope, rules, root, root_rules)
                || non_empty_items(exclusions).all(|proposition| {
                    !proposition_satisfied(proposition, scope, rules, root, root_rules)
                })
        }
        Constraint::Ordered(refs) => {
            let satisfied = rule_ref_list(refs)
                .map(|reference| resolve_occurrences(reference, scope, rules, root, root_rules))
                .filter(|occurrences| !occurrences.is_empty())
                .collect::<Vec<_>>();
            satisfied.windows(2).all(|pair| {
                let Some((left, right)) = pair
                    .split_first()
                    .and_then(|(left, rest)| rest.first().map(|right| (left, right)))
                else {
                    return true;
                };
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

fn proposition_satisfied<'d>(
    proposition: &Proposition,
    scope: &'d BoundScope<'d>,
    rules: &[SectionRule],
    root: &'d BoundScope<'d>,
    root_rules: &[SectionRule],
) -> bool {
    match proposition {
        Proposition::Rule(reference) => {
            !resolve_occurrences(reference, scope, rules, root, root_rules).is_empty()
        }
        Proposition::Frontmatter(reference) => frontmatter_satisfied(reference),
    }
}

fn frontmatter_satisfied(_reference: &FrontmatterRef) -> bool {
    false
}

fn resolve_occurrences<'d>(
    reference: &RuleRef,
    current: &'d BoundScope<'d>,
    current_rules: &[SectionRule],
    root: &'d BoundScope<'d>,
    root_rules: &[SectionRule],
) -> Vec<&'d BoundSection<'d>> {
    let (start_scope, start_rules) = match reference.anchor {
        RefAnchor::CurrentScope => (current, current_rules),
        RefAnchor::SchemaRoot => (root, root_rules),
    };
    let segments = std::iter::once(&reference.path.first)
        .chain(&reference.path.rest)
        .collect::<Vec<_>>();
    let mut candidate_scopes = vec![(start_scope, start_rules)];
    let mut found = Vec::new();
    for (position, id) in segments.iter().enumerate() {
        found.clear();
        let mut next_scopes = Vec::new();
        for (candidate, candidate_rules) in std::mem::take(&mut candidate_scopes) {
            let Some((index, rule)) = candidate_rules
                .iter()
                .enumerate()
                .find(|(_, rule)| rule.id.as_ref() == Some(*id))
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
        if position + 1 < segments.len() {
            candidate_scopes = next_scopes;
        }
    }
    found
}

fn constraint_occurrences<'d>(
    constraint: &Constraint,
    scope: &'d BoundScope<'d>,
    rules: &[SectionRule],
    root: &'d BoundScope<'d>,
    root_rules: &[SectionRule],
) -> Vec<&'d BoundSection<'d>> {
    let mut occurrences = Vec::new();
    match constraint {
        Constraint::OneOf(refs)
        | Constraint::AnyOf(refs)
        | Constraint::AtMostOne(refs)
        | Constraint::AllOrNone(refs) => {
            for proposition in proposition_list(refs) {
                add_proposition_occurrences(
                    proposition,
                    scope,
                    rules,
                    root,
                    root_rules,
                    &mut occurrences,
                );
            }
        }
        Constraint::Requires {
            condition,
            consequences,
        } => {
            add_proposition_occurrences(
                condition,
                scope,
                rules,
                root,
                root_rules,
                &mut occurrences,
            );
            for proposition in non_empty_items(consequences) {
                add_proposition_occurrences(
                    proposition,
                    scope,
                    rules,
                    root,
                    root_rules,
                    &mut occurrences,
                );
            }
        }
        Constraint::Conflicts {
            condition,
            exclusions,
        } => {
            add_proposition_occurrences(
                condition,
                scope,
                rules,
                root,
                root_rules,
                &mut occurrences,
            );
            for proposition in non_empty_items(exclusions) {
                add_proposition_occurrences(
                    proposition,
                    scope,
                    rules,
                    root,
                    root_rules,
                    &mut occurrences,
                );
            }
        }
        Constraint::Ordered(refs) => {
            for reference in rule_ref_list(refs) {
                occurrences.extend(resolve_occurrences(
                    reference, scope, rules, root, root_rules,
                ));
            }
        }
    }
    occurrences.sort_by_key(|occurrence| occurrence.section.heading.location.range.start.0);
    occurrences.dedup_by_key(|occurrence| occurrence.section.heading.location.range.start.0);
    occurrences
}

fn constraint_references(
    constraint: &Constraint,
    current_rules: &[SectionRule],
    root_rules: &[SectionRule],
) -> Vec<DiagnosticReference> {
    let mut references = Vec::new();
    match constraint {
        Constraint::OneOf(items)
        | Constraint::AnyOf(items)
        | Constraint::AtMostOne(items)
        | Constraint::AllOrNone(items) => {
            references.extend(proposition_list(items).filter_map(|proposition| {
                diagnostic_reference(proposition, current_rules, root_rules)
            }));
        }
        Constraint::Requires {
            condition,
            consequences,
        } => {
            references.extend(
                std::iter::once(condition)
                    .chain(non_empty_items(consequences))
                    .filter_map(|proposition| {
                        diagnostic_reference(proposition, current_rules, root_rules)
                    }),
            );
        }
        Constraint::Conflicts {
            condition,
            exclusions,
        } => {
            references.extend(
                std::iter::once(condition)
                    .chain(non_empty_items(exclusions))
                    .filter_map(|proposition| {
                        diagnostic_reference(proposition, current_rules, root_rules)
                    }),
            );
        }
        Constraint::Ordered(items) => {
            references.extend(rule_ref_list(items).filter_map(|reference| {
                rule_for_ref(reference, current_rules, root_rules).map(|rule| {
                    DiagnosticReference::Rule {
                        reference: reference.clone(),
                        matcher: rule.matcher.clone(),
                    }
                })
            }));
        }
    }
    references
}

fn diagnostic_reference(
    proposition: &Proposition,
    current_rules: &[SectionRule],
    root_rules: &[SectionRule],
) -> Option<DiagnosticReference> {
    match proposition {
        Proposition::Rule(reference) => {
            rule_for_ref(reference, current_rules, root_rules).map(|rule| {
                DiagnosticReference::Rule {
                    reference: reference.clone(),
                    matcher: rule.matcher.clone(),
                }
            })
        }
        Proposition::Frontmatter(reference) => {
            Some(DiagnosticReference::Frontmatter(reference.clone()))
        }
    }
}

fn rule_for_ref<'a>(
    reference: &RuleRef,
    current_rules: &'a [SectionRule],
    root_rules: &'a [SectionRule],
) -> Option<&'a SectionRule> {
    let mut rules = match reference.anchor {
        RefAnchor::CurrentScope => current_rules,
        RefAnchor::SchemaRoot => root_rules,
    };
    let mut target = None;
    for id in std::iter::once(&reference.path.first).chain(&reference.path.rest) {
        target = rules.iter().find(|rule| rule.id.as_ref() == Some(id));
        rules = &target?.sections;
    }
    target
}

fn add_proposition_occurrences<'d>(
    proposition: &Proposition,
    scope: &'d BoundScope<'d>,
    rules: &[SectionRule],
    root: &'d BoundScope<'d>,
    root_rules: &[SectionRule],
    output: &mut Vec<&'d BoundSection<'d>>,
) {
    if let Proposition::Rule(reference) = proposition {
        output.extend(resolve_occurrences(
            reference, scope, rules, root, root_rules,
        ));
    }
}

fn proposition_list<T>(items: &AtLeastTwo<T>) -> impl Iterator<Item = &T> {
    std::iter::once(&items.first)
        .chain(std::iter::once(&items.second))
        .chain(&items.rest)
}

fn rule_ref_list(items: &AtLeastTwo<RuleRef>) -> impl Iterator<Item = &RuleRef> {
    proposition_list(items)
}

fn non_empty_items<T>(items: &NonEmpty<T>) -> impl Iterator<Item = &T> {
    std::iter::once(&items.first).chain(&items.rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        load_schema, parse_markdown, ExactText, GlobPattern, MarkdownOptions, RegexPattern,
    };

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
    }

    #[test]
    fn inline_regex_flags_compose_with_match_case() {
        let matcher = Matcher::Regex(RegexPattern("(?i:api)".into()));
        assert!(matcher_matches(&matcher, "API", true));
        assert!(matcher_matches(&matcher, "api", true));
    }

    #[test]
    fn malformed_manually_constructed_regex_is_total() {
        assert!(!matcher_matches(
            &Matcher::Regex(RegexPattern("(".into())),
            "anything",
            false
        ));
    }

    #[test]
    fn diagnostics_retain_normative_document_and_schema_anchors() {
        let loaded = load_schema("version: 1\nsections:\n  - match: Item\n    repeat: 2..2\n")
            .expect("test schema is valid");
        let document = parse_markdown("## Item\n## Item\n## Item\n", MarkdownOptions::default());
        let diagnostics = validate(&loaded.schema, &document);

        assert_eq!(diagnostics.len(), 1);
        let diagnostic = diagnostics.first().expect("one diagnostic was asserted");
        assert_eq!(diagnostic.id, DiagnosticId::TooManySections);
        assert_eq!(diagnostic.location.line, 3);
        assert_eq!(diagnostic.path.display(), "Item");
        assert_eq!(
            diagnostic.schema_node,
            Some(SchemaNode::Rule(crate::RulePath {
                scope: ScopePath(Vec::new()),
                index: RuleIndex(0),
            }))
        );
    }
}
