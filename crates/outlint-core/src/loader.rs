//! Loading, semantic validation, and normalization of Outlint schemas.
//!
//! `InvalidSchema` intentionally owns all source and error data, so boxing it
//! merely to reduce the result enum would complicate the public loader API.

#![allow(clippy::result_large_err)]

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::Path,
    sync::Arc,
};

use regex::Regex;
use serde::Deserialize;
use serde_yaml::{Mapping, Value};
use unicode_normalization::UnicodeNormalization;

use crate::{
    AtLeastTwo, ByteOffset, CanonicalFloat, CanonicalInteger, Cardinality, Constraint,
    ConstraintIndex, ConstraintPath, ExactText, FrontmatterKey, FrontmatterPolicy, FrontmatterRef,
    FrontmatterScalar, GlobPattern, HeaderLevel, InvalidSchema, LoadSchemaResult, LoadedSchema,
    Matcher, NonEmpty, Options, Proposition, RefAnchor, RegexPattern, RelatedLocation, RuleId,
    RuleIndex, RuleOutcome, RulePath, RuleRef, Schema, SchemaError, SchemaErrorKind,
    SchemaLocations, SchemaNode, SchemaSource, SchemaSources, SchemaVersion, ScopePath,
    SectionRule, SourceId, SourceLabel, SourceRange, TextRange, UpperBound,
};

/// Loads an Outlint schema from UTF-8 source text.
///
/// The returned model contains only normalized values. Errors are accumulated
/// where later checks do not depend on an earlier invalid value.
pub fn load_schema(source: &str) -> LoadSchemaResult {
    load_schema_with_label(source, None)
}

/// Loads an Outlint schema from source text with a diagnostic display label.
pub fn load_schema_with_label(source: &str, label: Option<SourceLabel>) -> LoadSchemaResult {
    Loader::new(Arc::from(source), label).load()
}

/// Reads and loads an Outlint schema file.
///
/// The path becomes the primary source label. A read failure is represented as
/// ordinary loader data so callers do not need a second error channel.
pub fn load_schema_file(path: &Path) -> LoadSchemaResult {
    let label = SourceLabel(path.display().to_string());
    match fs::read_to_string(path) {
        Ok(source) => load_schema_with_label(&source, Some(label)),
        Err(error) => {
            let source = Arc::<str>::from("");
            let sources = primary_sources(source, Some(label));
            Err(InvalidSchema {
                sources,
                errors: NonEmpty {
                    first: SchemaError {
                        kind: SchemaErrorKind::SourceRead,
                        range: empty_primary_range(),
                        related: Vec::new(),
                        message: format!("failed to read schema: {error}"),
                    },
                    rest: Vec::new(),
                },
            })
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSchema {
    version: i64,
    title: Option<String>,
    #[serde(default)]
    options: RawOptions,
    #[serde(default)]
    frontmatter: Option<Value>,
    sections: Vec<RawRule>,
    #[serde(default)]
    constraints: Vec<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOptions {
    match_case: Option<bool>,
    strip_inline_markup: Option<bool>,
    allow_skipped_levels: Option<bool>,
    root_level: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    id: Option<String>,
    #[serde(rename = "match")]
    matcher: String,
    #[serde(default = "default_true")]
    allow: bool,
    required: Option<bool>,
    repeat: Option<String>,
    #[serde(default)]
    strict: bool,
    #[serde(default)]
    sections: Vec<RawRule>,
    #[serde(default)]
    constraints: Vec<Value>,
}

const fn default_true() -> bool {
    true
}

struct Loader {
    source: Arc<str>,
    sources: SchemaSources,
    document_range: SourceRange,
    errors: Vec<SchemaError>,
    nodes: BTreeMap<SchemaNode, SourceRange>,
    raw_constraints: BTreeMap<ScopePath, Vec<Value>>,
}

impl Loader {
    fn new(source: Arc<str>, label: Option<SourceLabel>) -> Self {
        let document_range = SourceRange {
            source: SourceId(0),
            range: TextRange {
                start: ByteOffset(0),
                end: ByteOffset(source.len()),
            },
        };
        Self {
            sources: primary_sources(Arc::clone(&source), label),
            source,
            document_range,
            errors: Vec::new(),
            nodes: BTreeMap::new(),
            raw_constraints: BTreeMap::new(),
        }
    }

    fn load(mut self) -> LoadSchemaResult {
        let value: Value = match serde_yaml::from_str(&self.source) {
            Ok(value) => value,
            Err(error) => {
                let range = self.range_for_yaml_error(&error);
                self.error_at(
                    SchemaErrorKind::Syntax,
                    range,
                    format!("invalid YAML: {error}"),
                );
                return self.failure();
            }
        };

        let frontmatter_declared = value
            .as_mapping()
            .is_some_and(|mapping| mapping.contains_key(Value::String("frontmatter".into())));
        let raw: RawSchema = match serde_yaml::from_value(value) {
            Ok(raw) => raw,
            Err(error) => {
                self.error(
                    SchemaErrorKind::InvalidDocumentShape,
                    format!("invalid schema document shape: {error}"),
                );
                return self.failure();
            }
        };

        let version = if raw.version == 1 {
            Some(SchemaVersion::V1)
        } else {
            self.error(
                SchemaErrorKind::UnsupportedVersion,
                format!("unsupported schema version {}; expected 1", raw.version),
            );
            None
        };

        if frontmatter_declared {
            let detail = if raw.frontmatter.is_some() {
                "frontmatter declarations are not implemented yet"
            } else {
                "frontmatter must be an object when support is implemented"
            };
            self.error(SchemaErrorKind::InvalidDocumentShape, detail);
        }

        let options = self.build_options(&raw.options);
        let title = raw.title.as_deref().and_then(|matcher| {
            self.nodes.insert(SchemaNode::Title, self.document_range);
            self.build_matcher(matcher)
        });
        if raw.title.is_some_and(|_| {
            options
                .as_ref()
                .is_some_and(|options| options.root_level == HeaderLevel::H1)
        }) {
            self.error(
                SchemaErrorKind::InvalidTitleLevel,
                "title cannot be declared when root_level is 1",
            );
        }

        let root_scope = ScopePath(Vec::new());
        self.raw_constraints
            .insert(root_scope.clone(), raw.constraints);
        let sections = self.build_scope(raw.sections, &root_scope);

        if !self.errors.is_empty() {
            return self.failure();
        }

        let (Some(version), Some(options), Some(sections)) = (version, options, sections) else {
            return self.failure();
        };
        let mut schema = Schema {
            version,
            title,
            options,
            frontmatter: FrontmatterPolicy::Optional { schema: None },
            sections,
            constraints: Vec::new(),
        };

        let mut normalized = BTreeMap::new();
        for (scope, constraints) in std::mem::take(&mut self.raw_constraints) {
            let mut built = Vec::with_capacity(constraints.len());
            for (index, constraint) in constraints.into_iter().enumerate() {
                self.nodes.insert(
                    SchemaNode::Constraint(ConstraintPath {
                        scope: scope.clone(),
                        index: ConstraintIndex(index),
                    }),
                    self.document_range,
                );
                if let Some(constraint) = self.build_constraint(&schema, &scope, constraint) {
                    built.push(constraint);
                }
            }
            normalized.insert(scope, built);
        }

        if !self.errors.is_empty() {
            return self.failure();
        }
        for (scope, constraints) in normalized {
            if let Some(target) = constraints_mut(&mut schema, &scope) {
                *target = constraints;
            }
        }

        Ok(LoadedSchema {
            schema,
            sources: self.sources,
            locations: SchemaLocations {
                document: self.document_range,
                nodes: self.nodes,
            },
        })
    }

    fn build_options(&mut self, raw: &RawOptions) -> Option<Options> {
        let root_level = match raw.root_level.unwrap_or(2) {
            1 => Some(HeaderLevel::H1),
            2 => Some(HeaderLevel::H2),
            3 => Some(HeaderLevel::H3),
            4 => Some(HeaderLevel::H4),
            5 => Some(HeaderLevel::H5),
            6 => Some(HeaderLevel::H6),
            level => {
                self.error(
                    SchemaErrorKind::InvalidRootLevel,
                    format!("root_level must be between 1 and 6, got {level}"),
                );
                None
            }
        };
        root_level.map(|root_level| Options {
            match_case: raw.match_case.unwrap_or(false),
            strip_inline_markup: raw.strip_inline_markup.unwrap_or(true),
            allow_skipped_levels: raw.allow_skipped_levels.unwrap_or(false),
            root_level,
        })
    }

    fn build_scope(&mut self, rules: Vec<RawRule>, scope: &ScopePath) -> Option<Vec<SectionRule>> {
        let mut semantic = Vec::with_capacity(rules.len());
        let mut complete = true;
        for (index, raw) in rules.into_iter().enumerate() {
            let rule_path = RulePath {
                scope: scope.clone(),
                index: RuleIndex(index),
            };
            self.nodes
                .insert(SchemaNode::Rule(rule_path), self.document_range);
            let mut child_scope = scope.clone();
            child_scope.0.push(RuleIndex(index));
            self.raw_constraints
                .insert(child_scope.clone(), raw.constraints);

            let matcher = self.build_matcher(&raw.matcher);
            let id = self.build_rule_id(raw.id.as_deref(), matcher.as_ref(), scope);
            let outcome = self.build_outcome(raw.allow, raw.required, raw.repeat.as_deref());
            let children = self.build_scope(raw.sections, &child_scope);
            match (matcher, outcome, children) {
                (Some(matcher), Some(outcome), Some(sections)) => semantic.push(SectionRule {
                    id,
                    matcher,
                    outcome,
                    strict: raw.strict,
                    sections,
                    constraints: Vec::new(),
                }),
                _ => complete = false,
            }
        }

        let mut ids: HashMap<RuleId, usize> = HashMap::new();
        for (index, rule) in semantic.iter().enumerate() {
            let Some(id) = &rule.id else { continue };
            if let Some(first_index) = ids.insert(id.clone(), index) {
                self.error_with_related(
                    SchemaErrorKind::DuplicateId,
                    format!("duplicate rule id `{}` in one scope", id.0),
                    vec![RelatedLocation {
                        range: self.document_range,
                        message: format!("first declared by sibling rule {first_index}"),
                    }],
                );
                complete = false;
            }
        }

        complete.then_some(semantic)
    }

    fn build_rule_id(
        &mut self,
        explicit: Option<&str>,
        matcher: Option<&Matcher>,
        scope: &ScopePath,
    ) -> Option<RuleId> {
        if let Some(id) = explicit {
            if !is_slug(id) {
                self.error(
                    SchemaErrorKind::InvalidDocumentShape,
                    format!("rule id `{id}` is not a lowercase slug"),
                );
                return None;
            }
            if scope.0.is_empty() && id == "fm" {
                self.error(
                    SchemaErrorKind::ReservedId,
                    "top-level rule id `fm` is reserved for frontmatter refs",
                );
            }
            return Some(RuleId(id.to_owned()));
        }

        let Matcher::Exact(text) = matcher? else {
            return None;
        };
        auto_id(&text.0).map(RuleId)
    }

    fn build_matcher(&mut self, source: &str) -> Option<Matcher> {
        if source == "*" {
            return Some(Matcher::Any);
        }
        if source.starts_with('/') && source.ends_with('/') {
            let Some(body) = source
                .strip_prefix('/')
                .and_then(|body| body.strip_suffix('/'))
            else {
                self.error(
                    SchemaErrorKind::InvalidMatcher,
                    "a regex matcher needs separate opening and closing `/` delimiters",
                );
                return None;
            };
            let Some(body) = regex_body(body) else {
                self.error(
                    SchemaErrorKind::InvalidMatcher,
                    format!("regex matcher `{source}` contains an unescaped `/`"),
                );
                return None;
            };
            let anchored = format!(r"\A(?:{body})\z");
            if let Err(error) = Regex::new(&anchored) {
                self.error(
                    SchemaErrorKind::InvalidMatcher,
                    format!("invalid regex matcher `{source}`: {error}"),
                );
                return None;
            }
            return Some(Matcher::Regex(RegexPattern(body)));
        }
        if source.contains('*') {
            return Some(Matcher::Glob(GlobPattern(source.to_owned())));
        }
        Some(Matcher::Exact(ExactText(source.to_owned())))
    }

    fn build_outcome(
        &mut self,
        allow: bool,
        required: Option<bool>,
        repeat: Option<&str>,
    ) -> Option<RuleOutcome> {
        if required.is_some() && repeat.is_some() {
            self.error(
                SchemaErrorKind::ConflictingCardinality,
                "required and repeat cannot both be declared",
            );
            return None;
        }
        if !allow && (required.is_some() || repeat.is_some()) {
            self.error(
                SchemaErrorKind::ConflictingCardinality,
                "allow: false cannot be combined with required or repeat",
            );
            return None;
        }
        if !allow {
            return Some(RuleOutcome::Deny);
        }
        let cardinality = match (required, repeat) {
            (Some(true), None) => Cardinality {
                min: 1,
                max: UpperBound::Bounded(1),
            },
            (Some(false), None) => Cardinality {
                min: 0,
                max: UpperBound::Bounded(1),
            },
            (None, Some(repeat)) => match parse_repeat(repeat) {
                Some(cardinality) => cardinality,
                None => {
                    self.error(
                        SchemaErrorKind::InvalidRepeat,
                        format!("invalid repeat `{repeat}`"),
                    );
                    return None;
                }
            },
            (None, None) => Cardinality {
                min: 0,
                max: UpperBound::Unbounded,
            },
            (Some(_), Some(_)) => return None,
        };
        Some(RuleOutcome::Allow(cardinality))
    }

    fn build_constraint(
        &mut self,
        schema: &Schema,
        scope: &ScopePath,
        value: Value,
    ) -> Option<Constraint> {
        let Some(mapping) = value.as_mapping() else {
            self.shape_error("constraint must be a single-key object");
            return None;
        };
        if mapping.len() != 1 {
            self.shape_error("constraint must contain exactly one keyword");
            return None;
        }
        let Some((Value::String(keyword), operand)) = mapping.iter().next() else {
            self.shape_error("constraint keyword must be a string");
            return None;
        };
        match keyword.as_str() {
            "one_of" | "any_of" | "at_most_one" | "all_or_none" => {
                let refs = self.parse_proposition_list(schema, scope, operand, false)?;
                let refs = at_least_two(refs).or_else(|| {
                    self.shape_error(format!("{keyword} requires at least two refs"));
                    None
                })?;
                Some(match keyword.as_str() {
                    "one_of" => Constraint::OneOf(refs),
                    "any_of" => Constraint::AnyOf(refs),
                    "at_most_one" => Constraint::AtMostOne(refs),
                    "all_or_none" => Constraint::AllOrNone(refs),
                    _ => return None,
                })
            }
            "requires" => self.build_implication(schema, scope, operand, true),
            "conflicts" => self.build_implication(schema, scope, operand, false),
            "ordered" => self.build_ordered(schema, scope, operand),
            _ => {
                self.shape_error(format!("unknown constraint keyword `{keyword}`"));
                None
            }
        }
    }

    fn build_implication(
        &mut self,
        schema: &Schema,
        scope: &ScopePath,
        operand: &Value,
        requires: bool,
    ) -> Option<Constraint> {
        let Some(mapping) = operand.as_mapping() else {
            self.shape_error("requires/conflicts operand must be an object");
            return None;
        };
        let consequence_key = if requires { "then" } else { "then_not" };
        if mapping.len() != 2 {
            self.shape_error(format!(
                "{} requires exactly `if` and `{consequence_key}`",
                if requires { "requires" } else { "conflicts" }
            ));
            return None;
        }
        let Some(condition_value) = mapping_get(mapping, "if") else {
            self.shape_error("requires/conflicts operand is missing `if`");
            return None;
        };
        let Some(consequence_value) = mapping_get(mapping, consequence_key) else {
            self.shape_error(format!(
                "requires/conflicts operand is missing `{consequence_key}`"
            ));
            return None;
        };
        let condition = self.parse_proposition(schema, scope, condition_value, false);
        let consequence_values = scalar_or_sequence(consequence_value);
        if consequence_values.is_empty() {
            self.shape_error(format!("`{consequence_key}` must contain at least one ref"));
            return None;
        }
        let mut identities = HashSet::new();
        if let Some((_, identity)) = &condition {
            identities.insert(identity.clone());
        }
        let mut consequences = Vec::new();
        let mut complete = condition.is_some();
        for value in consequence_values {
            if let Some((proposition, identity)) =
                self.parse_proposition(schema, scope, value, false)
            {
                if !identities.insert(identity) {
                    self.error(
                        SchemaErrorKind::DuplicateRef,
                        format!("duplicate ref in `{consequence_key}`"),
                    );
                }
                consequences.push(proposition);
            } else {
                complete = false;
            }
        }
        if !complete {
            return None;
        }
        let (condition, _) = condition?;
        let consequences = non_empty(consequences)?;
        Some(if requires {
            Constraint::Requires {
                condition,
                consequences,
            }
        } else {
            Constraint::Conflicts {
                condition,
                exclusions: consequences,
            }
        })
    }

    fn build_ordered(
        &mut self,
        schema: &Schema,
        scope: &ScopePath,
        operand: &Value,
    ) -> Option<Constraint> {
        let values = operand.as_sequence().or_else(|| {
            self.shape_error("ordered requires a list of refs");
            None
        })?;
        let mut refs = Vec::new();
        let mut identities = HashSet::new();
        let mut parent_scope: Option<Vec<usize>> = None;
        let mut complete = true;
        for value in values {
            let Some((proposition, identity)) = self.parse_proposition(schema, scope, value, true)
            else {
                complete = false;
                continue;
            };
            let Proposition::Rule(rule_ref) = proposition else {
                self.error(
                    SchemaErrorKind::OrderedScopeMismatch,
                    "frontmatter refs cannot be used in ordered",
                );
                continue;
            };
            let ResolvedIdentity::Rule(target) = &identity else {
                continue;
            };
            let Some((_, target_parent)) = target.split_last() else {
                continue;
            };
            let target_parent = target_parent.to_vec();
            if parent_scope
                .as_ref()
                .is_some_and(|existing| existing != &target_parent)
            {
                self.error(
                    SchemaErrorKind::OrderedScopeMismatch,
                    "all ordered refs must resolve in the same scope",
                );
            } else {
                parent_scope = Some(target_parent);
            }
            if !identities.insert(identity) {
                self.error(SchemaErrorKind::DuplicateRef, "duplicate ref in ordered");
            }
            refs.push(rule_ref);
        }
        if !complete {
            return None;
        }
        let refs = at_least_two(refs).or_else(|| {
            self.shape_error("ordered requires at least two refs");
            None
        })?;
        Some(Constraint::Ordered(refs))
    }

    fn parse_proposition_list(
        &mut self,
        schema: &Schema,
        scope: &ScopePath,
        operand: &Value,
        ordered: bool,
    ) -> Option<Vec<Proposition>> {
        let values = operand.as_sequence().or_else(|| {
            self.shape_error("constraint operand must be a list of refs");
            None
        })?;
        let mut identities = HashSet::new();
        let mut result = Vec::new();
        let mut complete = true;
        for value in values {
            if let Some((proposition, identity)) =
                self.parse_proposition(schema, scope, value, ordered)
            {
                if !identities.insert(identity) {
                    self.error(
                        SchemaErrorKind::DuplicateRef,
                        "constraint contains a duplicate ref",
                    );
                }
                result.push(proposition);
            } else {
                complete = false;
            }
        }
        complete.then_some(result)
    }

    fn parse_proposition(
        &mut self,
        schema: &Schema,
        scope: &ScopePath,
        value: &Value,
        ordered: bool,
    ) -> Option<(Proposition, ResolvedIdentity)> {
        let Some(source) = value.as_str() else {
            self.shape_error("constraint refs must be strings");
            return None;
        };
        if source.starts_with("fm.") {
            let Some(reference) = parse_frontmatter_ref(source) else {
                self.error(
                    SchemaErrorKind::UnresolvedRef,
                    format!("invalid frontmatter ref `{source}`"),
                );
                return None;
            };
            let identity = frontmatter_identity(&reference, schema.options.match_case);
            return Some((
                Proposition::Frontmatter(reference.clone()),
                ResolvedIdentity::Frontmatter(identity),
            ));
        }

        let Some(reference) = parse_rule_ref(source) else {
            self.error(
                SchemaErrorKind::UnresolvedRef,
                format!("invalid or unresolved ref `{source}`"),
            );
            return None;
        };
        let Some(resolved) = resolve_ref(schema, scope, &reference) else {
            self.error(
                SchemaErrorKind::UnresolvedRef,
                format!("unresolved ref `{source}`"),
            );
            return None;
        };
        if resolved.denied {
            self.error(
                SchemaErrorKind::ForbiddenRef,
                format!("ref `{source}` passes through or targets an allow: false rule"),
            );
        }
        if ordered && resolved.repeated_non_final {
            self.error(
                SchemaErrorKind::OrderedScopeMismatch,
                format!("ordered ref `{source}` passes through a repeatable ancestor"),
            );
        }
        Some((
            Proposition::Rule(reference),
            ResolvedIdentity::Rule(resolved.structural_path),
        ))
    }

    fn range_for_yaml_error(&self, error: &serde_yaml::Error) -> SourceRange {
        let Some(location) = error.location() else {
            return self.document_range;
        };
        let line_start = self
            .source
            .split_inclusive('\n')
            .take(location.line().saturating_sub(1))
            .map(str::len)
            .sum::<usize>();
        let start = line_start
            .saturating_add(location.column().saturating_sub(1))
            .min(self.source.len());
        SourceRange {
            source: SourceId(0),
            range: TextRange {
                start: ByteOffset(start),
                end: ByteOffset(start.saturating_add(1).min(self.source.len())),
            },
        }
    }

    fn shape_error(&mut self, message: impl Into<String>) {
        self.error(SchemaErrorKind::InvalidDocumentShape, message);
    }

    fn error(&mut self, kind: SchemaErrorKind, message: impl Into<String>) {
        self.error_at(kind, self.document_range, message);
    }

    fn error_at(&mut self, kind: SchemaErrorKind, range: SourceRange, message: impl Into<String>) {
        self.errors.push(SchemaError {
            kind,
            range,
            related: Vec::new(),
            message: message.into(),
        });
    }

    fn error_with_related(
        &mut self,
        kind: SchemaErrorKind,
        message: impl Into<String>,
        related: Vec<RelatedLocation>,
    ) {
        self.errors.push(SchemaError {
            kind,
            range: self.document_range,
            related,
            message: message.into(),
        });
    }

    fn failure(mut self) -> LoadSchemaResult {
        if self.errors.is_empty() {
            self.errors.push(SchemaError {
                kind: SchemaErrorKind::InvalidDocumentShape,
                range: self.document_range,
                related: Vec::new(),
                message: "schema could not be loaded".into(),
            });
        }
        let first = self.errors.remove(0);
        Err(InvalidSchema {
            sources: self.sources,
            errors: NonEmpty {
                first,
                rest: self.errors,
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ResolvedIdentity {
    Rule(Vec<usize>),
    Frontmatter(FrontmatterRef),
}

struct ResolvedRule {
    structural_path: Vec<usize>,
    denied: bool,
    repeated_non_final: bool,
}

fn resolve_ref(schema: &Schema, scope: &ScopePath, reference: &RuleRef) -> Option<ResolvedRule> {
    let (mut rules, mut structural_path) = match reference.anchor {
        RefAnchor::SchemaRoot => (&schema.sections[..], Vec::new()),
        RefAnchor::CurrentScope => (
            rules_at_scope(schema, scope)?,
            scope.0.iter().map(|index| index.0).collect(),
        ),
    };
    let mut denied = false;
    let mut repeated_non_final = false;
    let segments = std::iter::once(&reference.path.first).chain(&reference.path.rest);
    let segment_count = 1 + reference.path.rest.len();
    for (position, id) in segments.enumerate() {
        let (index, rule) = rules
            .iter()
            .enumerate()
            .find(|(_, rule)| rule.id.as_ref() == Some(id))?;
        structural_path.push(index);
        denied |= matches!(rule.outcome, RuleOutcome::Deny);
        if position + 1 < segment_count {
            repeated_non_final |= !matches!(
                rule.outcome,
                RuleOutcome::Allow(Cardinality {
                    max: UpperBound::Bounded(0 | 1),
                    ..
                })
            );
        }
        rules = &rule.sections;
    }
    Some(ResolvedRule {
        structural_path,
        denied,
        repeated_non_final,
    })
}

fn rules_at_scope<'a>(schema: &'a Schema, scope: &ScopePath) -> Option<&'a [SectionRule]> {
    let mut rules = &schema.sections[..];
    for index in &scope.0 {
        let rule = rules.get(index.0)?;
        rules = &rule.sections;
    }
    Some(rules)
}

fn constraints_mut<'a>(
    schema: &'a mut Schema,
    scope: &ScopePath,
) -> Option<&'a mut Vec<Constraint>> {
    if scope.0.is_empty() {
        return Some(&mut schema.constraints);
    }
    constraints_in_rules_mut(&mut schema.sections, &scope.0)
}

fn constraints_in_rules_mut<'a>(
    rules: &'a mut [SectionRule],
    path: &[RuleIndex],
) -> Option<&'a mut Vec<Constraint>> {
    let (index, rest) = path.split_first()?;
    let rule = rules.get_mut(index.0)?;
    if rest.is_empty() {
        Some(&mut rule.constraints)
    } else {
        constraints_in_rules_mut(&mut rule.sections, rest)
    }
}

fn primary_sources(text: Arc<str>, label: Option<SourceLabel>) -> SchemaSources {
    SchemaSources {
        primary: SourceId(0),
        documents: BTreeMap::from([(SourceId(0), SchemaSource { label, text })]),
    }
}

const fn empty_primary_range() -> SourceRange {
    SourceRange {
        source: SourceId(0),
        range: TextRange {
            start: ByteOffset(0),
            end: ByteOffset(0),
        },
    }
}

fn is_slug(value: &str) -> bool {
    let mut previous_hyphen = true;
    for byte in value.bytes() {
        match byte {
            b'a'..=b'z' | b'0'..=b'9' => previous_hyphen = false,
            b'-' if !previous_hyphen => previous_hyphen = true,
            _ => return false,
        }
    }
    !value.is_empty() && !previous_hyphen
}

fn auto_id(value: &str) -> Option<String> {
    let mut result = String::new();
    let mut separator_pending = false;
    for character in value.nfkd().flat_map(char::to_lowercase) {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            if separator_pending && !result.is_empty() {
                result.push('-');
            }
            result.push(character);
            separator_pending = false;
        } else {
            separator_pending = true;
        }
    }
    (!result.is_empty()).then_some(result)
}

fn regex_body(source: &str) -> Option<String> {
    let mut result = String::with_capacity(source.len());
    let mut characters = source.chars();
    while let Some(character) = characters.next() {
        if character == '/' {
            return None;
        }
        if character != '\\' {
            result.push(character);
            continue;
        }
        match characters.next() {
            Some('/') => result.push('/'),
            Some(next) => {
                result.push('\\');
                result.push(next);
            }
            None => result.push('\\'),
        }
    }
    Some(result)
}

fn parse_repeat(source: &str) -> Option<Cardinality> {
    let (min, max) = source.split_once("..")?;
    if min.is_empty() || max.is_empty() || max.contains("..") || !valid_decimal(min) {
        return None;
    }
    let min = min.parse::<u32>().ok()?;
    let max = if max == "n" {
        UpperBound::Unbounded
    } else {
        if !valid_decimal(max) {
            return None;
        }
        let max = max.parse::<u32>().ok()?;
        if max < min || max == 0 {
            return None;
        }
        UpperBound::Bounded(max)
    };
    Some(Cardinality { min, max })
}

fn valid_decimal(value: &str) -> bool {
    value == "0"
        || value
            .strip_prefix(|character: char| ('1'..='9').contains(&character))
            .is_some_and(|rest| rest.bytes().all(|byte| byte.is_ascii_digit()))
}

fn parse_rule_ref(source: &str) -> Option<RuleRef> {
    let (anchor, path) = if let Some(path) = source.strip_prefix("$.") {
        (RefAnchor::SchemaRoot, path)
    } else {
        (RefAnchor::CurrentScope, source)
    };
    let mut segments = path.split('.');
    let first = segments.next()?;
    if !is_slug(first) {
        return None;
    }
    let rest = segments
        .map(|segment| is_slug(segment).then(|| RuleId(segment.to_owned())))
        .collect::<Option<Vec<_>>>()?;
    Some(RuleRef {
        anchor,
        path: NonEmpty {
            first: RuleId(first.to_owned()),
            rest,
        },
    })
}

fn parse_frontmatter_ref(source: &str) -> Option<FrontmatterRef> {
    let body = source.strip_prefix("fm.")?;
    let (path, equals) = match body.split_once('=') {
        Some((path, literal)) => (path, Some(parse_frontmatter_scalar(literal))),
        None => (body, None),
    };
    let mut keys = path.split('.');
    let first = keys.next()?;
    if first.is_empty() {
        return None;
    }
    let rest = keys
        .map(|key| (!key.is_empty()).then(|| FrontmatterKey(key.to_owned())))
        .collect::<Option<Vec<_>>>()?;
    Some(FrontmatterRef {
        path: NonEmpty {
            first: FrontmatterKey(first.to_owned()),
            rest,
        },
        equals,
    })
}

fn frontmatter_identity(reference: &FrontmatterRef, match_case: bool) -> FrontmatterRef {
    let mut identity = reference.clone();
    if !match_case {
        if let Some(FrontmatterScalar::String(value)) = &mut identity.equals {
            *value = value.to_lowercase();
        }
    }
    identity
}

fn parse_frontmatter_scalar(source: &str) -> FrontmatterScalar {
    match source {
        "" | "~" | "null" | "Null" | "NULL" => FrontmatterScalar::Null,
        "true" | "True" | "TRUE" => FrontmatterScalar::Boolean(true),
        "false" | "False" | "FALSE" => FrontmatterScalar::Boolean(false),
        _ => {
            if let Some(integer) = canonical_integer(source) {
                FrontmatterScalar::Integer(CanonicalInteger(integer))
            } else if let Some(float) = canonical_float(source) {
                FrontmatterScalar::Float(CanonicalFloat(float))
            } else {
                FrontmatterScalar::String(source.to_owned())
            }
        }
    }
}

fn canonical_integer(source: &str) -> Option<String> {
    let (negative, unsigned) = strip_sign(source);
    let (base, digits) = if let Some(digits) = unsigned.strip_prefix("0o") {
        (8_u8, digits)
    } else if let Some(digits) = unsigned.strip_prefix("0x") {
        (16, digits)
    } else {
        (10, unsigned)
    };
    if digits.is_empty()
        || !digits
            .bytes()
            .all(|byte| digit_value(byte).is_some_and(|d| d < base))
    {
        return None;
    }
    let mut decimal = vec![0_u8];
    for byte in digits.bytes() {
        let digit = digit_value(byte)?;
        let mut carry = u16::from(digit);
        for place in decimal.iter_mut().rev() {
            let value = u16::from(*place) * u16::from(base) + carry;
            *place = (value % 10) as u8;
            carry = value / 10;
        }
        while carry > 0 {
            decimal.insert(0, (carry % 10) as u8);
            carry /= 10;
        }
    }
    let first_nonzero = decimal.iter().position(|digit| *digit != 0);
    let Some(first_nonzero) = first_nonzero else {
        return Some("0".into());
    };
    let mut result = String::new();
    if negative {
        result.push('-');
    }
    result.extend(
        decimal[first_nonzero..]
            .iter()
            .map(|digit| char::from(b'0' + digit)),
    );
    Some(result)
}

fn canonical_float(source: &str) -> Option<String> {
    let (negative, unsigned) = strip_sign(source);
    if matches!(unsigned, ".inf" | ".Inf" | ".INF") {
        return Some(if negative { "-inf" } else { "inf" }.into());
    }
    if matches!(unsigned, ".nan" | ".NaN" | ".NAN") {
        return Some("nan".into());
    }
    let (mantissa, exponent) = unsigned
        .split_once(['e', 'E'])
        .map_or((unsigned, "0"), |parts| parts);
    let has_float_marker = mantissa.contains('.') || unsigned.contains(['e', 'E']);
    if !has_float_marker {
        return None;
    }
    let exponent = exponent.parse::<i64>().ok()?;
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    if !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || (whole.is_empty() && fraction.is_empty())
    {
        return None;
    }
    let digits = format!("{whole}{fraction}");
    if digits.is_empty() {
        return None;
    }
    let trimmed_leading = digits.trim_start_matches('0');
    if trimmed_leading.is_empty() {
        return Some("0e0".into());
    }
    let trailing = trimmed_leading.len() - trimmed_leading.trim_end_matches('0').len();
    let coefficient = trimmed_leading.trim_end_matches('0');
    let adjusted = exponent
        .checked_sub(i64::try_from(fraction.len()).ok()?)?
        .checked_add(i64::try_from(trailing).ok()?)?;
    Some(format!(
        "{}{coefficient}e{adjusted}",
        if negative { "-" } else { "" }
    ))
}

fn strip_sign(source: &str) -> (bool, &str) {
    if let Some(unsigned) = source.strip_prefix('-') {
        (true, unsigned)
    } else if let Some(unsigned) = source.strip_prefix('+') {
        (false, unsigned)
    } else {
        (false, source)
    }
}

const fn digit_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn mapping_get<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a Value> {
    mapping.get(Value::String(key.to_owned()))
}

fn scalar_or_sequence(value: &Value) -> Vec<&Value> {
    value
        .as_sequence()
        .map_or_else(|| vec![value], |values| values.iter().collect())
}

fn non_empty<T>(mut values: Vec<T>) -> Option<NonEmpty<T>> {
    if values.is_empty() {
        return None;
    }
    let first = values.remove(0);
    Some(NonEmpty {
        first,
        rest: values,
    })
}

fn at_least_two<T>(mut values: Vec<T>) -> Option<AtLeastTwo<T>> {
    if values.len() < 2 {
        return None;
    }
    let first = values.remove(0);
    let second = values.remove(0);
    Some(AtLeastTwo {
        first,
        second,
        rest: values,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid(source: &str) -> Schema {
        match load_schema(source) {
            Ok(loaded) => loaded.schema,
            Err(invalid) => panic!("unexpected errors: {:#?}", invalid.errors),
        }
    }

    fn error_kinds(source: &str) -> Vec<SchemaErrorKind> {
        match load_schema(source) {
            Ok(loaded) => panic!("unexpected valid schema: {:#?}", loaded.schema),
            Err(invalid) => std::iter::once(invalid.errors.first.kind)
                .chain(invalid.errors.rest.iter().map(|error| error.kind))
                .collect(),
        }
    }

    #[test]
    fn applies_defaults_and_normalizes_rules() {
        let schema = valid(
            r#"
version: 1
sections:
  - match: API Reference
    required: true
  - id: api
    match: "/API: .+/"
    repeat: 0..n
  - match: "*"
    allow: false
"#,
        );
        assert_eq!(schema.options.root_level, HeaderLevel::H2);
        assert!(!schema.options.match_case);
        assert!(schema.options.strip_inline_markup);
        assert_eq!(schema.sections[0].id, Some(RuleId("api-reference".into())));
        assert_eq!(
            schema.sections[0].outcome,
            RuleOutcome::Allow(Cardinality {
                min: 1,
                max: UpperBound::Bounded(1)
            })
        );
        assert!(matches!(schema.sections[2].outcome, RuleOutcome::Deny));
    }

    #[test]
    fn classifies_matcher_forms_and_unescapes_regex_delimiter() {
        let schema = valid(
            r#"
version: 1
sections:
  - match: exact
  - match: prefix*suffix
  - match: "*"
  - match: /a\/b/
"#,
        );
        assert!(matches!(schema.sections[0].matcher, Matcher::Exact(_)));
        assert!(matches!(schema.sections[1].matcher, Matcher::Glob(_)));
        assert_eq!(schema.sections[2].matcher, Matcher::Any);
        assert_eq!(
            schema.sections[3].matcher,
            Matcher::Regex(RegexPattern("a/b".into()))
        );
    }

    #[test]
    fn rejects_invalid_regex_and_repeat_while_collecting_errors() {
        let kinds = error_kinds(
            r#"
version: 1
sections:
  - match: /(?=lookaround)/
    repeat: 01..2
  - match: ok
    allow: false
    required: true
"#,
        );
        assert!(kinds.contains(&SchemaErrorKind::InvalidMatcher));
        assert!(kinds.contains(&SchemaErrorKind::InvalidRepeat));
        assert!(kinds.contains(&SchemaErrorKind::ConflictingCardinality));
    }

    #[test]
    fn rejects_a_single_regex_delimiter_without_panicking() {
        let kinds = error_kinds(
            r#"
version: 1
sections:
  - match: "/"
"#,
        );
        assert_eq!(kinds, vec![SchemaErrorKind::InvalidMatcher]);
    }

    #[test]
    fn detects_auto_id_collisions_per_scope() {
        let kinds = error_kinds(
            r#"
version: 1
sections:
  - match: API
  - id: api
    match: Something else
"#,
        );
        assert!(kinds.contains(&SchemaErrorKind::DuplicateId));
    }

    #[test]
    fn resolves_constraints_and_normalizes_frontmatter_scalars() {
        let schema = valid(
            r#"
version: 1
sections:
  - match: Overview
    sections:
      - match: Goals
  - match: Deployment
constraints:
  - requires: { if: deployment, then: [$.overview.goals, fm.count=0x10] }
"#,
        );
        let Constraint::Requires { consequences, .. } = &schema.constraints[0] else {
            panic!("expected requires")
        };
        assert_eq!(
            consequences.rest[0],
            Proposition::Frontmatter(FrontmatterRef {
                path: NonEmpty {
                    first: FrontmatterKey("count".into()),
                    rest: vec![]
                },
                equals: Some(FrontmatterScalar::Integer(CanonicalInteger("16".into())))
            })
        );
    }

    #[test]
    fn rejects_dangling_forbidden_duplicate_and_mis_scoped_ordered_refs() {
        let kinds = error_kinds(
            r#"
version: 1
sections:
  - id: repeated
    match: Repeated
    sections:
      - match: Child
  - id: denied
    match: Denied
    allow: false
constraints:
  - any_of: [missing, missing]
  - requires: { if: denied, then: denied }
  - ordered: [repeated.child, denied]
"#,
        );
        assert!(kinds.contains(&SchemaErrorKind::UnresolvedRef));
        assert!(kinds.contains(&SchemaErrorKind::ForbiddenRef));
        assert!(kinds.contains(&SchemaErrorKind::OrderedScopeMismatch));
    }

    #[test]
    fn rejects_declared_frontmatter_until_it_is_implemented() {
        let kinds = error_kinds(
            r#"
version: 1
frontmatter: { required: true }
sections: []
"#,
        );
        assert!(kinds.contains(&SchemaErrorKind::InvalidDocumentShape));
    }

    #[test]
    fn rejects_implication_objects_with_the_wrong_keys() {
        let kinds = error_kinds(
            r#"
version: 1
sections: []
constraints:
  - requires: { condition: foo, consequence: bar }
"#,
        );
        assert!(kinds.contains(&SchemaErrorKind::InvalidDocumentShape));
    }
}
