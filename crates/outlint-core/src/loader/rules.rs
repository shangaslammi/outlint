//! Construction of options, scopes, rules, matchers, and cardinalities.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

use crate::matcher::{compile_anchored_pattern, compile_glob_pattern};
use crate::regex_capture;
use crate::typed_value::ValueType;
use crate::{
    CaptureName, CapturePath, Cardinality, ExactText, GlobPattern, Matcher, Options,
    OrderEntryPath, OrderIndex, RegexPattern, RelatedLocation, RuleCapture, RuleId, RuleIndex,
    RuleOutcome, RulePath, SchemaErrorKind, SchemaNode, ScopePath, SectionRule, SourceRange,
    UpperBound, ValueOrderDirection, ValueOrderEntry,
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
    pub(super) fn build_outline_scope(
        &mut self,
        entries: Vec<RawRule>,
        root_scope: &ScopePath,
        match_case: bool,
        ordered_default: bool,
    ) -> Option<Vec<SectionRule>> {
        if entries.is_empty() {
            self.shape_error_at(
                self.range(RangeKey::DocumentField("outline".into())),
                "outline must declare at least one rule; a document with no h1 headers \
                 is declared with `title: null`",
            );
            return None;
        }
        self.build_scope(entries, root_scope, match_case, ordered_default)
    }

    pub(super) fn build_options(raw: &RawOptions) -> Options {
        Options {
            match_case: raw.match_case.unwrap_or(false),
            strip_inline_markup: raw.strip_inline_markup.unwrap_or(true),
            allow_skipped_levels: raw.allow_skipped_levels.unwrap_or(false),
            ordered_sections: raw.ordered_sections.unwrap_or(true),
        }
    }

    pub(super) fn build_scope(
        &mut self,
        rules: Vec<RawRule>,
        scope: &ScopePath,
        match_case: bool,
        ordered_default: bool,
    ) -> Option<Vec<SectionRule>> {
        let mut semantic = Vec::with_capacity(rules.len());
        let mut semantic_indices = Vec::with_capacity(rules.len());
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
            self.raw_constraints
                .insert(child_scope.clone(), raw.constraints);

            let matcher_range = self.range(RangeKey::RuleField(rule_path.clone(), "match".into()));
            let matcher = self.build_matcher(&raw.matcher, match_case, matcher_range);
            let id_range = self.range(RangeKey::RuleField(
                rule_path.clone(),
                if raw.id.is_some() { "id" } else { "match" }.into(),
            ));
            let id = self.build_rule_id(raw.id.as_deref(), matcher.as_ref(), scope, id_range);
            let cardinality_field = if raw.repeat.is_some() {
                "repeat"
            } else if raw.required.is_some() {
                "required"
            } else {
                "allow"
            };
            let outcome_range = self.range(RangeKey::RuleField(
                rule_path.clone(),
                cardinality_field.into(),
            ));
            let outcome = self.build_outcome(
                raw.allow,
                raw.required,
                raw.repeat.as_deref(),
                outcome_range,
            );
            let captures = self.build_rule_captures(
                raw.captures.as_ref(),
                &rule_path,
                matcher.as_ref(),
                raw.allow,
            );
            let order =
                self.build_rule_order(raw.order.as_ref(), &rule_path, captures.as_ref(), outcome);
            let children =
                self.build_scope(raw.sections, &child_scope, match_case, ordered_default);
            match (matcher, outcome, children, captures, order) {
                (Some(matcher), Some(outcome), Some(sections), Some(captures), Some(order)) => {
                    semantic_indices.push(index);
                    semantic.push(SectionRule {
                        id,
                        matcher,
                        outcome,
                        strict: raw.strict,
                        ordered: raw.ordered.unwrap_or(ordered_default),
                        sections,
                        constraints: Vec::new(),
                        captures,
                        order,
                    });
                }
                _ => complete = false,
            }
        }

        let mut ids: HashMap<RuleId, usize> = HashMap::new();
        for (&index, rule) in semantic_indices.iter().zip(&semantic) {
            let Some(id) = &rule.id else { continue };
            if let Some(first_index) = ids.get(id).copied() {
                let duplicate_path = RulePath {
                    scope: scope.clone(),
                    index: RuleIndex(index),
                };
                let first_path = RulePath {
                    scope: scope.clone(),
                    index: RuleIndex(first_index),
                };
                self.error_with_related_at(
                    SchemaErrorKind::DuplicateId,
                    self.rule_id_range(&duplicate_path),
                    format!("duplicate rule id `{}` in one scope", id.0),
                    vec![RelatedLocation {
                        range: self.rule_id_range(&first_path),
                        message: format!("first declared by sibling rule {first_index}"),
                    }],
                );
                complete = false;
            } else {
                ids.insert(id.clone(), index);
            }
        }

        complete.then_some(semantic)
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
        allow: bool,
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
            let Some(value_type) = self.build_rule_capture(name, value, range, matcher, allow)
            else {
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
        allow: bool,
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
        if !allow {
            self.error_at(
                SchemaErrorKind::InvalidCapture,
                range,
                format!("capture `{name}` cannot be declared on an `allow: false` rule"),
            );
            return None;
        }
        Some(value_type)
    }

    /// Normalizes one rule's `order` list (§2.1, §3.8).
    ///
    /// The parse runs in two halves, because §6.3 makes only one of them
    /// depend on anything: an entry's own shape, field set, and value types
    /// are decided against the entry alone, and so are the duplicates that
    /// appear once defaults are applied. Only `by` needs the capture mapping,
    /// and only the maximum needs the cardinality, so a rule whose captures or
    /// whose `repeat` never normalized still reports every structural fault
    /// its `order` has — and reports nothing that would have to guess at the
    /// value it lost.
    ///
    /// Like [`Self::build_rule_captures`], `None` distinguishes a collection
    /// that never became one from an absent declaration's valid empty list.
    fn build_rule_order(
        &mut self,
        raw: Option<&Value>,
        rule_path: &RulePath,
        captures: Option<&BTreeMap<CaptureName, RuleCapture>>,
        outcome: Option<RuleOutcome>,
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
        let mut entries = Vec::with_capacity(elements.len());
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
            let parsed = self.parse_order_entry(element, range);
            entries.push((range, parsed));
        }
        let mut complete = entries.iter().all(|(_, entry)| entry.is_some());

        // §3.8 compares entries after defaults are applied, so `by: v` and
        // `{by: v, dir: asc, strict: false}` are the same entry twice. The
        // later spelling is the one a reader meets as the repetition.
        let mut seen = HashSet::new();
        let duplicates = entries
            .iter()
            .enumerate()
            .filter_map(|(index, (range, entry))| {
                let entry = entry.as_ref()?;
                let key = (entry.by.clone(), entry.direction, entry.strict);
                (!seen.insert(key)).then(|| (index, *range, entry.by.clone()))
            })
            .collect::<Vec<_>>();
        for (index, range, by) in duplicates {
            self.error_at(
                SchemaErrorKind::InvalidOrder,
                range,
                format!("`order` already declares this ordering of capture `{by}`"),
            );
            entries[index].1 = None;
            complete = false;
        }

        match captures {
            Some(captures) => {
                let unresolved = entries
                    .iter()
                    .enumerate()
                    .filter_map(|(index, (range, entry))| {
                        let entry = entry.as_ref()?;
                        let declared = captures.contains_key(&CaptureName(entry.by.clone()));
                        (!declared).then(|| (index, *range, entry.by.clone()))
                    })
                    .collect::<Vec<_>>();
                for (index, range, by) in unresolved {
                    self.error_at(
                        SchemaErrorKind::InvalidOrder,
                        range,
                        format!("`order` entry `by: {by}` names no capture declared by this rule"),
                    );
                    entries[index].1 = None;
                    complete = false;
                }
            }
            // §6.3: the names `by` would resolve against were never entered,
            // so the resolution is not attempted rather than failed. The
            // collection still cannot be built.
            None => complete = false,
        }

        // §3.8 orders a rule's own repeated matches, which a rule that can
        // match at most once does not have. A cardinality that never
        // normalized supplies no maximum to test, so the check is skipped
        // rather than run against an invented one.
        if let Some(RuleOutcome::Allow(cardinality)) = outcome {
            if matches!(cardinality.max, UpperBound::Bounded(max) if max <= 1) {
                let offending = entries
                    .iter()
                    .enumerate()
                    .filter_map(|(index, (range, entry))| entry.as_ref().map(|_| (index, *range)))
                    .collect::<Vec<_>>();
                for (index, range) in offending {
                    self.error_at(
                        SchemaErrorKind::InvalidOrder,
                        range,
                        "`order` needs a rule that can match more than once, and this rule's \
                         effective maximum is one",
                    );
                    entries[index].1 = None;
                    complete = false;
                }
            }
        }

        complete.then(|| {
            entries
                .into_iter()
                .map(|(_, entry)| {
                    let entry = entry.expect("a complete collection has every entry");
                    ValueOrderEntry {
                        by: CaptureName(entry.by),
                        direction: entry.direction,
                        strict: entry.strict,
                    }
                })
                .collect()
        })
    }

    /// Parses one `order` entry's own shape, independently of every other
    /// entry and of the rule around it (§2.1).
    ///
    /// Faults within one entry are independent of each other, so all of them
    /// are reported rather than only the first.
    fn parse_order_entry(&mut self, element: &Value, range: SourceRange) -> Option<OrderEntry> {
        let Some(mapping) = element.as_object() else {
            self.error_at(
                SchemaErrorKind::InvalidOrder,
                range,
                "each `order` entry must be a mapping",
            );
            return None;
        };
        let mut valid = true;
        for key in mapping.keys() {
            if !matches!(key.as_str(), "by" | "dir" | "strict") {
                self.error_at(
                    SchemaErrorKind::InvalidOrder,
                    range,
                    format!("unknown `order` entry field `{key}`"),
                );
                valid = false;
            }
        }
        let by = match mapping.get("by") {
            Some(Value::String(by)) => Some(by.clone()),
            Some(_) => {
                self.error_at(
                    SchemaErrorKind::InvalidOrder,
                    range,
                    "`order` entry `by` must be a capture name string",
                );
                None
            }
            None => {
                self.error_at(
                    SchemaErrorKind::InvalidOrder,
                    range,
                    "each `order` entry must declare `by`",
                );
                None
            }
        };
        let direction = match mapping.get("dir") {
            None => Some(ValueOrderDirection::Ascending),
            Some(Value::String(dir)) if dir == "asc" => Some(ValueOrderDirection::Ascending),
            Some(Value::String(dir)) if dir == "desc" => Some(ValueOrderDirection::Descending),
            Some(_) => {
                self.error_at(
                    SchemaErrorKind::InvalidOrder,
                    range,
                    "`order` entry `dir` must be `asc` or `desc`",
                );
                None
            }
        };
        let strict = match mapping.get("strict") {
            None => Some(false),
            Some(Value::Bool(strict)) => Some(*strict),
            Some(_) => {
                self.error_at(
                    SchemaErrorKind::InvalidOrder,
                    range,
                    "`order` entry `strict` must be a bool",
                );
                None
            }
        };
        match (valid, by, direction, strict) {
            (true, Some(by), Some(direction), Some(strict)) => Some(OrderEntry {
                by,
                direction,
                strict,
            }),
            _ => None,
        }
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
            if scope.0.is_empty() && id == "fm" {
                self.error_at(
                    SchemaErrorKind::ReservedId,
                    range,
                    "top-level rule id `fm` is reserved for frontmatter refs",
                );
            }
            return Some(RuleId(id.to_owned()));
        }

        let Matcher::Exact(text) = matcher? else {
            return None;
        };
        let generated = auto_id(&text.0).map(RuleId);
        if scope.0.is_empty() && generated.as_ref().is_some_and(|id| id.0 == "fm") {
            self.error_at(
                SchemaErrorKind::ReservedId,
                range,
                "top-level auto-generated rule id `fm` is reserved for frontmatter refs",
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

    fn build_outcome(
        &mut self,
        allow: bool,
        required: Option<bool>,
        repeat: Option<&str>,
        range: SourceRange,
    ) -> Option<RuleOutcome> {
        if required.is_some() && repeat.is_some() {
            self.error_at(
                SchemaErrorKind::ConflictingCardinality,
                range,
                "required and repeat cannot both be declared",
            );
            return None;
        }
        if !allow && (required.is_some() || repeat.is_some()) {
            self.error_at(
                SchemaErrorKind::ConflictingCardinality,
                range,
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
                    self.error_at(
                        SchemaErrorKind::InvalidRepeat,
                        range,
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
