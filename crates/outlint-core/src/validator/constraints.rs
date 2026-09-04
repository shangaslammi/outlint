//! Truth evaluation of constraints over a bound scope tree.

use crate::yaml::parse_frontmatter_scalar;
use crate::{
    BoundRuleStep, Constraint, FrontmatterScalar, NonEmpty, Proposition, RefAnchor,
    ResolvedFrontmatterQuery, ResolvedRuleLocator, SectionRule,
};

use super::diagnostic::{Diagnostic, DiagnosticReference};
use super::engine::{BoundScope, BoundSection};
use super::frontmatter_values::FrontmatterValues;

/// What one proposition, or one whole constraint, evaluated to (§5.3).
///
/// The third value is not "unknown". It says the evaluation had a
/// prerequisite that did not hold — an unsingular locator step (§4.4), an
/// invalid typed value, or an unusable frontmatter block (§4.6) — so the
/// question was never a fair one to answer. §5.3 makes that infectious across
/// the whole boolean constraint "without three-valued short-circuiting": one
/// suppressed operand suppresses the constraint whatever the others say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Truth {
    Satisfied,
    Unsatisfied,
    Suppressed,
}

impl Truth {
    pub(super) fn from_bool(value: bool) -> Self {
        if value {
            Self::Satisfied
        } else {
            Self::Unsatisfied
        }
    }
}

/// One constraint's complete evaluation.
///
/// Everything a caller needs comes out of one pass over the operands, because
/// every operand is evaluated exactly once: resolving them a second time to
/// collect involved headers would evaluate a query twice and risk two answers
/// to one question.
pub(super) struct ConstraintEvaluation<'s, 'd> {
    /// The constraint's tri-state result.
    pub(super) truth: Truth,
    /// Concrete headers the constraint's locators resolved to, in document
    /// order and without repeats.
    pub(super) occurrences: Vec<&'s BoundSection<'d>>,
    /// Primary diagnostics the operands produced while being evaluated.
    ///
    /// These stand whatever `truth` turns out to be: §4.6 has a query node
    /// produce `invalid-value` *and* suppress its constraint, so the two are
    /// reported independently rather than one instead of the other.
    pub(super) pending: Vec<Diagnostic>,
}

/// Evaluates an `fm[...]` proposition against the frontmatter view (§4.6).
///
/// PHASE 4A DEBT, in the bare form only: §4.6 says "Every non-boolean,
/// non-null result node produces `invalid-value`, and the entire constraint
/// containing the proposition is suppressed". Neither the diagnostic nor the
/// suppression exists yet, so such a node reads as unsatisfied here. The
/// equality form is complete: it is existential over non-null result nodes
/// with §4.6's typed scalar equality, and nothing about it is deferred.
pub(super) fn frontmatter_query_satisfied(
    root: Option<&serde_json::Value>,
    proposition: &ResolvedFrontmatterQuery,
    match_case: bool,
) -> bool {
    // §4.6: "If the block is absent, the query produces an empty result: a
    // bare boolean read is unsatisfied, and an equality proposition is
    // unsatisfied." An `invalid-frontmatter` block arrives here as `None` too;
    // suppressing its containing constraint is 4A's.
    let Some(document) = root else {
        return false;
    };
    let Ok(prepared) = proposition.parsed().query().prepare() else {
        // The source was validated when the schema loaded, so a provider that
        // now refuses it is a provider bug, not an authoring one.
        return false;
    };
    let nodes = prepared.evaluate(document);
    match proposition.equals() {
        // §4.6: "A bare `fm[...]` is a typed boolean read, not a presence
        // test. It is satisfied iff at least one result node is the YAML/JSON
        // boolean `true`."
        None => nodes
            .iter()
            .any(|(_, value)| matches!(value, serde_json::Value::Bool(true))),
        // §4.6: "Equality is existential over non-null result nodes [...]
        // satisfied iff at least one such node has the same resolved scalar
        // type and value."
        Some(expected) => nodes.iter().any(|(_, value)| {
            !value.is_null() && frontmatter_scalar_equals(value, expected, match_case)
        }),
    }
}

/// Typed equality between a frontmatter value and a resolved literal.
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
        // A null node is filtered out before this is reached, and a mapping
        // or sequence is unsatisfied by every `=` form.
        _ => false,
    }
}

#[derive(Clone, Copy)]
pub(super) struct EvalCtx<'s, 'd> {
    pub(super) current: &'s BoundScope<'d>,
    pub(super) current_rules: &'s [SectionRule],
    pub(super) root: &'s BoundScope<'d>,
    pub(super) root_rules: &'s [SectionRule],
    /// The document's frontmatter runtime view. `fm.` propositions address
    /// the document rather than a scope, so this is the same from every
    /// constraint node.
    pub(super) frontmatter: &'s FrontmatterValues,
    pub(super) match_case: bool,
}

impl<'s, 'd> EvalCtx<'s, 'd> {
    /// Evaluates one constraint completely (§5, §5.3).
    ///
    /// Every operand is evaluated before any truth rule is applied. That is
    /// §5.3's "without three-valued short-circuiting" read as an instruction
    /// to the implementation: an operand that would have been skipped by a
    /// decisive earlier one still has to be evaluated, both because its
    /// suppression must reach this constraint and because evaluating it is
    /// what produces its own primary diagnostics (§4.6).
    pub(super) fn constraint_evaluation(
        self,
        constraint: &Constraint,
    ) -> ConstraintEvaluation<'s, 'd> {
        let mut resolved = Resolved::default();
        let truth = match constraint {
            Constraint::OneOf(refs) => {
                let values = resolved.propositions(self, refs.iter());
                combine(&values, |satisfied| count(satisfied) == 1)
            }
            Constraint::AnyOf(refs) => {
                let values = resolved.propositions(self, refs.iter());
                combine(&values, |satisfied| count(satisfied) >= 1)
            }
            Constraint::AtMostOne(refs) => {
                let values = resolved.propositions(self, refs.iter());
                combine(&values, |satisfied| count(satisfied) <= 1)
            }
            Constraint::AllOrNone(refs) => {
                let values = resolved.propositions(self, refs.iter());
                combine(&values, |satisfied| {
                    count(satisfied) == 0 || count(satisfied) == satisfied.len()
                })
            }
            Constraint::Requires {
                condition,
                consequences,
            } => {
                let values = resolved
                    .propositions(self, std::iter::once(condition).chain(consequences.iter()));
                combine(&values, |satisfied| {
                    let (condition, consequences) = satisfied
                        .split_first()
                        .expect("a `requires` always evaluates its condition");
                    !*condition || consequences.iter().all(|value| *value)
                })
            }
            Constraint::Conflicts {
                condition,
                exclusions,
            } => {
                let values = resolved
                    .propositions(self, std::iter::once(condition).chain(exclusions.iter()));
                combine(&values, |satisfied| {
                    let (condition, exclusions) = satisfied
                        .split_first()
                        .expect("a `conflicts` always evaluates its condition");
                    !*condition || exclusions.iter().all(|value| !*value)
                })
            }
            // §5.1's pairwise `last(A) < first(B)` over the locators whose
            // terminal lists are non-empty.
            Constraint::Ordered(refs) => {
                let lists = refs
                    .iter()
                    .map(|locator| resolved.locator(self, locator))
                    .collect::<Vec<_>>();
                if lists.iter().any(Option::is_none) {
                    Truth::Suppressed
                } else {
                    let present = lists
                        .into_iter()
                        .flatten()
                        .filter(|occurrences| !occurrences.is_empty())
                        .collect::<Vec<_>>();
                    Truth::from_bool(present.iter().zip(present.iter().skip(1)).all(
                        |(left, right)| {
                            let last_left = left
                                .iter()
                                .map(|occurrence| occurrence.section.heading.location.range.start.0)
                                .max();
                            let first_right = right
                                .iter()
                                .map(|occurrence| occurrence.section.heading.location.range.start.0)
                                .min();
                            matches!((last_left, first_right), (Some(left), Some(right)) if left < right)
                        },
                    ))
                }
            }
        };
        let Resolved {
            mut occurrences,
            pending,
        } = resolved;
        occurrences.sort_by_key(|occurrence| occurrence.section.heading.location.range.start.0);
        occurrences.dedup_by_key(|occurrence| occurrence.section.heading.location.range.start.0);
        ConstraintEvaluation {
            truth,
            occurrences,
            pending,
        }
    }

    /// Walks a bound locator's steps to its terminal occurrence list, or
    /// reports that the descent depended on a singularity that did not hold.
    ///
    /// Binding already resolved every name to a structural index, so this
    /// walks indices rather than searching ids a second time, and applies each
    /// step's `[i]` through the kernel's one checked conversion.
    ///
    /// §4.4 attaches one runtime dependency to that walk. A non-terminal step
    /// left unnarrowed is statically singular — the loader refuses any other
    /// kind there — and if its rule nevertheless matched several headers in
    /// this concrete scope, "every constraint evaluation that depends on
    /// descending through that step is suppressed in that scope". The
    /// singularity read here is the scope's own record of its raw match
    /// counts, taken before any diagnostic was filtered, so hiding
    /// `too-many-sections` cannot make the descent evaluable. A step narrowed
    /// with `[i]` names one occurrence outright and carries no such
    /// dependency, so it descends through the very same violation — the
    /// occurrence in excess of the bound included.
    fn resolve_occurrences(
        self,
        locator: &ResolvedRuleLocator,
    ) -> Option<Vec<&'s BoundSection<'d>>> {
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
            let terminal = position == last;
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
                    None => {
                        if !terminal && !candidate.is_singular(index) {
                            return None;
                        }
                        matched
                    }
                };
                for occurrence in selected {
                    found.push(occurrence);
                    next_scopes.push((&occurrence.child, &rule.sections[..]));
                }
            }
            if !terminal {
                candidate_scopes = next_scopes;
            }
        }
        Some(found)
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
                references.extend(items.iter().filter_map(|locator| {
                    self.rule_for_locator(locator)
                        .map(|rule| DiagnosticReference::Rule {
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
            Proposition::Rule(locator) => {
                self.rule_for_locator(locator)
                    .map(|rule| DiagnosticReference::Rule {
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
}

/// Operand results accumulated while one constraint is evaluated.
///
/// It exists so that one pass answers every question the caller has: the
/// truth of each operand, the concrete headers the locators named, and the
/// primaries the operands produced. Asking for those separately would
/// evaluate each operand more than once, and a query evaluated twice is a
/// question that can be answered twice.
#[derive(Default)]
struct Resolved<'s, 'd> {
    occurrences: Vec<&'s BoundSection<'d>>,
    pending: Vec<Diagnostic>,
}

impl<'s, 'd> Resolved<'s, 'd> {
    /// Evaluates every proposition in the order the constraint spells them.
    ///
    /// The whole iterator is consumed: no truth rule is applied until every
    /// operand has been evaluated, so nothing here can short-circuit past an
    /// operand whose suppression or whose primary diagnostic still matters
    /// (§5.3, §4.6).
    fn propositions<'p>(
        &mut self,
        context: EvalCtx<'s, 'd>,
        propositions: impl Iterator<Item = &'p Proposition>,
    ) -> Vec<Truth> {
        propositions
            .map(|proposition| self.proposition(context, proposition))
            .collect()
    }

    fn proposition(&mut self, context: EvalCtx<'s, 'd>, proposition: &Proposition) -> Truth {
        match proposition {
            // §4.5: an outline locator ending in a rule id "is satisfied iff
            // its terminal node list is non-empty. Positional narrowing does
            // not change that definition."
            Proposition::Rule(locator) => match self.locator(context, locator) {
                Some(found) => Truth::from_bool(!found.is_empty()),
                None => Truth::Suppressed,
            },
            Proposition::FrontmatterQuery(proposition) => {
                Truth::from_bool(frontmatter_query_satisfied(
                    context.frontmatter.root(),
                    proposition,
                    context.match_case,
                ))
            }
            // §4.6's `fm.<name>`, answered from the state the capture
            // evaluation retained rather than from the diagnostics it
            // produced.
            Proposition::FrontmatterCapture(reference) => {
                context.frontmatter.truth(reference.name())
            }
        }
    }

    /// Resolves one locator, retaining the occurrences it named.
    ///
    /// A suppressed descent contributes no involved header, which costs
    /// nothing: a suppressed constraint emits no diagnostic for them to
    /// appear in.
    fn locator(
        &mut self,
        context: EvalCtx<'s, 'd>,
        locator: &ResolvedRuleLocator,
    ) -> Option<Vec<&'s BoundSection<'d>>> {
        let found = context.resolve_occurrences(locator)?;
        self.occurrences.extend(found.iter().copied());
        Some(found)
    }
}

/// Applies a boolean truth rule to operands that have all been evaluated.
///
/// §5.3: "Suppression applies to the whole boolean constraint without
/// three-valued short-circuiting." So one suppressed operand is the answer
/// however the rest of the constraint reads — an `any_of` with a satisfied
/// operand beside a suppressed one is suppressed, not satisfied.
fn combine(values: &[Truth], decide: impl Fn(&[bool]) -> bool) -> Truth {
    if values.contains(&Truth::Suppressed) {
        return Truth::Suppressed;
    }
    let satisfied = values
        .iter()
        .map(|value| *value == Truth::Satisfied)
        .collect::<Vec<_>>();
    Truth::from_bool(decide(&satisfied))
}

fn count(satisfied: &[bool]) -> usize {
    satisfied.iter().filter(|value| **value).count()
}
