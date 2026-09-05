//! Constraint construction, locator binding, and proposition normalization.
//!
//! §4.4's binding-time principle divides the work here in two. The locator
//! kernel decides what a locator *says* without a schema; this module decides
//! what it *denotes*, against the built rule forest: every rule id, capture
//! name, and structural kind resolves at schema load, while concrete indices,
//! empty matched sets, frontmatter queries, and equality literals stay
//! document data.
//!
//! A name step resolves in exactly one named scope — the one the locator has
//! reached — and there is no upward or downward search anywhere below.

use std::collections::HashSet;

use num_bigint::BigUint;
use serde_json::Value;

use crate::locator::{parse_locator, ParsedLocator, UnboundOutlineLocator};
use crate::schema::resolved_anchor;
use crate::yaml::parse_frontmatter_scalar;
use crate::{
    AtLeastTwo, BoundRuleStep, CaptureName, Cardinality, Constraint, ConstraintIndex,
    ConstraintPath, FrontmatterScalar, NonEmpty, Proposition, RefAnchor,
    ResolvedFrontmatterCapture, ResolvedFrontmatterQuery, ResolvedIntrinsicTextLocator,
    ResolvedRuleCaptureLocator, ResolvedRuleLocator, RuleIndex, Schema, SchemaErrorKind, ScopePath,
    SectionRule, SourceRange, UpperBound,
};

use super::{Loader, RangeKey};

impl Loader {
    /// Refuses malformed constraint locators when no schema could be built.
    ///
    /// Without a rule forest nothing binds, so this checks syntax and nothing
    /// else: §4.4 makes invalid locator syntax `invalid-document-shape`, while
    /// an unbound name and a duplicate identity are both answers only a schema
    /// can give. Reporting either from here would invent a resolution failure
    /// for a locator that was never resolved.
    pub(super) fn validate_constraint_lexical_refs(&mut self) {
        let constraints = self.raw_constraints.clone();
        for (scope, values) in constraints {
            for (index, value) in values.iter().enumerate() {
                let range = self.range(RangeKey::Constraint(ConstraintPath {
                    scope: scope.clone(),
                    index: ConstraintIndex(index),
                }));
                for reference in constraint_ref_strings(value) {
                    if let Err(error) = parse_locator(reference) {
                        self.shape_error_at(
                            range,
                            format!("invalid locator `{reference}`: {error}"),
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
                let refs = self.parse_proposition_list(schema, scope, operand, range)?;
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
            // §5.5 reserves `equal-values`, `subset-values`, `select`,
            // `sequence`, and `numbered` without activating any syntax, so
            // each of them arrives here as an unknown keyword.
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
        let condition = self.parse_proposition(schema, scope, condition_value, range);
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
                self.parse_proposition(schema, scope, value, range)
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
        let mut parent_scope: Option<Vec<CanonicalStep>> = None;
        let mut mixed_scopes = false;
        let mut complete = true;
        for value in values {
            let Some(operand) = self.bind_operand(schema, scope, value, Context::Ordered, range)
            else {
                complete = false;
                continue;
            };
            let BoundOperand::Rule {
                locator,
                identity,
                scope: scope_key,
            } = operand
            else {
                // §5.1: every listed locator must terminate in a rule id;
                // anything else has no header position at all.
                self.error_at(
                    SchemaErrorKind::OrderedScopeMismatch,
                    range,
                    format!(
                        "ordered ref `{}` does not terminate in a rule id, so it has no header \
                         position",
                        value.as_str().unwrap_or_default()
                    ),
                );
                complete = false;
                continue;
            };
            // §5.1: the locators must share one *concrete* scope. The key
            // compared here keeps every non-terminal subscript that narrows
            // anything, and drops only the terminal rule step — the scope a
            // locator resolves *in* is everything above its target.
            let Some((_, parent)) = scope_key.split_last() else {
                continue;
            };
            let parent = parent.to_vec();
            if parent_scope
                .as_ref()
                .is_some_and(|existing| existing != &parent)
            {
                self.error_at(
                    SchemaErrorKind::OrderedScopeMismatch,
                    range,
                    "all ordered refs must resolve in the same concrete scope",
                );
                mixed_scopes = true;
            } else {
                parent_scope = Some(parent);
            }
            if !identities.insert(identity) {
                self.error_at(
                    SchemaErrorKind::DuplicateRef,
                    range,
                    "duplicate ref in ordered",
                );
            }
            refs.push(locator);
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
                .is_some_and(|parent| scope_is_ordered(schema, parent))
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
                self.parse_proposition(schema, scope, value, range)
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

    /// Binds one operand and admits only what §4.5 and §4.6 call a proposition.
    fn parse_proposition(
        &mut self,
        schema: &Schema,
        scope: &ScopePath,
        value: &Value,
        range: SourceRange,
    ) -> Option<(Proposition, ResolvedIdentity)> {
        match self.bind_operand(schema, scope, value, Context::Proposition, range)? {
            BoundOperand::Rule {
                locator, identity, ..
            } => Some((Proposition::Rule(locator), ResolvedIdentity::Rule(identity))),
            BoundOperand::FrontmatterQuery(proposition) => {
                let identity = query_identity(&proposition, schema.options.match_case);
                Some((Proposition::FrontmatterQuery(proposition), identity))
            }
            BoundOperand::FrontmatterCapture(proposition) => {
                let identity = ResolvedIdentity::FrontmatterCapture(proposition.name().clone());
                Some((Proposition::FrontmatterCapture(proposition), identity))
            }
            // §4.5: "Locators ending in a capture or intrinsic value are value
            // locators and are not propositions in this version", and §5's
            // table refuses them rather than projecting a value to a boolean.
            BoundOperand::Capture(locator) => {
                self.shape_error_at(
                    range,
                    format!(
                        "ref `{}` ends at a declared capture, which is a value and not a \
                         proposition",
                        locator.locator()
                    ),
                );
                None
            }
            BoundOperand::IntrinsicText(locator) => {
                self.shape_error_at(
                    range,
                    format!(
                        "ref `{}` ends at the `/text` intrinsic, which is a value and not a \
                         proposition",
                        locator.locator()
                    ),
                );
                None
            }
        }
    }

    /// Parses one operand and binds every schema name it spells.
    fn bind_operand(
        &mut self,
        schema: &Schema,
        scope: &ScopePath,
        value: &Value,
        context: Context,
        range: SourceRange,
    ) -> Option<BoundOperand> {
        let Some(source) = value.as_str() else {
            self.shape_error_at(range, "constraint refs must be strings");
            return None;
        };
        // §4.4: invalid locator syntax is `invalid-document-shape`. That
        // includes the retired dotted `fm.key=value` spelling, which `fm.`
        // now reads as one capture name.
        let parsed = match parse_locator(source) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.shape_error_at(range, format!("invalid locator `{source}`: {error}"));
                return None;
            }
        };
        match &parsed {
            ParsedLocator::Outline(outline) => {
                self.bind_outline(schema, scope, outline, context, range)
            }
            // §4.6: "`fm[$.x]` performs a document-time query, while `fm.x` is
            // the typo-safe reference to a declaration." Only the second binds
            // a schema name; the query's contents are document data.
            ParsedLocator::FrontmatterQuery(query) => {
                let equals = query.equality().map(parse_frontmatter_scalar);
                Some(BoundOperand::FrontmatterQuery(
                    ResolvedFrontmatterQuery::new(query.clone(), equals),
                ))
            }
            ParsedLocator::FrontmatterCapture(capture) => {
                let name = CaptureName(capture.name().as_str().to_owned());
                // §4.6: "Unknown capture names are `unresolved-ref`, even if a
                // YAML key of the same name exists."
                let Some(declaration) = schema.frontmatter.captures().get(&name) else {
                    self.error_at(
                        SchemaErrorKind::UnresolvedRef,
                        range,
                        format!("unresolved ref `{source}`"),
                    );
                    return None;
                };
                Some(BoundOperand::FrontmatterCapture(
                    ResolvedFrontmatterCapture::new(
                        capture.clone(),
                        name,
                        declaration.value_type(),
                    ),
                ))
            }
        }
    }

    /// Resolves an outline locator's names against the built rule forest.
    fn bind_outline(
        &mut self,
        schema: &Schema,
        scope: &ScopePath,
        parsed: &UnboundOutlineLocator,
        context: Context,
        range: SourceRange,
    ) -> Option<BoundOperand> {
        let source = parsed.source();
        let anchor = resolved_anchor(parsed.anchor());
        // §4.5: `$.` starts at the outermost named scope — the `outline` rules
        // in the general form, the `sections` rules under the sugar, whose
        // synthesized title rule is transparent and declares no captures. A
        // bare name starts in the scope the constraint is attached to, which
        // also exposes the captures of the rule that owns that scope.
        let (mut rules, prefix, mut captures) = match anchor {
            RefAnchor::SchemaRoot => (schema.addressed_root_rules(), Vec::new(), None),
            RefAnchor::CurrentScope => (
                rules_at_scope(schema, scope)?,
                attachment_identity(schema, scope),
                rule_at_scope(schema, scope).map(|rule| &rule.captures),
            ),
        };
        // The two keys start alike and part company only where a subscript is
        // written: an attachment ancestor carries none.
        let mut identity = prefix.clone();
        let mut scope_key = prefix;

        let mut first_step = None;
        let mut rest_steps = Vec::new();
        let mut singular: Vec<bool> = Vec::new();
        let mut capture = None;
        let name_steps = parsed.name_steps();
        let step_count = name_steps.rest.len() + 1;
        for (position, step) in name_steps.iter().enumerate() {
            let name = step.name().as_str();
            // §4.4: a name step inspects the rule ids of the current named
            // scope and the captures declared by the rule that opened it, and
            // nothing else. §4.3 makes those two sets disjoint within a scope.
            if let Some((index, rule, id)) = rules.iter().enumerate().find_map(|(index, rule)| {
                rule.id
                    .as_ref()
                    .filter(|id| id.as_str() == name)
                    .cloned()
                    .map(|id| (index, rule, id))
            }) {
                let singular_rule = is_statically_singular(rule);
                let bound_position = step.position().cloned();
                let selector = match &bound_position {
                    Some(subscript) => ScopeSelector::ExplicitIndex(subscript.value().clone()),
                    None => ScopeSelector::ImplicitSingular,
                };
                identity.push(CanonicalStep {
                    index,
                    selector: selector.clone(),
                });
                scope_key.push(CanonicalStep {
                    index,
                    selector: scope_equivalent(selector, singular_rule),
                });
                singular.push(bound_position.is_some() || singular_rule);
                let bound_step = BoundRuleStep::new(id, RuleIndex(index), bound_position);
                if first_step.is_some() {
                    rest_steps.push(bound_step);
                } else {
                    first_step = Some(bound_step);
                }
                captures = Some(&rule.captures);
                rules = rule.children.rules();
                continue;
            }
            let capture_name = CaptureName(name.to_owned());
            if let Some(declaration) = captures.and_then(|declared| declared.get(&capture_name)) {
                // §4.3: "A declared capture is a terminal typed value, not a
                // child scope."
                if position + 1 != step_count
                    || !parsed.structural_steps().is_empty()
                    || parsed.intrinsic_text().is_some()
                {
                    self.shape_error_at(
                        range,
                        format!(
                            "ref `{source}` continues past the declared capture `{name}`, which \
                             is a terminal value"
                        ),
                    );
                    return None;
                }
                capture = Some((
                    capture_name,
                    declaration.value_type(),
                    step.position().cloned(),
                ));
                break;
            }
            // §4.4 gives a name step exactly one scope, so a name that is
            // neither a rule id nor a declared capture there resolves nowhere.
            self.error_at(
                SchemaErrorKind::UnresolvedRef,
                range,
                format!("unresolved ref `{source}`"),
            );
            return None;
        }

        // §4.4: a schema-resident structural kind step "MUST land on a declared
        // structural rule of that kind", and this version declares no content
        // or item rules, so none of them binds.
        if let Some(structural) = parsed.structural_steps().first() {
            self.error_at(
                SchemaErrorKind::UnresolvedRef,
                range,
                format!(
                    "unresolved ref `{source}`: the structural kind `/{}` is not allocated in \
                     this version",
                    structural.kind().as_str()
                ),
            );
            return None;
        }
        // §4.4: "Every non-terminal step MUST be singular [...] Only the
        // terminal step may remain plural." A capture and `/text` are terminal
        // values, so the rule step in front of either is itself non-terminal
        // and takes the same check.
        let step_count = usize::from(first_step.is_some()) + rest_steps.len();
        let non_terminal = if capture.is_some() || parsed.intrinsic_text().is_some() {
            step_count
        } else {
            step_count.saturating_sub(1)
        };
        let bound_steps = first_step.iter().chain(&rest_steps);
        if let Some((_, plural)) = singular
            .iter()
            .zip(bound_steps)
            .take(non_terminal)
            .find(|(singular, _)| !**singular)
        {
            // §5.1 gives `ordered` its own, more specific error for the same
            // condition.
            let kind = match context {
                Context::Ordered => SchemaErrorKind::OrderedScopeMismatch,
                Context::Proposition => SchemaErrorKind::InvalidDocumentShape,
            };
            self.error_at(
                kind,
                range,
                format!(
                    "ref `{source}` descends through the repeatable rule `{}`; narrow that step \
                     with `[i]`",
                    plural.id().as_str()
                ),
            );
            return None;
        }

        if let Some((name, value_type, subscript)) = capture {
            let steps = first_step.into_iter().chain(rest_steps).collect();
            return Some(BoundOperand::Capture(ResolvedRuleCaptureLocator::new(
                source.clone(),
                anchor,
                steps,
                name,
                value_type,
                subscript,
            )));
        }
        let Some(first) = first_step else {
            // The parsed locator has a non-empty name path. Every name either
            // bound a rule step, returned an error above, or terminated at a
            // capture (also returned above), so this state cannot arise from
            // schema input. Keep the fallback structured if those cases ever
            // change instead of turning an internal mismatch into a panic.
            self.shape_error_at(range, format!("ref `{source}` has no bound name step"));
            return None;
        };
        let steps = NonEmpty {
            first,
            rest: rest_steps,
        };
        if let Some(text) = parsed.intrinsic_text() {
            return Some(BoundOperand::IntrinsicText(
                ResolvedIntrinsicTextLocator::new(
                    source.clone(),
                    anchor,
                    steps,
                    text.position().cloned(),
                ),
            ));
        }
        Some(BoundOperand::Rule {
            locator: ResolvedRuleLocator::new(source.clone(), anchor, steps),
            identity,
            scope: scope_key,
        })
    }
}

/// Which constraint position an operand was written in.
///
/// The two differ only in the diagnostic id they assign to one fault: §5.1
/// gives `ordered` `ordered-scope-mismatch` where an ordinary position takes
/// §4.4's `invalid-document-shape`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
    Proposition,
    Ordered,
}

/// One successfully bound constraint operand.
///
/// Every terminal kind §4.4 can reach has its own variant, so a consuming
/// context refuses what it does not accept by matching rather than by
/// inspecting a locator after the fact.
enum BoundOperand {
    Rule {
        locator: ResolvedRuleLocator,
        /// The §5.4 duplicate identity: every subscript exactly as written.
        identity: Vec<CanonicalStep>,
        /// The §5.1 concrete-scope key: the same steps with each subscript
        /// reduced to the occurrence it can actually denote.
        scope: Vec<CanonicalStep>,
    },
    Capture(ResolvedRuleCaptureLocator),
    IntrinsicText(ResolvedIntrinsicTextLocator),
    FrontmatterQuery(ResolvedFrontmatterQuery),
    FrontmatterCapture(ResolvedFrontmatterCapture),
}

/// One step of a locator's canonical key.
///
/// The index is the *declared* rule's position in its sibling scope, so a
/// relative and an absolute spelling of one rule produce the same step and
/// duplicate as §5.4 requires.
///
/// Two keys are built from these, and they answer different questions.
/// §5.4's duplicate identity is about *spelling*: `owner.x` and
/// `owner[0].x` are two ways of writing one locator and stay two locators.
/// §5.1's scope key is about *denotation*: it asks which concrete scope a
/// locator resolves in, so a subscript that narrows nothing is reduced away
/// by [`scope_equivalent`] before the comparison.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CanonicalStep {
    index: usize,
    selector: ScopeSelector,
}

/// Which occurrence of a rule a step's scope belongs to.
///
/// [`ImplicitSingular`](Self::ImplicitSingular) and
/// [`ExplicitIndex`](Self::ExplicitIndex) are not simply "unsubscripted" and
/// "subscripted": on a statically singular rule the two can name the same
/// single occurrence, which is what [`scope_equivalent`] settles.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ScopeSelector {
    /// No `[i]`: the rule is statically singular, so it opens one scope.
    ImplicitSingular,
    /// An explicit `[i]`, kept as the arbitrary-precision index §4.4 requires.
    ExplicitIndex(BigUint),
    /// A repeatable ancestor of the constraint itself. §3.1 binds a rule's
    /// constraints per instance, so every operand of one constraint instance
    /// sits inside the same occurrence of that ancestor.
    ///
    /// No comparison can currently tell this from
    /// [`Self::ImplicitSingular`]: only the attachment path can put a
    /// repeatable ancestor in a key, every operand of one constraint shares
    /// that path, and an absolute operand reaching the same ancestor must
    /// narrow it — which yields [`Self::ExplicitIndex`], distinct from both.
    /// It is kept apart anyway, because recording a repeatable ancestor as
    /// "singular" would make the key say something false the moment a
    /// consumer can traverse one without a subscript.
    CurrentOccurrence,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ResolvedIdentity {
    Rule(Vec<CanonicalStep>),
    /// §5.4: "frontmatter captures duplicate when they name the same
    /// declaration."
    FrontmatterCapture(CaptureName),
    /// §5.4: `fm[...]` propositions duplicate "when their query source is
    /// identical and either both lack equality or their equality literals
    /// resolve to values equal under §4.6".
    ///
    /// The query is compared as written, so "syntactically different JSONPath
    /// queries are not treated as duplicates merely because they may select
    /// the same nodes". A bare read carries no literal at all, which is what
    /// keeps `fm[$.x]` and `fm[$.x]=` apart.
    FrontmatterQuery {
        query: String,
        equals: Option<FrontmatterScalar>,
    },
}

/// The §5.4 identity of one `fm[...]` proposition.
///
/// §4.6 makes string equality follow `options.match_case`, so two literals
/// that compare equal against a document must also compare equal here.
fn query_identity(proposition: &ResolvedFrontmatterQuery, match_case: bool) -> ResolvedIdentity {
    let mut equals = proposition.equals().cloned();
    if !match_case {
        if let Some(FrontmatterScalar::String(value)) = &mut equals {
            *value = crate::case_fold::simple_fold(value).collect();
        }
    }
    ResolvedIdentity::FrontmatterQuery {
        query: proposition.query().to_owned(),
        equals,
    }
}

/// Whether a rule's effective maximum makes an unnarrowed step singular.
fn is_statically_singular(rule: &SectionRule) -> bool {
    matches!(
        rule.cardinality,
        Cardinality {
            max: UpperBound::Bounded(0 | 1),
            ..
        }
    )
}

/// Reduces a step's selector to the occurrence it can actually denote.
///
/// §4.4 permits `[i]` "after any step that produces a node list", and a
/// statically singular rule produces a list of at most one node — so `[0]` on
/// such a step selects that one node whenever it exists and nothing when it
/// does not, which is exactly what the unsubscripted step does. The two
/// spellings therefore denote one concrete scope, and §5.1 compares concrete
/// scopes rather than spellings.
///
/// An index of one or more is left alone. On a singular rule it names a
/// position that rule can never occupy, so its scope is not the singular
/// occurrence's scope and never shares one with it; on a repeatable rule it
/// names a specific occurrence among several, which is the whole point of
/// writing it.
fn scope_equivalent(selector: ScopeSelector, singular_rule: bool) -> ScopeSelector {
    match selector {
        ScopeSelector::ExplicitIndex(index) if singular_rule && index == BigUint::from(0_u8) => {
            ScopeSelector::ImplicitSingular
        }
        other => other,
    }
}

/// The canonical prefix a relative locator inherits from its attachment path.
fn attachment_identity(schema: &Schema, scope: &ScopePath) -> Vec<CanonicalStep> {
    let mut rules = schema.addressed_root_rules();
    let mut identity = Vec::with_capacity(scope.0.len());
    for index in &scope.0 {
        let Some(rule) = rules.get(index.0) else {
            break;
        };
        identity.push(CanonicalStep {
            index: index.0,
            selector: if is_statically_singular(rule) {
                ScopeSelector::ImplicitSingular
            } else {
                ScopeSelector::CurrentOccurrence
            },
        });
        rules = rule.children.rules();
    }
    identity
}

/// Whether the scope a canonical step path names binds its rules in document
/// order.
///
/// The empty path is the addressed root — the outline scope or the sugar's
/// `sections` scope — which follows `options.ordered_sections`, as the
/// synthesized title rule does.
fn scope_is_ordered(schema: &Schema, structural_scope: &[CanonicalStep]) -> bool {
    let mut rules = schema.addressed_root_rules();
    let mut ordered = match &schema.document {
        crate::DocumentShape::Outline(scope) => scope.mode == crate::ScopeMode::Ordered,
        crate::DocumentShape::Title(title) => match &title.children {
            crate::ChildScope::Declared(scope) => scope.mode == crate::ScopeMode::Ordered,
            _ => true,
        },
    };
    for step in structural_scope {
        let Some(rule) = rules.get(step.index) else {
            return ordered;
        };
        ordered = match &rule.children {
            crate::ChildScope::Declared(scope) => scope.mode == crate::ScopeMode::Ordered,
            _ => true,
        };
        rules = rule.children.rules();
    }
    ordered
}

fn rules_at_scope<'a>(schema: &'a Schema, scope: &ScopePath) -> Option<&'a [SectionRule]> {
    let mut rules = schema.addressed_root_rules();
    for index in &scope.0 {
        let rule = rules.get(index.0)?;
        rules = rule.children.rules();
    }
    Some(rules)
}

/// The rule that owns a scope, and therefore declares its capture names.
///
/// The addressed root is owned by no rule: the general form's outline scope
/// has nothing above it, and the sugar's synthesized title rule is transparent
/// and declares nothing (§4.5).
fn rule_at_scope<'a>(schema: &'a Schema, scope: &ScopePath) -> Option<&'a SectionRule> {
    let mut rules = schema.addressed_root_rules();
    let mut owner = None;
    for index in &scope.0 {
        let rule = rules.get(index.0)?;
        owner = Some(rule);
        rules = rule.children.rules();
    }
    owner
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
    let root = match &mut schema.document {
        crate::DocumentShape::Outline(root) => root,
        crate::DocumentShape::Title(title) => match &mut title.children {
            crate::ChildScope::Declared(root) => root,
            _ => return None,
        },
    };
    if scope.0.is_empty() {
        Some(&mut root.constraints)
    } else {
        constraints_in_rules_mut(&mut root.rules, &scope.0)
    }
}

fn constraints_in_rules_mut<'a>(
    rules: &'a mut [SectionRule],
    path: &[RuleIndex],
) -> Option<&'a mut Vec<Constraint>> {
    let (index, rest) = path.split_first()?;
    let rule = rules.get_mut(index.0)?;
    if rest.is_empty() {
        match &mut rule.children {
            crate::ChildScope::Declared(scope) => Some(&mut scope.constraints),
            _ => None,
        }
    } else {
        constraints_in_rules_mut(
            match &mut rule.children {
                crate::ChildScope::Declared(scope) => &mut scope.rules,
                _ => return None,
            },
            rest,
        )
    }
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
