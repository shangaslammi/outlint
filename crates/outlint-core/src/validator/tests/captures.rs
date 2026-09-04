//! Rule capture extraction and the `invalid-value` diagnostics it produces
//! (§2.4, §3.3, §6.2).

use crate::validator::{DiagnosticId, DiagnosticTarget, HeaderPath};
use crate::{
    CaptureName, CapturePath, MarkdownOptions, RuleIndex, RulePath, SchemaNode, ScopePath,
};

use super::{diagnostics, diagnostics_with};

/// A one-rule sugar schema whose only rule declares `captures`.
///
/// Every capture test needs the same shape — a repeatable regex rule under the
/// title — and differs only in the pattern and the declared types.
fn capture_schema(pattern: &str, declarations: &str) -> String {
    format!(
        "version: 1\nsections:\n  - match: \"/{pattern}/\"\n    repeat: 0..n\n    \
         captures:\n{declarations}"
    )
}

/// The schema node one capture of the single top-level rule is addressed by.
fn capture_node(name: &str) -> SchemaNode {
    SchemaNode::Capture(CapturePath {
        rule: RulePath {
            scope: ScopePath(Vec::new()),
            index: RuleIndex(0),
        },
        name: CaptureName(name.to_owned()),
    })
}

#[test]
fn a_case_insensitive_match_captures_the_document_spelling() {
    // §2.4: a rule capture's source is "the case-preserving substring of the
    // §1.3 matcher input selected by the named group [...] before any case
    // folding used to decide the match", and §1.3 confines folding to
    // matching. So a `bool` capture reached through a case-insensitive match
    // still sees `TRUE`, which the header grammar of §2.4 refuses — the
    // folded text would have been the valid `true`.
    let schema = capture_schema("Flag (?<flag>true)", "      flag: bool\n");
    let reported = diagnostics(&schema, "# T\n## Flag TRUE\n");
    assert_eq!(reported.len(), 1);
    assert_eq!(reported[0].id, DiagnosticId::InvalidValue);
    assert!(
        reported[0].message.contains("TRUE"),
        "the message must name the document spelling, not the folded one: {}",
        reported[0].message
    );
    // The lowercase spelling the grammar does admit is valid, which is what
    // makes the case above a capture question rather than a matching one.
    assert!(diagnostics(&schema, "# T\n## Flag true\n").is_empty());
}

#[test]
fn capture_substrings_come_from_the_configured_matcher_input() {
    // §2.4 takes the source string from the §1.3 matcher input "after the
    // configured inline markup handling", so the same document yields
    // different capture text under the two settings.
    let schema = capture_schema("Release (?<version>.+)", "      version: semver\n");
    let markdown = "# T\n## Release **1.0.0**\n";
    assert!(diagnostics_with(&schema, markdown, MarkdownOptions::default()).is_empty());

    let unstripped = diagnostics_with(
        &schema,
        markdown,
        MarkdownOptions {
            strip_inline_markup: false,
        },
    );
    assert_eq!(unstripped.len(), 1);
    assert_eq!(unstripped[0].id, DiagnosticId::InvalidValue);
    assert!(
        unstripped[0].message.contains("**1.0.0**"),
        "the capture keeps the emphasis the matcher input kept: {}",
        unstripped[0].message
    );
    // §6.1 strips markup from diagnostic text under either setting, so the
    // header path reads the same both ways.
    assert_eq!(
        unstripped[0].target,
        DiagnosticTarget::Header(HeaderPath(vec!["T".into(), "Release 1.0.0".into()]))
    );
}

#[test]
fn every_declared_capture_is_parsed_including_one_nothing_reads() {
    // §3.3: on a match "every declared capture is parsed". Nothing reads `d`
    // — only `n` is ordered by — and it is parsed and reported all the same.
    let schema = capture_schema(
        "Item (?<n>[^ ]+) (?<d>[^ ]+)",
        "      n: int\n      d: date\n    order:\n      - by: n\n",
    );
    let reported = diagnostics(&schema, "# T\n## Item x 2023-02-29\n");
    assert_eq!(
        reported
            .iter()
            .map(|diagnostic| (diagnostic.id, diagnostic.schema_node.clone()))
            .collect::<Vec<_>>(),
        [
            (DiagnosticId::InvalidValue, Some(capture_node("d"))),
            (DiagnosticId::InvalidValue, Some(capture_node("n"))),
        ]
    );
}

#[test]
fn each_typed_failure_reports_one_invalid_value_naming_its_reason() {
    // §2.4's five failure kinds for a header source: lexical, bound,
    // calendar, SemVer build metadata, and a bounded component. §6.2 requires
    // each message to "identify the expected type and the responsible
    // capture", and §2.4 requires a build-metadata message to "identify that
    // suffix as the reason".
    for (declared, source, fragment) in [
        ("int", "abc", "abc"),
        ("int", "9223372036854775808", "signed 64-bit"),
        ("date", "2023-02-29", "calendar"),
        ("semver", "1.0.0+build.7", "+build.7"),
        ("dotted", "1.4294967296", "unsigned 32-bit"),
    ] {
        let schema = capture_schema("V (?<v>.+)", &format!("      v: {declared}\n"));
        let reported = diagnostics(&schema, &format!("# T\n## V {source}\n"));
        assert_eq!(reported.len(), 1, "{declared} `{source}`");
        let diagnostic = &reported[0];
        assert_eq!(diagnostic.id, DiagnosticId::InvalidValue);
        assert_eq!(diagnostic.schema_node, Some(capture_node("v")));
        assert_eq!(
            diagnostic.target,
            DiagnosticTarget::Header(HeaderPath(vec!["T".into(), format!("V {source}")]))
        );
        assert_eq!(diagnostic.location.line, 2);
        assert!(
            diagnostic.message.contains("`v`") && diagnostic.message.contains(declared),
            "{declared} `{source}`: {}",
            diagnostic.message
        );
        assert!(
            diagnostic.message.contains(fragment),
            "{declared} `{source}` must name `{fragment}`: {}",
            diagnostic.message
        );
    }
}

#[test]
fn a_valid_capture_of_every_type_reports_nothing() {
    for (declared, source) in [
        ("int", "-01"),
        ("bool", "false"),
        ("date", "2024-02-29"),
        ("semver", "1.0.0-rc.1"),
        ("dotted", "1.02.0"),
        ("text", "*anything*"),
    ] {
        let schema = capture_schema("V (?<v>.+)", &format!("      v: {declared}\n"));
        assert_eq!(
            diagnostics(&schema, &format!("# T\n## V {source}\n")),
            [],
            "{declared} `{source}`"
        );
    }
}

#[test]
fn first_match_ownership_decides_which_rule_extracts_captures() {
    // §3.2: the first rule whose matcher matches is the matched rule, and
    // "later rules are not consulted". Both rules here match `Item 1.0.0`;
    // only the first one's `int` capture is parsed, so exactly one
    // `invalid-value` appears and it belongs to the first rule.
    let schema = "version: 1\nsections:\n  - match: \"/Item (?<n>.+)/\"\n    repeat: 0..n\n    \
                  captures:\n      n: int\n  - match: \"/Item (?<v>.+)/\"\n    repeat: 0..n\n    \
                  captures:\n      v: semver\n";
    let reported = diagnostics(schema, "# T\n## Item 1.0.0\n");
    assert_eq!(reported.len(), 1);
    assert_eq!(reported[0].id, DiagnosticId::InvalidValue);
    assert_eq!(reported[0].schema_node, Some(capture_node("n")));
}

#[test]
fn headings_that_bind_no_rule_contribute_no_capture_diagnostic() {
    // §3.3: an unmatched header's subtree is not recursed into, so a rule
    // below it never sees the heading its capture would have read. §1.5: a
    // heading that skips a level "takes part in no rule", and neither does
    // anything below it.
    let schema = "version: 1\noutline:\n  - match: Part\n    repeat: 0..n\n    \
                  sections:\n      - match: \"/V (?<v>.+)/\"\n        repeat: 0..n\n        \
                  captures:\n          v: semver\n";
    // Bound: one diagnostic. Below an unmatched sibling: none. Below a
    // top-level heading the root never admitted: none either.
    let reported = diagnostics(
        schema,
        "# Part\n## V nope\n# Other\n## V nope\n## Stray\n### V nope\n",
    );
    assert_eq!(reported.len(), 1);
    assert_eq!(reported[0].id, DiagnosticId::InvalidValue);
    assert_eq!(
        reported[0].target,
        DiagnosticTarget::Header(HeaderPath(vec!["Part".into(), "V nope".into()]))
    );

    // §1.5 reads the same inside a bound scope: this `h3` is a child of the
    // `h1` and skips the `h2` level, so its capture is never extracted.
    // §3.3 puts capture parsing among the effects of matching, and a
    // skipping header matches nothing.
    let nested = "version: 1\noutline:\n  - match: Part\n    repeat: 0..n\n    \
                  sections:\n      - match: \"/V (?<v>.+)/\"\n        repeat: 0..n\n        \
                  captures:\n          v: semver\n";
    assert_eq!(
        diagnostics(nested, "# Part\n### V nope\n")
            .iter()
            .map(|diagnostic| diagnostic.id)
            .collect::<Vec<_>>(),
        [DiagnosticId::SkippedLevel]
    );
    // Admitting the skip makes it an ordinary member, capture and all.
    let admitted = format!(
        "version: 1\noptions:\n  allow_skipped_levels: true\n{}",
        nested
            .strip_prefix("version: 1\n")
            .expect("the schema spells the version first")
    );
    assert_eq!(
        diagnostics(&admitted, "# Part\n### V nope\n")
            .iter()
            .map(|diagnostic| diagnostic.id)
            .collect::<Vec<_>>(),
        [DiagnosticId::InvalidValue]
    );

    // A denied heading is rejected wholesale, and a capture cannot be
    // declared on a denying rule at all (§2.1), so the subtree below one
    // contributes nothing either.
    let denied = "version: 1\noutline:\n  - match: Part\n    allow: false\n  - match: \"*\"\n    \
                  repeat: 0..n\n    sections:\n      - match: \"/V (?<v>.+)/\"\n        \
                  repeat: 0..n\n        captures:\n          v: semver\n";
    let reported = diagnostics(denied, "# Part\n## V nope\n");
    assert_eq!(
        reported
            .iter()
            .map(|diagnostic| diagnostic.id)
            .collect::<Vec<_>>(),
        [DiagnosticId::NotAllowed]
    );
}

#[test]
fn suppression_hides_an_invalid_value_without_being_asked_anything_else() {
    // §6.3: an `outlint-disable` on the preceding line hides a diagnostic
    // anchored to that header, and `outlint-disable-file` hides it
    // file-wide. What suppression must *not* do is change what the validator
    // concluded about the value; that half becomes observable where a
    // dependent check reads the stored state.
    let schema = capture_schema("V (?<v>.+)", "      v: semver\n");
    assert_eq!(diagnostics(&schema, "# T\n## V nope\n").len(), 1);
    assert_eq!(
        diagnostics(
            &schema,
            "# T\n<!-- outlint-disable invalid-value -->\n## V nope\n"
        ),
        []
    );
    assert_eq!(
        diagnostics(
            &schema,
            "<!-- outlint-disable-file invalid-value -->\n\n# T\n## V nope\n"
        ),
        []
    );
    // A directive naming another id leaves this one standing.
    assert_eq!(
        diagnostics(
            &schema,
            "# T\n<!-- outlint-disable too-many-sections -->\n## V nope\n"
        )
        .len(),
        1
    );
}
