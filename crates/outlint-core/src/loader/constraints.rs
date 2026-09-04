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
    ResolvedRuleCaptureLocator, ResolvedRuleLocator, RuleIndex, RuleOutcome, Schema,
    SchemaErrorKind, ScopePath, SectionRule, SourceRange, UpperBound,
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
            let BoundOperand::Rule { locator, identity } = operand else {
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
            // §5.1: the locators must share one *concrete* scope, so the key
            // compared here keeps every non-terminal subscript and drops only
            // the terminal rule step.
            let Some((_, parent)) = identity.split_last() else {
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
            BoundOperand::Rule { locator, identity } => {
                Some((Proposition::Rule(locator), ResolvedIdentity::Rule(identity)))
            }
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
        let (mut rules, mut identity, mut captures) = match anchor {
            RefAnchor::SchemaRoot => (schema.addressed_root_rules(), Vec::new(), None),
            RefAnchor::CurrentScope => (
                rules_at_scope(schema, scope)?,
                attachment_identity(schema, scope),
                rule_at_scope(schema, scope).map(|rule| &rule.captures),
            ),
        };

        let mut steps: Vec<BoundRuleStep> = Vec::new();
        let mut singular: Vec<bool> = Vec::new();
        let mut denied = false;
        let mut capture = None;
        let name_steps = parsed.name_steps();
        let step_count = name_steps.rest.len() + 1;
        for (position, step) in name_steps.iter().enumerate() {
            let name = step.name().as_str();
            // §4.4: a name step inspects the rule ids of the current named
            // scope and the captures declared by the rule that opened it, and
            // nothing else. §4.3 makes those two sets disjoint within a scope.
            if let Some((index, rule)) = rules
                .iter()
                .enumerate()
                .find(|(_, rule)| rule.id.as_ref().is_some_and(|id| id.as_str() == name))
            {
                let id = rule
                    .id
                    .clone()
                    .expect("the rule was found by its declared id");
                steps.push(BoundRuleStep::new(
                    id,
                    RuleIndex(index),
                    step.position().cloned(),
                ));
                identity.push(CanonicalStep {
                    index,
                    selector: match step.position() {
                        Some(subscript) => ScopeSelector::ExplicitIndex(subscript.value().clone()),
                        None => ScopeSelector::ImplicitSingular,
                    },
                });
                singular.push(step.position().is_some() || is_statically_singular(rule));
                denied |= matches!(rule.outcome, RuleOutcome::Deny);
                captures = Some(&rule.captures);
                rules = &rule.sections;
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
        // §4.3: "a reference through a rule with `allow: false` is
        // `forbidden-ref`", so a denied step is refused whether it is the
        // target or merely on the way.
        if denied {
            self.error_at(
                SchemaErrorKind::ForbiddenRef,
                range,
                format!("ref `{source}` passes through or targets an allow: false rule"),
            );
            return None;
        }

        // §4.4: "Every non-terminal step MUST be singular [...] Only the
        // terminal step may remain plural." A capture and `/text` are terminal
        // values, so the rule step in front of either is itself non-terminal
        // and takes the same check.
        let non_terminal = if capture.is_some() || parsed.intrinsic_text().is_some() {
            steps.len()
        } else {
            steps.len().saturating_sub(1)
        };
        if let Some(plural) = (0..non_terminal).find(|index| !singular[*index]) {
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
                    steps[plural].id().as_str()
                ),
            );
            return None;
        }

        if let Some((name, value_type, subscript)) = capture {
            return Some(BoundOperand::Capture(ResolvedRuleCaptureLocator::new(
                source.clone(),
                anchor,
                steps,
                name,
                value_type,
                subscript,
            )));
        }
        let steps = non_empty(steps).expect("the locator grammar requires one name step");
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
        identity: Vec<CanonicalStep>,
    },
    Capture(ResolvedRuleCaptureLocator),
    IntrinsicText(ResolvedIntrinsicTextLocator),
    FrontmatterQuery(ResolvedFrontmatterQuery),
    FrontmatterCapture(ResolvedFrontmatterCapture),
}

/// One step of a locator's canonical §5.4 identity.
///
/// The index is the *declared* rule's position in its sibling scope, so a
/// relative and an absolute spelling of one rule produce the same step and
/// duplicate as §5.4 requires. The selector is what §5.1 compares to decide
/// whether two `ordered` locators share one concrete scope.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CanonicalStep {
    index: usize,
    selector: ScopeSelector,
}

/// Which occurrence of a rule a step's scope belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ScopeSelector {
    /// No `[i]`: the rule is statically singular, so it opens one scope.
    ImplicitSingular,
    /// An explicit `[i]`, kept as the arbitrary-precision index §4.4 requires.
    ExplicitIndex(BigUint),
    /// A repeatable ancestor of the constraint itself. §3.1 binds a rule's
    /// constraints per instance, so every operand of one constraint instance
    /// sits inside the same occurrence of that ancestor.
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
        rule.outcome,
        RuleOutcome::Allow(Cardinality {
            max: UpperBound::Bounded(0 | 1),
            ..
        })
    )
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
        rules = &rule.sections;
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
    let mut ordered = schema.options.ordered_sections;
    for step in structural_scope {
        let Some(rule) = rules.get(step.index) else {
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
        rules = &rule.sections;
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
