//! The official compliance suite, filtered to the Outlint core: secondary
//! evidence only.
//!
//! The primary release gate is `jsonpath_core.rs`, Outlint's own authored
//! corpus. This file adds independent corroboration from the official RFC 9535
//! compliance suite, but only over the cases that fall inside the §4.6
//! guaranteed core. §4.6 is explicit that "vendor-tier query outcomes are not
//! an Outlint conformance or release gate", so the other cases are classified
//! and then left alone — the provider is never even evaluated for them.
//!
//! Membership is decided by `support::jsonpath_core_recognizer`, which reads
//! query text only and never consults the provider, a case name, or a tag. The
//! decision for every case is recorded in the generated `core-manifest.json`,
//! which this file recomputes and compares exactly.
//!
//! Nothing here claims RFC nodelist-order conformance: §4.6 makes the order at
//! Outlint's boundary unobservable, so results are compared as unordered node
//! sets keyed by path identity.

mod support;

use std::collections::BTreeMap;

use serde_json::Value;
use serde_json_path::JsonPath;

use support::jsonpath_core_manifest::{
    build_manifest, build_manifest_with_exclusions, read_suite, to_canonical_json, Case,
    Expectation, Manifest, ReviewedExclusion,
};
use support::jsonpath_core_recognizer::{classify, Classification};
use support::jsonpath_path::{render_json_pointer, render_normalized_path};

const CTS: &str = include_str!("fixtures/jsonpath/cts.json");
const CHECKED_IN_MANIFEST: &str = include_str!("fixtures/jsonpath/core-manifest.json");

// ---------------------------------------------------------------------------
// Manifest agreement
// ---------------------------------------------------------------------------

#[test]
fn the_generated_manifest_matches_the_checked_in_file() {
    let regenerated = build_manifest(CTS);
    let checked_in: Manifest = serde_json::from_str(CHECKED_IN_MANIFEST)
        .expect("the checked-in manifest must deserialize");

    let mut differences = Vec::new();

    if regenerated.suite != checked_in.suite {
        differences.push(format!(
            "suite metadata changed: {:?} became {:?}",
            checked_in.suite, regenerated.suite
        ));
    }
    if regenerated.summary != checked_in.summary {
        differences.push(format!(
            "summary counts changed: {:?} became {:?}",
            checked_in.summary, regenerated.summary
        ));
    }

    let before: BTreeMap<usize, (&str, &str)> = checked_in
        .included
        .iter()
        .map(|case| (case.ordinal, (case.name.as_str(), case.selector.as_str())))
        .collect();
    let after: BTreeMap<usize, (&str, &str)> = regenerated
        .included
        .iter()
        .map(|case| (case.ordinal, (case.name.as_str(), case.selector.as_str())))
        .collect();

    for (ordinal, (name, selector)) in &after {
        match before.get(ordinal) {
            None => differences.push(format!(
                "newly recognized as core: #{ordinal} `{name}` ({selector})"
            )),
            Some((was_name, was_selector)) => {
                if was_name != name {
                    differences.push(format!(
                        "#{ordinal} was named `{was_name}`, is now `{name}`"
                    ));
                }
                if was_selector != selector {
                    differences.push(format!(
                        "#{ordinal} `{name}` had selector `{was_selector}`, now `{selector}`"
                    ));
                }
            }
        }
    }
    for (ordinal, (name, selector)) in &before {
        if !after.contains_key(ordinal) {
            differences.push(format!(
                "no longer recognized as core: #{ordinal} `{name}` ({selector})"
            ));
        }
    }

    assert!(
        differences.is_empty(),
        "the checked-in core manifest is stale. Regenerate it with\n  \
         cargo run -q -p outlint-core --example generate_jsonpath_core_manifest --locked \\\n    \
         > crates/outlint-core/tests/fixtures/jsonpath/core-manifest.json\n\
         and review every change; see tests/fixtures/jsonpath/UPDATING.md.\n\n{}",
        differences.join("\n")
    );

    // Byte-for-byte, so the checked-in file is exactly what the generator emits.
    assert_eq!(
        to_canonical_json(&regenerated),
        CHECKED_IN_MANIFEST,
        "the checked-in manifest differs from the generator's canonical output"
    );
}

#[test]
fn every_recognized_case_is_accounted_for_exactly_once() {
    // Deliberately the checked-in file, not a freshly generated one: a stale
    // or duplicated exclusion committed by hand must fail here even before
    // anyone regenerates the manifest.
    let manifest: Manifest = serde_json::from_str(CHECKED_IN_MANIFEST)
        .expect("the checked-in manifest must deserialize");
    let suite = read_suite(CTS);

    let recognized: Vec<&Case> = suite
        .iter()
        .filter(|case| classify(&case.selector).is_core() && !case.is_invalid_selector())
        .collect();

    assert_eq!(
        manifest.included.len() + manifest.exclusions.len(),
        recognized.len(),
        "every recognized core case must appear once in `included` or in a \
         reviewed exclusion"
    );

    let mut ordinals: Vec<usize> = manifest.included.iter().map(|case| case.ordinal).collect();
    let total = ordinals.len();
    ordinals.sort_unstable();
    ordinals.dedup();
    assert_eq!(
        ordinals.len(),
        total,
        "`included` must not repeat an ordinal"
    );

    let mut names: Vec<&str> = manifest
        .exclusions
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    let total = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), total, "exclusions must not repeat a name");

    // No case may be both evaluated and excluded.
    for exclusion in &manifest.exclusions {
        assert!(
            !manifest
                .included
                .iter()
                .any(|case| case.name == exclusion.name),
            "`{}` appears in both `included` and `exclusions`",
            exclusion.name
        );
    }

    for exclusion in &manifest.exclusions {
        assert!(
            recognized.iter().any(|case| case.name == exclusion.name),
            "exclusion `{}` is stale: no case of that name is currently \
             recognized as core. Either the suite moved or the recognizer \
             changed; review it rather than deleting it silently.",
            exclusion.name
        );
        assert!(
            !exclusion.reason.trim().is_empty(),
            "exclusion `{}` needs a written reason",
            exclusion.name
        );
    }

    // The pinned suite has no reviewed exclusion, and a failing core case is
    // an escalation rather than grounds to add one.
    assert!(
        manifest.exclusions.is_empty(),
        "exclusions must be empty for this pin; see UPDATING.md"
    );
}

/// The exclusion mechanism must actually work, not merely be documented.
///
/// This drives a synthetic exclusion through the real builder and checks that
/// the case moves out of `included`, into `exclusions` with its reason, and
/// that every count follows. It never touches the checked-in manifest, which
/// stays at zero exclusions.
#[test]
fn a_reviewed_exclusion_moves_a_case_out_of_the_evaluated_set() {
    // A real recognized case, named as a literal so the exclusion set is
    // shaped exactly like a genuine reviewed entry.
    const VICTIM: &str = "basic, name shorthand";

    let baseline = build_manifest(CTS);
    assert!(
        baseline.exclusions.is_empty(),
        "the checked-in configuration must have no exclusions"
    );
    assert!(
        baseline.included.iter().any(|case| case.name == VICTIM),
        "`{VICTIM}` must be a recognized core case for this test to mean anything"
    );

    let excluded = build_manifest_with_exclusions(
        CTS,
        &[ReviewedExclusion {
            name: VICTIM,
            reason: "synthetic exclusion exercising the mechanism",
        }],
    );

    assert!(
        !excluded.included.iter().any(|case| case.name == VICTIM),
        "an excluded case must leave `included`"
    );
    assert_eq!(excluded.exclusions.len(), 1);
    assert_eq!(excluded.exclusions[0].name, VICTIM);
    assert_eq!(
        excluded.exclusions[0].reason,
        "synthetic exclusion exercising the mechanism"
    );

    assert_eq!(excluded.summary.excluded, 1);
    assert_eq!(excluded.summary.included, baseline.summary.included - 1);
    // Exclusion is not reclassification: the case is still recognized as core,
    // so the non-core count must not move.
    assert_eq!(excluded.summary.non_core, baseline.summary.non_core);
    assert_eq!(excluded.summary.examined, baseline.summary.examined);
    assert_eq!(
        excluded.summary.deterministic + excluded.summary.nondeterministic,
        excluded.summary.included,
        "the deterministic split must cover exactly the evaluated cases"
    );

    // Accounting still balances across every bucket.
    assert_eq!(
        excluded.summary.included
            + excluded.summary.excluded
            + excluded.summary.invalid_recognized_as_core
            + excluded.summary.non_core,
        excluded.summary.examined
    );
}

// ---------------------------------------------------------------------------
// Secondary semantic gate
// ---------------------------------------------------------------------------

/// One result node at Outlint's boundary: value plus rendered path identity.
type NodeSet = BTreeMap<String, Value>;

/// Collapses duplicate path identities into a set, per §4.6.
fn collapse(pairs: Vec<(String, Value)>) -> NodeSet {
    let mut set = NodeSet::new();
    for (path, value) in pairs {
        set.entry(path).or_insert(value);
    }
    set
}

#[test]
fn every_core_compliance_case_passes() {
    let manifest = build_manifest(CTS);
    let suite = read_suite(CTS);
    let by_ordinal: BTreeMap<usize, &Case> =
        suite.iter().map(|case| (case.ordinal, case)).collect();

    let mut failures = Vec::new();
    let mut evaluated = 0usize;

    for entry in &manifest.included {
        let case = by_ordinal
            .get(&entry.ordinal)
            .unwrap_or_else(|| panic!("manifest ordinal {} is not in the suite", entry.ordinal));
        assert_eq!(
            case.selector, entry.selector,
            "manifest ordinal {} does not match the suite",
            entry.ordinal
        );

        evaluated += 1;
        if let Some(failure) = run_case(case) {
            failures.push(failure);
        }
    }

    assert_eq!(
        evaluated, manifest.summary.included,
        "every manifest-included case must be evaluated"
    );
    assert!(
        failures.is_empty(),
        "{} of {evaluated} core compliance cases failed. A failing core case is \
         an escalation, not grounds for an exclusion; see UPDATING.md.{}",
        failures.len(),
        failures.join("")
    );
}

fn run_case(case: &Case) -> Option<String> {
    let (document, alternatives) = match &case.expectation {
        // The manifest never includes an invalid-selector case; the
        // recognizer would have to have accepted a query the RFC rejects.
        Expectation::InvalidSelector => {
            return Some(format!(
                "\ncase: {} (#{})\n  an invalid-selector case reached the semantic gate",
                case.name, case.ordinal
            ))
        }
        Expectation::Deterministic { document, only } => (document, std::slice::from_ref(only)),
        Expectation::Nondeterministic {
            document,
            alternatives,
        } => (document, alternatives.as_slice()),
    };

    let parsed = match JsonPath::parse(&case.selector) {
        Ok(parsed) => parsed,
        Err(error) => {
            return Some(format!(
                "\ncase: {} (#{})\n  selector: {}\n  expected a successful parse, got: {error}",
                case.name, case.ordinal, case.selector
            ))
        }
    };

    let located = parsed.query_located(document);
    let actual = collapse(
        located
            .iter()
            .map(|node| (render_normalized_path(node.location()), node.node().clone()))
            .collect(),
    );

    // Accept exactly one complete alternative: values from one alternative are
    // never paired with paths from another.
    let matched = alternatives.iter().any(|alternative| {
        let expected = collapse(
            alternative
                .paths
                .iter()
                .cloned()
                .zip(alternative.values.iter().cloned())
                .collect(),
        );
        expected == actual
    });

    if matched {
        return None;
    }

    let expectations: Vec<String> = alternatives
        .iter()
        .map(|alternative| {
            format!(
                "{:?} paired with {:?}",
                alternative.paths, alternative.values
            )
        })
        .collect();
    Some(format!(
        "\ncase: {} (#{})\n  selector: {}\n  expected one of: {}\n  actual: {:?}",
        case.name,
        case.ordinal,
        case.selector,
        expectations.join(" | "),
        actual
    ))
}

/// Outlint's JSON Pointer rendering must address the same node the query did.
///
/// This checks the §6.1 `pointer` derivation against `serde_json`'s own RFC
/// 6901 resolver across every core compliance case, so the escaping is
/// validated by something other than the renderer that produced it.
#[test]
fn rendered_pointers_address_the_nodes_they_name() {
    let manifest = build_manifest(CTS);
    let suite = read_suite(CTS);
    let by_ordinal: BTreeMap<usize, &Case> =
        suite.iter().map(|case| (case.ordinal, case)).collect();

    let mut checked = 0usize;
    for entry in &manifest.included {
        let case = by_ordinal[&entry.ordinal];
        let document = match &case.expectation {
            Expectation::Deterministic { document, .. }
            | Expectation::Nondeterministic { document, .. } => document,
            Expectation::InvalidSelector => continue,
        };
        let parsed = JsonPath::parse(&case.selector).expect("an included case must parse");
        for node in parsed.query_located(document).iter() {
            let pointer = render_json_pointer(node.location());
            let addressed = document.pointer(&pointer).unwrap_or_else(|| {
                panic!(
                    "pointer `{pointer}` from case `{}` addresses nothing",
                    case.name
                )
            });
            assert_eq!(
                addressed,
                node.node(),
                "pointer `{pointer}` from case `{}` addresses the wrong node",
                case.name
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "the core cases must produce some nodes");
}

// ---------------------------------------------------------------------------
// Recognizer unit tests
// ---------------------------------------------------------------------------

mod recognizer {
    use super::support::jsonpath_core_recognizer::{classify, Classification};

    #[track_caller]
    fn assert_core(query: &str) {
        assert_eq!(
            classify(query),
            Classification::Core,
            "`{query}` should be core, but was rejected: {:?}",
            classify(query).reason()
        );
    }

    #[track_caller]
    fn assert_non_core(query: &str) {
        assert!(
            !classify(query).is_core(),
            "`{query}` should be classified non-core"
        );
    }

    // --- positive: every core production ----------------------------------

    #[test]
    fn the_bare_root_is_core() {
        assert_core("$");
    }

    #[test]
    fn dot_name_segments_are_core() {
        assert_core("$.a");
        assert_core("$.status");
        assert_core("$._underscore");
        assert_core("$.a1b2");
        assert_core("$.a.b.c");
    }

    #[test]
    fn unicode_shorthand_names_are_core() {
        assert_core("$.\u{e9}");
        assert_core("$.\u{4e2d}\u{6587}");
        assert_core("$.\u{263a}");
        // Beyond the BMP.
        assert_core("$.\u{1f600}");
    }

    #[test]
    fn dot_wildcards_are_core() {
        assert_core("$.*");
        assert_core("$.a.*");
        assert_core("$.*.*");
    }

    #[test]
    fn bracket_wildcards_are_core() {
        assert_core("$[*]");
        assert_core("$[*][*]");
    }

    #[test]
    fn quoted_name_selectors_are_core() {
        assert_core("$['a']");
        assert_core("$[\"a\"]");
        assert_core("$['decision-makers']");
        assert_core("$['a.b']");
        assert_core("$['with space']");
        assert_core("$[']']");
        assert_core("$['=']");
        // The opposite quote is literal inside each form.
        assert_core("$['say \"hi\"']");
        assert_core("$[\"it's\"]");
    }

    #[test]
    fn legal_escapes_in_quoted_names_are_core() {
        assert_core(r"$['it\'s']");
        assert_core(r#"$["say \"hi\""]"#);
        assert_core(r"$['back\\slash']");
        assert_core(r"$['\b\f\n\r\t']");
        assert_core(r"$['\/']");
        // `\uXXXX`, in both hexadecimal cases.
        assert_core(r"$['\u0041']");
        assert_core(r"$['\u00e9']");
        assert_core(r"$['\u00E9']");
        // A C0 control is legal when written as an escape.
        assert_core(r"$['\u0000']");
        assert_core(r"$['a\u001fb']");
        // A valid surrogate pair, in both hexadecimal cases.
        assert_core(r"$['\uD83D\uDE00']");
        assert_core(r"$['\ud83d\ude00']");
    }

    #[test]
    fn index_selectors_are_core() {
        assert_core("$[0]");
        assert_core("$[1]");
        assert_core("$[42]");
        assert_core("$[-1]");
        assert_core("$[9007199254740991]");
        assert_core("$[-9007199254740991]");
    }

    #[test]
    fn admitted_whitespace_is_core() {
        assert_core("$ .a");
        assert_core("$ ['a']");
        assert_core("$[ 'a' ]");
        assert_core("$[ 0 ]");
        assert_core("$[ * ]");
        assert_core("$\t.a");
        assert_core("$\n.a");
        assert_core("$\r.a");
        assert_core("$  .a  ['b']");
    }

    #[test]
    fn arbitrary_chaining_of_core_segments_is_core() {
        assert_core("$.a[0].b[*]['c']");
        assert_core("$[*][0]['x'].y");
    }

    // --- negative: index spelling and bounds -------------------------------

    #[test]
    fn a_leading_zero_index_is_not_core() {
        assert_non_core("$[01]");
        assert_non_core("$[00]");
        assert_non_core("$[0123]");
        assert_non_core("$[-01]");
    }

    #[test]
    fn negative_zero_is_not_core() {
        assert_non_core("$[-0]");
    }

    #[test]
    fn an_index_outside_the_i_json_range_is_not_core() {
        assert_non_core("$[9007199254740992]");
        assert_non_core("$[-9007199254740992]");
        assert_non_core("$[9999999999999999]");
        // Long spellings are rejected on length, with no value-sized work.
        assert_non_core(&format!("$[{}]", "9".repeat(100)));
        assert_non_core(&format!("$[-{}]", "9".repeat(10_000)));
    }

    // --- negative: every vendor-tier construct ------------------------------
    //
    // These assert classification only. What the provider does with them is
    // vendor-tier behavior and deliberately untested here.

    #[test]
    fn slices_are_not_core() {
        assert_non_core("$[0:2]");
        assert_non_core("$[:]");
        assert_non_core("$[::2]");
        assert_non_core("$[1:5:2]");
    }

    #[test]
    fn unions_are_not_core() {
        assert_non_core("$[0,1]");
        assert_non_core("$['a','b']");
        assert_non_core("$[0, 1]");
    }

    #[test]
    fn descendant_segments_are_not_core() {
        assert_non_core("$..a");
        assert_non_core("$..*");
        assert_non_core("$..[0]");
    }

    #[test]
    fn filters_are_not_core() {
        assert_non_core("$[?@.a]");
        assert_non_core("$[?@.a == 1]");
        assert_non_core("$[?(@.a)]");
    }

    #[test]
    fn function_expressions_are_not_core() {
        assert_non_core("$[?length(@.a) > 1]");
        assert_non_core("$[?count(@.*) == 1]");
        assert_non_core("$[?match(@.a, 'x')]");
        assert_non_core("$[?search(@.a, 'x')]");
        assert_non_core("$[?value(@.a) == 1]");
    }

    #[test]
    fn malformed_and_vendor_syntax_is_not_core() {
        assert_non_core("");
        assert_non_core("a.b");
        assert_non_core("@.a");
        assert_non_core("$.");
        assert_non_core("$[");
        assert_non_core("$[]");
        assert_non_core("$['a");
        assert_non_core("$.a]");
        assert_non_core("$..");
        // Vendor extensions that no RFC production admits.
        assert_non_core("$.a~");
        assert_non_core("$.^a");
    }

    #[test]
    fn illegal_escapes_and_unescaped_characters_are_not_core() {
        assert_non_core(r"$['\x41']");
        assert_non_core(r"$['\q']");
        assert_non_core(r"$['\u00']");
        assert_non_core(r"$['\uZZZZ']");
        // An unpaired surrogate.
        assert_non_core(r"$['\uD83D']");
        assert_non_core(r"$['\uDE00']");
        assert_non_core(r"$['\uD83DA']");
        // A raw C0 control must be escaped.
        assert_non_core("$['\u{1}']");
        assert_non_core("$['\t']");
        // The delimiter of the other form is not escapable.
        assert_non_core(r#"$['\"']"#);
        assert_non_core(r#"$["\'"]"#);
    }

    #[test]
    fn trailing_whitespace_is_not_core() {
        assert_non_core("$ ");
        assert_non_core("$.a ");
        assert_non_core("$.a\t");
        assert_non_core("$['a'] ");
    }

    #[test]
    fn more_than_one_selector_per_bracket_is_not_core() {
        assert_non_core("$[0 1]");
        assert_non_core("$['a' 'b']");
        assert_non_core("$[* *]");
    }
}

// ---------------------------------------------------------------------------
// The classifier is not a rejection gate
// ---------------------------------------------------------------------------

/// §4.6: a query outside the core "MUST NOT be rejected merely for falling
/// outside the guaranteed core; it is submitted in full to the implementation's
/// JSONPath provider."
///
/// This is advisory and deliberately does not evaluate the slice or assert any
/// result: doing so would make a vendor-tier outcome into a gate, which is
/// exactly what §4.6 forbids.
#[test]
fn a_non_core_query_is_still_accepted_by_the_provider() {
    let query = "$[0:2]";

    assert!(
        matches!(classify(query), Classification::NonCore(_)),
        "`{query}` is a slice and must classify as non-core"
    );
    assert!(
        JsonPath::parse(query).is_ok(),
        "`{query}` must still be accepted by the provider: classification \
         records the absence of an Outlint guarantee, it does not reject"
    );
}
