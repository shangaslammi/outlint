use super::{error_kinds, invalid, source_slice, valid};
use crate::loader::load_schema;
use crate::loader::rules::{auto_id, is_capture_name, parse_repeat, regex_body};
use crate::{
    CaptureName, Cardinality, ConstraintIndex, ConstraintPath, DocumentShape, ExactText,
    InvalidSchema, LoadedSchema, Matcher, OrderEntryPath, OrderIndex, OutlineProvenance,
    RegexPattern, RuleId, RuleIndex, RulePath, SchemaError, SchemaErrorKind, SchemaNode, ScopePath,
    UpperBound, ValueOrderDirection, ValueOrderEntry,
};
use proptest::prelude::*;

#[test]
fn applies_defaults_and_normalizes_rules() {
    let schema = valid(
        r#"
version: 2
sections:
  - match: API Reference
    required: true
  - id: api
    match: "/API: .+/"
    repeat: 0..n
forbid_sections:
  - match: "*"
"#,
    );
    assert!(!schema.options.match_case);
    assert!(schema.options.strip_inline_markup);
    assert!(!schema.options.allow_skipped_levels);
    let rules = schema.addressed_root_rules();
    assert_eq!(rules[0].id, Some(RuleId("api-reference".into())));
    assert_eq!(
        rules[0].cardinality,
        Cardinality {
            min: 1,
            max: UpperBound::Bounded(1)
        }
    );
    assert_eq!(schema.document, schema.document.clone());
}

#[test]
fn classifies_matcher_forms_and_unescapes_regex_delimiter() {
    let schema = valid(
        r#"
version: 2
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
version: 2
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
version: 2
sections:
  - match: "/"
"#,
    );
    assert_eq!(kinds, vec![SchemaErrorKind::InvalidMatcher]);
}

#[test]
fn regex_load_validation_uses_the_normalized_match_case_setting() {
    let body = "[a-z]{100000}";
    let case_insensitive = format!("version: 2\nsections:\n  - match: \"/{body}/\"\n");
    let invalid = load_schema(&case_insensitive)
        .expect_err("case-insensitive compiled regex exceeds the size limit");
    assert_eq!(invalid.errors.first.kind, SchemaErrorKind::InvalidMatcher);

    let case_sensitive =
        format!("version: 2\noptions:\n  match_case: true\nsections:\n  - match: \"/{body}/\"\n");
    let loaded = load_schema(&case_sensitive).expect("the same regex fits when case-sensitive");
    crate::PreparedValidator::new(&loaded.schema)
        .expect("loader and validator use identical case-sensitive settings");
}

#[test]
fn oversized_glob_is_invalid_at_its_matcher_range_and_errors_are_collected() {
    let glob = format!("{}*", "a".repeat(200_000));
    let source = format!("version: 2\nsections:\n  - match: {glob}\n    repeat: 01..2\n");
    let invalid = load_schema(&source).expect_err("oversized glob must fail during loading");
    let errors = invalid.errors.iter().collect::<Vec<_>>();

    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].kind, SchemaErrorKind::InvalidMatcher);
    assert_eq!(source_slice(&source, errors[0].range), glob);
    assert_eq!(errors[1].kind, SchemaErrorKind::InvalidRepeat);

    let case_sensitive =
        format!("version: 2\noptions:\n  match_case: true\nsections:\n  - match: {glob}\n");
    let loaded =
        load_schema(&case_sensitive).expect("the same glob fits when matching case-sensitively");
    crate::PreparedValidator::new(&loaded.schema)
        .expect("loader and validator use identical case-sensitive glob settings");
}

#[test]
fn detects_auto_id_collisions_per_scope() {
    let kinds = error_kinds(
        r#"
version: 2
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
version: 2
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
    let source = "version: 2\noptions:\n  root_level: 3\nsections: []\n";
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
    let source = r#"version: 2
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
    let source = "version: 2\ntitle: null\nsections:\n  - match: Overview\n";
    let loaded = load_schema(source).expect("title: null loads");
    let DocumentShape::Title(title) = &loaded.schema.document else {
        panic!("expected title form")
    };
    assert!(title.matcher.is_none());
    assert_eq!(title.children.rules().len(), 1);
    assert_eq!(
        loaded.schema.outline_provenance(),
        OutlineProvenance::NoTitle
    );
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
    let titled = valid("version: 2\ntitle: Doc\nsections: []\n");
    assert_eq!(titled.outline_provenance(), OutlineProvenance::Title);
    let bare = valid("version: 2\nsections: []\n");
    assert_eq!(bare.outline_provenance(), OutlineProvenance::BareSections);
}

#[test]
fn outline_rules_are_the_canonical_model_and_anchor_at_their_spellings() {
    let source = r#"version: 2
outline:
  - match: Part
    required: true
    sections:
      - match: Overview
        required: true
"#;
    let loaded = load_schema(source).expect("a single-rule outline loads");
    let schema = &loaded.schema;
    assert_eq!(schema.outline_provenance(), OutlineProvenance::Outline);
    assert_eq!(
        schema.outline()[0].matcher,
        Matcher::Exact(ExactText("Part".into()))
    );
    assert_eq!(
        schema.outline()[0].cardinality,
        Cardinality {
            min: 1,
            max: UpperBound::Bounded(1)
        }
    );
    assert_eq!(
        schema.outline()[0].children.rules()[0].matcher,
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
        r#"version: 2
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
        r#"version: 2
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
    assert_eq!(sugar.outline_provenance(), OutlineProvenance::Title);
    assert_eq!(general.outline_provenance(), OutlineProvenance::Outline);
    assert_ne!(sugar, general);
}

#[test]
fn an_outline_declares_any_number_of_ordinary_h1_rules() {
    let schema = valid(
        r#"version: 2
outline:
  - match: "Part *"
    repeat: "1..n"
  - id: appendix
    match: Appendix
"#,
    );
    assert_eq!(schema.outline().len(), 2);
    assert_eq!(
        schema.outline()[0].cardinality,
        Cardinality {
            min: 1,
            max: UpperBound::Unbounded
        }
    );
    assert_eq!(schema.outline()[1].id, Some(RuleId("appendix".into())));
}

#[test]
fn an_empty_outline_is_a_declared_empty_grammar() {
    let schema = valid("version: 2\noutline: []\n");
    assert!(schema.outline().is_empty());
}

#[test]
fn outline_conflicts_with_title_at_the_second_declared_key() {
    let source = "version: 2\ntitle: Doc\noutline:\n  - match: Doc\n    required: true\n";
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
    let source = "version: 2\noutline:\n  - match: Doc\n    required: true\nsections: []\n";
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
        "version: 2\nunordered: true\noutline:\n  - id: intro\n\
         \x20   match: Intro\n  - id: body\n    match: Body\nconstraints:\n\
         \x20 - ordered: [intro, body]\n",
    );
    assert_eq!(schema.constraints().len(), 1);
    assert!(schema
        .outline()
        .iter()
        .all(|rule| rule.children.constraints().is_empty()));

    // A sugar schema's top-level constraints attach to the `sections`
    // scope instead — the desugared rule's child scope — leaving the
    // schema-level list empty.
    let sugar = valid(
        "version: 2\nunordered: true\nsections:\n  - id: a\n\
         \x20   match: A\n  - id: b\n    match: B\nconstraints:\n  - ordered: [a, b]\n",
    );
    assert!(sugar.constraints().is_empty());
    assert_eq!(sugar.addressed_root_constraints().len(), 1);
}

#[test]
fn schema_root_refs_anchor_at_the_outline_scope_in_the_general_form() {
    // `$` names the h1 rules for `outline:` schemas; a sugar schema's
    // `$.` refs keep resolving against its `sections` scope.
    let schema = valid(
        "version: 2\noutline:\n  - id: doc\n    match: Doc\n    required: true\n\
         \x20   sections:\n      - id: a\n        match: A\n        constraints:\n\
         \x20         - requires: { if: \"$.doc.a\", then: \"$.doc\" }\n",
    );
    assert_eq!(
        schema.outline()[0].children.rules()[0]
            .children
            .constraints()
            .len(),
        1
    );
    // The same spelling that resolved through `sections` before still
    // does: `$.a` in sugar reaches the top-level `sections` rule.
    let sugar = valid(
        "version: 2\nsections:\n  - id: a\n    match: A\n    sections:\n\
         \x20     - id: b\n        match: B\n    constraints:\n\
         \x20     - requires: { if: b, then: \"$.a\" }\n",
    );
    assert_eq!(
        sugar.outline()[0].children.rules()[0]
            .children
            .constraints()
            .len(),
        1
    );
    // An unresolved `$.` ref in the general form is a real error, not a
    // gate: `$.a` skips the outline level.
    let unresolved = invalid(
        "version: 2\noutline:\n  - id: doc\n    match: Doc\n    required: true\n\
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
    let schema = valid("version: 2\noutline:\n  - match: Doc\n    repeat: \"1..1\"\n");
    assert_eq!(
        schema.outline()[0].cardinality,
        Cardinality {
            min: 1,
            max: UpperBound::Bounded(1)
        }
    );
    // Exact matchers use the exact-one default.
    let default = valid("version: 2\noutline:\n  - match: Doc\n");
    assert_eq!(
        default.outline()[0].cardinality,
        Cardinality {
            min: 1,
            max: UpperBound::Bounded(1)
        }
    );
}

#[test]
fn errors_inside_an_outline_rule_anchor_at_their_own_spellings() {
    let source = r#"version: 2
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
    let source = r#"version: 2
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
        "version: 2\noutline:\n  - id: part\n    match: \"Part *\"\n    repeat: \"1..n\"\n\
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
    let source = r#"version: 2
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
    let source = r#"version: 2
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
version: 2
sections:
  - match: many
    repeat: 4294967295..4294967295
"#,
    );
    assert_eq!(
        schema.addressed_root_rules()[0].cardinality,
        Cardinality {
            min: u32::MAX,
            max: UpperBound::Bounded(u32::MAX)
        }
    );
    let kinds = error_kinds(
        r#"
version: 2
sections:
  - match: too-many
    repeat: 4294967296..n
"#,
    );
    assert_eq!(kinds, vec![SchemaErrorKind::InvalidRepeat]);
}

#[test]
fn unordered_is_local_to_the_declared_scope() {
    let schema = valid("version: 2\nunordered: true\nsections:\n  - match: A\n");
    let DocumentShape::Title(title) = schema.document else {
        panic!("expected title form")
    };
    let crate::ChildScope::Declared(scope) = title.children else {
        panic!("expected declared scope")
    };
    assert_eq!(scope.mode, crate::ScopeMode::Unordered);
}

#[test]
fn v2_only_and_removed_keys_are_rejected() {
    assert_eq!(
        error_kinds("version: 1\nsections: []\n"),
        vec![SchemaErrorKind::UnsupportedVersion]
    );
    for declaration in [
        "sections:\n  - match: A\n    allow: true\n",
        "sections:\n  - match: A\n    strict: false\n",
        "sections:\n  - match: A\n    ordered: true\n",
        "options:\n  ordered_sections: false\nsections: []\n",
    ] {
        assert_eq!(
            error_kinds(&format!("version: 2\n{declaration}")),
            vec![SchemaErrorKind::InvalidDocumentShape]
        );
    }
}

#[test]
fn collection_matchers_require_cardinality_but_title_does_not() {
    for matcher in ["'*'", "'A*'", "'/A/'"] {
        let invalid = invalid(&format!("version: 2\nsections:\n  - match: {matcher}\n"));
        assert_eq!(
            invalid.errors.first.kind,
            SchemaErrorKind::MissingCardinality
        );
    }
    valid("version: 2\ntitle: '*'\nsections: []\n");
}

#[test]
fn guards_and_declared_scope_modes_normalize_separately() {
    let schema = valid("version: 2\ntitle: Doc\nforbid_sections:\n  - match: Secret\nunordered: true\nextras: anywhere\nsections: []\n");
    let DocumentShape::Title(title) = schema.document else {
        panic!("expected title")
    };
    let crate::ChildScope::Declared(scope) = title.children else {
        panic!("expected declared scope")
    };
    assert_eq!(scope.guards.len(), 1);
    assert_eq!(scope.extras, crate::ExtrasMode::Anywhere);
    assert_eq!(scope.mode, crate::ScopeMode::Unordered);

    let guard_only = valid("version: 2\ntitle: Doc\nforbid_sections:\n  - match: Secret\n");
    let DocumentShape::Title(title) = guard_only.document else {
        panic!("expected title")
    };
    assert!(matches!(title.children, crate::ChildScope::GuardsOnly(_)));
}

#[test]
fn unordered_wildcard_shadows_every_later_rule() {
    let invalid = invalid("version: 2\nunordered: true\nsections:\n  - match: '*'\n    repeat: 0..n\n  - match: A\n  - match: B\n");
    assert_eq!(
        invalid
            .errors
            .iter()
            .filter(|error| error.kind == SchemaErrorKind::UnreachableRule)
            .count(),
        2
    );
}

#[test]
fn reserved_content_keys_are_rejected_in_every_section_mapping() {
    for key in ["content", "block"] {
        let invalid = invalid(&format!(
            "version: 2\nsections:\n  - match: A\n    {key}: []\n"
        ));
        assert_eq!(
            invalid.errors.first.kind,
            SchemaErrorKind::InvalidDocumentShape
        );
    }
}

#[test]
fn ordered_must_be_a_bool_and_the_option_must_be_known() {
    let invalid = invalid("version: 2\nsections:\n  - match: A\n    ordered: yes please\n");
    assert!(invalid
        .errors
        .iter()
        .any(|error| error.kind == SchemaErrorKind::InvalidDocumentShape
            && error.message == "rule `ordered` must be a bool and cannot be null"));
    let invalid =
        self::invalid("version: 2\noptions:\n  ordered: false\nsections:\n  - match: A\n");
    assert!(invalid
        .errors
        .iter()
        .any(|error| error.kind == SchemaErrorKind::InvalidDocumentShape
            && error.message == "unknown field `ordered`"));
}

proptest! {
    /// The property is about sources that carry no backslash of their own, so
    /// the strategy removes them rather than rejecting the strings that have
    /// one. Rejection would make the property's cost depend on how often
    /// `any::<String>()` happens to produce a backslash, and a raised
    /// `PROPTEST_CASES` would eventually exhaust the global rejection budget
    /// and fail the run for a reason that is not about `regex_body`. Removal
    /// leaves every already-qualifying string reachable and spelled the same,
    /// and maps the rest onto neighbours in the same set.
    #[test]
    fn regex_body_round_trips_delimiter_escaping_without_other_escapes(
        source in any::<String>().prop_map(|mut source| {
            source.retain(|character| character != '\\');
            source
        }),
    ) {
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

    /// §2.1 admits a finite `a..b` when `b >= a` and `b >= 1`, so the bounds
    /// are derived from two arbitrary integers rather than filtered for that
    /// pair of conditions: sorting them satisfies the first and raising the
    /// larger to at least one satisfies the second. Every valid `(min, max)`
    /// is still reachable — a target pair drawn in either order sorts back to
    /// itself, and its maximum is already at least one — while no draw is
    /// rejected, so raising `PROPTEST_CASES` cannot exhaust the global
    /// rejection budget.
    #[test]
    fn parse_repeat_normalizes_valid_finite_bounds(a in any::<u32>(), b in any::<u32>()) {
        let min = a.min(b);
        let max = a.max(b).max(1);
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
    let source = r#"version: 2
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
    let source = r#"version: 2
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
    let source = r#"version: 2
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
    let source = r#"version: 2
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
    let source = r#"version: 2
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

// --- §2.1/§2.2/§2.4 capture normalization ---------------------------------

/// Every type name of the closed §2.4 set resolves, and each capture keeps the
/// spelling it was declared with.
#[test]
fn every_capture_type_normalizes_to_its_declared_name() {
    let schema = valid(
        r#"version: 2
sections:
  - match: "/(?<a>.)(?<b>.)(?<c>.)(?<d>.)(?<e>.)(?<f>.)/"
    captures:
      a: int
      b: bool
      c: date
      d: semver
      e: dotted
      f: text
"#,
    );
    let declared = schema.addressed_root_rules()[0]
        .captures
        .iter()
        .map(|(name, capture)| (name.as_str(), capture.type_name()))
        .collect::<Vec<_>>();
    assert_eq!(
        declared,
        vec![
            ("a", "int"),
            ("b", "bool"),
            ("c", "date"),
            ("d", "semver"),
            ("e", "dotted"),
            ("f", "text"),
        ]
    );
}

/// §2.1 makes the mapping's source order non-semantic, so two schemas that
/// spell one set of captures in different orders normalize to one rule.
#[test]
fn capture_mapping_source_order_is_not_semantic() {
    let first = valid(
        "version: 2\nsections:\n  - match: \"/(?<major>[0-9]+)\\\\.(?<minor>[0-9]+)/\"\n\
         \x20   captures:\n      major: int\n      minor: int\n",
    );
    let second = valid(
        "version: 2\nsections:\n  - match: \"/(?<major>[0-9]+)\\\\.(?<minor>[0-9]+)/\"\n\
         \x20   captures:\n      minor: int\n      major: int\n",
    );
    assert_eq!(first, second);
}

/// §2.2 admits named groups in either spelling the dialect defines.
#[test]
fn both_named_group_spellings_bind_a_capture() {
    for pattern in ["/v(?<major>[0-9]+)/", "/v(?P<major>[0-9]+)/"] {
        let schema = valid(&format!(
            "version: 2\nsections:\n  - match: \"{pattern}\"\n    captures:\n      major: int\n"
        ));
        assert_eq!(schema.addressed_root_rules()[0].captures.len(), 1);
    }
}

/// The mandatory-participation restriction of §2.2 is about *ancestors*: an
/// alternation written inside the declared group leaves it participating in
/// every match, while one written above it does not.
#[test]
fn declared_group_cannot_be_under_alternation() {
    let schema = valid(
        r#"version: 2
sections:
  - match: "/Release (?<kind>alpha|beta)/"
    captures:
      kind: text
"#,
    );
    assert_eq!(schema.addressed_root_rules()[0].captures.len(), 1);

    let source = r#"version: 2
sections:
  - match: "/(?<kind>alpha)|beta/"
    captures:
      kind: text
"#;
    let error = assert_anchored(source, SchemaErrorKind::InvalidCapture, "kind: text");
    assert_eq!(
        error.message,
        "capture `kind` is enclosed by an alternation, so its group does not participate in \
         every match"
    );
}

/// Every zero-minimum repetition spelling of §2.2 defeats participation when
/// it encloses the declared group, in greedy and lazy form alike.
#[test]
fn declared_group_must_participate() {
    for quantifier in [
        "?", "??", "*", "*?", "{0}", "{0}?", "{0,}", "{0,}?", "{0,3}", "{0,3}?",
    ] {
        let source = format!(
            "version: 2\nsections:\n  - match: \"/a(?:(?<n>[0-9]+))\
             {quantifier}/\"\n    captures:\n      n: int\n"
        );
        let error = assert_anchored(&source, SchemaErrorKind::InvalidCapture, "n: int");
        assert!(
            error.message.contains("zero-minimum repetition"),
            "`{quantifier}` must be reported as a zero-minimum repetition, got {:?}",
            error.message
        );
    }
}

/// A repetition whose minimum is at least one keeps the group participating,
/// so it stays a legal declaration target.
#[test]
fn positive_minimum_repetitions_keep_a_capture_legal() {
    for quantifier in ["+", "+?", "{1}", "{1,}", "{2,4}"] {
        let source = format!(
            "version: 2\nsections:\n  - match: \"/a(?:(?<n>[0-9]))\
             {quantifier}/\"\n    captures:\n      n: int\n"
        );
        assert_eq!(valid(&source).addressed_root_rules()[0].captures.len(), 1);
    }
}

/// §2.2 constrains only the groups a rule declares. An undeclared group —
/// named or not — stays an ordinary regex group, optional or otherwise.
#[test]
fn undeclared_groups_are_unconstrained() {
    let schema = valid(
        r#"version: 2
sections:
  - match: "/(?<major>[0-9]+)(?:-(?<tag>[a-z]+))?( draft)?/"
    captures:
      major: int
"#,
    );
    assert_eq!(schema.addressed_root_rules()[0].captures.len(), 1);
}

/// A declaration must name a group that exists (§2.2).
#[test]
fn declared_group_must_exist() {
    let source = r#"version: 2
sections:
  - match: "/(?<major>[0-9]+)/"
    captures:
      minor: int
"#;
    let error = assert_anchored(source, SchemaErrorKind::InvalidCapture, "minor: int");
    assert_eq!(
        error.message,
        "capture `minor` names no group in this rule's regex"
    );
}

/// The `captures` collection itself must be a non-empty mapping (§2.1). Each
/// malformed collection anchors at the `captures` value, not at any entry.
#[test]
fn captures_require_a_mapping() {
    for spelling in ["null", "semver", "[major]"] {
        let source = format!(
            "version: 2\nsections:\n  - match: \"/(?<major>[0-9]+)/\"\n    captures: {spelling}\n"
        );
        assert_anchored(&source, SchemaErrorKind::InvalidCapture, spelling);
    }
}

#[test]
fn captures_must_be_nonempty() {
    let source = "version: 2\nsections:\n  - match: \"/(?<major>[0-9]+)/\"\n    captures: {}\n";
    let error = assert_anchored(source, SchemaErrorKind::InvalidCapture, "{}");
    assert_eq!(
        error.message,
        "rule `captures` must declare at least one capture"
    );
}

/// §2.2 gives capture names their own grammar, distinct from the §4.1 rule-id
/// slug: lowercase-initial, then lowercase, digits, and `_`.
#[test]
fn capture_names_follow_exact_grammar() {
    // The spelling as written, and the name the loader reads out of it: a
    // quoted empty key is written `""` and names the empty string.
    for (spelling, name) in [
        ("Major", "Major"),
        ("_major", "_major"),
        ("9major", "9major"),
        ("ma-jor", "ma-jor"),
        ("mäjor", "mäjor"),
        ("\"\"", ""),
    ] {
        let source = format!(
            "version: 2\nsections:\n  - match: \"/(?<major>[0-9]+)/\"\n\
             \x20   captures:\n      {spelling}: int\n"
        );
        let error = sole_error(
            &source,
            SchemaErrorKind::InvalidCapture,
            &format!("{spelling}: int"),
        );
        assert_eq!(
            error.message,
            format!("capture name `{name}` must match `[a-z][a-z0-9_]*`")
        );
    }
    // `_` and digits are legal after the first character.
    let schema = valid(
        "version: 2\nsections:\n  - match: \"/(?<major_2>[0-9]+)/\"\n\
         \x20   captures:\n      major_2: int\n",
    );
    assert_eq!(schema.addressed_root_rules()[0].captures.len(), 1);
}

#[test]
fn capture_type_must_be_a_string() {
    for spelling in ["null", "1", "true", "[text]", "{a: b}"] {
        let source = format!(
            "version: 2\nsections:\n  - match: \"/(?<major>[0-9]+)/\"\n\
             \x20   captures:\n      major: {spelling}\n"
        );
        let error = assert_anchored(
            &source,
            SchemaErrorKind::InvalidCapture,
            &format!("major: {spelling}"),
        );
        assert_eq!(
            error.message,
            "capture `major` must declare its type as a string"
        );
    }
}

/// §2.4's type set is closed: an unknown name is a load-time error, never an
/// extension point.
#[test]
fn capture_type_set_is_closed() {
    let source = r#"version: 2
sections:
  - match: "/(?<major>[0-9]+)/"
    captures:
      major: integer
"#;
    let error = assert_anchored(source, SchemaErrorKind::InvalidCapture, "major: integer");
    assert_eq!(
        error.message,
        "capture `major` declares unknown type `integer`"
    );
}

/// Only a regex declares named groups, so only a regex rule can capture
/// (§2.1). Each offending declaration is reported.
#[test]
fn captures_require_regex_matcher() {
    for matcher in ["Release", "Release *", "\"*\""] {
        let source = format!(
            "version: 2\nsections:\n  - match: {matcher}\n\
             \x20   captures:\n      major: int\n      minor: int\n"
        );
        assert_errors(
            &source,
            &[
                (SchemaErrorKind::InvalidCapture, "major: int"),
                (SchemaErrorKind::InvalidCapture, "minor: int"),
            ],
        );
    }
}

/// A denied rule exports nothing, so it cannot declare a capture (§2.1).
#[test]
fn deny_rules_cannot_capture() {
    let source = r#"version: 2
sections:
  - match: "/(?<major>[0-9]+)/"
    allow: false
    captures:
      major: int
      minor: text
"#;
    assert_errors(
        source,
        &[
            (SchemaErrorKind::InvalidCapture, "major: int"),
            (SchemaErrorKind::InvalidCapture, "minor: text"),
        ],
    );
    assert_eq!(
        error_messages(source),
        vec![
            "capture `major` cannot be declared on an `allow: false` rule".to_owned(),
            "capture `minor` cannot be declared on an `allow: false` rule".to_owned(),
        ]
    );
}

/// §2.2: capture names have no reserved words. `fm` and `linkdefs` are
/// reserved only as top-level rule ids (§4.1), and a capture is not one.
#[test]
fn reserved_rule_ids_are_ordinary_capture_names() {
    let schema = valid(
        r#"version: 2
sections:
  - match: "/(?<fm>[a-z]+) (?<linkdefs>[a-z]+)/"
    captures:
      fm: text
      linkdefs: text
"#,
    );
    let declared = schema.addressed_root_rules()[0]
        .captures
        .keys()
        .map(CaptureName::as_str)
        .collect::<Vec<_>>();
    assert_eq!(declared, vec!["fm", "linkdefs"]);
}

/// Capture declarations are addressable, in every form a rule is spelled in.
#[test]
fn capture_nodes_are_addressable_in_every_rule_form() {
    let sugar = r#"version: 2
title: Doc
sections:
  - match: "/(?<major>[0-9]+)/"
    captures:
      major: int
    sections:
      - match: "/(?<minor>[0-9]+)/"
        captures:
          minor: int
"#;
    let loaded = load_schema(sugar).expect("the sugar schema loads");
    assert_eq!(
        capture_slice(&loaded, sugar, ScopePath(Vec::new()), 0, "major"),
        "major: int"
    );
    assert_eq!(
        capture_slice(&loaded, sugar, ScopePath(vec![RuleIndex(0)]), 0, "minor"),
        "minor: int"
    );

    let outline = r#"version: 2
outline:
  - match: "/(?<major>[0-9]+)/"
    captures:
      major: int
    sections:
      - match: "/(?<minor>[0-9]+)/"
        captures:
          minor: int
"#;
    let loaded = load_schema(outline).expect("the outline schema loads");
    assert_eq!(
        capture_slice(&loaded, outline, ScopePath(Vec::new()), 0, "major"),
        "major: int"
    );
    assert_eq!(
        capture_slice(&loaded, outline, ScopePath(vec![RuleIndex(0)]), 0, "minor"),
        "minor: int"
    );
}

/// The source text one rule's capture node addresses.
#[track_caller]
fn capture_slice<'a>(
    loaded: &LoadedSchema,
    source: &'a str,
    scope: ScopePath,
    index: usize,
    name: &str,
) -> &'a str {
    let rule = RulePath {
        scope,
        index: RuleIndex(index),
    };
    let (_, range) = loaded
        .locations
        .nodes
        .iter()
        .find(|(node, _)| match node {
            SchemaNode::Capture(path) => path.rule == rule && path.name.as_str() == name,
            _ => false,
        })
        .unwrap_or_else(|| panic!("missing capture node `{name}` for {rule:?}"));
    source_slice(source, *range)
}

proptest! {
    /// The §2.2 grammar is decided for every string, not just the ones a
    /// schema is likely to spell: the check never panics, and it accepts
    /// exactly `[a-z][a-z0-9_]*`.
    #[test]
    fn capture_name_grammar_accepts_exactly_its_language(candidate in any::<String>()) {
        let mut characters = candidate.chars();
        let expected = characters.next().is_some_and(|first| first.is_ascii_lowercase())
            && characters.all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
            });
        prop_assert_eq!(is_capture_name(&candidate), expected);
    }
}

// --- §2.1/§3.8 value-order normalization ----------------------------------

/// An entry declares only `by`; §3.8 supplies ascending and non-strict.
#[test]
fn order_defaults_to_ascending_and_non_strict() {
    let schema = valid(
        r#"version: 2
sections:
  - match: "/v(?<major>[0-9]+)/"
    captures:
      major: int
    order:
      - by: major
"#,
    );
    assert_eq!(
        schema.addressed_root_rules()[0].order,
        vec![ValueOrderEntry {
            by: capture_name("major"),
            direction: ValueOrderDirection::Ascending,
            strict: false,
        }]
    );
}

/// Explicit values normalize to the same shape, and §3.8 makes entry order
/// semantic, so the list keeps the order it was written in.
#[test]
fn explicit_order_values_normalize_and_keep_their_list_order() {
    let schema = valid(
        r#"version: 2
sections:
  - match: "/v(?<major>[0-9]+)\\.(?<minor>[0-9]+)/"
    captures:
      major: int
      minor: int
    order:
      - by: minor
        dir: desc
        strict: true
      - by: major
        dir: asc
"#,
    );
    assert_eq!(
        schema.addressed_root_rules()[0].order,
        vec![
            ValueOrderEntry {
                by: capture_name("minor"),
                direction: ValueOrderDirection::Descending,
                strict: true,
            },
            ValueOrderEntry {
                by: capture_name("major"),
                direction: ValueOrderDirection::Ascending,
                strict: false,
            },
        ]
    );
}

/// The `order` collection must be a non-empty list (§2.1); a malformed
/// collection anchors at the `order` value rather than at any entry.
#[test]
fn order_requires_a_list() {
    for spelling in ["null", "major", "{by: major}"] {
        let source = format!(
            "version: 2\nsections:\n  - match: \"/v(?<major>[0-9]+)/\"\n\
             \x20   captures:\n      major: int\n    order: {spelling}\n"
        );
        assert_anchored(&source, SchemaErrorKind::InvalidOrder, spelling);
    }
}

#[test]
fn order_must_be_nonempty() {
    let source = "version: 2\nsections:\n  - match: \"/v(?<major>[0-9]+)/\"\n\
                  \x20   captures:\n      major: int\n    order: []\n";
    let error = assert_anchored(source, SchemaErrorKind::InvalidOrder, "[]");
    assert_eq!(
        error.message,
        "rule `order` must declare at least one entry"
    );
}

#[test]
fn order_entries_require_objects() {
    let source = ORDER_RULE.replace("<ENTRY>", "- major");
    assert_anchored(&source, SchemaErrorKind::InvalidOrder, "major");
}

#[test]
fn order_entries_require_by() {
    let source = ORDER_RULE.replace("<ENTRY>", "- dir: desc");
    let error = assert_anchored(&source, SchemaErrorKind::InvalidOrder, "dir: desc\n");
    assert_eq!(error.message, "each `order` entry must declare `by`");
}

/// §2.1 sends an unknown key inside an order entry to `invalid-order` rather
/// than to the general unknown-key rule.
#[test]
fn unknown_order_entry_fields_are_invalid_order() {
    let source = ORDER_RULE.replace("<ENTRY>", "- by: major\n        reverse: true");
    let error = assert_anchored(
        &source,
        SchemaErrorKind::InvalidOrder,
        "by: major\n        reverse: true\n",
    );
    assert_eq!(error.message, "unknown `order` entry field `reverse`");
}

#[test]
fn order_by_must_be_a_string() {
    for spelling in ["null", "1", "true", "[major]"] {
        let source = ORDER_RULE.replace("<ENTRY>", &format!("- by: {spelling}"));
        let error = assert_anchored(
            &source,
            SchemaErrorKind::InvalidOrder,
            &format!("by: {spelling}\n"),
        );
        assert_eq!(
            error.message,
            "`order` entry `by` must be a capture name string"
        );
    }
}

#[test]
fn order_direction_is_closed() {
    for spelling in ["null", "ascending", "1", "DESC"] {
        let source =
            ORDER_RULE.replace("<ENTRY>", &format!("- by: major\n        dir: {spelling}"));
        let error = assert_anchored(
            &source,
            SchemaErrorKind::InvalidOrder,
            &format!("by: major\n        dir: {spelling}\n"),
        );
        assert_eq!(error.message, "`order` entry `dir` must be `asc` or `desc`");
    }
}

#[test]
fn order_strict_must_be_boolean() {
    for spelling in ["null", "\"true\"", "1"] {
        let source = ORDER_RULE.replace(
            "<ENTRY>",
            &format!("- by: major\n        strict: {spelling}"),
        );
        let error = assert_anchored(
            &source,
            SchemaErrorKind::InvalidOrder,
            &format!("by: major\n        strict: {spelling}\n"),
        );
        assert_eq!(error.message, "`order` entry `strict` must be a bool");
    }
}

/// §3.8: `by` must name one of the rule's own captures.
#[test]
fn order_by_must_name_declared_capture() {
    let source = ORDER_RULE.replace("<ENTRY>", "- by: minor");
    let error = assert_anchored(&source, SchemaErrorKind::InvalidOrder, "by: minor\n");
    assert_eq!(
        error.message,
        "`order` entry `by: minor` names no capture declared by this rule"
    );
}

/// §3.8 compares entries after defaults are applied, so a defaulted entry and
/// its fully spelled equivalent are duplicates. Only the later one is the
/// error.
#[test]
fn duplicate_order_is_checked_after_defaults() {
    let source = r#"version: 2
sections:
  - match: "/v(?<major>[0-9]+)/"
    captures:
      major: int
    order:
      - by: major
      - by: major
        dir: asc
        strict: false
"#;
    let error = assert_anchored(
        source,
        SchemaErrorKind::InvalidOrder,
        "by: major\n        dir: asc\n        strict: false\n",
    );
    assert_eq!(
        error.message,
        "`order` already declares this ordering of capture `major`"
    );

    // Entries differing in any normalized component are not duplicates.
    let distinct = valid(
        r#"version: 2
sections:
  - match: "/v(?<major>[0-9]+)/"
    captures:
      major: int
    order:
      - by: major
      - by: major
        dir: desc
      - by: major
        strict: true
"#,
    );
    assert_eq!(distinct.addressed_root_rules()[0].order.len(), 3);
}

/// §3.8 orders a rule's repeated matches, so a rule that can match at most
/// once has nothing to order. Every cardinality spelling bounded at one is
/// refused, and each entry is reported.
#[test]
fn order_requires_repeatable_rule() {
    for cardinality in ["required: true", "required: false", "repeat: \"0..1\""] {
        let source = format!(
            "version: 2\nsections:\n  - match: \"/v(?<major>[0-9]+)/\"\n    {cardinality}\n\
             \x20   captures:\n      major: int\n    order:\n      - by: major\n\
             \x20     - by: major\n        dir: desc\n"
        );
        assert_errors(
            &source,
            &[
                (SchemaErrorKind::InvalidOrder, "by: major\n      "),
                (
                    SchemaErrorKind::InvalidOrder,
                    "by: major\n        dir: desc\n",
                ),
            ],
        );
    }

    // The open default and every bounded maximum above one are accepted.
    for cardinality in ["", "    repeat: \"0..2\"\n", "    repeat: \"1..n\"\n"] {
        let source = format!(
            "version: 2\nsections:\n  - match: \"/v(?<major>[0-9]+)/\"\n{cardinality}\
             \x20   captures:\n      major: int\n    order:\n      - by: major\n"
        );
        assert_eq!(valid(&source).addressed_root_rules()[0].order.len(), 1);
    }
}

/// §6.3: a cardinality that never normalized supplies no maximum, so the
/// maximum check is skipped rather than run against a guess. Nothing else
/// about the order depends on it.
#[test]
fn invalid_repeat_suppresses_order_max_check() {
    let source = r#"version: 2
sections:
  - match: "/v(?<major>[0-9]+)/"
    repeat: 01..2
    captures:
      major: int
    order:
      - by: major
"#;
    assert_errors(source, &[(SchemaErrorKind::InvalidRepeat, "01..2")]);
}

#[test]
fn conflicting_cardinality_suppresses_order_max_check() {
    let source = r#"version: 2
sections:
  - match: "/v(?<major>[0-9]+)/"
    required: true
    repeat: "1..3"
    captures:
      major: int
    order:
      - by: major
"#;
    assert_errors(
        source,
        &[(SchemaErrorKind::ConflictingCardinality, "\"1..3\"")],
    );
}

/// §6.3 again, from the other side: a capture mapping that never became one
/// cannot answer `by`, so no `invalid-order` is invented for a well-shaped
/// entry — while an entry that is independently malformed still reports.
#[test]
fn malformed_captures_suppress_only_capture_dependent_order_checks() {
    let well_shaped = r#"version: 2
sections:
  - match: "/v(?<major>[0-9]+)/"
    captures:
      major: integer
    order:
      - by: major
"#;
    assert_errors(
        well_shaped,
        &[(SchemaErrorKind::InvalidCapture, "major: integer")],
    );

    let independently_malformed = r#"version: 2
sections:
  - match: "/v(?<major>[0-9]+)/"
    captures:
      major: integer
    order:
      - by: major
        reverse: true
"#;
    assert_errors(
        independently_malformed,
        &[
            (SchemaErrorKind::InvalidCapture, "major: integer"),
            (
                SchemaErrorKind::InvalidOrder,
                "by: major\n        reverse: true\n",
            ),
        ],
    );
}

/// Order entries are addressable by position, in every form a rule is spelled
/// in.
#[test]
fn order_nodes_are_addressable_in_every_rule_form() {
    let sugar = r#"version: 2
title: Doc
sections:
  - match: "/v(?<major>[0-9]+)/"
    captures:
      major: int
    order:
      - by: major
    sections:
      - match: "/r(?<minor>[0-9]+)/"
        captures:
          minor: int
        order:
          - by: minor
            dir: desc
"#;
    let loaded = load_schema(sugar).expect("the sugar schema loads");
    assert_eq!(
        order_slice(&loaded, sugar, ScopePath(Vec::new()), 0, 0),
        "by: major\n    "
    );
    assert_eq!(
        order_slice(&loaded, sugar, ScopePath(vec![RuleIndex(0)]), 0, 0),
        "by: minor\n            dir: desc\n"
    );

    let outline = r#"version: 2
outline:
  - match: "/v(?<major>[0-9]+)/"
    captures:
      major: int
    order:
      - by: major
"#;
    let loaded = load_schema(outline).expect("the outline schema loads");
    assert_eq!(
        order_slice(&loaded, outline, ScopePath(Vec::new()), 0, 0),
        "by: major\n"
    );
}

/// A rule with `<ENTRY>` substituted into a one-capture `order` list.
const ORDER_RULE: &str = "version: 2\nsections:\n  - match: \"/v(?<major>[0-9]+)/\"\n\
                          \x20   captures:\n      major: int\n    order:\n      <ENTRY>\n";

/// The source text one rule's order-entry node addresses.
#[track_caller]
fn order_slice<'a>(
    loaded: &LoadedSchema,
    source: &'a str,
    scope: ScopePath,
    index: usize,
    order_index: usize,
) -> &'a str {
    let path = OrderEntryPath {
        rule: RulePath {
            scope,
            index: RuleIndex(index),
        },
        order_index: OrderIndex(order_index),
    };
    let range = loaded
        .locations
        .nodes
        .get(&SchemaNode::OrderEntry(path.clone()))
        .unwrap_or_else(|| panic!("missing order node for {path:?}"));
    source_slice(source, *range)
}

/// A validated capture name, taken from a schema that declares it: the type
/// deliberately has no public constructor.
#[track_caller]
fn capture_name(name: &str) -> CaptureName {
    valid(&format!(
        "version: 2\nsections:\n  - match: \"/(?<{name}>.+)/\"\n    captures:\n      {name}: text\n"
    ))
    .addressed_root_rules()[0]
        .captures
        .keys()
        .next()
        .expect("the schema declares one capture")
        .clone()
}

// --- §4.1/§4.3 named scopes -----------------------------------------------

/// A rule's captures and its direct child ids share one named scope, so a
/// name declared in both collides (§4.3). §6.3 anchors at whichever
/// declaration the document spells second and relates the first — here, the
/// child id — for an explicit and a default id alike.
#[test]
fn a_capture_declared_before_a_child_id_anchors_the_child() {
    for (child, id_anchor) in CHILD_ID_SPELLINGS {
        let source = format!(
            "version: 2\nsections:\n  - match: \"/v(?<major>[0-9]+)/\"\n\
             \x20   captures:\n      major: int\n    sections:\n      - {child}\n"
        );
        let error = assert_anchored(&source, SchemaErrorKind::DuplicateId, id_anchor);
        assert_eq!(
            error.message,
            "rule id `major` collides with a capture in the same named scope"
        );
        assert_related(&source, &error, "major: int");
    }
}

/// The same collision written the other way round: the capture comes second,
/// so the capture entry anchors and the child id is the related location.
#[test]
fn a_capture_declared_after_a_child_id_anchors_the_capture() {
    for (child, id_anchor) in CHILD_ID_SPELLINGS {
        let source = format!(
            "version: 2\nsections:\n  - match: \"/v(?<major>[0-9]+)/\"\n\
             \x20   sections:\n      - {child}\n    captures:\n      major: int\n"
        );
        let error = assert_anchored(&source, SchemaErrorKind::DuplicateId, "major: int");
        assert_eq!(
            error.message,
            "capture `major` collides with a rule id in the same named scope"
        );
        assert_related(&source, &error, id_anchor);
    }
}

/// A child rule named `major`, spelled explicitly and by §4.2 default, with
/// the scalar each spelling anchors its identity at.
const CHILD_ID_SPELLINGS: [(&str, &str); 2] = [
    ("id: major\n        match: Major", "major"),
    ("match: Major", "Major"),
];

/// §2.1: names enter the scope only once their mapping is well-formed, so one
/// invalid declaration keeps every name beside it out of the comparison.
#[test]
fn an_invalid_capture_prevents_every_capture_child_collision() {
    let source = r#"version: 2
sections:
  - match: "/v(?<major>[0-9]+)/"
    captures:
      major: int
      bogus: nope
    sections:
      - id: major
        match: Major
"#;
    assert_errors(source, &[(SchemaErrorKind::InvalidCapture, "bogus: nope")]);
}

/// An `order` that failed says nothing about the capture mapping, which is
/// what the named scope reads, so the collision is still reported.
#[test]
fn an_invalid_order_does_not_suppress_a_capture_child_collision() {
    let source = r#"version: 2
sections:
  - match: "/v(?<major>[0-9]+)/"
    captures:
      major: int
    order:
      - by: minor
    sections:
      - id: major
        match: Major
"#;
    assert_errors(
        source,
        &[
            (SchemaErrorKind::InvalidOrder, "by: minor\n    "),
            (SchemaErrorKind::DuplicateId, "major"),
        ],
    );
}

/// The scope a rule opens holds its captures and its direct children. The
/// rule's own id is a name one scope up, and a grandchild's is one scope
/// down, so neither collides with a capture.
#[test]
fn a_capture_collides_with_neither_its_own_rule_nor_a_grandchild() {
    let schema = valid(
        r#"version: 2
sections:
  - id: major
    match: "/v(?<major>[0-9]+)/"
    captures:
      major: int
    sections:
      - match: Section
        sections:
          - id: major
            match: Deep
"#,
    );
    let rule = &schema.addressed_root_rules()[0];
    assert_eq!(rule.id, Some(RuleId("major".into())));
    assert_eq!(rule.captures.len(), 1);
    assert_eq!(
        rule.children.rules()[0].children.rules()[0].id,
        Some(RuleId("major".into()))
    );
}

/// §4.3 makes names unique within a scope, not globally: two rules may each
/// declare the same capture name.
#[test]
fn separate_rules_may_declare_the_same_capture_name() {
    let schema = valid(
        r#"version: 2
sections:
  - match: "/v(?<major>[0-9]+)/"
    captures:
      major: int
  - match: "/r(?<major>[0-9]+)/"
    captures:
      major: int
"#,
    );
    let rules = schema.addressed_root_rules();
    assert_eq!(rules[0].captures, rules[1].captures);
}

/// §4.1 reserves both leading names at the schema root, explicitly spelled or
/// derived from an exact matcher.
#[test]
fn reserved_root_ids_are_rejected() {
    for id in ["fm", "linkdefs"] {
        let source = format!("version: 2\nsections:\n  - id: {id}\n    match: Intro\n");
        let error = assert_anchored(&source, SchemaErrorKind::ReservedId, id);
        assert!(
            error
                .message
                .starts_with(&format!("top-level rule id `{id}` is reserved for")),
            "unexpected message: {:?}",
            error.message
        );
    }
}

#[test]
fn generated_reserved_root_ids_are_rejected() {
    for (matcher, id) in [
        ("fm", "fm"),
        ("Link Defs", "link-defs"),
        ("Linkdefs", "linkdefs"),
    ] {
        let source = format!("version: 2\nsections:\n  - match: {matcher}\n");
        if id == "link-defs" {
            // Only the exact reserved spellings are held back; a slug that
            // merely resembles one is an ordinary id.
            assert_eq!(
                valid(&source).addressed_root_rules()[0].id,
                Some(RuleId(id.into()))
            );
            continue;
        }
        let error = assert_anchored(&source, SchemaErrorKind::ReservedId, matcher);
        assert!(
            error.message.starts_with(&format!(
                "top-level auto-generated rule id `{id}` is reserved for"
            )),
            "unexpected message: {:?}",
            error.message
        );
    }
}

/// The reservation is on top-level ids alone: nested rules may take either
/// name.
#[test]
fn nested_reserved_names_are_ordinary_rule_ids() {
    let schema = valid(
        r#"version: 2
sections:
  - match: Doc
    sections:
      - id: fm
        match: Front
      - id: linkdefs
        match: Links
"#,
    );
    let children = &schema.addressed_root_rules()[0].children.rules();
    assert_eq!(children[0].id, Some(RuleId("fm".into())));
    assert_eq!(children[1].id, Some(RuleId("linkdefs".into())));
}

/// A schema that loads publishes both new node kinds, each narrower than the
/// document.
#[test]
fn successful_locations_carry_capture_and_order_nodes() {
    let source = r#"version: 2
sections:
  - match: "/v(?<major>[0-9]+)/"
    captures:
      major: int
    order:
      - by: major
        dir: desc
"#;
    let loaded = load_schema(source).expect("the schema loads");
    let rule = RulePath {
        scope: ScopePath(Vec::new()),
        index: RuleIndex(0),
    };
    let capture = loaded
        .locations
        .nodes
        .keys()
        .filter_map(|node| match node {
            SchemaNode::Capture(path) if path.rule == rule => Some(path.name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(capture, vec!["major"]);
    assert_eq!(
        order_slice(&loaded, source, ScopePath(Vec::new()), 0, 0),
        "by: major\n        dir: desc\n"
    );
}

/// The matcher's compiler and the capture analyzer share one pinned
/// `regex-syntax`, so they agree on what a pattern is. A body either rejects
/// is `invalid-matcher` alone, never a capture fault reported twice: a
/// repeated group name is refused by both, and only the matcher says so.
#[test]
fn the_matcher_and_the_capture_analyzer_agree_on_a_pattern() {
    let source = r#"version: 2
sections:
  - match: "/(?<major>x)(?<major>y)/"
    captures:
      major: text
"#;
    assert_errors(
        source,
        &[(
            SchemaErrorKind::InvalidMatcher,
            "\"/(?<major>x)(?<major>y)/\"",
        )],
    );
}

/// §6.3 collects independent schema errors together, and §3.8's four order
/// checks are independent past an entry's own structure: a duplicate, an
/// undeclared `by`, and an unrepeatable rule are three separate faults, and
/// finding one must not hide another. Each is reported against every entry it
/// applies to, so one entry can carry more than one.
#[test]
fn undeclared_by_on_an_unrepeatable_rule_reports_both_faults() {
    let source = r#"version: 2
sections:
  - match: "/v(?<major>[0-9]+)/"
    required: true
    captures:
      major: int
    order:
      - by: minor
"#;
    assert_errors(
        source,
        &[
            (SchemaErrorKind::InvalidOrder, "by: minor\n"),
            (SchemaErrorKind::InvalidOrder, "by: minor\n"),
        ],
    );
    assert_eq!(
        error_messages(source),
        vec![
            "`order` entry `by: minor` names no capture declared by this rule".to_owned(),
            "`order` needs a rule that can match more than once, and this rule's effective \
             maximum is one"
                .to_owned(),
        ]
    );
}

/// The maximum is a fact about the rule, not about any one entry, so §3.8
/// rejects every entry — the duplicate included, which an earlier pass must
/// not have removed from this one's view.
#[test]
fn an_unrepeatable_rule_rejects_every_order_entry_including_duplicates() {
    let source = r#"version: 2
sections:
  - match: "/v(?<major>[0-9]+)/"
    required: true
    captures:
      major: int
    order:
      - by: major
      - by: major
"#;
    assert_errors(
        source,
        &[
            (SchemaErrorKind::InvalidOrder, "by: major\n      "),
            (SchemaErrorKind::InvalidOrder, "by: major\n"),
            (SchemaErrorKind::InvalidOrder, "by: major\n"),
        ],
    );
    assert_eq!(
        error_messages(source),
        vec![
            "`order` needs a rule that can match more than once, and this rule's effective \
             maximum is one"
                .to_owned(),
            "`order` already declares this ordering of capture `major`".to_owned(),
            "`order` needs a rule that can match more than once, and this rule's effective \
             maximum is one"
                .to_owned(),
        ]
    );
}

/// A duplicate entry is still an entry: its `by` is resolved like any other's,
/// rather than going unchecked because an earlier pass had already rejected
/// it.
#[test]
fn a_duplicate_entry_still_reports_its_undeclared_capture() {
    let source = r#"version: 2
sections:
  - match: "/v(?<major>[0-9]+)/"
    captures:
      major: int
    order:
      - by: minor
      - by: minor
"#;
    assert_eq!(
        error_messages(source),
        vec![
            "`order` entry `by: minor` names no capture declared by this rule".to_owned(),
            "`order` already declares this ordering of capture `minor`".to_owned(),
            "`order` entry `by: minor` names no capture declared by this rule".to_owned(),
        ]
    );
}

/// Every message one invalid schema produced, in the order the loader
/// collected them.
fn error_messages(source: &str) -> Vec<String> {
    invalid(source)
        .errors
        .iter()
        .map(|error| error.message.clone())
        .collect()
}

/// Asserts the loader collected exactly one error, of `kind` and anchored at
/// `anchor`, and returns it.
#[track_caller]
fn sole_error(source: &str, kind: SchemaErrorKind, anchor: &str) -> SchemaError {
    let invalid = invalid(source);
    let errors = invalid.errors.iter().cloned().collect::<Vec<_>>();
    assert_eq!(errors.len(), 1, "expected one error, collected {errors:#?}");
    assert_eq!(errors[0].kind, kind);
    assert_eq!(source_slice(source, errors[0].range), anchor);
    errors[0].clone()
}
