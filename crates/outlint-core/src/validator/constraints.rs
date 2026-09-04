//! Truth evaluation of constraints over a bound scope tree.

use crate::yaml::parse_frontmatter_scalar;
use crate::{
    BoundRuleStep, Constraint, FrontmatterRef, FrontmatterScalar, NonEmpty, Proposition, RefAnchor,
    ResolvedFrontmatterQuery, ResolvedRuleLocator, RuleRef, SectionRule,
};

use super::diagnostic::DiagnosticReference;
use super::engine::{BoundScope, BoundSection};

/// Evaluates an `fm.` proposition against the document's frontmatter (§4.6).
///
/// The bare form is satisfied iff the addressed value exists and is not null —
/// mappings and sequences included. The `=` form additionally requires typed
/// scalar equality, so it is never satisfied by a mapping or sequence value.
pub(super) fn frontmatter_satisfied(
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
pub(super) struct EvalCtx<'s, 'd> {
    pub(super) current: &'s BoundScope<'d>,
    pub(super) current_rules: &'s [SectionRule],
    pub(super) root: &'s BoundScope<'d>,
    pub(super) root_rules: &'s [SectionRule],
    /// The document's frontmatter mapping, when one parsed. `fm.` propositions
    /// address the document rather than a scope, so this is the same from
    /// every constraint node.
    pub(super) frontmatter: Option<&'d serde_json::Map<String, serde_json::Value>>,
    pub(super) match_case: bool,
}

impl<'s, 'd> EvalCtx<'s, 'd> {
    pub(super) fn constraint_satisfied(self, constraint: &Constraint) -> bool {
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
            Constraint::Ordered(refs) => self.ordered_satisfied(
                refs.iter()
                    .map(|reference| self.resolve_occurrences(reference)),
            ),
            Constraint::OrderedLocators(refs) => self.ordered_satisfied(
                refs.iter()
                    .map(|locator| self.resolve_bound_occurrences(locator)),
            ),
        }
    }

    /// §5.1's pairwise `last(A) < first(B)` over the non-empty terminal lists.
    fn ordered_satisfied(self, lists: impl Iterator<Item = Vec<&'s BoundSection<'d>>>) -> bool {
        let satisfied = lists
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

    fn proposition_satisfied(self, proposition: &Proposition) -> bool {
        match proposition {
            Proposition::Rule(reference) => !self.resolve_occurrences(reference).is_empty(),
            Proposition::Frontmatter(reference) => {
                frontmatter_satisfied(self.frontmatter, reference, self.match_case)
            }
            // §4.5: an outline locator ending in a rule id "is satisfied iff
            // its terminal node list is non-empty. Positional narrowing does
            // not change that definition."
            Proposition::ResolvedRule(locator) => {
                !self.resolve_bound_occurrences(locator).is_empty()
            }
            Proposition::FrontmatterQuery(proposition) => {
                self.frontmatter_query_satisfied(proposition)
            }
            // PHASE 4A DEBT: capture evaluation does not exist yet, so a
            // declared `fm.<name>` reads as unsatisfied. §4.6 makes it
            // "satisfied iff the capture is valid and bound, except that a
            // bound `bool` capture contributes its boolean value", and makes
            // an invalid value, a missing required capture, invalid
            // frontmatter, or an absent required block suppress the whole
            // containing constraint after its primary diagnostic. None of
            // that is implemented here; nothing observable depends on it
            // until the lane that evaluates typed values lands.
            Proposition::FrontmatterCapture(_) => false,
        }
    }

    /// Evaluates an `fm[...]` proposition against the frontmatter view (§4.6).
    ///
    /// PHASE 4A DEBT, in the bare form only: §4.6 says "Every non-boolean,
    /// non-null result node produces `invalid-value`, and the entire
    /// constraint containing the proposition is suppressed". Neither the
    /// diagnostic nor the suppression exists yet, so such a node reads as
    /// unsatisfied here. The equality form is complete: it is existential
    /// over non-null result nodes with §4.6's typed scalar equality, and
    /// nothing about it is deferred.
    fn frontmatter_query_satisfied(self, proposition: &ResolvedFrontmatterQuery) -> bool {
        // §4.6: "If the block is absent, the query produces an empty result: a
        // bare boolean read is unsatisfied, and an equality proposition is
        // unsatisfied." An `invalid-frontmatter` block arrives here as `None`
        // too; suppressing its containing constraint is 4A's.
        let Some(frontmatter) = self.frontmatter else {
            return false;
        };
        let Ok(prepared) = proposition.parsed().query().prepare() else {
            // The source was validated when the schema loaded, so a provider
            // that now refuses it is a provider bug, not an authoring one.
            return false;
        };
        // PHASE 4A DEBT: the frontmatter view is rebuilt per proposition
        // because the engine carries the mapping rather than a JSON document.
        // Frontmatter is small and this is temporary; evaluation moves behind
        // a prepared document in 4A.
        let document = serde_json::Value::Object(frontmatter.clone());
        let nodes = prepared.evaluate(&document);
        match proposition.equals() {
            // §4.6: "A bare `fm[...]` is a typed boolean read, not a presence
            // test. It is satisfied iff at least one result node is the
            // YAML/JSON boolean `true`."
            None => nodes
                .iter()
                .any(|(_, value)| matches!(value, serde_json::Value::Bool(true))),
            // §4.6: "Equality is existential over non-null result nodes [...]
            // satisfied iff at least one such node has the same resolved
            // scalar type and value."
            Some(expected) => nodes.iter().any(|(_, value)| {
                !value.is_null() && frontmatter_scalar_equals(value, expected, self.match_case)
            }),
        }
    }

    /// Walks a bound locator's steps to its terminal occurrence list.
    ///
    /// Binding already resolved every name to a structural index, so this
    /// walks indices rather than searching ids a second time, and applies each
    /// step's `[i]` through the kernel's one checked conversion.
    fn resolve_bound_occurrences(self, locator: &ResolvedRuleLocator) -> Vec<&'s BoundSection<'d>> {
        let (start_scope, start_rules) = match locator.anchor() {
            RefAnchor::CurrentScope => (self.current, self.current_rules),
            RefAnchor::SchemaRoot => (self.root, self.root_rules),
        };
        let mut candidate_scopes = vec![(start_scope, start_rules)];
        let mut found = Vec::new();
        let steps: &NonEmpty<BoundRuleStep> = locator.steps();
        let last = steps.rest.len();
        for (position, step) in steps.iter().enumerate() {
            found.clear();
            let mut next_scopes = Vec::new();
            for (candidate, candidate_rules) in std::mem::take(&mut candidate_scopes) {
                let index = step.index().0;
                let Some(rule) = candidate_rules.get(index) else {
                    continue;
                };
                let matched = candidate
                    .occurrences
                    .iter()
                    .filter(|occurrence| occurrence.rule_index == index)
                    .collect::<Vec<_>>();
                // §4.4: "`[i]` then retains only the i-th result in document
                // order, or the empty list if it does not exist."
                let selected = match step.position() {
                    Some(subscript) => subscript.select(&matched).copied().into_iter().collect(),
                    None => matched,
                };
                for occurrence in selected {
                    found.push(occurrence);
                    next_scopes.push((&occurrence.child, &rule.sections[..]));
                }
            }
            if position < last {
                candidate_scopes = next_scopes;
            }
        }
        found
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

    pub(super) fn constraint_occurrences(
        self,
        constraint: &Constraint,
    ) -> Vec<&'s BoundSection<'d>> {
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
            Constraint::OrderedLocators(refs) => {
                for locator in refs.iter() {
                    occurrences.extend(self.resolve_bound_occurrences(locator));
                }
            }
        }
        occurrences.sort_by_key(|occurrence| occurrence.section.heading.location.range.start.0);
        occurrences.dedup_by_key(|occurrence| occurrence.section.heading.location.range.start.0);
        occurrences
    }

    pub(super) fn constraint_references(self, constraint: &Constraint) -> Vec<DiagnosticReference> {
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
            Constraint::OrderedLocators(items) => {
                references.extend(items.iter().filter_map(|locator| {
                    self.rule_for_locator(locator)
                        .map(|rule| DiagnosticReference::ResolvedRule {
                            locator: locator.clone(),
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
            Proposition::ResolvedRule(locator) => {
                self.rule_for_locator(locator)
                    .map(|rule| DiagnosticReference::ResolvedRule {
                        locator: locator.clone(),
                        matcher: rule.matcher.clone(),
                    })
            }
            Proposition::FrontmatterQuery(proposition) => {
                Some(DiagnosticReference::FrontmatterQuery(proposition.clone()))
            }
            Proposition::FrontmatterCapture(proposition) => {
                Some(DiagnosticReference::FrontmatterCapture(proposition.clone()))
            }
        }
    }

    /// The declared rule a bound locator's terminal step named.
    fn rule_for_locator(self, locator: &ResolvedRuleLocator) -> Option<&'s SectionRule> {
        let mut rules = match locator.anchor() {
            RefAnchor::CurrentScope => self.current_rules,
            RefAnchor::SchemaRoot => self.root_rules,
        };
        let mut target = None;
        for step in locator.steps().iter() {
            target = rules.get(step.index().0);
            rules = &target?.sections;
        }
        target
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
        match proposition {
            Proposition::Rule(reference) => output.extend(self.resolve_occurrences(reference)),
            Proposition::ResolvedRule(locator) => {
                output.extend(self.resolve_bound_occurrences(locator));
            }
            // A frontmatter proposition names no header, so it contributes
            // no occurrence to a constraint's involved headers.
            Proposition::Frontmatter(_)
            | Proposition::FrontmatterQuery(_)
            | Proposition::FrontmatterCapture(_) => {}
        }
    }
}
