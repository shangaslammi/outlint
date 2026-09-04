//! Truth evaluation of constraints over a bound scope tree.

use crate::locator::PreparedQuery;
use crate::yaml::parse_frontmatter_scalar;
use crate::{
    BoundRuleStep, Constraint, ConstraintPath, FrontmatterScalar, NonEmpty, Proposition, RefAnchor,
    ResolvedFrontmatterQuery, ResolvedRuleLocator, SchemaNode, SectionRule,
};

use super::diagnostic::{Diagnostic, DiagnosticId, DiagnosticReference};
use super::engine::{BoundScope, BoundSection};
use super::frontmatter_values::FrontmatterValues;
use super::prepare::PreparedQueries;

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

/// Evaluates an `fm[...]` proposition against one document (§4.6).
///
/// The two forms of the proposition differ in more than their answer. A bare
/// read is a *typed boolean read*: "every non-boolean, non-null result node
/// produces `invalid-value`, and the entire constraint containing the
/// proposition is suppressed". An equality proposition never invalidates
/// anything — "mappings and sequences never equal the literal" — so it is
/// existential typed equality over the non-null nodes and nothing more.
///
/// Every node is inspected before either form answers. §4.6 says so of the
/// bare read in as many words: "a true sibling result or another already-true
/// operand does not short-circuit that suppression". The pointers of the
/// offending nodes are appended to `invalid`; turning them into diagnostics
/// belongs to the caller, which knows the constraint they are attributed to.
pub(super) fn frontmatter_query_truth(
    root: Option<&serde_json::Value>,
    block_is_invalid: bool,
    prepared: &PreparedQuery,
    proposition: &ResolvedFrontmatterQuery,
    match_case: bool,
    invalid: &mut Vec<String>,
) -> Truth {
    // §4.6: "If the block is `invalid-frontmatter`, the query is unevaluated
    // and the entire containing constraint is suppressed."
    if block_is_invalid {
        return Truth::Suppressed;
    }
    // §4.6: "If the block is absent, the query produces an empty result: a
    // bare boolean read is unsatisfied, and an equality proposition is
    // unsatisfied."
    let Some(document) = root else {
        return Truth::Unsatisfied;
    };
    let nodes = prepared.evaluate(document);
    match proposition.equals() {
        // §4.6: "A bare `fm[...]` is a typed boolean read, not a presence
        // test. It is satisfied iff at least one result node is the YAML/JSON
        // boolean `true`."
        None => {
            let mut satisfied = false;
            let before = invalid.len();
            for (path, value) in nodes.iter() {
                match value {
                    serde_json::Value::Bool(true) => satisfied = true,
                    // "Boolean `false`, an empty result, and null are
                    // unsatisfied" — and neither is an invalid value.
                    serde_json::Value::Bool(false) | serde_json::Value::Null => {}
                    // §6.1 makes the pointer Outlint's own rendering of the
                    // node's path components, never the provider's spelling.
                    _ => invalid.push(path.render_pointer()),
                }
            }
            if invalid.len() > before {
                Truth::Suppressed
            } else {
                Truth::from_bool(satisfied)
            }
        }
        // §4.6: "Equality is existential over non-null result nodes [...]
        // satisfied iff at least one such node has the same resolved scalar
        // type and value."
        Some(expected) => Truth::from_bool(nodes.iter().any(|(_, value)| {
            !value.is_null() && frontmatter_scalar_equals(value, expected, match_case)
        })),
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
    pub(super) frontmatter: &'s FrontmatterValues<'d>,
    /// Every §4.6 query the schema spells, compiled once with the plan.
    pub(super) queries: &'s PreparedQueries,
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
        node: &ConstraintPath,
    ) -> ConstraintEvaluation<'s, 'd> {
        let mut resolved = Resolved::new(node.clone());
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
            ..
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
struct Resolved<'s, 'd> {
    /// The constraint being evaluated, which §6.2 makes the schema node an
    /// invalid boolean-read value is attributed to.
    node: ConstraintPath,
    occurrences: Vec<&'s BoundSection<'d>>,
    pending: Vec<Diagnostic>,
}

impl<'s, 'd> Resolved<'s, 'd> {
    fn new(node: ConstraintPath) -> Self {
        Self {
            node,
            occurrences: Vec::new(),
            pending: Vec::new(),
        }
    }

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
            Proposition::FrontmatterQuery(proposition) => self.query(context, proposition),
            // §4.6's `fm.<name>`, answered from the state the capture
            // evaluation retained rather than from the diagnostics it
            // produced.
            Proposition::FrontmatterCapture(reference) => {
                context.frontmatter.truth(reference.name())
            }
        }
    }

    /// Evaluates one `fm[...]` proposition, keeping the primaries it raises.
    fn query(&mut self, context: EvalCtx<'s, 'd>, proposition: &ResolvedFrontmatterQuery) -> Truth {
        let Some(prepared) = context.queries.get(proposition.query()) else {
            // Unreachable: preparation compiled every query the schema
            // spells, and this plan was built from that schema.
            return Truth::Unsatisfied;
        };
        let mut invalid = Vec::new();
        let truth = frontmatter_query_truth(
            context.frontmatter.root(),
            context.frontmatter.block_is_invalid(),
            prepared,
            proposition,
            context.match_case,
            &mut invalid,
        );
        for pointer in invalid {
            let message = format!(
                "the frontmatter query `{}` selects a value that is not a `bool`: a bare \
                 `fm[...]` reads a boolean rather than testing presence",
                proposition.query()
            );
            if let Some(diagnostic) = context.frontmatter.entry_diagnostic(
                DiagnosticId::InvalidValue,
                Some(pointer.clone()),
                &pointer,
                SchemaNode::Constraint(self.node.clone()),
                message,
            ) {
                self.pending.push(diagnostic);
            }
        }
        truth
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
