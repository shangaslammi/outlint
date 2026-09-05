//! Construction of options, named scopes, rules, matchers, cardinalities,
//! capture declarations, and value orders.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

use crate::matcher::{compile_anchored_pattern, compile_glob_pattern};
use crate::regex_capture;
use crate::typed_value::ValueType;
use crate::{
    CaptureName, CapturePath, Cardinality, ChildScope, DeclaredScope, ExactText, ExtrasMode,
    GlobPattern, GuardIndex, GuardPath, Matcher, NonEmpty, Options, OrderEntryPath, OrderIndex,
    RegexPattern, RelatedLocation, RuleCapture, RuleId, RuleIndex, RulePath, SchemaErrorKind,
    SchemaNode, ScopeMode, ScopePath, SectionGuard, SectionRule, SourceRange, UpperBound,
    ValueOrderDirection, ValueOrderEntry,
};

use super::shape::{CAPTURES_FIELD, ORDER_FIELD};
use super::{Loader, RangeKey, RawOptions, RawRule};

impl Loader {
    /// Builds the general `outline:` form: the canonical `h1`-rule list.
    ///
    /// Outline rules are ordinary rules — `id`, `strict`, any cardinality and
    /// nested constraints all mean what they mean in every other scope — so
    /// the list is built by the same scope builder, at the empty scope the
    /// rules semantically live in. Only their source spelling differs, which
    /// [`Loader::source_key`] maps on range lookup.
    ///
    /// An empty outline is refused rather than accepted as vacuous: an empty
    /// rule list constrains nothing (the outline scope is open, so `h1`
    /// headers would pass unvalidated), while the schema author who writes it
    /// almost certainly means "this document has no `h1`" — which
    /// `title: null` declares, keeping a `sections` list for the real top
    /// level. Accepting `outline: []` would validate nothing and pass every
    /// document silently.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_outline_scope(
        &mut self,
        entries: Vec<RawRule>,
        root_scope: &ScopePath,
        match_case: bool,
        guards: Vec<super::RawGuard>,
        extras: Option<String>,
        unordered: Option<bool>,
        constraints: Vec<Value>,
    ) -> Option<DeclaredScope> {
        self.build_declared_scope(
            entries,
            guards,
            extras,
            unordered,
            constraints,
            root_scope,
            match_case,
        )
    }

    pub(super) fn build_options(raw: &RawOptions) -> Options {
        Options {
            match_case: raw.match_case.unwrap_or(false),
            strip_inline_markup: raw.strip_inline_markup.unwrap_or(true),
            allow_skipped_levels: raw.allow_skipped_levels.unwrap_or(false),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_child_scope(
        &mut self,
        rules: Option<Vec<RawRule>>,
        guards: Vec<super::RawGuard>,
        extras: Option<String>,
        unordered: Option<bool>,
        constraints: Vec<Value>,
        scope: &ScopePath,
        match_case: bool,
        owner: Option<(&RulePath, &BTreeMap<CaptureName, RuleCapture>)>,
    ) -> Option<ChildScope> {
        match rules {
            Some(rules) => self
                .build_declared_scope_with_owner(
                    rules,
                    guards,
                    extras,
                    unordered,
                    constraints,
                    scope,
                    match_case,
                    owner,
                )
                .map(ChildScope::Declared),
            None if guards.is_empty() => Some(ChildScope::Omitted),
            None => self
                .build_guards(guards, scope, match_case)
                .and_then(|guards| {
                    let mut iter = guards.into_iter();
                    let first = iter.next()?;
                    Some(ChildScope::GuardsOnly(NonEmpty {
                        first,
                        rest: iter.collect(),
                    }))
                }),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_declared_scope(
        &mut self,
        rules: Vec<RawRule>,
        guards: Vec<super::RawGuard>,
        extras: Option<String>,
        unordered: Option<bool>,
        constraints: Vec<Value>,
        scope: &ScopePath,
        match_case: bool,
    ) -> Option<DeclaredScope> {
        self.build_declared_scope_with_owner(
            rules,
            guards,
            extras,
            unordered,
            constraints,
            scope,
            match_case,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_declared_scope_with_owner(
        &mut self,
        rules: Vec<RawRule>,
        guards: Vec<super::RawGuard>,
        extras: Option<String>,
        unordered: Option<bool>,
        constraints: Vec<Value>,
        scope: &ScopePath,
        match_case: bool,
        owner: Option<(&RulePath, &BTreeMap<CaptureName, RuleCapture>)>,
    ) -> Option<DeclaredScope> {
        self.raw_constraints.insert(scope.clone(), constraints);
        let mode = if unordered == Some(true) {
            ScopeMode::Unordered
        } else {
            ScopeMode::Ordered
        };
        let semantic_rules = self.build_named_scope(rules, scope, match_case, owner);
        let semantic_guards = self.build_guards(guards, scope, match_case);
        if mode == ScopeMode::Unordered {
            let mut wildcard_seen = false;
            if let Some(built) = semantic_rules.as_ref() {
                for (index, rule) in built.iter().enumerate() {
                    if wildcard_seen {
                        let path = RulePath {
                            scope: scope.clone(),
                            index: RuleIndex(index),
                        };
                        self.error_at(
                            SchemaErrorKind::UnreachableRule,
                            self.range(RangeKey::RuleField(path, "match".into())),
                            "rule is unreachable after the first wildcard in an unordered scope",
                        );
                    }
                    wildcard_seen |= matches!(rule.matcher, Matcher::Any);
                }
            }
        }
        match (semantic_rules, semantic_guards) {
            (Some(rules), Some(guards)) => Some(DeclaredScope {
                rules,
                guards,
                extras: if extras.is_some() {
                    ExtrasMode::Anywhere
                } else {
                    ExtrasMode::Reject
                },
                mode,
                constraints: Vec::new(),
            }),
            _ => None,
        }
    }

    fn build_guards(
        &mut self,
        guards: Vec<super::RawGuard>,
        scope: &ScopePath,
        match_case: bool,
    ) -> Option<Vec<SectionGuard>> {
        let mut built = Vec::with_capacity(guards.len());
        let mut complete = true;
        for (index, guard) in guards.into_iter().enumerate() {
            let path = GuardPath {
                scope: scope.clone(),
                index: GuardIndex(index),
            };
            let range = self.range(RangeKey::GuardField(path.clone(), "match".into()));
            self.nodes.insert(SchemaNode::Guard(path), range);
            match self.build_matcher(&guard.matcher, match_case, range) {
                Some(matcher) => built.push(SectionGuard { matcher }),
                None => complete = false,
            }
        }
        complete.then_some(built)
    }

    /// Builds one named scope: a rule list plus the captures of the rule that
    /// opens the scope those rules live in.
    ///
    /// `owner` is `None` for the schema root, and for a rule whose `captures`
    /// mapping never normalized — §2.1 enters a name into the scope only once
    /// its mapping is well-formed, so a failed mapping contributes nothing to
    /// compare against rather than contributing a partial set.
    fn build_named_scope(
        &mut self,
        rules: Vec<RawRule>,
        scope: &ScopePath,
        match_case: bool,
        owner: Option<(&RulePath, &BTreeMap<CaptureName, RuleCapture>)>,
    ) -> Option<Vec<SectionRule>> {
        let mut semantic = Vec::with_capacity(rules.len());
        // Collected for every rule, not only the ones that built: an id is a
        // declaration in this scope whether or not the rule around it turned
        // out to be constructible, and §4.3 compares declarations.
        let mut ids = Vec::with_capacity(rules.len());
        let mut complete = true;
        for (index, raw) in rules.into_iter().enumerate() {
            let rule_path = RulePath {
                scope: scope.clone(),
                index: RuleIndex(index),
            };
            let rule_range = self.range(RangeKey::Rule(rule_path.clone()));
            self.nodes
                .insert(SchemaNode::Rule(rule_path.clone()), rule_range);
            let mut child_scope = scope.clone();
            child_scope.0.push(RuleIndex(index));

            let matcher_range = self.range(RangeKey::RuleField(rule_path.clone(), "match".into()));
            let matcher = self.build_matcher(&raw.matcher, match_case, matcher_range);
            let id_range = self.range(RangeKey::RuleField(
                rule_path.clone(),
                if raw.id.is_some() { "id" } else { "match" }.into(),
            ));
            let id = self.build_rule_id(raw.id.as_deref(), matcher.as_ref(), scope, id_range);
            ids.push(id.clone());
            let cardinality_field = if raw.repeat.is_some() {
                "repeat"
            } else if raw.required.is_some() {
                "required"
            } else {
                "match"
            };
            let outcome_range = self.range(RangeKey::RuleField(
                rule_path.clone(),
                cardinality_field.into(),
            ));
            let cardinality = self.build_cardinality(
                raw.required,
                raw.repeat.as_deref(),
                matcher.as_ref(),
                outcome_range,
            );
            let captures =
                self.build_rule_captures(raw.captures.as_ref(), &rule_path, matcher.as_ref());
            let order = self.build_rule_order(
                raw.order.as_ref(),
                &rule_path,
                captures.as_ref(),
                cardinality,
            );
            let children = self.build_child_scope(
                raw.sections,
                raw.forbid_sections,
                raw.extras,
                raw.unordered,
                raw.constraints,
                &child_scope,
                match_case,
                captures.as_ref().map(|entries| (&rule_path, entries)),
            );
            match (matcher, cardinality, children, captures, order) {
                (Some(matcher), Some(cardinality), Some(children), Some(captures), Some(order)) => {
                    semantic.push(SectionRule {
                        id,
                        matcher,
                        cardinality,
                        children,
                        captures,
                        order,
                    });
                }
                _ => complete = false,
            }
        }

        complete &= self.check_named_scope(scope, owner, &ids);
        complete.then_some(semantic)
    }

    /// Reports every §4.3 collision in one named scope, and says whether the
    /// scope came out free of them.
    ///
    /// The scope's names are the opening rule's valid captures together with
    /// the valid explicit and default ids of the rules directly in it — no
    /// deeper, and not the opening rule's own id, which is a name in the scope
    /// above. §6.3 anchors a collision at whichever declaration the document
    /// spells second and relates the first, so the names are put into
    /// schema-document order before they are compared: the capture mapping's
    /// own order is a `BTreeMap`'s, not the source's.
    fn check_named_scope(
        &mut self,
        scope: &ScopePath,
        owner: Option<(&RulePath, &BTreeMap<CaptureName, RuleCapture>)>,
        ids: &[Option<RuleId>],
    ) -> bool {
        // `None` marks a capture; `Some(index)` the sibling rule that declared
        // the id.
        let mut declarations: Vec<(String, SourceRange, Option<usize>)> = Vec::new();
        if let Some((owner_path, captures)) = owner {
            let field = self.range(RangeKey::RuleField(
                owner_path.clone(),
                CAPTURES_FIELD.into(),
            ));
            declarations.extend(captures.keys().map(|name| {
                (
                    name.as_str().to_owned(),
                    self.capture_declaration_range(owner_path, name.as_str(), field),
                    None,
                )
            }));
        }
        for (index, id) in ids.iter().enumerate() {
            let Some(id) = id else { continue };
            let path = RulePath {
                scope: scope.clone(),
                index: RuleIndex(index),
            };
            declarations.push((id.0.clone(), self.rule_id_range(&path), Some(index)));
        }
        // Stable, so two declarations sharing one range — an alias expanded
        // twice — keep the order they were collected in.
        declarations.sort_by_key(|(_, range, _)| (range.source, range.range.start));

        let mut earliest: HashMap<&str, (SourceRange, Option<usize>)> = HashMap::new();
        let mut free = true;
        for (name, range, origin) in &declarations {
            match earliest.get(name.as_str()) {
                Some((first_range, first_origin)) => {
                    free = false;
                    let (message, related) = match (first_origin, origin) {
                        (Some(first_index), Some(_)) => (
                            format!("duplicate rule id `{name}` in one scope"),
                            format!("first declared by sibling rule {first_index}"),
                        ),
                        (Some(first_index), None) => (
                            format!(
                                "capture `{name}` collides with a rule id in the same named scope"
                            ),
                            format!("first declared by sibling rule {first_index}"),
                        ),
                        (None, _) => (
                            format!(
                                "rule id `{name}` collides with a capture in the same named scope"
                            ),
                            format!("capture `{name}` declared here"),
                        ),
                    };
                    self.error_with_related_at(
                        SchemaErrorKind::DuplicateId,
                        *range,
                        message,
                        vec![RelatedLocation {
                            range: *first_range,
                            message: related,
                        }],
                    );
                }
                None => {
                    earliest.insert(name, (*range, *origin));
                }
            }
        }
        free
    }

    /// Normalizes one rule's `captures` mapping (§2.1, §2.2, §2.4).
    ///
    /// Three outcomes are kept apart, because §6.3 makes them mean different
    /// things downstream: an absent mapping and a fully valid one both yield
    /// `Some` — the absent one empty — while a mapping that failed anywhere
    /// yields `None`. `None` is not "no captures": it says the mapping never
    /// became a collection at all, so no name from it enters the named scope
    /// (§4.3), no `order` entry may resolve against it, and the rule cannot be
    /// built. Every reason for `None` has already reported itself here, except
    /// the one case §6.3 requires be left silent: a matcher that never
    /// compiled says nothing about the groups its captures would have named.
    fn build_rule_captures(
        &mut self,
        raw: Option<&Value>,
        rule_path: &RulePath,
        matcher: Option<&Matcher>,
    ) -> Option<BTreeMap<CaptureName, RuleCapture>> {
        let field = self.range(RangeKey::RuleField(
            rule_path.clone(),
            CAPTURES_FIELD.into(),
        ));
        let Some(raw) = raw else {
            if self.field_is_spelled(rule_path, CAPTURES_FIELD) {
                self.error_at(
                    SchemaErrorKind::InvalidCapture,
                    field,
                    "rule `captures` must be a non-empty mapping and cannot be null",
                );
                return None;
            }
            return Some(BTreeMap::new());
        };
        let Some(declared) = raw.as_object() else {
            self.error_at(
                SchemaErrorKind::InvalidCapture,
                field,
                "rule `captures` must be a mapping from capture names to types",
            );
            return None;
        };
        if declared.is_empty() {
            self.error_at(
                SchemaErrorKind::InvalidCapture,
                field,
                "rule `captures` must declare at least one capture",
            );
            return None;
        }

        // The JSON object sorted the mapping's keys, and a reader meets the
        // declarations in the order the document spells them; the retained
        // declaration ranges put that order back.
        let mut ordered = declared
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str(),
                    value,
                    self.capture_declaration_range(rule_path, name, field),
                )
            })
            .collect::<Vec<_>>();
        ordered.sort_by_key(|(name, _, range)| (range.source, range.range.start, *name));

        // One analysis serves every declaration: it reports the ancestor facts
        // of all named groups at once, and the pattern does not change between
        // declarations.
        let groups = match matcher {
            Some(Matcher::Regex(pattern)) => regex_capture::analyze(&pattern.0).ok(),
            _ => None,
        };

        let mut entries = BTreeMap::new();
        let mut valid = true;
        for (name, value, range) in ordered {
            let Some(value_type) = self.build_rule_capture(name, value, range, matcher) else {
                valid = false;
                continue;
            };
            let Some(analysis) = groups.as_ref() else {
                // Unreachable by construction: `compile_anchored_pattern` and
                // `regex_capture::analyze` share one pinned `regex-syntax`, so
                // a body the matcher accepted parses here too. Were that ever
                // to diverge, the group facts would be unknown rather than
                // favourable, so the declaration is refused rather than
                // credited with a participation it was never shown to have.
                valid = false;
                continue;
            };
            let Some(ancestors) = analysis.get(name) else {
                self.error_at(
                    SchemaErrorKind::InvalidCapture,
                    range,
                    format!("capture `{name}` names no group in this rule's regex"),
                );
                valid = false;
                continue;
            };
            if ancestors.alternation || ancestors.min_zero_repetition {
                let cause = match (ancestors.alternation, ancestors.min_zero_repetition) {
                    (true, true) => "an alternation and a zero-minimum repetition",
                    (true, false) => "an alternation",
                    _ => "a zero-minimum repetition",
                };
                self.error_at(
                    SchemaErrorKind::InvalidCapture,
                    range,
                    format!(
                        "capture `{name}` is enclosed by {cause}, so its group does not \
                         participate in every match"
                    ),
                );
                valid = false;
                continue;
            }
            let name = CaptureName(name.to_owned());
            self.nodes.insert(
                SchemaNode::Capture(CapturePath {
                    rule: rule_path.clone(),
                    name: name.clone(),
                }),
                range,
            );
            entries.insert(name, RuleCapture::new(value_type));
        }
        valid.then_some(entries)
    }

    /// Checks one capture declaration's name, type, and rule context.
    ///
    /// Returns the resolved type when the declaration is admissible so far;
    /// the group facts of §2.2 are checked by the caller, which holds the one
    /// analysis they all read.
    fn build_rule_capture(
        &mut self,
        name: &str,
        value: &Value,
        range: SourceRange,
        matcher: Option<&Matcher>,
    ) -> Option<ValueType> {
        if !is_capture_name(name) {
            self.error_at(
                SchemaErrorKind::InvalidCapture,
                range,
                format!("capture name `{name}` must match `[a-z][a-z0-9_]*`"),
            );
            return None;
        }
        let Some(declared) = value.as_str() else {
            self.error_at(
                SchemaErrorKind::InvalidCapture,
                range,
                format!("capture `{name}` must declare its type as a string"),
            );
            return None;
        };
        let Some(value_type) = ValueType::from_name(declared) else {
            self.error_at(
                SchemaErrorKind::InvalidCapture,
                range,
                format!("capture `{name}` declares unknown type `{declared}`"),
            );
            return None;
        };
        match matcher {
            // §6.3: a matcher that never compiled is already an
            // `invalid-matcher`, and it left no form to judge this
            // declaration against. The capture collection still fails, so
            // nothing partial reaches the model, but silently.
            None => return None,
            Some(Matcher::Regex(_)) => {}
            Some(_) => {
                self.error_at(
                    SchemaErrorKind::InvalidCapture,
                    range,
                    format!(
                        "capture `{name}` needs a regex matcher; only a regex declares the \
                         named groups a capture binds"
                    ),
                );
                return None;
            }
        }
        Some(value_type)
    }

    /// Normalizes one rule's `order` list (§2.1, §3.8).
    ///
    /// Four checks run, and §6.3 makes them independent of one another. An
    /// entry's own shape, field set, and value types are decided against the
    /// entry alone, and so are the duplicates that appear once defaults are
    /// applied; only `by` needs the capture mapping, and only the maximum
    /// needs the cardinality. So a rule whose captures or whose `repeat` never
    /// normalized still reports every structural fault its `order` has, and
    /// reports nothing it would have to guess the lost value to know.
    ///
    /// Independence is symmetric, so it also holds within one entry: only the
    /// entry's own structure gates the other three, and past that a duplicate,
    /// an undeclared `by`, and an unrepeatable rule are each reported wherever
    /// they apply. An entry wrong in two independent ways therefore says so
    /// twice, rather than one check silently consuming what the next would
    /// have found.
    ///
    /// Like [`Self::build_rule_captures`], `None` distinguishes a collection
    /// that never became one from an absent declaration's valid empty list.
    fn build_rule_order(
        &mut self,
        raw: Option<&Value>,
        rule_path: &RulePath,
        captures: Option<&BTreeMap<CaptureName, RuleCapture>>,
        cardinality: Option<Cardinality>,
    ) -> Option<Vec<ValueOrderEntry>> {
        let field = self.range(RangeKey::RuleField(rule_path.clone(), ORDER_FIELD.into()));
        let Some(raw) = raw else {
            if self.field_is_spelled(rule_path, ORDER_FIELD) {
                self.error_at(
                    SchemaErrorKind::InvalidOrder,
                    field,
                    "rule `order` must be a non-empty list and cannot be null",
                );
                return None;
            }
            return Some(Vec::new());
        };
        let Some(elements) = raw.as_array() else {
            self.error_at(
                SchemaErrorKind::InvalidOrder,
                field,
                "rule `order` must be a list of order entries",
            );
            return None;
        };
        if elements.is_empty() {
            self.error_at(
                SchemaErrorKind::InvalidOrder,
                field,
                "rule `order` must declare at least one entry",
            );
            return None;
        }

        // An entry is addressed by its position whether or not anything in it
        // is understood, so every element gets its node before any of them is
        // read.
        //
        // The four checks that follow are independent in the sense of §6.3:
        // only an entry's own structure gates the rest, and a fault one check
        // finds never hides what another would have found. So faults
        // accumulate against the entry they belong to and are reported
        // together, in document order — no pass consumes the entries a later
        // pass has still to read.
        let mut reports = Vec::with_capacity(elements.len());
        for (index, element) in elements.iter().enumerate() {
            let order_index = OrderIndex(index);
            let range = self.range(RangeKey::RuleOrderEntry(rule_path.clone(), order_index));
            self.nodes.insert(
                SchemaNode::OrderEntry(OrderEntryPath {
                    rule: rule_path.clone(),
                    order_index,
                }),
                range,
            );
            let (entry, faults) = parse_order_entry(element);
            reports.push(OrderEntryReport {
                range,
                entry,
                faults,
            });
        }

        // §3.8 compares entries after defaults are applied, so `by: v` and
        // `{by: v, dir: asc, strict: false}` are the same entry twice. The
        // later spelling is the one a reader meets as the repetition.
        let mut seen = HashSet::new();
        for report in &mut reports {
            let Some(entry) = &report.entry else { continue };
            let key = (entry.by.clone(), entry.direction, entry.strict);
            if !seen.insert(key) {
                report.faults.push(format!(
                    "`order` already declares this ordering of capture `{}`",
                    entry.by
                ));
            }
        }

        // §6.3: the names `by` would resolve against were never entered when
        // the capture mapping failed, so the resolution is not attempted
        // rather than failed. Nothing else here reads those names, and the
        // collection still cannot be built.
        let captures_known = captures.is_some();
        if let Some(captures) = captures {
            for report in &mut reports {
                let Some(entry) = &report.entry else { continue };
                if !captures.contains_key(&CaptureName(entry.by.clone())) {
                    report.faults.push(format!(
                        "`order` entry `by: {}` names no capture declared by this rule",
                        entry.by
                    ));
                }
            }
        }

        // §3.8 orders a rule's own repeated matches, which a rule that can
        // match at most once does not have. Every entry is refused, including
        // the ones already faulted for a reason of their own: the maximum is a
        // fact about the rule, not about any entry, so no entry escapes it by
        // being wrong in some other way first. A cardinality that never
        // normalized supplies no maximum to test, so the check is skipped
        // rather than run against an invented one.
        if let Some(cardinality) = cardinality {
            if matches!(cardinality.max, UpperBound::Bounded(max) if max <= 1) {
                for report in reports.iter_mut().filter(|report| report.entry.is_some()) {
                    report.faults.push(
                        "`order` needs a rule that can match more than once, and this rule's \
                         effective maximum is one"
                            .to_owned(),
                    );
                }
            }
        }

        let mut complete = captures_known;
        let mut entries = Vec::with_capacity(reports.len());
        for report in reports {
            for fault in &report.faults {
                self.error_at(SchemaErrorKind::InvalidOrder, report.range, fault.clone());
            }
            match report {
                OrderEntryReport {
                    entry: Some(entry),
                    faults,
                    ..
                } if faults.is_empty() => entries.push(ValueOrderEntry {
                    by: CaptureName(entry.by),
                    direction: entry.direction,
                    strict: entry.strict,
                }),
                _ => complete = false,
            }
        }

        complete.then_some(entries)
    }

    /// The range of one capture declaration — its key through its value —
    /// falling back to the `captures` collection when the key had no scalar
    /// spelling of its own to anchor at.
    fn capture_declaration_range(
        &self,
        rule_path: &RulePath,
        name: &str,
        fallback: SourceRange,
    ) -> SourceRange {
        self.ranges.get(
            &self.source_key(RangeKey::RuleCapture(rule_path.clone(), name.to_owned())),
            fallback,
        )
    }

    /// Whether a rule spelled `field:` at all.
    ///
    /// Serde cannot answer this for `captures` and `order`: an explicit null
    /// deserializes to the same `None` an absent key does. The range index
    /// keeps a range for every key the source wrote, so presence there is the
    /// distinction the two need.
    fn field_is_spelled(&self, rule_path: &RulePath, field: &str) -> bool {
        self.ranges
            .ranges
            .contains_key(&self.source_key(RangeKey::RuleField(rule_path.clone(), field.into())))
    }

    fn build_rule_id(
        &mut self,
        explicit: Option<&str>,
        matcher: Option<&Matcher>,
        scope: &ScopePath,
        range: SourceRange,
    ) -> Option<RuleId> {
        if let Some(id) = explicit {
            if !is_slug(id) {
                self.error_at(
                    SchemaErrorKind::InvalidDocumentShape,
                    range,
                    format!("rule id `{id}` is not a lowercase slug"),
                );
                return None;
            }
            if let Some(purpose) = scope.0.is_empty().then(|| reserved_root_id(id)).flatten() {
                self.error_at(
                    SchemaErrorKind::ReservedId,
                    range,
                    format!("top-level rule id `{id}` is reserved for {purpose}"),
                );
            }
            return Some(RuleId(id.to_owned()));
        }

        let Matcher::Exact(text) = matcher? else {
            return None;
        };
        let generated = auto_id(&text.0).map(RuleId);
        let reserved = generated
            .as_ref()
            .filter(|_| scope.0.is_empty())
            .and_then(|id| Some((id.0.as_str(), reserved_root_id(&id.0)?)));
        if let Some((id, purpose)) = reserved {
            self.error_at(
                SchemaErrorKind::ReservedId,
                range,
                format!("top-level auto-generated rule id `{id}` is reserved for {purpose}"),
            );
        }
        generated
    }

    pub(super) fn build_matcher(
        &mut self,
        source: &str,
        match_case: bool,
        range: SourceRange,
    ) -> Option<Matcher> {
        if source == "*" {
            return Some(Matcher::Any);
        }
        if source.starts_with('/') && source.ends_with('/') {
            let Some(body) = source
                .strip_prefix('/')
                .and_then(|body| body.strip_suffix('/'))
            else {
                self.error_at(
                    SchemaErrorKind::InvalidMatcher,
                    range,
                    "a regex matcher needs separate opening and closing `/` delimiters",
                );
                return None;
            };
            let Some(body) = regex_body(body) else {
                self.error_at(
                    SchemaErrorKind::InvalidMatcher,
                    range,
                    format!("regex matcher `{source}` contains an unescaped `/`"),
                );
                return None;
            };
            if let Err(error) = compile_anchored_pattern(&body, match_case, false) {
                self.error_at(
                    SchemaErrorKind::InvalidMatcher,
                    range,
                    format!("invalid regex matcher `{source}`: {error}"),
                );
                return None;
            }
            return Some(Matcher::Regex(RegexPattern(body)));
        }
        if source.contains('*') {
            if let Err(error) = compile_glob_pattern(source, match_case) {
                self.error_at(
                    SchemaErrorKind::InvalidMatcher,
                    range,
                    format!("invalid glob matcher `{source}`: {error}"),
                );
                return None;
            }
            return Some(Matcher::Glob(GlobPattern(source.to_owned())));
        }
        Some(Matcher::Exact(ExactText(source.to_owned())))
    }

    fn build_cardinality(
        &mut self,
        required: Option<bool>,
        repeat: Option<&str>,
        matcher: Option<&Matcher>,
        range: SourceRange,
    ) -> Option<Cardinality> {
        if required.is_some() && repeat.is_some() {
            self.error_at(
                SchemaErrorKind::ConflictingCardinality,
                range,
                "required and repeat cannot both be declared",
            );
            return None;
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
                    self.error_at(
                        SchemaErrorKind::InvalidRepeat,
                        range,
                        format!("invalid repeat `{repeat}`"),
                    );
                    return None;
                }
            },
            (None, None) if matches!(matcher, Some(Matcher::Exact(_))) => Cardinality {
                min: 1,
                max: UpperBound::Bounded(1),
            },
            (None, None) => {
                self.error_at(
                    SchemaErrorKind::MissingCardinality,
                    range,
                    "regex, glob, and wildcard rules must declare `required` or `repeat`",
                );
                return None;
            }
            (Some(_), Some(_)) => return None,
        };
        Some(cardinality)
    }
}

/// What a name is reserved for at the schema root, or `None` when it is an
/// ordinary top-level rule id (§4.1).
///
/// The reservation is on top-level *rule ids* only: a nested rule may take
/// either name, and §2.2 gives capture names no reserved words at all.
fn reserved_root_id(id: &str) -> Option<&'static str> {
    match id {
        "fm" => Some("frontmatter refs"),
        // §4.1 holds the name for a later document source; it has no
        // behavior in this version.
        "linkdefs" => Some("a later document source"),
        _ => None,
    }
}

/// Parses one `order` entry's own shape, independently of every other entry
/// and of the rule around it (§2.1).
///
/// Faults within one entry are independent of each other, so all of them are
/// collected rather than only the first, and the entry normalizes only when
/// none was found. Nothing is reported from here: the caller holds the
/// entry's range and reports its faults beside those the checks that need the
/// rule's captures and cardinality add later.
fn parse_order_entry(element: &Value) -> (Option<OrderEntry>, Vec<String>) {
    let mut faults = Vec::new();
    let Some(mapping) = element.as_object() else {
        faults.push("each `order` entry must be a mapping".to_owned());
        return (None, faults);
    };
    for key in mapping.keys() {
        if !matches!(key.as_str(), "by" | "dir" | "strict") {
            faults.push(format!("unknown `order` entry field `{key}`"));
        }
    }
    let by = match mapping.get("by") {
        Some(Value::String(by)) => Some(by.clone()),
        Some(_) => {
            faults.push("`order` entry `by` must be a capture name string".to_owned());
            None
        }
        None => {
            faults.push("each `order` entry must declare `by`".to_owned());
            None
        }
    };
    let direction = match mapping.get("dir") {
        None => Some(ValueOrderDirection::Ascending),
        Some(Value::String(dir)) if dir == "asc" => Some(ValueOrderDirection::Ascending),
        Some(Value::String(dir)) if dir == "desc" => Some(ValueOrderDirection::Descending),
        Some(_) => {
            faults.push("`order` entry `dir` must be `asc` or `desc`".to_owned());
            None
        }
    };
    let strict = match mapping.get("strict") {
        None => Some(false),
        Some(Value::Bool(strict)) => Some(*strict),
        Some(_) => {
            faults.push("`order` entry `strict` must be a bool".to_owned());
            None
        }
    };
    let form = match (by, direction, strict) {
        (Some(by), Some(direction), Some(strict)) if faults.is_empty() => Some(OrderEntry {
            by,
            direction,
            strict,
        }),
        _ => None,
    };
    (form, faults)
}

/// One `order` entry as the normalization sees it: where it is, what it
/// normalized to if it did, and every fault found against it (§3.8).
///
/// Faults accumulate rather than replacing the normalized form, so a check
/// that rejects an entry does not remove it from the checks that follow. §6.3
/// makes the four independent — only the entry's own structure gates the rest
/// — and an entry that is wrong in two independent ways says so twice.
struct OrderEntryReport {
    range: SourceRange,
    entry: Option<OrderEntry>,
    faults: Vec<String>,
}

/// One `order` entry after its own shape is normalized but before `by` is
/// resolved (§3.8).
///
/// `by` is still the raw spelling: a [`CaptureName`] is a name that reached
/// the model, and this one has not yet been shown to name a capture of the
/// rule that wrote it. Duplicate detection reads this form, so two entries
/// that agree are found whether or not either resolves.
struct OrderEntry {
    by: String,
    direction: ValueOrderDirection,
    strict: bool,
}

/// Whether a string is a capture name under the §2.2 grammar
/// `[a-z][a-z0-9_]*`.
///
/// Deliberately distinct from [`is_slug`]: a rule id separates words with
/// `-`, a capture name with `_`, and neither spelling is admissible as the
/// other. The test is over bytes, so every non-ASCII scalar is rejected by
/// the same arm that rejects an ASCII one outside the set.
pub(super) fn is_capture_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

pub(super) fn is_slug(value: &str) -> bool {
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

pub(super) fn auto_id(value: &str) -> Option<String> {
    let mut result = String::new();
    let mut separator_pending = false;
    for character in value.nfkd().flat_map(char::to_lowercase) {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            if separator_pending && !result.is_empty() {
                result.push('-');
            }
            result.push(character);
            separator_pending = false;
        } else if is_combining_mark(character) {
            // NFKD splits letters such as `ä` into an ASCII base followed by
            // a combining mark. The mark modifies that base; it is not a word
            // boundary and therefore must not introduce a slug separator.
        } else {
            separator_pending = true;
        }
    }
    (!result.is_empty()).then_some(result)
}

pub(super) fn regex_body(source: &str) -> Option<String> {
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

pub(super) fn parse_repeat(source: &str) -> Option<Cardinality> {
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
