use super::{error_kinds, invalid, source_slice, valid};
use crate::loader::load_schema;
use crate::loader::rules::{auto_id, parse_repeat, regex_body};
use crate::{
    Cardinality, ConstraintIndex, ConstraintPath, ExactText, InvalidSchema, Matcher,
    OutlineProvenance, RegexPattern, RuleId, RuleIndex, RuleOutcome, RulePath, SchemaError,
    SchemaErrorKind, SchemaNode, ScopePath, UpperBound,
};
use proptest::prelude::*;

#[test]
fn applies_defaults_and_normalizes_rules() {
    let schema = valid(
        r#"
version: 1
sections:
  - match: API Reference
    required: true
  - id: api
    match: "/API: .+/"
    repeat: 0..n
  - match: "*"
    allow: false
"#,
    );
    assert!(!schema.options.match_case);
    assert!(schema.options.strip_inline_markup);
    assert!(!schema.options.allow_skipped_levels);
    let rules = schema.addressed_root_rules();
    assert_eq!(rules[0].id, Some(RuleId("api-reference".into())));
    assert_eq!(
        rules[0].outcome,
        RuleOutcome::Allow(Cardinality {
            min: 1,
            max: UpperBound::Bounded(1)
        })
    );
    assert!(matches!(rules[2].outcome, RuleOutcome::Deny));
}

#[test]
fn classifies_matcher_forms_and_unescapes_regex_delimiter() {
    let schema = valid(
        r#"
version: 1
sections:
  - match: exact
  - match: prefix*suffix
  - match: "*"
  - match: /a\/b/
"#,
    );
    let rules = schema.addressed_root_rules();
    assert!(matches!(rules[0].matcher, Matcher::Exact(_)));
    assert!(matches!(rules[1].matcher, Matcher::Glob(_)));
    assert_eq!(rules[2].matcher, Matcher::Any);
    assert_eq!(rules[3].matcher, Matcher::Regex(RegexPattern("a/b".into())));
}

#[test]
fn rejects_invalid_regex_and_repeat_while_collecting_errors() {
    let kinds = error_kinds(
        r#"
version: 1
sections:
  - match: /(?=lookaround)/
    repeat: 01..2
  - match: ok
    allow: false
    required: true
"#,
    );
    assert!(kinds.contains(&SchemaErrorKind::InvalidMatcher));
    assert!(kinds.contains(&SchemaErrorKind::InvalidRepeat));
    assert!(kinds.contains(&SchemaErrorKind::ConflictingCardinality));
}

#[test]
fn rejects_a_single_regex_delimiter_without_panicking() {
    let kinds = error_kinds(
        r#"
version: 1
sections:
  - match: "/"
"#,
    );
    assert_eq!(kinds, vec![SchemaErrorKind::InvalidMatcher]);
}

#[test]
fn regex_load_validation_uses_the_normalized_match_case_setting() {
    let body = "[a-z]{100000}";
    let case_insensitive = format!("version: 1\nsections:\n  - match: \"/{body}/\"\n");
    let invalid = load_schema(&case_insensitive)
        .expect_err("case-insensitive compiled regex exceeds the size limit");
    assert_eq!(invalid.errors.first.kind, SchemaErrorKind::InvalidMatcher);

    let case_sensitive =
        format!("version: 1\noptions:\n  match_case: true\nsections:\n  - match: \"/{body}/\"\n");
    let loaded = load_schema(&case_sensitive).expect("the same regex fits when case-sensitive");
    crate::PreparedValidator::new(&loaded.schema)
        .expect("loader and validator use identical case-sensitive settings");
}

#[test]
fn oversized_glob_is_invalid_at_its_matcher_range_and_errors_are_collected() {
    let glob = format!("{}*", "a".repeat(200_000));
    let source = format!("version: 1\nsections:\n  - match: {glob}\n    repeat: 01..2\n");
    let invalid = load_schema(&source).expect_err("oversized glob must fail during loading");
    let errors = invalid.errors.iter().collect::<Vec<_>>();

    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].kind, SchemaErrorKind::InvalidMatcher);
    assert_eq!(source_slice(&source, errors[0].range), glob);
    assert_eq!(errors[1].kind, SchemaErrorKind::InvalidRepeat);

    let case_sensitive =
        format!("version: 1\noptions:\n  match_case: true\nsections:\n  - match: {glob}\n");
    let loaded =
        load_schema(&case_sensitive).expect("the same glob fits when matching case-sensitively");
    crate::PreparedValidator::new(&loaded.schema)
        .expect("loader and validator use identical case-sensitive glob settings");
}

#[test]
fn detects_auto_id_collisions_per_scope() {
    let kinds = error_kinds(
        r#"
version: 1
sections:
  - match: API
  - id: api
    match: Something else
"#,
    );
    assert!(kinds.contains(&SchemaErrorKind::DuplicateId));
}

#[test]
fn auto_ids_discard_decomposed_marks_without_splitting_words() {
    assert_eq!(auto_id("Mälardalen"), Some("malardalen".to_owned()));
    assert_eq!(auto_id("nai\u{308}ve café"), Some("naive-cafe".to_owned()));
    assert_eq!(auto_id("a—b"), Some("a-b".to_owned()));
}

#[test]
fn rejects_auto_generated_reserved_fm_id() {
    let kinds = error_kinds(
        r#"
version: 1
sections:
  - match: fm
"#,
    );
    assert_eq!(kinds, vec![SchemaErrorKind::ReservedId]);
}

/// `root_level` was removed from the format: the title is always the `h1`
/// and `sections` always describes `h2`. A schema still declaring it is
/// rejected as an unknown option rather than silently ignored.
#[test]
fn rejects_the_removed_root_level_option() {
    let source = "version: 1\noptions:\n  root_level: 3\nsections: []\n";
    let invalid = invalid(source);
    let messages = invalid
        .errors
        .iter()
        .map(|error| (error.kind, error.message.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        messages,
        vec![(
            SchemaErrorKind::InvalidDocumentShape,
            "unknown field `root_level`".to_owned()
        )]
    );
}

#[test]
fn rejects_every_explicit_null_typed_field_and_collects_them() {
    // `title: null` is the one legal null: it declares a document with no
    // h1, so only the four other nulls are rejected.
    let source = r#"version: 1
title: null
options:
  match_case: null
sections:
  - id: null
    match: valid
    required: null
    repeat: null
"#;
    let invalid = invalid(source);
    let errors = invalid.errors.iter().collect::<Vec<_>>();
    assert_eq!(errors.len(), 4);
    assert!(errors
        .iter()
        .all(|error| error.kind == SchemaErrorKind::InvalidDocumentShape
            && source_slice(source, error.range) == "null"));
    let mut actual = errors
        .iter()
        .map(|error| error.range.range.start.0)
        .collect::<Vec<_>>();
    actual.sort_unstable();
    let expected = source
        .match_indices("null")
        .map(|(offset, _)| offset)
        .skip(1)
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

#[test]
fn title_null_declares_a_document_without_h1() {
    let source = "version: 1\ntitle: null\nsections:\n  - match: Overview\n";
    let loaded = load_schema(source).expect("title: null loads");
    // The declaration desugars to a denied any-text h1 rule carrying the
    // `sections` scope: a present h1 is not-allowed, and the sections
    // describe the document's top-level h2s.
    let rule = &loaded.schema.outline[0];
    assert_eq!(rule.matcher, Matcher::Any);
    assert_eq!(rule.outcome, RuleOutcome::Deny);
    assert_eq!(rule.sections.len(), 1);
    assert_eq!(loaded.schema.outline_provenance, OutlineProvenance::NoTitle);
    assert_eq!(
        source_slice(
            source,
            *loaded
                .locations
                .nodes
                .get(&SchemaNode::Title)
                .expect("title: null anchors the title node")
        ),
        "null"
    );
}

#[test]
fn sugar_forms_carry_their_provenance() {
    let titled = valid("version: 1\ntitle: Doc\nsections: []\n");
    assert_eq!(titled.outline_provenance, OutlineProvenance::Title);
    let bare = valid("version: 1\nsections: []\n");
    assert_eq!(bare.outline_provenance, OutlineProvenance::BareSections);
}

#[test]
fn outline_rules_are_the_canonical_model_and_anchor_at_their_spellings() {
    let source = r#"version: 1
outline:
  - match: Part
    required: true
    sections:
      - match: Overview
        required: true
"#;
    let loaded = load_schema(source).expect("a single-rule outline loads");
    let schema = &loaded.schema;
    assert_eq!(schema.outline_provenance, OutlineProvenance::Outline);
    assert_eq!(
        schema.outline[0].matcher,
        Matcher::Exact(ExactText("Part".into()))
    );
    assert_eq!(
        schema.outline[0].outcome,
        RuleOutcome::Allow(Cardinality {
            min: 1,
            max: UpperBound::Bounded(1)
        })
    );
    assert_eq!(
        schema.outline[0].sections[0].matcher,
        Matcher::Exact(ExactText("Overview".into()))
    );
    // The outline rule is an ordinary rule at the empty scope; its child
    // anchors one scope below. There is no title node: nothing in this
    // schema is a title.
    assert!(!loaded.locations.nodes.contains_key(&SchemaNode::Title));
    assert_eq!(
        source_slice(
            source,
            *loaded
                .locations
                .nodes
                .get(&SchemaNode::Rule(RulePath {
                    scope: ScopePath(Vec::new()),
                    index: RuleIndex(0),
                }))
                .expect("the outline rule is the root scope's first rule")
        ),
        "match: Part\n    required: true\n    sections:\n      - match: Overview\n        required: true\n"
    );
    assert_eq!(
        source_slice(
            source,
            *loaded
                .locations
                .nodes
                .get(&SchemaNode::Rule(RulePath {
                    scope: ScopePath(vec![RuleIndex(0)]),
                    index: RuleIndex(0),
                }))
                .expect("the outline rule's child sits one scope below")
        ),
        "match: Overview\n        required: true\n"
    );
}

#[test]
fn sugar_and_outline_forms_parse_to_the_same_model() {
    let sugar = valid(
        r#"version: 1
title: "Doc *"
sections:
  - match: Overview
    required: true
    sections:
      - match: Details
  - match: Second
constraints:
  - any_of: [overview, second]
"#,
    );
    let general = valid(
        r#"version: 1
outline:
  - match: "Doc *"
    required: true
    sections:
      - match: Overview
        required: true
        sections:
          - match: Details
      - match: Second
    constraints:
      - any_of: [overview, second]
"#,
    );
    assert_eq!(sugar.outline_provenance, OutlineProvenance::Title);
    assert_eq!(general.outline_provenance, OutlineProvenance::Outline);
    let mut general_as_sugar = general;
    general_as_sugar.outline_provenance = OutlineProvenance::Title;
    assert_eq!(sugar, general_as_sugar);
}

#[test]
fn an_outline_declares_any_number_of_ordinary_h1_rules() {
    let schema = valid(
        r#"version: 1
outline:
  - match: "Part *"
    repeat: "1..n"
  - id: appendix
    match: Appendix
    strict: true
"#,
    );
    assert_eq!(schema.outline.len(), 2);
    assert_eq!(
        schema.outline[0].outcome,
        RuleOutcome::Allow(Cardinality {
            min: 1,
            max: UpperBound::Unbounded
        })
    );
    assert_eq!(schema.outline[1].id, Some(RuleId("appendix".into())));
    assert!(schema.outline[1].strict);
}

#[test]
fn an_empty_outline_is_refused_toward_title_null() {
    // `outline: []` would constrain nothing — the outline scope is open,
    // so h1 headers would pass unvalidated — while its author almost
    // certainly means "no h1", which `title: null` declares.
    let invalid = invalid("version: 1\noutline: []\n");
    assert_eq!(
        invalid.errors.first.message,
        "outline must declare at least one rule; a document with no h1 headers \
         is declared with `title: null`"
    );
}

#[test]
fn outline_conflicts_with_title_at_the_second_declared_key() {
    let source = "version: 1\ntitle: Doc\noutline:\n  - match: Doc\n    required: true\n";
    let invalid = invalid(source);
    let errors = invalid.errors.iter().collect::<Vec<_>>();
    assert_eq!(errors.len(), 1);
    let error = errors[0];
    assert_eq!(error.kind, SchemaErrorKind::ConflictingOutline);
    assert_eq!(
        error.message,
        "`outline` cannot be declared together with `title`"
    );
    assert_eq!(
        source_slice(source, error.range),
        "- match: Doc\n    required: true\n"
    );
    assert_eq!(error.related.len(), 1);
    assert_eq!(source_slice(source, error.related[0].range), "Doc");
    assert_eq!(error.related[0].message, "`title` declared here");
}

#[test]
fn outline_conflicts_with_sections_anchoring_whichever_comes_second() {
    // `outline` first: the error anchors at `sections`.
    let source = "version: 1\noutline:\n  - match: Doc\n    required: true\nsections: []\n";
    let invalid = invalid(source);
    let errors = invalid.errors.iter().collect::<Vec<_>>();
    assert_eq!(errors.len(), 1);
    let error = errors[0];
    assert_eq!(error.kind, SchemaErrorKind::ConflictingOutline);
    assert_eq!(
        error.message,
        "`sections` cannot be declared together with `outline`"
    );
    assert_eq!(source_slice(source, error.range), "[]");
    assert_eq!(error.related[0].message, "`outline` declared here");
}

#[test]
fn top_level_constraints_beside_outline_attach_to_the_h1_scope() {
    // Their refs resolve among the outline rules themselves.
    let schema = valid(
        "version: 1\noptions:\n  ordered_sections: false\noutline:\n  - id: intro\n\
         \x20   match: Intro\n  - id: body\n    match: Body\nconstraints:\n\
         \x20 - ordered: [intro, body]\n",
    );
    assert_eq!(schema.constraints.len(), 1);
    assert!(schema
        .outline
        .iter()
        .all(|rule| rule.constraints.is_empty()));

    // A sugar schema's top-level constraints attach to the `sections`
    // scope instead — the desugared rule's child scope — leaving the
    // schema-level list empty.
    let sugar = valid(
        "version: 1\noptions:\n  ordered_sections: false\nsections:\n  - id: a\n\
         \x20   match: A\n  - id: b\n    match: B\nconstraints:\n  - ordered: [a, b]\n",
    );
    assert!(sugar.constraints.is_empty());
    assert_eq!(sugar.outline[0].constraints.len(), 1);
}

#[test]
fn schema_root_refs_anchor_at_the_outline_scope_in_the_general_form() {
    // `$` names the h1 rules for `outline:` schemas; a sugar schema's
    // `$.` refs keep resolving against its `sections` scope.
    let schema = valid(
        "version: 1\noutline:\n  - id: doc\n    match: Doc\n    required: true\n\
         \x20   sections:\n      - id: a\n        match: A\n        constraints:\n\
         \x20         - requires: { if: \"$.doc.a\", then: \"$.doc\" }\n",
    );
    assert_eq!(schema.outline[0].sections[0].constraints.len(), 1);
    // The same spelling that resolved through `sections` before still
    // does: `$.a` in sugar reaches the top-level `sections` rule.
    let sugar = valid(
        "version: 1\nsections:\n  - id: a\n    match: A\n    sections:\n\
         \x20     - id: b\n        match: B\n    constraints:\n\
         \x20     - requires: { if: b, then: \"$.a\" }\n",
    );
    assert_eq!(sugar.outline[0].sections[0].constraints.len(), 1);
    // An unresolved `$.` ref in the general form is a real error, not a
    // gate: `$.a` skips the outline level.
    let unresolved = invalid(
        "version: 1\noutline:\n  - id: doc\n    match: Doc\n    required: true\n\
         \x20   sections:\n      - id: a\n        match: A\n    constraints:\n\
         \x20     - requires: { if: a, then: \"$.a\" }\n",
    );
    assert!(unresolved
        .errors
        .iter()
        .any(|error| error.kind == SchemaErrorKind::UnresolvedRef
            && error.message == "unresolved ref `$.a`"));
}

#[test]
fn outline_rules_take_every_cardinality_spelling() {
    let schema = valid("version: 1\noutline:\n  - match: Doc\n    repeat: \"1..1\"\n");
    assert_eq!(
        schema.outline[0].outcome,
        RuleOutcome::Allow(Cardinality {
            min: 1,
            max: UpperBound::Bounded(1)
        })
    );
    // No cardinality at all is the ordinary open default.
    let default = valid("version: 1\noutline:\n  - match: Doc\n");
    assert_eq!(
        default.outline[0].outcome,
        RuleOutcome::Allow(Cardinality {
            min: 0,
            max: UpperBound::Unbounded
        })
    );
}

#[test]
fn errors_inside_an_outline_rule_anchor_at_their_own_spellings() {
    let source = r#"version: 1
outline:
  - match: Doc
    required: true
    sections:
      - match: "/(/"
"#;
    let invalid = invalid(source);
    let regex = invalid
        .errors
        .iter()
        .find(|error| error.kind == SchemaErrorKind::InvalidMatcher)
        .expect("the child rule's regex is invalid");
    assert_eq!(source_slice(source, regex.range), "\"/(/\"");
}

#[test]
fn constraints_on_an_outline_rule_anchor_at_their_own_spellings() {
    let source = r#"version: 1
outline:
  - match: Doc
    required: true
    sections:
      - match: Overview
    constraints:
      - one_of: [missing, alike]
"#;
    let invalid = invalid(source);
    let unresolved = invalid
        .errors
        .iter()
        .find(|error| error.kind == SchemaErrorKind::UnresolvedRef)
        .expect("the constraint refs do not resolve");
    assert_eq!(
        source_slice(source, unresolved.range),
        "one_of: [missing, alike]\n"
    );
}

#[test]
fn ordered_refs_through_a_repeatable_h1_rule_are_refused() {
    // §5.1 at the outline level: an ordered ref whose path crosses a
    // repeatable ancestor has no single document position to compare, so
    // `Part` under `repeat: 1..n` cannot carry an ordered ref path.
    let invalid = invalid(
        "version: 1\noutline:\n  - id: part\n    match: \"Part *\"\n    repeat: \"1..n\"\n\
         \x20   sections:\n      - id: a\n        match: A\n      - id: b\n        match: B\n\
         constraints:\n  - ordered: [part.a, part.b]\n",
    );
    assert!(invalid
        .errors
        .iter()
        .any(|error| error.kind == SchemaErrorKind::OrderedScopeMismatch));
}

#[test]
fn duplicate_id_error_and_related_location_point_to_each_scalar() {
    let source = r#"version: 1
sections:
  - id: duplicate
    match: First
  - id: duplicate
    match: Second
"#;
    let error = assert_anchored(source, SchemaErrorKind::DuplicateId, "duplicate");
    assert_related(source, &error, "duplicate");
    assert_ne!(error.range.range.start, error.related[0].range.range.start);
}

#[test]
fn successful_node_locations_are_narrower_than_the_document() {
    let source = r#"version: 1
options:
  ordered_sections: false
title: "*"
sections:
  - match: Overview
  - match: Details
constraints:
  - ordered: [overview, details]
"#;
    let loaded = match load_schema(source) {
        Ok(loaded) => loaded,
        Err(invalid) => panic!("unexpected errors: {:#?}", invalid.errors),
    };
    let addresses = [
        SchemaNode::Title,
        SchemaNode::Rule(RulePath {
            scope: ScopePath(Vec::new()),
            index: RuleIndex(0),
        }),
        SchemaNode::Constraint(ConstraintPath {
            scope: ScopePath(Vec::new()),
            index: ConstraintIndex(0),
        }),
    ];
    for address in addresses {
        let range = loaded
            .locations
            .nodes
            .get(&address)
            .copied()
            .unwrap_or_else(|| panic!("missing range for {address:?}"));
        assert!(range.range.start > loaded.locations.document.range.start);
        assert!(range.range.end <= loaded.locations.document.range.end);
        assert!(range.range.start < range.range.end);
        assert_ne!(range, loaded.locations.document);
    }
}

#[test]
fn repeat_accepts_u32_boundary_and_rejects_overflow() {
    let schema = valid(
        r#"
version: 1
sections:
  - match: many
    repeat: 4294967295..4294967295
"#,
    );
    assert_eq!(
        schema.addressed_root_rules()[0].outcome,
        RuleOutcome::Allow(Cardinality {
            min: u32::MAX,
            max: UpperBound::Bounded(u32::MAX)
        })
    );
    let kinds = error_kinds(
        r#"
version: 1
sections:
  - match: too-many
    repeat: 4294967296..n
"#,
    );
    assert_eq!(kinds, vec![SchemaErrorKind::InvalidRepeat]);
}

#[test]
fn ordered_resolves_from_the_rule_or_else_the_option() {
    let schema = valid(
        "version: 1\nsections:\n  - match: A\n  - match: B\n    ordered: false\n    sections:\n      - match: C\n        ordered: true\n",
    );
    assert!(schema.options.ordered_sections);
    let title = &schema.outline[0];
    assert!(title.ordered);
    assert!(title.sections[0].ordered);
    assert!(!title.sections[1].ordered);
    assert!(title.sections[1].sections[0].ordered);

    let opted_out = valid(
        "version: 1\noptions:\n  ordered_sections: false\noutline:\n  - match: A\n  - match: B\n    ordered: true\n",
    );
    assert!(!opted_out.options.ordered_sections);
    assert!(!opted_out.outline[0].ordered);
    assert!(opted_out.outline[1].ordered);
}

#[test]
fn ordered_must_be_a_bool_and_the_option_must_be_known() {
    let invalid = invalid("version: 1\nsections:\n  - match: A\n    ordered: yes please\n");
    assert!(invalid
        .errors
        .iter()
        .any(|error| error.kind == SchemaErrorKind::InvalidDocumentShape
            && error.message == "rule `ordered` must be a bool and cannot be null"));
    let invalid =
        self::invalid("version: 1\noptions:\n  ordered: false\nsections:\n  - match: A\n");
    assert!(invalid
        .errors
        .iter()
        .any(|error| error.kind == SchemaErrorKind::InvalidDocumentShape
            && error.message == "unknown field `ordered`"));
}

proptest! {
    #[test]
    fn regex_body_round_trips_delimiter_escaping_without_other_escapes(
        source in any::<String>(),
    ) {
        prop_assume!(!source.contains('\\'));
        let encoded = source
            .chars()
            .flat_map(|character| {
                if character == '/' {
                    vec!['\\', '/']
                } else {
                    vec![character]
                }
            })
            .collect::<String>();
        let decoded = regex_body(&encoded);
        prop_assert_eq!(decoded.as_deref(), Some(source.as_str()));
    }

    #[test]
    fn parse_repeat_normalizes_valid_finite_bounds(min in any::<u32>(), max in any::<u32>()) {
        prop_assume!(max >= min && max > 0);
        let source = format!("{min}..{max}");
        prop_assert_eq!(
            parse_repeat(&source),
            Some(Cardinality {
                min,
                max: UpperBound::Bounded(max),
            })
        );
    }

    #[test]
    fn parse_repeat_normalizes_unbounded_bounds(min in any::<u32>()) {
        let source = format!("{min}..n");
        prop_assert_eq!(
            parse_repeat(&source),
            Some(Cardinality {
                min,
                max: UpperBound::Unbounded,
            })
        );
    }
}

// --- Shared assertions for capture and order diagnostics -------------------
//
// Every capture and order contract is stated the same way: one error of an
// exact kind, anchored at an exact stretch of the schema source, sometimes
// with one related location. Spelling that out per test buried the contract
// in `find`/`expect` noise, so the three shapes get named assertions.

/// The single error of `kind`, or a panic naming what was collected instead.
#[track_caller]
fn only_error_of(invalid: &InvalidSchema, kind: SchemaErrorKind) -> SchemaError {
    let matching = invalid
        .errors
        .iter()
        .filter(|error| error.kind == kind)
        .cloned()
        .collect::<Vec<_>>();
    match matching.len() {
        1 => matching.into_iter().next().expect("one error was matched"),
        _ => panic!(
            "expected exactly one {kind:?} error, collected {:#?}",
            invalid.errors.iter().collect::<Vec<_>>()
        ),
    }
}

/// Asserts the loader collected exactly the given `(kind, anchor)` pairs, in
/// the order it produced them.
#[track_caller]
fn assert_errors(source: &str, expected: &[(SchemaErrorKind, &str)]) {
    let invalid = invalid(source);
    let actual = invalid
        .errors
        .iter()
        .map(|error| (error.kind, source_slice(source, error.range)))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected.to_vec());
}

/// Asserts one error is the only one of its kind and anchors at `anchor`.
#[track_caller]
fn assert_anchored(source: &str, kind: SchemaErrorKind, anchor: &str) -> SchemaError {
    let invalid = invalid(source);
    let error = only_error_of(&invalid, kind);
    assert_eq!(source_slice(source, error.range), anchor);
    error
}

/// Asserts an error carries exactly one related location, at `anchor`.
#[track_caller]
fn assert_related(source: &str, error: &SchemaError, anchor: &str) {
    assert_eq!(
        error.related.len(),
        1,
        "expected one related location, got {:#?}",
        error.related
    );
    assert_eq!(source_slice(source, error.related[0].range), anchor);
}

// --- §2.1 classification and precedence contracts -------------------------

/// A key repeated inside one `captures` mapping is `invalid-capture`, not the
/// `syntax` every other repeated YAML key is (§2.1). The later occurrence is
/// the one a reader meets as the contradiction, so it anchors the error.
#[test]
fn duplicate_capture_key_has_special_classification() {
    let source = r#"version: 1
sections:
  - match: "/(?<major>[0-9]+)/"
    captures:
      major: int
      major: text
"#;
    let error = assert_anchored(source, SchemaErrorKind::InvalidCapture, "major");
    assert_eq!(error.message, "duplicate capture name `major`");
    // The second `major:` key, not the first.
    assert_eq!(
        error.range.range.start.0,
        source.rfind("major").expect("the source repeats the key")
    );
}

/// The special classification stops at the `captures` mapping's own keys: a
/// key repeated inside one `order` entry stays `syntax` (§2.1).
#[test]
fn duplicate_order_entry_key_remains_syntax() {
    let source = r#"version: 1
sections:
  - match: "/(?<major>[0-9]+)/"
    captures:
      major: int
    order:
      - by: major
        by: major
"#;
    let error = assert_anchored(source, SchemaErrorKind::Syntax, "by");
    assert_eq!(error.message, "invalid YAML: duplicate mapping key `by`");
}

/// And it stops at the rule object too: an ordinary repeated rule key is the
/// same `syntax` it always was.
#[test]
fn duplicate_rule_keys_remain_syntax() {
    let source = r#"version: 1
sections:
  - match: First
    match: Second
"#;
    let error = assert_anchored(source, SchemaErrorKind::Syntax, "match");
    assert_eq!(error.message, "invalid YAML: duplicate mapping key `match`");
}

/// §6.3: a check whose input could not be built is not attempted. A regex the
/// matcher rejects yields no capture-group facts, so an otherwise valid
/// declaration against it reports `invalid-matcher` alone.
#[test]
fn invalid_regex_suppresses_capture_group_checks() {
    let source = r#"version: 1
sections:
  - match: "/(?<major>[0-9]+/"
    captures:
      major: int
"#;
    assert_errors(
        source,
        &[(SchemaErrorKind::InvalidMatcher, "\"/(?<major>[0-9]+/\"")],
    );
}

/// §2.1: capture names enter the named scope only after their mapping is
/// well-formed, so a repeated key is reported instead of — never beside — the
/// `duplicate-id` its name would otherwise raise against a child rule.
#[test]
fn invalid_capture_precedes_duplicate_id() {
    let source = r#"version: 1
sections:
  - match: "/(?<major>[0-9]+)/"
    captures:
      major: int
      major: text
    sections:
      - id: major
        match: Major
"#;
    assert_errors(source, &[(SchemaErrorKind::InvalidCapture, "major")]);
}
