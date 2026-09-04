//! The locator module's unit tests.
//!
//! [`provider_boundary`] states, as executable fact, what the pinned JSONPath
//! provider does at the edge this module wraps. Those facts are the premises
//! of the wrapper's design, not incidental observations, so they are asserted
//! directly against `serde_json_path` rather than inferred from the wrapper's
//! own behaviour. A provider bump that changes one of them must fail here,
//! where the reason is written down, instead of somewhere downstream where it
//! would look like an Outlint bug.

/// The Phase 0 path renderer, compiled into this test build unmodified.
///
/// It lives in `tests/support` because the integration suites established it
/// and pin every escaping rule it implements. Including it by path rather than
/// copying it keeps exactly one implementation of the proven behaviour in the
/// tree: the production renderer is checked against *this* code, and a copy
/// would turn that parity check into a tautology.
#[path = "../../tests/support/jsonpath_path.rs"]
mod proven_jsonpath_path;

/// What the pinned `serde_json_path = 0.7.2` guarantees this module.
///
/// Five facts are load-bearing, and each has tests below saying so:
///
/// 1. A `]` or `=` inside a quoted name is part of the name. This is why the
///    `fm[...]` wrapper cannot be split on the first `]` or `=` (§4.6: "the
///    wrapper ends after parsing that complete query, not at the first `]` or
///    `=` occurring inside it").
/// 2. Vendor-tier constructs — filters, nested selectors, escapes — parse as
///    complete queries, so admission must be delegation, not classification
///    (§4.6: a non-core query "MUST NOT be rejected merely for falling outside
///    the guaranteed core").
/// 3. Unknown functions and malformed queries fail, with an error carrying a
///    message and a position worth copying into an Outlint-owned error.
/// 4. Duplicate references to one node arrive *un*-collapsed, so §4.6's
///    "duplicate references to the same result node are collapsed" is work
///    this module has to do.
/// 5. A location exposes name and index components. Its *spelling* does not
///    round-trip, which is why §4.6 makes rendering Outlint's own job.
mod provider_boundary {
    use serde_json::{json, Map, Value};
    use serde_json_path::{JsonPath, PathElement};

    use super::proven_jsonpath_path::{render_json_pointer, render_normalized_path};

    /// An Outlint-owned copy of one provider path element.
    ///
    /// The production wrapper will own a type like this. Building it here
    /// proves the provider offers everything such a type needs: a borrowed
    /// name and an already-resolved non-negative index, with no dependence on
    /// any rendered text.
    #[derive(Debug, PartialEq, Eq)]
    enum Component {
        Name(String),
        Index(usize),
    }

    impl Component {
        fn from_provider(element: &PathElement<'_>) -> Self {
            match element {
                PathElement::Name(name) => Component::Name((*name).to_owned()),
                PathElement::Index(index) => Component::Index(*index),
            }
        }
    }

    /// Evaluates `query` and pairs each node's rendered path with its value.
    ///
    /// Rendering goes through the proven renderer because the provider's own
    /// `Display` is not trustworthy; see `tests/support/jsonpath_path.rs`.
    fn rendered(query: &str, document: &Value) -> Vec<(String, Value)> {
        let path = JsonPath::parse(query).expect("the query must parse");
        path.query_located(document)
            .iter()
            .map(|node| (render_normalized_path(node.location()), node.node().clone()))
            .collect()
    }

    /// Returns both renderings of the path of the one node `query` selects.
    fn render_only(query: &str, document: &Value) -> (String, String) {
        let path = JsonPath::parse(query).expect("the query must parse");
        let nodes = path.query_located(document);
        assert_eq!(nodes.len(), 1, "`{query}` must select exactly one node");
        let location = nodes.iter().next().expect("one node").location();
        (
            render_normalized_path(location),
            render_json_pointer(location),
        )
    }

    /// Returns the owned components of the one node `query` selects.
    fn only_components(query: &str, document: &Value) -> Vec<Component> {
        let path = JsonPath::parse(query).expect("the query must parse");
        let nodes = path.query_located(document);
        assert_eq!(nodes.len(), 1, "`{query}` must select exactly one node");
        nodes
            .iter()
            .next()
            .expect("one node")
            .location()
            .iter()
            .map(Component::from_provider)
            .collect()
    }

    /// Builds a one-member object and the query selecting that member.
    ///
    /// The member name is exactly the character `code_point`, which is how a
    /// control character reaches a name without being written literally here.
    fn single_character_member(code_point: u32) -> (String, Value) {
        let name = char::from_u32(code_point)
            .expect("a valid scalar value")
            .to_string();
        let mut object = Map::new();
        object.insert(name.clone(), Value::from(1));
        let query = format!(
            "$[{}]",
            serde_json::to_string(&name).expect("a JSON string")
        );
        (query, Value::Object(object))
    }

    // --- fact 1: a quoted `]` or `=` belongs to the name -------------------

    #[test]
    fn a_bracket_inside_a_quoted_name_does_not_close_the_segment() {
        let document = json!({ "a]b": 1 });
        assert_eq!(
            rendered("$['a]b']", &document),
            vec![("$['a]b']".to_owned(), json!(1))]
        );
    }

    #[test]
    fn an_equals_sign_inside_a_quoted_name_is_part_of_the_name() {
        let document = json!({ "a=b": "wanted" });
        assert_eq!(
            rendered("$['a=b']", &document),
            vec![("$['a=b']".to_owned(), json!("wanted"))]
        );
    }

    /// The two together, in both quote forms, are the wrapper's worst case:
    /// splitting on either character would truncate this query mid-name.
    #[test]
    fn both_quote_forms_carry_brackets_and_equals_signs() {
        let document = json!({ "]=[": 1 });
        assert_eq!(
            rendered(r#"$["]=["]"#, &document),
            vec![("$[']=[']".to_owned(), json!(1))]
        );
        assert_eq!(
            rendered("$[']=[']", &document),
            vec![("$[']=[']".to_owned(), json!(1))]
        );
    }

    // --- fact 2: vendor-tier constructs parse as complete queries ----------

    #[test]
    fn a_filter_holding_brackets_and_equals_inside_a_string_parses() {
        let document = json!({ "items": [{ "name": "a]=b" }, { "name": "other" }] });
        assert_eq!(
            rendered(r"$.items[?@['name'] == 'a]=b']", &document),
            vec![("$['items'][0]".to_owned(), json!({ "name": "a]=b" }))]
        );
    }

    #[test]
    fn an_escaped_quote_inside_a_filter_string_does_not_end_it() {
        let document = json!({ "items": [{ "name": "it's" }] });
        assert_eq!(
            rendered(r"$.items[?@['name'] == 'it\'s']", &document),
            vec![("$['items'][0]".to_owned(), json!({ "name": "it's" }))]
        );
    }

    #[test]
    fn escaped_quotes_and_a_reverse_solidus_parse_in_both_quote_forms() {
        let document = json!({ "it's": 1, "say \"hi\"": 2, "back\\slash": 3 });
        assert_eq!(
            rendered(r"$['it\'s']", &document),
            vec![(r"$['it\'s']".to_owned(), json!(1))]
        );
        assert_eq!(
            rendered(r#"$["say \"hi\""]"#, &document),
            vec![(r#"$['say "hi"']"#.to_owned(), json!(2))]
        );
        assert_eq!(
            rendered(r#"$["back\\slash"]"#, &document),
            vec![(r"$['back\\slash']".to_owned(), json!(3))]
        );
    }

    /// The registry §4.6 admits, exercised so a provider bump that drops one
    /// is caught here rather than in a schema author's file.
    #[test]
    fn the_admitted_function_registry_parses() {
        for query in [
            "$[?length(@.a) > 1]",
            "$[?count(@.a[*]) > 1]",
            r"$[?match(@.a, 'x.*')]",
            r"$[?search(@.a, 'x')]",
            "$[?value(@.a) == 1]",
        ] {
            assert!(
                JsonPath::parse(query).is_ok(),
                "`{query}` uses an admitted RFC 9535 function and must parse"
            );
        }
    }

    #[test]
    fn slices_descendants_and_unions_parse() {
        for query in [
            "$.a[1:3]",
            "$.a[::-1]",
            "$..status",
            "$['draft','published']",
        ] {
            assert!(
                JsonPath::parse(query).is_ok(),
                "`{query}` is vendor-tier but valid RFC 9535, and must parse"
            );
        }
    }

    // --- fact 3: failures are reported, with a message and a position ------

    #[test]
    fn an_unknown_function_is_rejected() {
        assert!(
            JsonPath::parse("$[?unknown(@.a)]").is_err(),
            "§4.6 admits exactly the initial RFC 9535 registry"
        );
    }

    #[test]
    fn malformed_queries_are_rejected() {
        for query in [
            "",                    // no root identifier
            "$[",                  // unterminated segment
            "$.",                  // dangling name separator
            "$['a",                // unterminated string
            r"$['a\']",            // the escape consumes the closing quote
            "$.a extra",           // trailing junk
            "$[01]",               // a leading zero is not an index
            "$[9007199254740992]", // one past the I-JSON exact bound
        ] {
            assert!(
                JsonPath::parse(query).is_err(),
                "`{query}` must not parse as one complete RFC 9535 query"
            );
        }
    }

    /// A relative query has no meaning at either Outlint binding site: §4.6
    /// evaluates `fm[...]` against the frontmatter root, and §2.3 calls an
    /// `@`-rooted capture path `invalid-capture` because "this binding site
    /// supplies no current node".
    #[test]
    fn a_relative_query_is_rejected_outside_a_filter() {
        assert!(JsonPath::parse("@").is_err());
        assert!(JsonPath::parse("@.a").is_err());
    }

    #[test]
    fn a_parse_error_carries_a_message_and_a_position() {
        let error = JsonPath::parse("$[").expect_err("`$[` is incomplete");
        assert!(
            !error.message().is_empty(),
            "an Outlint-owned error copies this message"
        );
        // The position is an offset into the query, so it stays meaningful
        // once the query source is stored on Outlint's side.
        assert!(error.position() <= "$[".len());
    }

    // --- fact 4: duplicates arrive un-collapsed ---------------------------

    #[test]
    fn a_repeated_selector_returns_one_located_node_per_reference() {
        let document = json!({ "a": 1 });
        assert_eq!(
            rendered("$['a','a']", &document),
            vec![
                ("$['a']".to_owned(), json!(1)),
                ("$['a']".to_owned(), json!(1)),
            ],
            "§4.6's collapse of duplicate node references is Outlint's work"
        );
    }

    /// The converse: two different paths holding equal values are two nodes,
    /// so deduplication must key on the path and never on the value.
    #[test]
    fn equal_values_at_different_paths_are_separate_nodes() {
        let document = json!({ "a": 1, "b": 1 });
        let both = rendered("$['a','b']", &document);
        assert_eq!(both.len(), 2);
        assert_ne!(both[0].0, both[1].0);
        assert_eq!(both[0].1, both[1].1);
    }

    // --- fact 5: locations expose components, not a trustworthy spelling ---

    #[test]
    fn a_location_exposes_name_and_index_components() {
        let document = json!({ "a": [{ "b": 1 }] });
        assert_eq!(
            only_components("$.a[0].b", &document),
            vec![
                Component::Name("a".to_owned()),
                Component::Index(0),
                Component::Name("b".to_owned()),
            ]
        );
    }

    #[test]
    fn the_root_location_has_no_components() {
        assert_eq!(only_components("$", &json!(1)), Vec::<Component>::new());
    }

    /// A negative index is resolved to its non-negative position before it
    /// reaches a component, so nothing downstream has to normalize one.
    #[test]
    fn a_negative_index_arrives_already_resolved() {
        let document = json!(["x", "y", "z"]);
        assert_eq!(
            only_components("$[-1]", &document),
            vec![Component::Index(2)]
        );
    }

    /// The defect that makes an Outlint-owned renderer necessary, stated at
    /// this module's own boundary: the provider's spelling of a name holding
    /// an apostrophe does not round-trip, while the proven renderer's does.
    #[test]
    fn the_provider_spelling_is_not_authoritative_but_the_components_are() {
        let document = json!({ "it's": 1 });
        let path = JsonPath::parse(r#"$["it's"]"#).expect("valid query");
        let nodes = path.query_located(&document);
        let provider = nodes
            .iter()
            .next()
            .expect("one node")
            .location()
            .to_string();
        assert!(
            JsonPath::parse(&provider).is_err(),
            "the provider's own spelling `{provider}` is expected not to parse"
        );

        let (owned, _) = render_only(r#"$["it's"]"#, &document);
        assert_eq!(owned, r"$['it\'s']");
        assert_eq!(rendered(&owned, &document), vec![(owned.clone(), json!(1))]);
    }

    #[test]
    fn the_proven_renderer_escapes_apostrophes_and_reverse_solidi() {
        let document = json!({ "it's": 1, "back\\slash": 2 });
        assert_eq!(render_only(r#"$["it's"]"#, &document).0, r"$['it\'s']");
        assert_eq!(
            render_only(r#"$["back\\slash"]"#, &document).0,
            r"$['back\\slash']"
        );
    }

    #[test]
    fn the_proven_renderer_escapes_every_c0_control_class() {
        for (code_point, escaped) in [
            (0x08_u32, r"\b"),
            (0x09, r"\t"),
            (0x0A, r"\n"),
            (0x0C, r"\f"),
            (0x0D, r"\r"),
            (0x00, r"\u0000"),
            (0x0B, r"\u000b"),
            (0x1F, r"\u001f"),
        ] {
            let (query, document) = single_character_member(code_point);
            assert_eq!(
                render_only(&query, &document).0,
                format!("$['{escaped}']"),
                "U+{code_point:04X}"
            );
        }
    }

    #[test]
    fn the_proven_renderer_escapes_tilde_and_solidus_in_pointers_only() {
        let document = json!({ "a~b": { "c/d": 1 } });
        let (normalized, pointer) = render_only(r#"$["a~b"]["c/d"]"#, &document);
        assert_eq!(pointer, "/a~0b/c~1d");
        // The normalized path leaves both literal: neither is special there.
        assert_eq!(normalized, "$['a~b']['c/d']");
        assert_eq!(document.pointer(&pointer), Some(&json!(1)));
    }

    #[test]
    fn the_root_pointer_is_empty() {
        assert_eq!(render_only("$", &json!(1)).1, "");
    }
}

/// The §4.4 outline grammar, parsed with no schema in hand.
///
/// Everything here is about what a locator *says*. Nothing asserts what a
/// name denotes, because parsing runs before binding and cannot know.
mod outline_syntax {
    use proptest::prelude::*;

    use crate::locator::syntax::{
        parse_locator, LocatorAnchor, LocatorParseErrorKind, ParsedLocator, UnboundOutlineLocator,
    };

    /// Parses `source`, requiring the outline form.
    fn outline(source: &str) -> UnboundOutlineLocator {
        match parse_locator(source) {
            Ok(ParsedLocator::Outline(locator)) => locator,
            Ok(other) => panic!("`{source}` must parse as an outline locator, not {other:?}"),
            Err(error) => panic!("`{source}` must parse: {error}"),
        }
    }

    /// The name path, as `(spelling, subscript)` pairs.
    fn names(locator: &UnboundOutlineLocator) -> Vec<(&str, Option<String>)> {
        locator
            .name_steps()
            .iter()
            .map(|step| {
                (
                    step.name().as_str(),
                    step.position().map(ToString::to_string),
                )
            })
            .collect()
    }

    /// The structural path, as `(kind, subscript)` pairs.
    fn structure(locator: &UnboundOutlineLocator) -> Vec<(&str, Option<String>)> {
        locator
            .structural_steps()
            .iter()
            .map(|step| {
                (
                    step.kind().as_str(),
                    step.position().map(ToString::to_string),
                )
            })
            .collect()
    }

    /// Parses `source`, requiring rejection, and returns why.
    fn rejection(source: &str) -> LocatorParseErrorKind {
        match parse_locator(source) {
            Err(error) => error.kind(),
            Ok(parsed) => panic!("`{source}` must be rejected, but parsed as {parsed:?}"),
        }
    }

    // --- the forms §4.4 spells out -----------------------------------------

    #[test]
    fn a_bare_name_is_one_relative_step() {
        let locator = outline("rollback-plan");
        assert_eq!(locator.anchor(), LocatorAnchor::CurrentScope);
        assert_eq!(names(&locator), vec![("rollback-plan", None)]);
        assert!(structure(&locator).is_empty());
        assert!(locator.intrinsic_text().is_none());
    }

    #[test]
    fn a_dotted_relative_path_is_a_sequence_of_name_steps() {
        let locator = outline("deployment.rollback-plan");
        assert_eq!(locator.anchor(), LocatorAnchor::CurrentScope);
        assert_eq!(
            names(&locator),
            vec![("deployment", None), ("rollback-plan", None)]
        );
    }

    #[test]
    fn a_leading_dollar_dot_anchors_at_the_outermost_scope() {
        let locator = outline("$.overview.goals");
        assert_eq!(locator.anchor(), LocatorAnchor::SchemaRoot);
        assert_eq!(names(&locator), vec![("overview", None), ("goals", None)]);
    }

    #[test]
    fn a_subscript_may_narrow_any_name_step() {
        let locator = outline("$.release[0].notes[2]");
        assert_eq!(
            names(&locator),
            vec![
                ("release", Some("0".to_owned())),
                ("notes", Some("2".to_owned())),
            ]
        );
    }

    /// §4.4 admits structural traversal "when those kinds exist"; none does in
    /// this version, so the tokens are retained and left unallocated.
    #[test]
    fn structural_steps_are_retained_without_being_allocated() {
        let locator = outline("$.section/list[0]/item[2]");
        assert_eq!(names(&locator), vec![("section", None)]);
        assert_eq!(
            structure(&locator),
            vec![
                ("list", Some("0".to_owned())),
                ("item", Some("2".to_owned()))
            ]
        );
        assert!(locator.intrinsic_text().is_none());
    }

    #[test]
    fn the_text_intrinsic_terminates_a_locator() {
        let locator = outline("$.release[0]/text");
        assert_eq!(names(&locator), vec![("release", Some("0".to_owned()))]);
        assert!(structure(&locator).is_empty());
        let text = locator.intrinsic_text().expect("`/text` was written");
        assert!(text.position().is_none());
    }

    /// A subscript on `/text` itself narrows the texts a plural prior step
    /// produced; it is the following *step* that §4.4 forbids, not a
    /// subscript on the terminal one.
    #[test]
    fn the_text_intrinsic_may_carry_its_own_subscript() {
        let locator = outline("$.release/text[1]");
        let text = locator.intrinsic_text().expect("`/text` was written");
        assert_eq!(
            text.position().map(ToString::to_string),
            Some("1".to_owned())
        );
    }

    #[test]
    fn text_may_follow_structural_traversal() {
        let locator = outline("$.section/list[0]/text");
        assert_eq!(structure(&locator), vec![("list", Some("0".to_owned()))]);
        assert!(locator.intrinsic_text().is_some());
    }

    /// A capture name is admitted wherever a rule id is, because §4.4 makes
    /// which one a step denotes a question about the scope it lands in.
    #[test]
    fn a_terminal_name_may_be_spelled_as_a_capture_name() {
        let locator = outline("$.release[0].schema_version");
        assert_eq!(
            names(&locator),
            vec![("release", Some("0".to_owned())), ("schema_version", None),]
        );
    }

    // --- the original spelling is kept -------------------------------------

    #[test]
    fn the_original_spelling_is_retained_byte_for_byte() {
        for source in [
            "rollback-plan",
            "$.overview.goals",
            "$.release[0].notes[2]",
            "$.section/list[0]/item[2]",
            "$.release[0]/text",
            "fm.version",
        ] {
            let parsed = parse_locator(source).expect("a valid locator");
            assert_eq!(parsed.source().as_str(), source);
        }
    }

    // --- subscripts --------------------------------------------------------

    #[test]
    fn zero_and_an_arbitrarily_long_digit_run_are_both_indices() {
        assert_eq!(names(&outline("a[0]")), vec![("a", Some("0".to_owned()))]);
        let long = "1".to_owned() + &"0".repeat(200);
        let locator = outline(&format!("a[{long}]"));
        assert_eq!(names(&locator), vec![("a", Some(long))]);
    }

    #[test]
    fn malformed_subscripts_are_rejected() {
        for source in ["a[00]", "a[01]", "a[-1]", "a[+1]", "a[]", "a[ 0]", "a[0 ]"] {
            assert_eq!(
                rejection(source),
                LocatorParseErrorKind::InvalidPosition,
                "`{source}`"
            );
        }
    }

    #[test]
    fn an_unterminated_subscript_is_rejected() {
        for source in ["a[", "a[0", "$.a[12"] {
            assert_eq!(
                rejection(source),
                LocatorParseErrorKind::UnterminatedPosition,
                "`{source}`"
            );
        }
    }

    #[test]
    fn one_step_takes_at_most_one_subscript() {
        for source in ["a[0][1]", "$.a/list[0][1]", "$.a/text[0][1]"] {
            assert_eq!(
                rejection(source),
                LocatorParseErrorKind::RepeatedPosition,
                "`{source}`"
            );
        }
    }

    // --- anchors -----------------------------------------------------------

    #[test]
    fn a_bare_dollar_is_not_a_locator() {
        assert_eq!(rejection("$"), LocatorParseErrorKind::BareSchemaRoot);
    }

    #[test]
    fn a_dangling_dollar_dot_has_no_first_step() {
        assert_eq!(rejection("$."), LocatorParseErrorKind::EmptyStep);
    }

    /// §4.4: "The former `@` prefix is not part of the locator language."
    #[test]
    fn the_at_prefix_is_gone() {
        assert_eq!(rejection("@"), LocatorParseErrorKind::MalformedAnchor);
        assert_eq!(rejection("@.x"), LocatorParseErrorKind::MalformedAnchor);
    }

    #[test]
    fn a_dollar_not_followed_by_a_dot_is_malformed() {
        for source in ["$x", "$[0]", "$/text", "$$"] {
            assert_eq!(
                rejection(source),
                LocatorParseErrorKind::MalformedAnchor,
                "`{source}`"
            );
        }
    }

    #[test]
    fn the_empty_locator_is_rejected() {
        assert_eq!(rejection(""), LocatorParseErrorKind::Empty);
    }

    // --- separators and steps ----------------------------------------------

    #[test]
    fn leading_and_trailing_separators_leave_an_empty_step() {
        for source in [".a", "/a", "a.", "a/", "a..b", "a//b", "$..a"] {
            assert_eq!(
                rejection(source),
                LocatorParseErrorKind::EmptyStep,
                "`{source}`"
            );
        }
    }

    #[test]
    fn a_name_outside_both_grammars_is_rejected() {
        for source in [
            "Foo", "a b", "a-", "-a", "a--b", "_a", "a-b_c", "föö", "a=b",
        ] {
            assert_eq!(
                rejection(source),
                LocatorParseErrorKind::InvalidName,
                "`{source}`"
            );
        }
    }

    #[test]
    fn a_structural_kind_outside_the_slug_grammar_is_rejected() {
        for source in ["a/List", "a/my_kind", "a/x-", "a/x y"] {
            assert_eq!(
                rejection(source),
                LocatorParseErrorKind::InvalidStructuralKind,
                "`{source}`"
            );
        }
    }

    #[test]
    fn whitespace_is_never_part_of_a_locator() {
        for source in [" a", "a ", "a . b", "$. a", "a\tb", "a\nb"] {
            assert!(parse_locator(source).is_err(), "`{source}` must not parse");
        }
    }

    // --- the two shape rules the types enforce -----------------------------

    /// §4.4: "A locator may move from names to structure but MUST NOT use a
    /// name step after a structural step."
    #[test]
    fn a_name_step_cannot_follow_a_structural_step() {
        for source in ["a/list.name", "$.a/list[0].name", "a/list/item.x"] {
            assert_eq!(
                rejection(source),
                LocatorParseErrorKind::NameAfterStructure,
                "`{source}`"
            );
        }
    }

    /// §4.4 makes `/text` "a terminal intrinsic value", so nothing continues
    /// past it — not a name, not another structural step, not another `/text`.
    #[test]
    fn nothing_may_follow_the_text_intrinsic() {
        for source in [
            "$.a/text.b",
            "$.a/text/label",
            "$.a/text/text",
            "$.a/text[0]/text",
            "$.a/text[0].b",
        ] {
            assert_eq!(
                rejection(source),
                LocatorParseErrorKind::StepAfterIntrinsicText,
                "`{source}`"
            );
        }
    }

    #[test]
    fn a_character_where_a_separator_belongs_is_rejected() {
        for source in ["a[0]b", "a[0]]", "$.a[0]="] {
            assert_eq!(
                rejection(source),
                LocatorParseErrorKind::UnexpectedCharacter,
                "`{source}`"
            );
        }
    }

    // --- the `fm` forms ----------------------------------------------------

    #[test]
    fn a_frontmatter_capture_is_its_own_form() {
        let parsed = parse_locator("fm.version").expect("a valid capture reference");
        let ParsedLocator::FrontmatterCapture(capture) = parsed else {
            panic!("`fm.version` must not parse as an outline locator");
        };
        assert_eq!(capture.name().as_str(), "version");
        assert_eq!(capture.source().as_str(), "fm.version");
    }

    #[test]
    fn a_malformed_frontmatter_capture_is_rejected() {
        for source in [
            "fm.",
            "fm.Version",
            "fm.decision-makers",
            "fm.0version",
            "fm.version.minor",
            "fm.version[0]",
            "fm.version/text",
        ] {
            assert_eq!(
                rejection(source),
                LocatorParseErrorKind::MalformedFrontmatterCapture,
                "`{source}`"
            );
        }
    }

    #[test]
    fn bare_fm_names_nothing() {
        assert_eq!(rejection("fm"), LocatorParseErrorKind::BareFrontmatterRoot);
    }

    /// `fm[` opens the §4.6 query form. It must not fall through to the
    /// outline grammar, where `fm` would look like an ordinary name with a
    /// subscript that happens not to be an index.
    #[test]
    fn the_query_form_is_reserved_rather_than_read_as_a_name() {
        assert_eq!(
            rejection("fm[$.draft]"),
            LocatorParseErrorKind::FrontmatterQueryUnsupported
        );
    }

    /// §4.1 reserves `fm` as a *leading* name only, so an ordinary name that
    /// merely starts with those letters is untouched.
    #[test]
    fn a_name_beginning_with_fm_is_an_ordinary_name() {
        assert_eq!(names(&outline("fmt")), vec![("fmt", None)]);
        assert_eq!(names(&outline("fm-plan")), vec![("fm-plan", None)]);
        assert_eq!(
            names(&outline("deployment.fm")),
            vec![("deployment", None), ("fm", None)]
        );
    }

    // --- totality ----------------------------------------------------------

    proptest! {
        /// Locators come out of schema files, which are untrusted input, and
        /// the library "must not panic on malformed input".
        #[test]
        fn parsing_arbitrary_text_never_panics(source in ".*") {
            let _ = parse_locator(&source);
        }

        /// The same, over strings drawn from the locator alphabet, so the
        /// generator actually reaches the branches the grammar has.
        #[test]
        fn parsing_locator_shaped_text_never_panics(
            source in "[$@a-zA-Z0-9._/\\[\\]=+-]{0,24}"
        ) {
            let _ = parse_locator(&source);
        }

        /// Whatever parses, parses to itself.
        #[test]
        fn a_parsed_locator_still_spells_its_source(source in "[$a-z0-9._/\\[\\]-]{0,24}") {
            if let Ok(parsed) = parse_locator(&source) {
                prop_assert_eq!(parsed.source().as_str(), source.as_str());
            }
        }
    }
}

/// An index far larger than any node list, which §4.4 requires to be cheap.
///
/// §4.4: an out-of-range index "selects nothing and produces the empty list;
/// its magnitude is never an error", and "implementations MUST NOT allocate
/// memory or perform work proportional to an index's numeric value; processing
/// an index may be proportional only to the length of its spelling".
///
/// The assertions below are on memory and on the answer, never on elapsed
/// time: a wall-clock bound would be flaky, and the property that matters is
/// structural. The code side of the guarantee is [`LocatorPosition::select`],
/// which converts once and indexes once, with no loop over the value.
mod oversized_positions {
    use num_bigint::BigUint;

    use crate::locator::syntax::{parse_locator, ParsedLocator};

    /// Decimal spellings far beyond any array length, at several sizes.
    const OVERSIZED_DIGIT_COUNTS: [usize; 4] = [20, 100, 1_000, 10_000];

    #[test]
    fn an_oversized_index_costs_only_what_its_spelling_costs() {
        for digits in OVERSIZED_DIGIT_COUNTS {
            let spelling = "9".repeat(digits);
            let source = format!("$.release[{spelling}]");

            let Ok(ParsedLocator::Outline(locator)) = parse_locator(&source) else {
                panic!("a {digits}-digit index must parse");
            };
            let position = locator
                .name_steps()
                .first
                .position()
                .expect("the step is subscripted");
            let value: &BigUint = position.value();

            // Round-tripping proves the bounds below are not measuring a
            // truncated or saturated parse.
            assert_eq!(value.to_string(), spelling);

            // log2(10) < 10/3, so a `digits`-digit number needs fewer than
            // ceil(digits * 10 / 3) bits: bounded by the spelling, not by the
            // magnitude the spelling denotes.
            let bit_bound = (digits as u64 * 10).div_ceil(3);
            assert!(
                value.bits() <= bit_bound,
                "a {digits}-digit value used {} bits, above the {bit_bound}-bit spelling bound",
                value.bits()
            );
            let byte_bound = bit_bound.div_ceil(8) as usize;
            assert!(
                value.to_bytes_be().len() <= byte_bound,
                "a {digits}-digit value used {} bytes, above the {byte_bound}-byte bound",
                value.to_bytes_be().len()
            );

            // The real lookup helper, against a list far shorter than the
            // index: the empty result, reached without counting to it.
            let nodes = ["first", "second", "third"];
            assert_eq!(position.select(&nodes), None);
        }
    }

    /// The same helper, on the indices that do address something.
    #[test]
    fn an_in_range_index_selects_the_element_it_names() {
        let nodes = ["first", "second", "third"];
        for (spelling, expected) in [
            ("0", Some("first")),
            ("1", Some("second")),
            ("2", Some("third")),
            ("3", None),
        ] {
            let Ok(ParsedLocator::Outline(locator)) = parse_locator(&format!("a[{spelling}]"))
            else {
                panic!("`a[{spelling}]` must parse");
            };
            let position = locator
                .name_steps()
                .first
                .position()
                .expect("the step is subscripted");
            assert_eq!(position.select(&nodes).copied(), expected, "[{spelling}]");
        }
    }
}
