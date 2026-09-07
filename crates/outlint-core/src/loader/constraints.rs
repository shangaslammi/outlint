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

use crate::locator::{parse_locator, ParsedLocator, StructuralStep, UnboundOutlineLocator};
use crate::schema::resolved_anchor;
use crate::yaml::parse_frontmatter_scalar;
use crate::{
    AtLeastTwo, BlockMatcher, BoundRuleStep, CaptureName, Constraint, ConstraintIndex,
    ConstraintPath, ContentOwner, ContentRule, ContentRulePath, ContentScope, FrontmatterScalar,
    ItemRule, ItemRulePath, ItemScope, Matcher, NonEmpty, Proposition, RefAnchor,
    ResolvedFrontmatterCapture, ResolvedFrontmatterQuery, ResolvedIntrinsicTextLocator,
    ResolvedRuleCaptureLocator, ResolvedRuleLocator, RuleIndex, RulePath, Schema, SchemaErrorKind,
    ScopePath, SectionRule, SourceRange, UpperBound,
};

use super::rules::{NamedEntry, NamedScope};
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
                 remove this constraint, or set `unordered: true` on that scope",
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
            BoundOperand::StructuralTerminal => {
                self.shape_error_at(
                    range,
                    "structural content and item locators are not propositions",
                );
                None
            }
            BoundOperand::ItemTextTerminal => {
                self.shape_error_at(range, "item `/text` is a value and not a proposition");
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
        let (mut named_scope, prefix) = match anchor {
            RefAnchor::SchemaRoot => (NamedScope::Sections(ScopePath(Vec::new())), Vec::new()),
            RefAnchor::CurrentScope => (
                NamedScope::Sections(scope.clone()),
                attachment_identity(schema, scope),
            ),
        };
        // The two keys start alike and part company only where a subscript is
        // written: an attachment ancestor carries none.
        let mut identity = prefix.clone();
        let mut scope_key = prefix;

        let mut first_step = None;
        let mut rest_steps = Vec::new();
        let mut names = Vec::new();
        let mut cursor = BinderCursor::SectionScope;
        let name_steps = parsed.name_steps();
        let step_count = name_steps.rest.len() + 1;
        for (position, step) in name_steps.iter().enumerate() {
            let name = step.name().as_str();
            let declaration = self
                .namespaces
                .get(&named_scope)
                .and_then(|entries| entries.iter().find(|entry| entry.name == name))
                .cloned();
            let Some(declaration) = declaration else {
                self.error_at(
                    SchemaErrorKind::UnresolvedRef,
                    range,
                    format!("unresolved ref `{source}`"),
                );
                return None;
            };
            let bound_position = step.position().cloned();
            let effective_maximum = declaration.entry.effective_maximum();
            names.push(BoundName {
                spelling: name.to_owned(),
                narrowed: bound_position.is_some(),
                statically_singular: effective_maximum.is_some_and(is_singular_maximum),
            });
            match declaration.entry {
                NamedEntry::Section { path, .. } => {
                    let rule = section_rule_at_path(schema, &path)?;
                    let selector = match &bound_position {
                        Some(subscript) => ScopeSelector::ExplicitIndex(subscript.value().clone()),
                        None => ScopeSelector::ImplicitSingular,
                    };
                    identity.push(CanonicalStep {
                        index: path.index.0,
                        selector: selector.clone(),
                    });
                    scope_key.push(CanonicalStep {
                        index: path.index.0,
                        selector: scope_equivalent(
                            selector,
                            is_singular_maximum(rule.cardinality.max()),
                        ),
                    });
                    let bound_step =
                        BoundRuleStep::new(rule.id.clone()?, path.index, bound_position);
                    if first_step.is_some() {
                        rest_steps.push(bound_step);
                    } else {
                        first_step = Some(bound_step);
                    }
                    let mut child_scope = path.scope.clone();
                    child_scope.0.push(path.index);
                    named_scope = NamedScope::Sections(child_scope);
                    cursor = BinderCursor::Section(path);
                }
                NamedEntry::Content { path, .. } => {
                    named_scope = NamedScope::Content(path.clone());
                    cursor = BinderCursor::Content(path);
                }
                NamedEntry::Item { path, .. } => {
                    named_scope = NamedScope::Item(path.clone());
                    cursor = BinderCursor::NamedItem(path);
                }
                NamedEntry::Capture(path) => {
                    if position + 1 != step_count
                        || !parsed.structural_steps().is_empty()
                        || parsed.intrinsic_text().is_some()
                    {
                        self.shape_error_at(
                            range,
                            format!(
                                "ref `{source}` continues past the declared capture `{name}`, \
                                 which is a terminal value"
                            ),
                        );
                        return None;
                    }
                    for name in names.iter().take(names.len().saturating_sub(1)) {
                        if !name.narrowed && !name.statically_singular {
                            self.plural_step_error(context, range, source.as_str(), &name.spelling);
                            return None;
                        }
                    }
                    let declaration = section_rule_at_path(schema, &path.rule)?
                        .captures
                        .get(&path.name)?;
                    let steps = first_step.into_iter().chain(rest_steps).collect();
                    return Some(BoundOperand::Capture(ResolvedRuleCaptureLocator::new(
                        source.clone(),
                        anchor,
                        steps,
                        path.name,
                        declaration.value_type(),
                        bound_position,
                    )));
                }
            }
        }

        let structural_steps: &[StructuralStep] = parsed.structural_steps();
        for structural in structural_steps {
            cursor = match structural.kind().as_str() {
                "p" => bind_paragraph_step(schema, &cursor),
                "list" => bind_list_step(schema, &cursor),
                "item" => bind_item_step(schema, &cursor),
                kind => {
                    self.error_at(
                        SchemaErrorKind::UnresolvedRef,
                        range,
                        format!(
                            "unresolved ref `{source}`: structural kind `/{kind}` is not allocated"
                        ),
                    );
                    return None;
                }
            }
            .or_else(|| {
                self.error_at(
                    SchemaErrorKind::UnresolvedRef,
                    range,
                    format!(
                        "unresolved ref `{source}`: no specific `/{}` declaration exists here",
                        structural.kind().as_str()
                    ),
                );
                None
            })?;
        }

        // §4.4 assigns plurality an error only after the complete locator is
        // otherwise known to bind. Resolve every structural declaration first
        // so an unallocated kind or missing declaration keeps `unresolved-ref`.
        let item_text_predecessor = structural_steps.is_empty()
            && parsed.intrinsic_text().is_some()
            && matches!(cursor, BinderCursor::NamedItem(_));
        let non_terminal_names = names.len().saturating_sub(usize::from(
            structural_steps.is_empty() && parsed.intrinsic_text().is_none(),
        ));
        for (index, name) in names.iter().take(non_terminal_names).enumerate() {
            if item_text_predecessor && index + 1 == names.len() {
                continue;
            }
            if !name.narrowed && !name.statically_singular {
                self.plural_step_error(context, range, source.as_str(), &name.spelling);
                return None;
            }
        }
        for (index, structural) in structural_steps.iter().enumerate() {
            let non_terminal =
                index + 1 < structural_steps.len() || parsed.intrinsic_text().is_some();
            if non_terminal && structural.position().is_none() {
                self.plural_step_error(
                    context,
                    range,
                    source.as_str(),
                    &format!("/{}", structural.kind().as_str()),
                );
                return None;
            }
        }

        if let Some(text) = parsed.intrinsic_text() {
            return match cursor {
                BinderCursor::Section(_) => {
                    let Some(first) = first_step else {
                        self.shape_error_at(range, format!("ref `{source}` has no bound heading"));
                        return None;
                    };
                    Some(BoundOperand::IntrinsicText(
                        ResolvedIntrinsicTextLocator::new(
                            source.clone(),
                            anchor,
                            NonEmpty {
                                first,
                                rest: rest_steps,
                            },
                            text.position().cloned(),
                        ),
                    ))
                }
                BinderCursor::NamedItem(path) => self.bind_item_text_intrinsic(
                    schema,
                    names.last().map(|name| (&path, name)),
                    source.as_str(),
                    range,
                ),
                BinderCursor::Items => {
                    self.bind_item_text_intrinsic(schema, None, source.as_str(), range)
                }
                _ => {
                    self.shape_error_at(
                        range,
                        format!("ref `{source}` has no `/text` intrinsic at this terminal"),
                    );
                    None
                }
            };
        }

        if matches!(cursor, BinderCursor::Section(_)) {
            let first = first_step?;
            Some(BoundOperand::Rule {
                locator: ResolvedRuleLocator::new(
                    source.clone(),
                    anchor,
                    NonEmpty {
                        first,
                        rest: rest_steps,
                    },
                ),
                identity,
                scope: scope_key,
            })
        } else {
            Some(BoundOperand::StructuralTerminal)
        }
    }

    fn plural_step_error(
        &mut self,
        context: Context,
        range: SourceRange,
        source: &str,
        step: &str,
    ) {
        let kind = match context {
            Context::Ordered => SchemaErrorKind::OrderedScopeMismatch,
            Context::Proposition => SchemaErrorKind::InvalidDocumentShape,
        };
        let message = if step.starts_with('/') {
            format!("ref `{source}` descends through plural step `{step}`; narrow it with `[i]`")
        } else {
            format!(
                "ref `{source}` descends through the repeatable rule `{step}`; narrow that step \
                 with `[i]`"
            )
        };
        self.error_at(kind, range, message);
    }

    /// The complete provisional §4.4 policy for schema-resident item `/text`.
    fn bind_item_text_intrinsic(
        &mut self,
        schema: &Schema,
        predecessor: Option<(&ItemRulePath, &BoundName)>,
        source: &str,
        range: SourceRange,
    ) -> Option<BoundOperand> {
        let Some((path, predecessor)) = predecessor else {
            self.shape_error_at(
                range,
                format!("ref `{source}` uses `/text` after structural `/item`"),
            );
            return None;
        };
        let Some(item) = item_rule_at_path(schema, path) else {
            self.shape_error_at(
                range,
                format!("ref `{source}` has no named item before `/text`"),
            );
            return None;
        };
        if matches!(item.matcher, Matcher::Any) {
            self.shape_error_at(
                range,
                format!("ref `{source}` uses `/text` after a wildcard item rule"),
            );
            return None;
        }
        if !predecessor.narrowed && !predecessor.statically_singular {
            self.shape_error_at(
                range,
                format!(
                    "ref `{source}` uses `/text` after plural item rule `{}`; narrow it with `[i]`",
                    predecessor.spelling
                ),
            );
            return None;
        }
        Some(BoundOperand::ItemTextTerminal)
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
    StructuralTerminal,
    ItemTextTerminal,
    FrontmatterQuery(ResolvedFrontmatterQuery),
    FrontmatterCapture(ResolvedFrontmatterCapture),
}

/// A schema cursor whose variants encode the only legal next structural step.
enum BinderCursor {
    SectionScope,
    Section(RulePath),
    Content(ContentRulePath),
    NamedItem(ItemRulePath),
    Paragraphs,
    Lists(Vec<ContentRulePath>),
    Items,
}

struct BoundName {
    spelling: String,
    narrowed: bool,
    statically_singular: bool,
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

fn bind_paragraph_step(schema: &Schema, cursor: &BinderCursor) -> Option<BinderCursor> {
    let scope = match cursor {
        BinderCursor::Section(path) => &section_rule_at_path(schema, path)?.content,
        BinderCursor::SectionScope
        | BinderCursor::Content(_)
        | BinderCursor::NamedItem(_)
        | BinderCursor::Paragraphs
        | BinderCursor::Lists(_)
        | BinderCursor::Items => return None,
    };
    content_rules(scope)
        .iter()
        .any(content_rule_declares_paragraph)
        .then_some(BinderCursor::Paragraphs)
}

fn bind_list_step(schema: &Schema, cursor: &BinderCursor) -> Option<BinderCursor> {
    let BinderCursor::Section(section) = cursor else {
        return None;
    };
    let rules = content_rules(&section_rule_at_path(schema, section)?.content);
    let mut lists = Vec::new();
    let mut declared = false;
    for (index, rule) in rules.iter().enumerate() {
        match rule {
            ContentRule::List { .. } => {
                declared = true;
                lists.push(ContentRulePath {
                    owner: ContentOwner::Rule(section.clone()),
                    index: crate::ContentRuleIndex(index),
                });
            }
            ContentRule::OneOf { alternatives, .. }
                if alternatives
                    .iter()
                    .any(|alternative| matches!(alternative, BlockMatcher::List { .. })) =>
            {
                declared = true;
            }
            ContentRule::Paragraph { .. } | ContentRule::Any { .. } | ContentRule::OneOf { .. } => {
            }
        }
    }
    declared.then_some(BinderCursor::Lists(lists))
}

fn bind_item_step(schema: &Schema, cursor: &BinderCursor) -> Option<BinderCursor> {
    let declared = match cursor {
        BinderCursor::Content(path) => list_rule_has_items(content_rule_at_path(schema, path)?),
        BinderCursor::Lists(paths) => paths
            .iter()
            .any(|path| content_rule_at_path(schema, path).is_some_and(list_rule_has_items)),
        BinderCursor::SectionScope
        | BinderCursor::Section(_)
        | BinderCursor::NamedItem(_)
        | BinderCursor::Paragraphs
        | BinderCursor::Items => false,
    };
    declared.then_some(BinderCursor::Items)
}

fn content_rule_declares_paragraph(rule: &ContentRule) -> bool {
    match rule {
        ContentRule::Paragraph { .. } => true,
        ContentRule::OneOf { alternatives, .. } => alternatives
            .iter()
            .any(|alternative| matches!(alternative, BlockMatcher::Paragraph)),
        ContentRule::List { .. } | ContentRule::Any { .. } => false,
    }
}

fn list_rule_has_items(rule: &ContentRule) -> bool {
    matches!(
        rule,
        ContentRule::List {
            items: ItemScope::Declared(items),
            ..
        } if !items.is_empty()
    )
}

fn content_rules(scope: &ContentScope) -> &[ContentRule] {
    match scope {
        ContentScope::Omitted => &[],
        ContentScope::Declared(rules) => rules,
    }
}

fn section_rule_at_path<'a>(schema: &'a Schema, path: &RulePath) -> Option<&'a SectionRule> {
    rules_at_scope(schema, &path.scope)?.get(path.index.0)
}

fn content_rule_at_path<'a>(schema: &'a Schema, path: &ContentRulePath) -> Option<&'a ContentRule> {
    let scope = match &path.owner {
        ContentOwner::Document => match &schema.document {
            crate::DocumentShape::Outline { content, .. } => content,
            crate::DocumentShape::Title(crate::TitleSlot::Forbidden { content, .. }) => content,
            crate::DocumentShape::Title(_) => return None,
        },
        ContentOwner::Title => match &schema.document {
            crate::DocumentShape::Title(title) => title.content(),
            crate::DocumentShape::Outline { .. } => return None,
        },
        ContentOwner::Rule(rule) => &section_rule_at_path(schema, rule)?.content,
    };
    content_rules(scope).get(path.index.0)
}

fn item_rule_at_path<'a>(schema: &'a Schema, path: &ItemRulePath) -> Option<&'a ItemRule> {
    let ContentRule::List {
        items: ItemScope::Declared(items),
        ..
    } = content_rule_at_path(schema, &path.content)?
    else {
        return None;
    };
    items.get(path.index.0)
}

fn is_singular_maximum(maximum: UpperBound) -> bool {
    matches!(maximum, UpperBound::Bounded(0 | 1))
}

/// Whether a rule's effective maximum makes an unnarrowed step singular.
fn is_statically_singular(rule: &SectionRule) -> bool {
    is_singular_maximum(rule.cardinality.max())
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
/// `sections` scope — and every declared scope carries its own local mode.
fn scope_is_ordered(schema: &Schema, structural_scope: &[CanonicalStep]) -> bool {
    let mut rules = schema.addressed_root_rules();
    let mut ordered = match &schema.document {
        crate::DocumentShape::Outline { scope, .. } => scope.mode == crate::ScopeMode::Ordered,
        crate::DocumentShape::Title(title) => match title.children() {
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
        crate::DocumentShape::Outline { scope, .. } => scope,
        crate::DocumentShape::Title(title) => match title.children_mut() {
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
