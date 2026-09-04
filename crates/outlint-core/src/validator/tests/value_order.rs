//! §3.8 ordering of a rule's repeated matches by captured value.

use crate::validator::{Diagnostic, DiagnosticId, DiagnosticTarget, HeaderPath};
use crate::{OrderEntryPath, OrderIndex, RuleIndex, RulePath, SchemaNode, ScopePath};

use super::diagnostics;

/// A one-rule sugar schema whose rule captures `v` and orders by it.
fn ordered_schema(declared: &str, entries: &str) -> String {
    format!(
        "version: 1\nsections:\n  - match: \"/V (?<v>.+)/\"\n    repeat: 0..n\n    \
         captures:\n      v: {declared}\n    order:\n{entries}"
    )
}

/// One `order` entry, spelled at the indentation a rule's list needs.
fn entry(by: &str, direction: &str, strict: bool) -> String {
    format!("      - by: {by}\n        dir: {direction}\n        strict: {strict}\n")
}

/// A titled document whose `V` headers carry `values` in order.
fn document(values: &[&str]) -> String {
    let mut markdown = String::from("# T\n");
    for value in values {
        markdown.push_str(&format!("## V {value}\n"));
    }
    markdown
}

/// The same document with `ids` disabled file-wide.
///
/// §6.3 decides dependency suppression "before these comments filter
/// diagnostics", so every row of the suppression matrix is asserted twice:
/// once as written, and once with its upstream primary hidden.
fn file_disabled(ids: &str, markdown: &str) -> String {
    format!("<!-- outlint-disable-file {ids} -->\n\n{markdown}")
}

fn ids(reported: &[Diagnostic]) -> Vec<DiagnosticId> {
    reported.iter().map(|diagnostic| diagnostic.id).collect()
}

fn order_violations(schema: &str, markdown: &str) -> Vec<Diagnostic> {
    diagnostics(schema, markdown)
        .into_iter()
        .filter(|diagnostic| diagnostic.id == DiagnosticId::OrderViolation)
        .collect()
}

/// The schema node one `order` entry of the single top-level rule addresses.
fn order_node(order_index: usize) -> SchemaNode {
    SchemaNode::OrderEntry(OrderEntryPath {
        rule: RulePath {
            scope: ScopePath(Vec::new()),
            index: RuleIndex(0),
        },
        order_index: OrderIndex(order_index),
    })
}

#[test]
fn every_type_orders_in_both_directions_and_both_strictnesses() {
    // §2.4 gives each type its relation and §3.8 applies that relation to
    // adjacent pairs: ascending requires `A ≤ B`, descending `A ≥ B`, and
    // `strict: true` replaces the inclusive relation with `<` or `>`.
    for (declared, sorted) in [
        ("int", ["-01", "1", "2"]),
        ("bool", ["false", "false", "true"]),
        ("date", ["0000-02-29", "2024-02-29", "2024-03-01"]),
        ("semver", ["1.0.0-rc.1", "1.0.0", "1.0.1"]),
        ("dotted", ["1.02", "1.2.0", "2"]),
        ("text", ["a", "b", "c"]),
    ] {
        let descending = sorted.iter().rev().copied().collect::<Vec<_>>();
        for (direction, ordered, reversed) in [
            ("asc", sorted.to_vec(), descending.clone()),
            ("desc", descending, sorted.to_vec()),
        ] {
            let schema = ordered_schema(declared, &entry("v", direction, false));
            assert_eq!(
                order_violations(&schema, &document(&ordered)),
                [],
                "{declared} {direction} {ordered:?}"
            );
            // Reversing a sorted sequence of three breaks both adjacent
            // pairs, except where the sequence repeats a value: `false`
            // twice is a legal non-strict neighbour either way round.
            let broken = order_violations(&schema, &document(&reversed));
            let expected = if declared == "bool" { 1 } else { 2 };
            assert_eq!(broken.len(), expected, "{declared} {direction} reversed");
        }
    }
}

#[test]
fn strictness_rejects_typed_equality_however_it_is_spelled() {
    // §3.8: "strict ordering also requires uniqueness under typed equality:
    // for `dotted`, adjacent spellings `1.02` and `1.2` violate a strict
    // entry." The non-strict entry accepts the same pair.
    let strict = ordered_schema("dotted", &entry("v", "asc", true));
    let loose = ordered_schema("dotted", &entry("v", "asc", false));
    let equal = document(&["1.02", "1.2"]);
    assert_eq!(order_violations(&strict, &equal).len(), 1);
    assert_eq!(order_violations(&loose, &equal), []);
    // `int` equality survives its own redundant spelling the same way.
    let strict_int = ordered_schema("int", &entry("v", "desc", true));
    assert_eq!(
        order_violations(&strict_int, &document(&["-01", "-1"])).len(),
        1
    );
}

#[test]
fn a_violation_is_targeted_and_anchored_at_the_pairs_second_header() {
    // §6.2: `order-violation` targets and anchors "the violating adjacent
    // pair's second header" and "lists exactly the first and second headers
    // of its violating adjacent pair, in that order". §6.2 also attributes it
    // to its order entry.
    let schema = ordered_schema("semver", &entry("v", "desc", false));
    let reported = order_violations(&schema, &document(&["1.0.0", "2.0.0"]));
    assert_eq!(reported.len(), 1);
    let diagnostic = &reported[0];
    assert_eq!(
        diagnostic.target,
        DiagnosticTarget::Header(HeaderPath(vec!["T".into(), "V 2.0.0".into()]))
    );
    assert_eq!(diagnostic.location.line, 3);
    assert_eq!(diagnostic.schema_node, Some(order_node(0)));
    assert!(diagnostic.references.is_empty());
    assert_eq!(
        diagnostic
            .involved_headers
            .iter()
            .map(|header| header.path.clone())
            .collect::<Vec<_>>(),
        [
            HeaderPath(vec!["T".into(), "V 1.0.0".into()]),
            HeaderPath(vec!["T".into(), "V 2.0.0".into()]),
        ]
    );
    // §3.8: the message "MUST identify both parsed values", and the parsed
    // values are what is shown — not the characters that produced them.
    assert!(
        diagnostic.message.contains("`1.0.0`")
            && diagnostic.message.contains("`2.0.0`")
            && diagnostic.message.contains("descending"),
        "{}",
        diagnostic.message
    );
    let canonical = ordered_schema("dotted", &entry("v", "desc", false));
    let reported = order_violations(&canonical, &document(&["1.02", "1.3"]));
    assert!(
        reported[0].message.contains("`1.2`") && reported[0].message.contains("`1.3`"),
        "the message shows the parsed values: {}",
        reported[0].message
    );
    // Inline suppression is available at the second header, which is where
    // the diagnostic is anchored.
    let suppressed = "# T\n## V 1.0.0\n<!-- outlint-disable order-violation -->\n## V 2.0.0\n";
    assert_eq!(order_violations(&schema, suppressed), []);
}

#[test]
fn one_misplaced_value_can_break_two_adjacent_pairs() {
    // §3.8: "Each violating adjacent pair produces one `order-violation` [...]
    // One misplaced value can therefore produce two diagnostics." Here `5`
    // is the one out of place, and it is out of place against both
    // neighbours, so both pairs around it are reported and both name it.
    let schema = ordered_schema("int", &entry("v", "asc", false));
    let reported = order_violations(&schema, &document(&["1", "9", "5", "2"]));
    assert_eq!(
        reported
            .iter()
            .map(|diagnostic| diagnostic.target.clone())
            .collect::<Vec<_>>(),
        [
            DiagnosticTarget::Header(HeaderPath(vec!["T".into(), "V 5".into()])),
            DiagnosticTarget::Header(HeaderPath(vec!["T".into(), "V 2".into()])),
        ]
    );
    let misplaced = HeaderPath(vec!["T".into(), "V 5".into()]);
    for diagnostic in &reported {
        assert!(
            diagnostic
                .involved_headers
                .iter()
                .any(|header| header.path == misplaced),
            "both pairs are the ones `5` belongs to: {:?}",
            diagnostic.involved_headers
        );
    }
}

#[test]
fn an_invalid_value_suppresses_its_whole_entry_and_scope() {
    // §3.8's own example: "under a SemVer capture ordered descending, the
    // sequence `2.0.0`, `not-a-version`, `1.0.0` produces `invalid-value` for
    // the middle header and no `order-violation` for that entry and scope. It
    // does not compare the first and third values as if they were adjacent."
    let schema = ordered_schema("semver", &entry("v", "desc", false));
    let markdown = document(&["2.0.0", "not-a-version", "1.0.0"]);
    assert_eq!(
        ids(&diagnostics(&schema, &markdown)),
        [DiagnosticId::InvalidValue]
    );
    // The comparison that suppression prevents would have succeeded here, so
    // the same sequence with the middle value *out* of order shows that the
    // first and third are genuinely not compared.
    let reversed = document(&["1.0.0", "not-a-version", "2.0.0"]);
    assert_eq!(
        ids(&diagnostics(&schema, &reversed)),
        [DiagnosticId::InvalidValue]
    );

    // Filtering variant: §6.3 forbids hiding `invalid-value` from
    // re-enabling the dependent ordering.
    assert_eq!(
        ids(&diagnostics(
            &schema,
            &file_disabled("invalid-value", &reversed)
        )),
        []
    );
    // And per header, at the invalid value's own anchor.
    let inline = "# T\n## V 1.0.0\n<!-- outlint-disable invalid-value -->\n\
                  ## V not-a-version\n## V 2.0.0\n";
    assert_eq!(ids(&diagnostics(&schema, inline)), []);
}

#[test]
fn too_many_sections_never_suppresses_value_ordering() {
    // §3.8: "Headers beyond the rule's cardinality maximum remain in it, so
    // `too-many-sections` does not suppress value ordering." §4.4 says the
    // same from the other side: value ordering "does not depend on the
    // cardinality bound holding".
    let schema = format!(
        "version: 1\nsections:\n  - match: \"/V (?<v>.+)/\"\n    repeat: 0..2\n    \
         captures:\n      v: semver\n    order:\n{}",
        entry("v", "desc", false)
    );
    // In order despite the excess: the bound is broken, the order is not.
    assert_eq!(
        ids(&diagnostics(
            &schema,
            &document(&["3.0.0", "2.0.0", "1.0.0"])
        )),
        [DiagnosticId::TooManySections]
    );
    // Out of order at the excess occurrence: it is still in the sequence.
    let broken = document(&["3.0.0", "1.0.0", "2.0.0"]);
    assert_eq!(
        ids(&diagnostics(&schema, &broken)),
        [DiagnosticId::TooManySections, DiagnosticId::OrderViolation]
    );
    assert_eq!(
        order_violations(&schema, &broken)[0].target,
        DiagnosticTarget::Header(HeaderPath(vec!["T".into(), "V 2.0.0".into()]))
    );

    // Filtering variant: hiding the cardinality primary changes nothing,
    // because ordering never depended on it.
    assert_eq!(
        ids(&diagnostics(
            &schema,
            &file_disabled("too-many-sections", &broken)
        )),
        [DiagnosticId::OrderViolation]
    );
}

#[test]
fn each_entry_is_evaluated_independently_of_the_others() {
    // §3.8: "Each `order` entry on a rule independently orders the
    // occurrences matched by that rule" — the list is not one compound sort
    // key. A compound key would settle this document on `a` alone and never
    // consult `b`; independent entries report `b`.
    let schema = format!(
        "version: 1\nsections:\n  - match: \"/V (?<a>[^ ]+) (?<b>[^ ]+)/\"\n    \
         repeat: 0..n\n    captures:\n      a: int\n      b: int\n    order:\n{}{}",
        entry("a", "asc", false),
        entry("b", "asc", false)
    );
    let markdown = "# T\n## V 1 2\n## V 2 1\n";
    let reported = order_violations(&schema, markdown);
    assert_eq!(reported.len(), 1);
    assert_eq!(reported[0].schema_node, Some(order_node(1)));
    assert!(
        reported[0].message.contains("`b`"),
        "{}",
        reported[0].message
    );
}

#[test]
fn an_invalid_capture_suppresses_only_the_entries_that_read_it() {
    // §3.8: "Other order entries, scopes, and primary `invalid-value`
    // diagnostics are unaffected."
    let schema = format!(
        "version: 1\nsections:\n  - match: \"/V (?<a>[^ ]+) (?<b>[^ ]+)/\"\n    \
         repeat: 0..n\n    captures:\n      a: semver\n      b: int\n    order:\n{}{}",
        entry("a", "asc", false),
        entry("b", "asc", false)
    );
    let markdown = "# T\n## V nope 2\n## V 1.0.0 1\n";
    let reported = diagnostics(&schema, markdown);
    assert_eq!(
        ids(&reported),
        [DiagnosticId::InvalidValue, DiagnosticId::OrderViolation]
    );
    // The surviving violation belongs to the entry reading the valid capture.
    assert_eq!(reported[1].schema_node, Some(order_node(1)));

    // Filtering variant: the entry reading `a` stays suppressed and the
    // entry reading `b` stays evaluated.
    assert_eq!(
        ids(&diagnostics(
            &schema,
            &file_disabled("invalid-value", markdown)
        )),
        [DiagnosticId::OrderViolation]
    );
}

#[test]
fn each_concrete_ancestor_instance_orders_its_own_sequence() {
    // §3.8: "When an ancestor repeats, each concrete ancestor instance
    // supplies a separate sequence; occurrences are never flattened across
    // instances." So an invalid value in one instance suppresses that
    // instance alone.
    let schema = "version: 1\noutline:\n  - match: Part\n    repeat: 0..n\n    \
                  sections:\n      - match: \"/V (?<v>.+)/\"\n        repeat: 0..n\n        \
                  captures:\n          v: semver\n        order:\n          - by: v\n            \
                  dir: desc\n";
    let markdown = "# Part\n## V 2.0.0\n## V nope\n## V 1.0.0\n\
                    # Part\n## V 1.0.0\n## V 2.0.0\n";
    let reported = diagnostics(schema, markdown);
    assert_eq!(
        ids(&reported),
        [DiagnosticId::InvalidValue, DiagnosticId::OrderViolation]
    );
    // The violation is the second instance's, and the two instances' headers
    // never met: the first instance's `2.0.0` did not become a neighbour of
    // the second instance's `1.0.0`.
    assert_eq!(
        reported[1].target,
        DiagnosticTarget::Header(HeaderPath(vec!["Part".into(), "V 2.0.0".into()]))
    );
    assert_eq!(reported[1].involved_headers.len(), 2);
    assert_eq!(reported[1].involved_headers[0].location.line, 6);
    assert_eq!(reported[1].involved_headers[1].location.line, 7);

    // Filtering variant: the first instance stays suppressed.
    assert_eq!(
        ids(&diagnostics(
            schema,
            &file_disabled("invalid-value", markdown)
        )),
        [DiagnosticId::OrderViolation]
    );
}

#[test]
fn headers_outside_the_sequence_do_not_break_its_adjacency() {
    // §3.8: "Headers matched by other rules and unmatched headers do not
    // break adjacency; they do not belong to the sequence."
    let schema = "version: 1\noptions:\n  ordered_sections: false\n\
                  sections:\n  - match: \"/V (?<v>.+)/\"\n    repeat: 0..n\n    \
                  captures:\n      v: semver\n    order:\n      - by: v\n        dir: desc\n  \
                  - match: Note\n    repeat: 0..n\n";
    // `Note` is another rule's; `Stray` matches nothing in this open scope.
    // Both sit between the two `V` headers, which are still adjacent.
    let interleaved = "# T\n## V 1.0.0\n## Note\n## Stray\n## V 2.0.0\n";
    let reported = order_violations(schema, interleaved);
    assert_eq!(reported.len(), 1);
    assert_eq!(
        reported[0]
            .involved_headers
            .iter()
            .map(|header| header.location.line)
            .collect::<Vec<_>>(),
        [2, 5]
    );
}

#[test]
fn denied_and_unvisited_headings_contribute_nothing_to_a_sequence() {
    // §3.8: "Headers matched by deny rules and every header in a skipped or
    // otherwise unvisited subtree contribute nothing."
    let denied = "version: 1\noptions:\n  ordered_sections: false\n\
                  sections:\n  - match: Skip\n    allow: false\n  \
                  - match: \"/V (?<v>.+)/\"\n    repeat: 0..n\n    \
                  captures:\n      v: semver\n    order:\n      - by: v\n        dir: desc\n";
    // The denied header sits between two `V`s that are themselves in order:
    // were it in the sequence it could not be compared at all.
    assert_eq!(
        ids(&diagnostics(
            denied,
            "# T\n## V 2.0.0\n## Skip\n## V 1.0.0\n"
        )),
        [DiagnosticId::NotAllowed]
    );

    // An unmatched header's subtree is never visited, so the `V` rule below
    // it binds nothing and its sequence stays empty.
    let nested = "version: 1\noutline:\n  - match: Part\n    repeat: 0..n\n    \
                  sections:\n      - match: \"/V (?<v>.+)/\"\n        repeat: 0..n\n        \
                  captures:\n          v: semver\n        order:\n          - by: v\n            \
                  dir: desc\n";
    assert_eq!(
        ids(&diagnostics(nested, "# Other\n## V 1.0.0\n## V 2.0.0\n")),
        []
    );
}
