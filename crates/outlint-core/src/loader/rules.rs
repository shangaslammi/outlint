//! Construction of options, scopes, rules, matchers, and cardinalities.

use std::collections::{BTreeMap, HashMap};

use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

use crate::matcher::{compile_anchored_pattern, compile_glob_pattern};
use crate::{
    Cardinality, ExactText, GlobPattern, Matcher, Options, RegexPattern, RelatedLocation, RuleId,
    RuleIndex, RuleOutcome, RulePath, SchemaErrorKind, SchemaNode, ScopePath, SectionRule,
    SourceRange, UpperBound,
};

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
            let children =
                self.build_scope(raw.sections, &child_scope, match_case, ordered_default);
            match (matcher, outcome, children) {
                (Some(matcher), Some(outcome), Some(sections)) => {
                    semantic_indices.push(index);
                    semantic.push(SectionRule {
                        id,
                        matcher,
                        outcome,
                        strict: raw.strict,
                        ordered: raw.ordered.unwrap_or(ordered_default),
                        sections,
                        constraints: Vec::new(),
                        // Lane 2A wires the shape through; declaration
                        // normalization belongs to the rule loader lane, so
                        // every rule still normalizes to no captures and no
                        // value order.
                        captures: BTreeMap::new(),
                        order: Vec::new(),
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
