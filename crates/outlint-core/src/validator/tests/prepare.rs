use crate::loader::{json_schema_reference_budget_message, MAX_JSON_SCHEMA_REFERENCES};
use crate::validator::prepare::PreparedMatcher;
use crate::validator::{validate, PreparedValidator, ValidationError};
use crate::{
    load_schema, parse_markdown, ExactText, FrontmatterPolicy, FrontmatterSchema, GlobPattern,
    MarkdownOptions, Matcher, RegexPattern,
};

fn matcher_matches(matcher: &Matcher, text: &str, match_case: bool) -> bool {
    PreparedMatcher::new(matcher, match_case)
        .expect("test matcher compiles")
        .matches(text)
}

#[test]
fn every_matcher_form_is_fully_anchored() {
    assert!(matcher_matches(
        &Matcher::Exact(ExactText("cat".into())),
        "cat",
        true
    ));
    assert!(!matcher_matches(
        &Matcher::Exact(ExactText("cat".into())),
        "cats",
        true
    ));
    assert!(matcher_matches(
        &Matcher::Glob(GlobPattern("c*t".into())),
        "coat",
        true
    ));
    assert!(!matcher_matches(
        &Matcher::Glob(GlobPattern("c*t".into())),
        "a coat",
        true
    ));
    assert!(matcher_matches(
        &Matcher::Regex(RegexPattern("c.+t".into())),
        "coat",
        true
    ));
    assert!(!matcher_matches(
        &Matcher::Regex(RegexPattern("c.+t".into())),
        "a coat",
        true
    ));
}

#[test]
fn glob_treats_every_non_star_character_literally() {
    let matcher = Matcher::Glob(GlobPattern("file[1].*".into()));
    assert!(matcher_matches(&matcher, "file[1].md", true));
    assert!(!matcher_matches(&matcher, "file1.md", true));
}

#[test]
fn glob_star_matches_newlines_in_multiline_setext_text() {
    let matcher = Matcher::Glob(GlobPattern("first*last".into()));
    assert!(matcher_matches(&matcher, "first\nmiddle\nlast", true));
}

#[test]
fn exact_matching_does_not_compile_input_as_a_regex() {
    let text = "x".repeat(1_000_000);
    let matcher = Matcher::Exact(ExactText(text.clone()));
    assert!(matcher_matches(&matcher, &text, true));
}

#[test]
fn case_insensitive_matching_is_unicode_aware_for_all_forms() {
    let matchers = [
        Matcher::Exact(ExactText("ÉCOLE".into())),
        Matcher::Glob(GlobPattern("ÉCO*".into())),
        Matcher::Regex(RegexPattern("ÉCO.*".into())),
    ];
    for matcher in matchers {
        assert!(matcher_matches(&matcher, "école", false));
        assert!(!matcher_matches(&matcher, "école", true));
    }
    let simple_fold_matchers = [
        Matcher::Exact(ExactText("S".into())),
        Matcher::Glob(GlobPattern("S*".into())),
        Matcher::Regex(RegexPattern("S.*".into())),
    ];
    for matcher in simple_fold_matchers {
        assert!(matcher_matches(&matcher, "ſ", false));
    }

    let full_only_fold_matchers = [
        Matcher::Exact(ExactText("Straße".into())),
        Matcher::Glob(GlobPattern("Straße*".into())),
        Matcher::Regex(RegexPattern("Straße.*".into())),
    ];
    for matcher in full_only_fold_matchers {
        assert!(!matcher_matches(&matcher, "STRASSE", false));
    }
}

#[test]
fn inline_regex_flags_compose_with_match_case() {
    let matcher = Matcher::Regex(RegexPattern("(?i:api)".into()));
    assert!(matcher_matches(&matcher, "API", true));
    assert!(matcher_matches(&matcher, "api", true));
}

#[test]
fn malformed_manually_constructed_regex_fails_preparation() {
    let mut schema = load_schema("version: 1\nsections: []\n")
        .expect("test schema is valid")
        .schema;
    schema.outline[0].matcher = Matcher::Regex(RegexPattern("(".into()));
    let error = PreparedValidator::new(&schema)
        .err()
        .expect("malformed regex must fail preparation");
    assert!(error.message.contains("cannot compile matcher"));
}

#[test]
fn preparing_refuses_a_reference_chain_longer_than_the_compiler_can_recurse_over() {
    // Preparing a validator compiles the linked graph a second time, and
    // compiling a reference re-enters the compiler at its target, so a
    // chain costs a stack frame per link here exactly as it does in the
    // loader -- while every link sits at the same JSON depth, which is why
    // no nesting bound sees it. An overrun aborts the process rather than
    // returning, so this path cannot rely on the loader having refused
    // first; it charges the budget itself. Both sides of the boundary are
    // pinned, since a bound that quietly drifted below what it promises
    // would refuse graphs the compiler handles comfortably.
    let document = parse_markdown("---\nstatus: draft\n---\n", MarkdownOptions::default());

    let mut schema = load_schema("version: 1\ntitle: null\nsections: []\n")
        .expect("test schema is valid")
        .schema;
    schema.frontmatter = FrontmatterPolicy::Optional {
        schema: Some(reference_chain_schema(MAX_JSON_SCHEMA_REFERENCES - 1)),
    };
    assert!(validate(&schema, &document)
        .expect("a graph spending the whole budget still prepares")
        .is_empty());

    schema.frontmatter = FrontmatterPolicy::Optional {
        schema: Some(reference_chain_schema(MAX_JSON_SCHEMA_REFERENCES)),
    };
    let error = validate(&schema, &document).expect_err("one reference more is refused");
    let ValidationError::Preparation(error) = error else {
        panic!("a reference budget overrun is a preparation failure, not an operational one")
    };
    assert_eq!(error.message, json_schema_reference_budget_message());
}

/// Builds a graph whose root reference starts a chain of `links` hops
/// ending at `true`, declaring `links + 1` references in all.
fn reference_chain_schema(links: usize) -> FrontmatterSchema {
    let mut definitions = serde_json::Map::new();
    definitions.insert("end".into(), serde_json::Value::Bool(true));
    for index in 0..links {
        let target = if index + 1 == links {
            "#/$defs/end".to_owned()
        } else {
            format!("#/$defs/{}", index + 1)
        };
        definitions.insert(index.to_string(), serde_json::json!({ "$ref": target }));
    }
    FrontmatterSchema {
        root_uri: "https://outlint.invalid/root.json".into(),
        root: serde_json::json!({ "$ref": "#/$defs/0", "$defs": definitions }),
        resources: std::collections::BTreeMap::new(),
    }
}
