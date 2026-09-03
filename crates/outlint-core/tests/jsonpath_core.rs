//! Outlint's primary JSONPath release gate.
//!
//! Specification §4.6 defines a *guaranteed core* of the RFC 9535 grammar —
//! root, child name, index, and wildcard segments — and states that "only this
//! core is covered by Outlint's self-verification corpus; vendor-tier query
//! outcomes are not an Outlint conformance or release gate."
//!
//! This file is that self-verification corpus. Its expectations are authored
//! from the specification, not recorded from any implementation, and its
//! fixture contains no slice, descendant, union, filter, or function anywhere.
//! The official compliance suite is run separately and filtered, as secondary
//! evidence only; see `jsonpath_cts_core.rs`.

mod support;

use std::collections::BTreeMap;
use std::str::FromStr;

use num_bigint::BigUint;
use serde::Deserialize;
use serde_json::Value;
use serde_json_path::JsonPath;

use support::jsonpath_path::render_normalized_path;

const CORPUS: &str = include_str!("fixtures/jsonpath/outlint-core.json");

// ---------------------------------------------------------------------------
// Corpus shape
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Corpus {
    #[allow(dead_code)]
    description: String,
    #[allow(dead_code)]
    profile: String,
    documents: BTreeMap<String, Value>,
    selection: Vec<SelectionCase>,
    rejected: Vec<RejectedCase>,
    propositions: Vec<PropositionCase>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectionCase {
    name: String,
    document: String,
    selector: String,
    nodes: Vec<ExpectedNode>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedNode {
    path: String,
    value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RejectedCase {
    name: String,
    selector: String,
    #[allow(dead_code)]
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PropositionCase {
    name: String,
    document: String,
    selector: String,
    #[serde(default)]
    bare: Option<String>,
    #[serde(default, deserialize_with = "deserialize_present")]
    equals: Option<Value>,
    #[serde(default)]
    equality: Option<String>,
}

/// Distinguishes an absent `equals` from one whose literal is JSON `null`,
/// which §4.6 gives its own always-false rule.
fn deserialize_present<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn corpus() -> Corpus {
    serde_json::from_str(CORPUS).expect("the authored core corpus must deserialize strictly")
}

fn document<'a>(corpus: &'a Corpus, name: &str) -> &'a Value {
    corpus
        .documents
        .get(name)
        .unwrap_or_else(|| panic!("the corpus must define a document named `{name}`"))
}

// ---------------------------------------------------------------------------
// Outlint's node set at the `fm[...]` boundary
// ---------------------------------------------------------------------------

/// One result node: its value, paired with the path identity Outlint renders.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Node {
    path: String,
    value: Value,
}

/// Evaluates a query and produces Outlint's boundary node set.
///
/// §4.6: "At the `fm[...]` boundary, duplicate references to the same result
/// node are collapsed; the resulting node set's order is not observable." The
/// set is therefore sorted by path identity so no test can depend on the
/// provider's nodelist order, and duplicates are collapsed.
fn node_set(query: &str, document: &Value) -> Vec<Node> {
    let path =
        JsonPath::parse(query).unwrap_or_else(|error| panic!("`{query}` must parse: {error}"));
    let located = path.query_located(document);
    let nodes: Vec<Node> = located
        .iter()
        .map(|node| Node {
            path: render_normalized_path(node.location()),
            value: node.node().clone(),
        })
        .collect();
    collapse(nodes)
}

/// Collapses duplicate path identities, keeping one node per distinct path,
/// and orders the result by path so it is a set rather than a list.
fn collapse(nodes: Vec<Node>) -> Vec<Node> {
    let mut by_path: BTreeMap<String, Value> = BTreeMap::new();
    for node in nodes {
        if let Some(existing) = by_path.get(&node.path) {
            assert_eq!(
                existing, &node.value,
                "two nodes shared the path identity `{}` but differed in value; \
                 collapsing them would discard information",
                node.path
            );
            continue;
        }
        by_path.insert(node.path, node.value);
    }
    by_path
        .into_iter()
        .map(|(path, value)| Node { path, value })
        .collect()
}

/// The outcome of a bare `fm[...]` typed boolean read (§4.6).
#[derive(Debug, PartialEq, Eq)]
enum BareRead {
    Satisfied,
    Unsatisfied,
    /// At least one node is neither boolean nor null: `invalid-value`, and the
    /// containing constraint is suppressed.
    InvalidValue,
}

fn bare_read(nodes: &[Node]) -> BareRead {
    // Invalidity is checked first and over every node: §4.6 says "a true
    // sibling result or another already-true operand does not short-circuit
    // that suppression".
    if nodes
        .iter()
        .any(|node| !node.value.is_boolean() && !node.value.is_null())
    {
        return BareRead::InvalidValue;
    }
    if nodes.iter().any(|node| node.value == Value::Bool(true)) {
        return BareRead::Satisfied;
    }
    BareRead::Unsatisfied
}

/// Existential equality for `fm[query]=literal` (§4.6).
fn equality(nodes: &[Node], literal: &Value) -> bool {
    // "`fm[query]=null` is always false."
    if literal.is_null() {
        return false;
    }
    nodes.iter().any(|node| {
        // Existential over non-null nodes; mappings and sequences never equal
        // a scalar literal, and there is no cross-type coercion.
        !node.value.is_null()
            && !node.value.is_object()
            && !node.value.is_array()
            && &node.value == literal
    })
}

// ---------------------------------------------------------------------------
// Corpus-driven tests
// ---------------------------------------------------------------------------

#[test]
fn the_corpus_is_well_formed() {
    let corpus = corpus();
    assert!(!corpus.selection.is_empty());
    assert!(!corpus.rejected.is_empty());
    assert!(!corpus.propositions.is_empty());

    let mut names: Vec<&str> = corpus
        .selection
        .iter()
        .map(|case| case.name.as_str())
        .chain(corpus.rejected.iter().map(|case| case.name.as_str()))
        .chain(corpus.propositions.iter().map(|case| case.name.as_str()))
        .collect();
    let total = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), total, "corpus case names must be unique");

    for case in &corpus.selection {
        document(&corpus, &case.document);
    }
    for case in &corpus.propositions {
        document(&corpus, &case.document);
        assert!(
            case.bare.is_some() || case.equality.is_some(),
            "proposition `{}` asserts nothing",
            case.name
        );
        assert_eq!(
            case.equals.is_some(),
            case.equality.is_some(),
            "proposition `{}` must pair `equals` with `equality`",
            case.name
        );
    }
}

#[test]
fn every_core_selection_case_matches_the_specification() {
    let corpus = corpus();
    let mut failures = Vec::new();

    for case in &corpus.selection {
        let expected: Vec<Node> = collapse(
            case.nodes
                .iter()
                .map(|node| Node {
                    path: node.path.clone(),
                    value: node.value.clone(),
                })
                .collect(),
        );
        let actual = node_set(&case.selector, document(&corpus, &case.document));
        if actual != expected {
            failures.push(format!(
                "\ncase: {}\n  selector: {}\n  expected: {}\n  actual:   {}",
                case.name,
                case.selector,
                render(&expected),
                render(&actual)
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} core selection cases failed:{}",
        failures.len(),
        corpus.selection.len(),
        failures.join("")
    );
}

#[test]
fn every_rejected_core_query_is_refused_at_binding() {
    let corpus = corpus();
    let mut failures = Vec::new();

    for case in &corpus.rejected {
        if JsonPath::parse(&case.selector).is_ok() {
            failures.push(format!(
                "\ncase: {}\n  selector: {}\n  expected: rejection ({})",
                case.name, case.selector, case.reason
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} rejection cases were accepted:{}",
        failures.len(),
        corpus.rejected.len(),
        failures.join("")
    );
}

#[test]
fn every_proposition_case_matches_the_specification() {
    let corpus = corpus();
    let mut failures = Vec::new();

    for case in &corpus.propositions {
        let nodes = node_set(&case.selector, document(&corpus, &case.document));

        if let Some(expected) = &case.bare {
            let expected = match expected.as_str() {
                "satisfied" => BareRead::Satisfied,
                "unsatisfied" => BareRead::Unsatisfied,
                "invalid-value" => BareRead::InvalidValue,
                other => panic!("case `{}` has an unknown bare outcome `{other}`", case.name),
            };
            let actual = bare_read(&nodes);
            if actual != expected {
                failures.push(format!(
                    "\ncase: {}\n  selector: {}\n  bare read expected {expected:?}, got {actual:?}",
                    case.name, case.selector
                ));
            }
        }

        if let (Some(literal), Some(expected)) = (&case.equals, &case.equality) {
            let expected = match expected.as_str() {
                "satisfied" => true,
                "unsatisfied" => false,
                other => panic!(
                    "case `{}` has an unknown equality outcome `{other}`",
                    case.name
                ),
            };
            let actual = equality(&nodes, literal);
            if actual != expected {
                failures.push(format!(
                    "\ncase: {}\n  selector: {}\n  equality against {literal} expected {expected}, got {actual}",
                    case.name, case.selector
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} proposition assertions failed:{}",
        failures.len(),
        failures.join("")
    );
}

fn render(nodes: &[Node]) -> String {
    let rendered: Vec<String> = nodes
        .iter()
        .map(|node| format!("{} => {}", node.path, node.value))
        .collect();
    format!("[{}]", rendered.join(", "))
}

// ---------------------------------------------------------------------------
// Duplicate path collapse
// ---------------------------------------------------------------------------

/// §4.6: "duplicate references to the same result node are collapsed; the
/// resulting node set's order is not observable."
///
/// A core query cannot itself produce a duplicate path — that needs a union or
/// a descendant segment, both vendor tier — so the collapse rule is exercised
/// against the boundary helper directly.
#[test]
fn duplicate_path_identities_collapse_to_one_node() {
    let duplicated = vec![
        Node {
            path: "$['a']".to_owned(),
            value: Value::from(1),
        },
        Node {
            path: "$['b']".to_owned(),
            value: Value::from(2),
        },
        Node {
            path: "$['a']".to_owned(),
            value: Value::from(1),
        },
    ];

    let collapsed = collapse(duplicated);
    assert_eq!(collapsed.len(), 2);
    assert_eq!(collapsed[0].path, "$['a']");
    assert_eq!(collapsed[1].path, "$['b']");
}

#[test]
fn collapse_orders_by_path_so_order_is_not_observable() {
    let one = vec![
        Node {
            path: "$['b']".to_owned(),
            value: Value::from(2),
        },
        Node {
            path: "$['a']".to_owned(),
            value: Value::from(1),
        },
    ];
    let other = vec![
        Node {
            path: "$['a']".to_owned(),
            value: Value::from(1),
        },
        Node {
            path: "$['b']".to_owned(),
            value: Value::from(2),
        },
    ];

    assert_eq!(collapse(one), collapse(other));
}

// ---------------------------------------------------------------------------
// Arbitrary-precision document numbers
// ---------------------------------------------------------------------------

/// Core selection must hand back the document's number untouched. No filter
/// comparison appears here: numeric comparison inside filters is vendor tier
/// and explicitly outside Outlint's guarantee.
#[test]
fn selected_numbers_retain_their_exact_representation() {
    let corpus = corpus();
    let numbers = document(&corpus, "numbers");

    for (query, spelling) in [
        ("$.huge", "123456789012345678901234567890"),
        ("$.precise", "0.1000000000000000000000000000000000001"),
        ("$.trailing", "1.00"),
        ("$.boundary", "9007199254740991"),
        ("$.negative_boundary", "-9007199254740991"),
        ("$.beyond", "9007199254740993"),
    ] {
        let nodes = node_set(query, numbers);
        assert_eq!(nodes.len(), 1, "`{query}` must select one node");
        let number = nodes[0]
            .value
            .as_number()
            .unwrap_or_else(|| panic!("`{query}` must select a number"));
        assert_eq!(
            number.to_string(),
            spelling,
            "`{query}` must retain its exact decimal spelling"
        );
    }
}

// ---------------------------------------------------------------------------
// Ordinary locator index resource behavior (§4.4)
// ---------------------------------------------------------------------------

/// Decimal spellings far beyond any array length, at several sizes.
const OVERSIZED_DIGIT_COUNTS: [usize; 4] = [20, 100, 1_000, 10_000];

/// An oversized ordinary locator index must cost only what its spelling costs.
///
/// There is deliberately no wall-clock assertion here; an elapsed-time bound
/// would be flaky. Phase 1B owns the end-to-end version of this against the
/// real Outlint locator parser and its over-the-end lookup.
#[test]
fn an_oversized_index_spelling_costs_only_its_own_length() {
    for digits in OVERSIZED_DIGIT_COUNTS {
        let spelling = "9".repeat(digits);
        let value = BigUint::from_str(&spelling).expect("a decimal spelling must parse");

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
            "a {digits}-digit value used {} bytes, above the {byte_bound}-byte spelling bound",
            value.to_bytes_be().len()
        );

        // Round-tripping proves the bound is not measuring a truncated parse.
        assert_eq!(value.to_string(), spelling);
    }
}

// ---------------------------------------------------------------------------
// Outlint-owned path rendering
// ---------------------------------------------------------------------------

/// Regressions for `support::jsonpath_path`, pinned by name.
///
/// §4.6 makes rendering Outlint's own responsibility and says "a JSONPath
/// provider's rendered path is not authoritative". These tests state each
/// escaping rule directly, including the two provider defects that made an
/// Outlint-owned renderer necessary, so the rules survive the Phase 1B move
/// into the production locator wrapper.
mod rendering {
    use super::support::jsonpath_path::{render_json_pointer, render_normalized_path};
    use serde_json::Value;
    use serde_json_path::JsonPath;

    /// Selects exactly one node and returns both renderings of its path.
    fn render_only(query: &str, document: &str) -> (String, String) {
        let document: Value = serde_json::from_str(document).expect("valid JSON");
        let path = JsonPath::parse(query).expect("valid query");
        let located = path.query_located(&document);
        assert_eq!(located.len(), 1, "`{query}` must select exactly one node");
        let location = located.iter().next().expect("one node").location();
        (
            render_normalized_path(location),
            render_json_pointer(location),
        )
    }

    fn normalized(query: &str, document: &str) -> String {
        render_only(query, document).0
    }

    fn pointer(query: &str, document: &str) -> String {
        render_only(query, document).1
    }

    /// Builds a one-member document and the query selecting it, for a member
    /// name consisting of the single character `code_point`.
    fn single_character_member(code_point: u32) -> (String, String) {
        let name = char::from_u32(code_point)
            .expect("a valid scalar value")
            .to_string();
        let mut object = serde_json::Map::new();
        object.insert(name.clone(), Value::from(1));
        let document =
            serde_json::to_string(&Value::Object(object)).expect("the document serializes");
        let query = format!(
            "$[{}]",
            serde_json::to_string(&name).expect("a JSON string")
        );
        (query, document)
    }

    // --- normalized paths, RFC 9535 section 2.7 ----------------------------

    #[test]
    fn the_root_renders_as_a_bare_dollar() {
        assert_eq!(normalized("$", "1"), "$");
    }

    #[test]
    fn indices_render_without_quotes_and_are_non_negative() {
        assert_eq!(normalized("$[0]", r#"["a", "b", "c"]"#), "$[0]");
        // A negative index normalizes to its non-negative position.
        assert_eq!(normalized("$[-1]", r#"["a", "b", "c"]"#), "$[2]");
    }

    #[test]
    fn ordinary_names_render_in_single_quotes() {
        assert_eq!(normalized("$.foo", r#"{"foo": 1}"#), "$['foo']");
    }

    #[test]
    fn nesting_concatenates_segments_in_order() {
        assert_eq!(
            normalized("$.a[1].b", r#"{"a": [{"b": 0}, {"b": 1}]}"#),
            "$['a'][1]['b']"
        );
    }

    /// Provider defect 1: `NormalizedPath: Display` applies no escaping at all,
    /// so an apostrophe in a member name produced an unparseable spelling.
    #[test]
    fn an_apostrophe_is_backslash_escaped() {
        assert_eq!(normalized(r#"$["it's"]"#, r#"{"it's": 1}"#), r"$['it\'s']");
    }

    /// Provider defect 2: `PathElement: Display` emitted one reverse solidus
    /// for a reverse solidus, where the RFC requires two.
    #[test]
    fn a_reverse_solidus_is_doubled() {
        assert_eq!(
            normalized(r#"$["back\\slash"]"#, r#"{"back\\slash": 1}"#),
            r"$['back\\slash']"
        );
    }

    #[test]
    fn the_five_short_control_escapes_are_used() {
        for (code_point, escaped) in [
            (0x08_u32, r"\b"),
            (0x09, r"\t"),
            (0x0A, r"\n"),
            (0x0C, r"\f"),
            (0x0D, r"\r"),
        ] {
            let (query, document) = single_character_member(code_point);
            assert_eq!(
                normalized(&query, &document),
                format!("$['{escaped}']"),
                "U+{code_point:04X} must use its short escape"
            );
        }
    }

    #[test]
    fn other_c0_controls_use_four_digit_lowercase_hex() {
        for (code_point, escaped) in [
            (0x00_u32, r"\u0000"),
            (0x01, r"\u0001"),
            (0x07, r"\u0007"),
            (0x0B, r"\u000b"),
            (0x0E, r"\u000e"),
            (0x1F, r"\u001f"),
        ] {
            let (query, document) = single_character_member(code_point);
            assert_eq!(
                normalized(&query, &document),
                format!("$['{escaped}']"),
                "U+{code_point:04X} must use the four-digit lowercase form"
            );
        }
    }

    #[test]
    fn double_quotes_and_non_ascii_are_left_literal() {
        // A normalized path is single-quoted, so a double quote needs no escape.
        assert_eq!(
            normalized(r#"$["dq\"uote"]"#, r#"{"dq\"uote": 1}"#),
            "$['dq\"uote']"
        );
        let (query, document) = single_character_member(0x4E2D);
        assert_eq!(normalized(&query, &document), "$['\u{4e2d}']");
        // Beyond the basic multilingual plane, so a surrogate pair in JSON.
        let (query, document) = single_character_member(0x1F600);
        assert_eq!(normalized(&query, &document), "$['\u{1f600}']");
    }

    /// Every rendered path must itself be a valid query selecting exactly the
    /// node it names. This is the property the escaping exists to preserve.
    #[test]
    fn a_rendered_path_round_trips_as_a_query() {
        let document: Value = serde_json::from_str(
            r#"{"it's": 1, "back\\slash": 2, "tab\there": 3, "中": 4, "a": [5, {"b": 6}]}"#,
        )
        .expect("valid JSON");

        // Chained child wildcards, not a descendant segment: the descendant
        // segment is vendor tier and has no place in the core corpus.
        let mut locations = Vec::new();
        for query in ["$[*]", "$[*][*]", "$[*][*][*]"] {
            let parsed = JsonPath::parse(query).expect("valid query");
            for node in parsed.query_located(&document).iter() {
                locations.push((render_normalized_path(node.location()), node.node().clone()));
            }
        }
        assert!(
            locations.len() >= 7,
            "the document must exercise every escaping rule"
        );

        for (rendered, value) in &locations {
            let reparsed = JsonPath::parse(rendered)
                .unwrap_or_else(|error| panic!("`{rendered}` must parse: {error}"));
            let refetched = reparsed.query_located(&document);
            assert_eq!(
                refetched.len(),
                1,
                "`{rendered}` must select exactly one node"
            );
            assert_eq!(
                refetched.iter().next().expect("one node").node(),
                value,
                "`{rendered}` must select the node it names"
            );
        }
    }

    // --- JSON Pointers, RFC 6901 -------------------------------------------

    #[test]
    fn the_root_becomes_the_empty_pointer() {
        assert_eq!(pointer("$", "1"), "");
    }

    #[test]
    fn array_indices_become_slash_prefixed_numbers() {
        assert_eq!(pointer("$[0]", r#"["a", "b", "c"]"#), "/0");
        assert_eq!(pointer("$[1]", r#"["a", "b", "c"]"#), "/1");
        // A negative index resolves to its non-negative position first.
        assert_eq!(pointer("$[-1]", r#"["a", "b", "c"]"#), "/2");
    }

    #[test]
    fn ordinary_names_and_nesting_become_reference_tokens() {
        assert_eq!(pointer("$.foo", r#"{"foo": 1}"#), "/foo");
        assert_eq!(
            pointer("$.a[1].b", r#"{"a": [{"b": 0}, {"b": 1}]}"#),
            "/a/1/b"
        );
    }

    #[test]
    fn a_tilde_becomes_tilde_zero() {
        assert_eq!(pointer(r#"$["a~b"]"#, r#"{"a~b": 1}"#), "/a~0b");
    }

    #[test]
    fn a_solidus_becomes_tilde_one() {
        assert_eq!(pointer(r#"$["a/b"]"#, r#"{"a/b": 1}"#), "/a~1b");
    }

    #[test]
    fn a_tilde_followed_by_a_solidus_becomes_tilde_zero_tilde_one() {
        assert_eq!(pointer(r#"$["~/"]"#, r#"{"~/": 1}"#), "/~0~1");
        // The escaping must not be re-applied to its own output: `~1` in the
        // source name is two literal characters and becomes `~01`.
        assert_eq!(pointer(r#"$["~1"]"#, r#"{"~1": 1}"#), "/~01");
    }

    /// RFC 6901 escapes only `~` and `/`. Quotes, backslashes, C0 controls,
    /// and non-ASCII characters are literal token characters; escaping them
    /// for transport belongs to whatever serializes the pointer into JSON.
    #[test]
    fn other_characters_stay_literal_in_a_pointer() {
        assert_eq!(pointer(r#"$["it's"]"#, r#"{"it's": 1}"#), "/it's");
        assert_eq!(
            pointer(r#"$["dq\"uote"]"#, r#"{"dq\"uote": 1}"#),
            "/dq\"uote"
        );
        assert_eq!(
            pointer(r#"$["back\\slash"]"#, r#"{"back\\slash": 1}"#),
            "/back\\slash"
        );

        let (query, document) = single_character_member(0x4E2D);
        assert_eq!(pointer(&query, &document), "/\u{4e2d}");

        let (query, document) = single_character_member(0x09);
        assert_eq!(pointer(&query, &document), "/\u{9}");

        let (query, document) = single_character_member(0x00);
        assert_eq!(pointer(&query, &document), "/\u{0}");
    }

    /// The literal token characters are what JSON serialization must escape;
    /// this pins that the pointer itself carries no JSON escaping of its own.
    #[test]
    fn json_serialization_owns_json_escaping_of_a_pointer() {
        let (query, document) = single_character_member(0x09);
        let rendered = pointer(&query, &document);
        assert_eq!(
            serde_json::to_string(&rendered).expect("a JSON string"),
            r#""/\t""#
        );
    }
}

// ---------------------------------------------------------------------------
// Core-membership contract
// ---------------------------------------------------------------------------

/// Every corpus selector must lie inside the §4.6 guaranteed core.
///
/// This is the contract commit 1C activates: once the independent recognizer
/// in `support::jsonpath_core_recognizer` lands, this test classifies every
/// corpus selector with it and requires `core`. Until then it enforces the
/// weaker, purely lexical property that no vendor-tier construct appears
/// anywhere in the corpus, so a non-core selector cannot be added in the
/// meantime.
#[test]
fn every_corpus_selector_lies_inside_the_guaranteed_core() {
    let corpus = corpus();
    let selectors = corpus
        .selection
        .iter()
        .map(|case| (case.name.as_str(), case.selector.as_str()))
        .chain(
            corpus
                .rejected
                .iter()
                .map(|case| (case.name.as_str(), case.selector.as_str())),
        )
        .chain(
            corpus
                .propositions
                .iter()
                .map(|case| (case.name.as_str(), case.selector.as_str())),
        );

    for (name, selector) in selectors {
        assert!(
            selector.starts_with('$'),
            "`{name}` must be a complete query rooted at `$`"
        );
        // Descendant segments, filters, and function calls are vendor tier.
        for vendor in ["..", "?", "("] {
            assert!(
                !selector.contains(vendor),
                "`{name}` uses the vendor-tier construct `{vendor}`: {selector}"
            );
        }
        // A slice or a union needs a `:` or a `,` inside a bracket segment.
        // Neither can occur in a core selector outside a quoted name, and no
        // corpus name contains one.
        for vendor in [':', ','] {
            assert!(
                !selector.contains(vendor),
                "`{name}` may use a slice or union: {selector}"
            );
        }
    }
}
