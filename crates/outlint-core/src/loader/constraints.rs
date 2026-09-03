//! Constraint construction, reference resolution, and proposition parsing.

use std::collections::HashSet;

use serde_json::Value;

use crate::yaml::parse_frontmatter_scalar;
use crate::{
    AtLeastTwo, Cardinality, Constraint, ConstraintIndex, ConstraintPath, FrontmatterKey,
    FrontmatterRef, FrontmatterScalar, NonEmpty, Proposition, RefAnchor, RuleId, RuleIndex,
    RuleOutcome, RuleRef, Schema, SchemaErrorKind, ScopePath, SectionRule, SourceRange, UpperBound,
};

use super::rules::is_slug;
use super::{Loader, RangeKey};

impl Loader {
    pub(super) fn validate_constraint_lexical_refs(&mut self) {
        let constraints = self.raw_constraints.clone();
        for (scope, values) in constraints {
            for (index, value) in values.iter().enumerate() {
                let range = self.range(RangeKey::Constraint(ConstraintPath {
                    scope: scope.clone(),
                    index: ConstraintIndex(index),
                }));
                let refs = constraint_ref_strings(value);
                let mut seen = HashSet::new();
                for reference in refs {
                    let valid = if reference.starts_with("fm.") {
                        parse_frontmatter_ref(reference).is_some()
                    } else {
                        parse_rule_ref(reference).is_some()
                    };
                    if !valid {
                        self.error_at(
                            SchemaErrorKind::UnresolvedRef,
                            range,
                            format!("invalid ref `{reference}`"),
                        );
                    }
                    if !seen.insert(reference) {
                        self.error_at(
                            SchemaErrorKind::DuplicateRef,
                            range,
                            format!("duplicate ref `{reference}` in one constraint"),
                        );
                    }
                }
            }
        }
    }

    pub(super) fn build_constraint(
        &mut self,
        schema: &Schema,
        scope: &ScopePath,
        value: Value,
        range: SourceRange,
    ) -> Option<Constraint> {
        let Some(mapping) = value.as_object() else {
            self.shape_error_at(range, "constraint must be a single-key object");
            return None;
        };
        if mapping.len() != 1 {
            self.shape_error_at(range, "constraint must contain exactly one keyword");
            return None;
        }
        let (keyword, operand) = mapping.iter().next()?;
        match keyword.as_str() {
            "one_of" | "any_of" | "at_most_one" | "all_or_none" => {
                let refs = self.parse_proposition_list(schema, scope, operand, false, range)?;
                let refs = at_least_two(refs).or_else(|| {
                    self.shape_error_at(range, format!("{keyword} requires at least two refs"));
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
            "requires" => self.build_implication(schema, scope, operand, true, range),
            "conflicts" => self.build_implication(schema, scope, operand, false, range),
            "ordered" => self.build_ordered(schema, scope, operand, range),
            _ => {
                self.shape_error_at(range, format!("unknown constraint keyword `{keyword}`"));
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
        range: SourceRange,
    ) -> Option<Constraint> {
        let Some(mapping) = operand.as_object() else {
            self.shape_error_at(range, "requires/conflicts operand must be an object");
            return None;
        };
        let consequence_key = if requires { "then" } else { "then_not" };
        if mapping.len() != 2 {
            self.shape_error_at(
                range,
                format!(
                    "{} requires exactly `if` and `{consequence_key}`",
                    if requires { "requires" } else { "conflicts" }
                ),
            );
            return None;
        }
        let Some(condition_value) = mapping.get("if") else {
            self.shape_error_at(range, "requires/conflicts operand is missing `if`");
            return None;
        };
        let Some(consequence_value) = mapping.get(consequence_key) else {
            self.shape_error_at(
                range,
                format!("requires/conflicts operand is missing `{consequence_key}`"),
            );
            return None;
        };
        let condition = self.parse_proposition(schema, scope, condition_value, false, range);
        let consequence_values = scalar_or_sequence(consequence_value);
        if consequence_values.is_empty() {
            self.shape_error_at(
                range,
                format!("`{consequence_key}` must contain at least one ref"),
            );
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
                self.parse_proposition(schema, scope, value, false, range)
            {
                if !identities.insert(identity) {
                    self.error_at(
                        SchemaErrorKind::DuplicateRef,
                        range,
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
        range: SourceRange,
    ) -> Option<Constraint> {
        let values = operand.as_array().or_else(|| {
            self.shape_error_at(range, "ordered requires a list of refs");
            None
        })?;
        let mut refs = Vec::new();
        let mut identities = HashSet::new();
        let mut parent_scope: Option<Vec<usize>> = None;
        let mut mixed_scopes = false;
        let mut complete = true;
        for value in values {
            let Some((proposition, identity)) =
                self.parse_proposition(schema, scope, value, true, range)
            else {
                complete = false;
                continue;
            };
            let Proposition::Rule(rule_ref) = proposition else {
                self.error_at(
                    SchemaErrorKind::OrderedScopeMismatch,
                    range,
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
                self.error_at(
                    SchemaErrorKind::OrderedScopeMismatch,
                    range,
                    "all ordered refs must resolve in the same scope",
                );
                mixed_scopes = true;
            } else {
                parent_scope = Some(target_parent);
            }
            if !identities.insert(identity) {
                self.error_at(
                    SchemaErrorKind::DuplicateRef,
                    range,
                    "duplicate ref in ordered",
                );
            }
            refs.push(rule_ref);
        }
        if !complete {
            return None;
        }
        // An ordered scope already orders every rule in it, so an explicit
        // `ordered` over that scope is either redundant — the same failure
        // reported twice — or contradicts the list order. When both rules in
        // a reversed pair are present, one of the two orders necessarily
        // fails; absent optional rules may satisfy both vacuously. Neither is
        // what the author meant, and the fix is the same either way.
        if !mixed_scopes
            && parent_scope
                .as_ref()
                .is_some_and(|target_scope| scope_is_ordered(schema, target_scope))
        {
            self.error_at(
                SchemaErrorKind::OrderedScopeMismatch,
                range,
                "the scope these refs resolve in is already ordered by its rule list; \
                 remove this constraint, or set `ordered: false` on the rule that owns \
                 the scope (`options.ordered_sections: false` for the top-level scope)",
            );
            return None;
        }
        let refs = at_least_two(refs).or_else(|| {
            self.shape_error_at(range, "ordered requires at least two refs");
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
        range: SourceRange,
    ) -> Option<Vec<Proposition>> {
        let values = operand.as_array().or_else(|| {
            self.shape_error_at(range, "constraint operand must be a list of refs");
            None
        })?;
        let mut identities = HashSet::new();
        let mut result = Vec::new();
        let mut complete = true;
        for value in values {
            if let Some((proposition, identity)) =
                self.parse_proposition(schema, scope, value, ordered, range)
            {
                if !identities.insert(identity) {
                    self.error_at(
                        SchemaErrorKind::DuplicateRef,
                        range,
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
        range: SourceRange,
    ) -> Option<(Proposition, ResolvedIdentity)> {
        let Some(source) = value.as_str() else {
            self.shape_error_at(range, "constraint refs must be strings");
            return None;
        };
        if source.starts_with("fm.") {
            let Some(reference) = parse_frontmatter_ref(source) else {
                self.error_at(
                    SchemaErrorKind::UnresolvedRef,
                    range,
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
            self.error_at(
                SchemaErrorKind::UnresolvedRef,
                range,
                format!("invalid or unresolved ref `{source}`"),
            );
            return None;
        };
        let Some(resolved) = resolve_ref(schema, scope, &reference) else {
            self.error_at(
                SchemaErrorKind::UnresolvedRef,
                range,
                format!("unresolved ref `{source}`"),
            );
            return None;
        };
        if resolved.denied {
            self.error_at(
                SchemaErrorKind::ForbiddenRef,
                range,
                format!("ref `{source}` passes through or targets an allow: false rule"),
            );
        }
        if ordered && resolved.repeated_non_final {
            self.error_at(
                SchemaErrorKind::OrderedScopeMismatch,
                range,
                format!("ordered ref `{source}` passes through a repeatable ancestor"),
            );
        }
        Some((
            Proposition::Rule(reference),
            ResolvedIdentity::Rule(resolved.structural_path),
        ))
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
    // Both anchors resolve from the addressed root: the outline (`h1`) scope
    // for the general form, the `sections` scope for sugar — so a sugar
    // schema's `$.` references keep meaning what they always meant, while in
    // an `outline` schema `$` names the `h1` rules.
    let (mut rules, mut structural_path) = match reference.anchor {
        RefAnchor::SchemaRoot => (schema.addressed_root_rules(), Vec::new()),
        RefAnchor::CurrentScope => (
            rules_at_scope(schema, scope)?,
            scope.0.iter().map(|index| index.0).collect(),
        ),
    };
    let mut denied = false;
    let mut repeated_non_final = false;
    let segment_count = reference.path.rest.len() + 1;
    for (position, id) in reference.path.iter().enumerate() {
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

/// Whether the scope at a structural path binds its rules in document order.
///
/// The empty path is the addressed root — the outline scope or the sugar's
/// `sections` scope — which follows `options.ordered_sections`, as the
/// synthesized title rule does.
fn scope_is_ordered(schema: &Schema, structural_scope: &[usize]) -> bool {
    let mut rules = schema.addressed_root_rules();
    let mut ordered = schema.options.ordered_sections;
    for &index in structural_scope {
        let Some(rule) = rules.get(index) else {
            return ordered;
        };
        ordered = rule.ordered;
        rules = &rule.sections;
    }
    ordered
}

fn rules_at_scope<'a>(schema: &'a Schema, scope: &ScopePath) -> Option<&'a [SectionRule]> {
    let mut rules = schema.addressed_root_rules();
    for index in &scope.0 {
        let rule = rules.get(index.0)?;
        rules = &rule.sections;
    }
    Some(rules)
}

/// The constraint list a public scope path names, in the built schema.
///
/// The empty scope names what the source's top level spelled: the outline
/// scope for the general form ([`Schema::constraints`]), the `sections` scope
/// for sugar — which is the synthesized rule's child scope, so its top-level
/// constraints live on that rule.
pub(super) fn constraints_mut<'a>(
    schema: &'a mut Schema,
    scope: &ScopePath,
) -> Option<&'a mut Vec<Constraint>> {
    if schema.is_sugar() {
        let rule = schema.outline.first_mut()?;
        if scope.0.is_empty() {
            return Some(&mut rule.constraints);
        }
        constraints_in_rules_mut(&mut rule.sections, &scope.0)
    } else {
        if scope.0.is_empty() {
            return Some(&mut schema.constraints);
        }
        constraints_in_rules_mut(&mut schema.outline, &scope.0)
    }
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
            *value = crate::case_fold::simple_fold(value).collect();
        }
    }
    identity
}

fn scalar_or_sequence(value: &Value) -> Vec<&Value> {
    value
        .as_array()
        .map_or_else(|| vec![value], |values| values.iter().collect())
}

fn constraint_ref_strings(value: &Value) -> Vec<&str> {
    let Some(mapping) = value.as_object() else {
        return Vec::new();
    };
    let Some((keyword, operand)) = mapping.iter().next() else {
        return Vec::new();
    };
    match keyword.as_str() {
        "one_of" | "any_of" | "at_most_one" | "all_or_none" | "ordered" => operand
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect(),
        "requires" | "conflicts" => {
            let Some(implication) = operand.as_object() else {
                return Vec::new();
            };
            let consequence = if keyword == "requires" {
                "then"
            } else {
                "then_not"
            };
            let mut result = implication
                .get("if")
                .and_then(Value::as_str)
                .into_iter()
                .collect::<Vec<_>>();
            if let Some(value) = implication.get(consequence) {
                result.extend(
                    scalar_or_sequence(value)
                        .into_iter()
                        .filter_map(Value::as_str),
                );
            }
            result
        }
        _ => Vec::new(),
    }
}

pub(super) fn non_empty<T>(mut values: Vec<T>) -> Option<NonEmpty<T>> {
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
