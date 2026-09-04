//! Typed Values against the version 3 envelope, end to end through the CLI.
//!
//! The unit fixtures beside the renderer pin each rendered shape in isolation;
//! these pin the shapes the loader and validator actually produce, so a
//! rendering that is correct for a hand-built value and unreachable from any
//! real schema still fails here.

mod common;

use common::*;
use serde_json::{json, Value};

/// A `[i]` subscript far beyond `u64::MAX`: 2^128. §4.4 gives a subscript "no
/// upper bound" and §11.3 tells consumers not to assume one fits 64 bits, so a
/// value this size has to survive from the locator text to the emitted number
/// without passing through a machine integer.
const ABOVE_U64: &str = "340282366920938463463374607431768211456";

fn diagnostic(output: &std::process::Output) -> Value {
    json_output(output)["results"][0]["diagnostics"][0].clone()
}

/// Every §11.3 reference kind, matcher form, and equality scalar type, taken
/// from one unsatisfied constraint over a loader-resolved schema.
///
/// The whole `references` array is compared at once rather than kind by kind:
/// an extra member, a missing optional one, or a member emitted as null where
/// §11.3 says to omit it would all survive a per-member probe.
///
/// The frontmatter capture's `path` is deliberately not its name, so the
/// emitted `fm.version` locator can only be the spelling the author wrote and
/// cannot have been rebuilt from the bound query.
#[test]
fn every_reference_kind_renders_its_exact_version_3_shape() {
    let directory = TempDir::new("typed-references");
    directory.write(
        "schema.yml",
        concat!(
            "version: 1\n",
            "title: null\n",
            "frontmatter:\n",
            "  captures:\n",
            "    version:\n",
            "      path: \"$['release-version']\"\n",
            "      type: semver\n",
            "sections:\n",
            "  - id: exact\n",
            "    match: Release\n",
            "    sections:\n",
            "      - id: notes\n",
            "        match: Notes\n",
            "  - id: glob\n",
            "    match: \"Step *\"\n",
            "  - id: regex\n",
            "    match: \"/Release (?<version>.+)/\"\n",
            "    captures:\n",
            "      version: semver\n",
            "  - id: any\n",
            "    match: \"*\"\n",
            "constraints:\n",
            "  - any_of:\n",
            "      - exact\n",
            "      - \"$.exact[340282366920938463463374607431768211456].notes\"\n",
            "      - glob\n",
            "      - regex\n",
            "      - any\n",
            "      - \"fm[$.draft]\"\n",
            "      - \"fm[$.count]=0x10\"\n",
            "      - \"fm[$.ratio]=15e-1\"\n",
            "      - \"fm[$.name]=release\"\n",
            "      - \"fm[$.flag]=true\"\n",
            "      - \"fm[$.void]=null\"\n",
            "      - \"fm.version\"\n",
        ),
    );
    directory.write("doc.md", "plain text\n");

    let output = run(
        &directory,
        &[
            "check",
            "doc.md",
            "--schema",
            "schema.yml",
            "--format",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let diagnostic = diagnostic(&output);
    assert_eq!(diagnostic["id"], "any_of");
    assert_eq!(
        diagnostic["references"],
        json!([
            {
                "kind": "rule",
                "locator": "exact",
                "anchor": "current_scope",
                "path": ["exact"],
                "matcher": {"kind": "exact", "value": "Release"}
            },
            {
                "kind": "rule",
                "locator": format!("$.exact[{ABOVE_U64}].notes"),
                "anchor": "schema_root",
                "path": ["exact", "notes"],
                "positions": [Value::Number(ABOVE_U64.parse().expect("a JSON number")), null],
                "matcher": {"kind": "exact", "value": "Notes"}
            },
            {
                "kind": "rule",
                "locator": "glob",
                "anchor": "current_scope",
                "path": ["glob"],
                "matcher": {"kind": "glob", "value": "Step *"}
            },
            {
                "kind": "rule",
                "locator": "regex",
                "anchor": "current_scope",
                "path": ["regex"],
                "matcher": {"kind": "regex", "value": "Release (?<version>.+)"}
            },
            {
                "kind": "rule",
                "locator": "any",
                "anchor": "current_scope",
                "path": ["any"],
                "matcher": {"kind": "any"}
            },
            {"kind": "frontmatter_query", "locator": "fm[$.draft]", "query": "$.draft"},
            {
                "kind": "frontmatter_query",
                "locator": "fm[$.count]=0x10",
                "query": "$.count",
                "equals": {"type": "integer", "value": "16"}
            },
            {
                "kind": "frontmatter_query",
                "locator": "fm[$.ratio]=15e-1",
                "query": "$.ratio",
                "equals": {"type": "float", "value": "15e-1"}
            },
            {
                "kind": "frontmatter_query",
                "locator": "fm[$.name]=release",
                "query": "$.name",
                "equals": {"type": "string", "value": "release"}
            },
            {
                "kind": "frontmatter_query",
                "locator": "fm[$.flag]=true",
                "query": "$.flag",
                "equals": {"type": "boolean", "value": true}
            },
            {
                "kind": "frontmatter_query",
                "locator": "fm[$.void]=null",
                "query": "$.void",
                "equals": {"type": "null", "value": null}
            },
            {
                "kind": "frontmatter_capture",
                "locator": "fm.version",
                "name": "version",
                "type": "semver"
            }
        ])
    );
}

/// The subscript reaches the wire as an unquoted JSON integer with every digit
/// intact, having been read from the locator text and never narrowed.
///
/// The raw stdout is inspected rather than only the parsed value, because a
/// value rounded through `f64` would print `3.402823669209385e38` and a value
/// quoted as a string would still compare equal to a fixture built the same
/// wrong way.
#[test]
fn positional_narrowing_survives_as_an_arbitrary_precision_json_integer() {
    let directory = TempDir::new("typed-positions");
    directory.write(
        "schema.yml",
        concat!(
            "version: 1\n",
            "title: null\n",
            "sections:\n",
            "  - id: release\n",
            "    match: Release\n",
            "    required: false\n",
            "    sections:\n",
            "      - id: notes\n",
            "        match: Notes\n",
            "constraints:\n",
            "  - one_of:\n",
            "      - \"$.release[340282366920938463463374607431768211456].notes\"\n",
            "      - \"$.release.notes[7]\"\n",
            "      - \"$.release.notes\"\n",
        ),
    );
    directory.write("doc.md", "plain text\n");

    let output = run(
        &directory,
        &[
            "check",
            "doc.md",
            "--schema",
            "schema.yml",
            "--format",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));

    assert!(
        stdout(&output).contains(&format!("\"positions\":[{ABOVE_U64},null]")),
        "the subscript must be emitted unquoted and undiminished: {}",
        stdout(&output)
    );

    let references = diagnostic(&output)["references"].clone();
    // §11.3: `positions` is aligned with `path` whenever any step carries a
    // subscript — one entry per step, null for the unsubscripted ones — and is
    // omitted entirely when no step does.
    assert_eq!(
        references[0]["positions"],
        json!([
            Value::Number(ABOVE_U64.parse().expect("a JSON number")),
            null
        ])
    );
    assert_eq!(references[1]["positions"], json!([null, 7]));
    assert!(
        references[2]
            .as_object()
            .expect("a reference is a JSON object")
            .get("positions")
            .is_none(),
        "an unsubscripted path carries no `positions` member: {}",
        references[2]
    );
}

/// The two Typed Values schema errors of §6.3, through `schema check`.
///
/// They are load-time failures about the schema document, so §6 gives them a
/// positioned `schema_location` and no `target` at all — and they arrive in
/// the same version 3 envelope as everything else. `invalid-capture` and
/// `invalid-order` are declared in separate schemas because §6.3 forbids
/// reporting an order error for entries referring to a capture mapping that
/// did not build.
#[test]
fn typed_value_schema_errors_are_positioned_and_carry_no_document_target() {
    let directory = TempDir::new("typed-schema-errors");
    // A capture needs a regex matcher: only a regex declares named groups.
    directory.write(
        "capture.yml",
        concat!(
            "version: 1\n",
            "title: null\n",
            "sections:\n",
            "  - match: Release\n",
            "    captures:\n",
            "      version: semver\n",
        ),
    );
    // The capture mapping here is well formed, so the order entry is reached;
    // `dir` admits only `asc` and `desc`.
    directory.write(
        "order.yml",
        concat!(
            "version: 1\n",
            "title: null\n",
            "sections:\n",
            "  - match: \"/Release (?<version>.+)/\"\n",
            "    captures:\n",
            "      version: semver\n",
            "    order:\n",
            "      - by: version\n",
            "        dir: sideways\n",
        ),
    );

    for (path, id, line, column) in [
        ("capture.yml", "invalid-capture", 6, 7),
        ("order.yml", "invalid-order", 8, 9),
    ] {
        let output = run(&directory, &["schema", "check", path, "--format", "json"]);
        assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
        assert_eq!(stderr(&output), "");

        let json = json_output(&output);
        assert_eq!(json["version"], 3);
        assert_eq!(json["results"][0]["kind"], "schema");
        assert_eq!(json["results"][0]["path"], path);
        assert_eq!(json["results"][0]["schema"], path);

        let diagnostic = &json["results"][0]["diagnostics"][0];
        assert_eq!(diagnostic["id"], id);
        assert_eq!(
            diagnostic["schema_location"],
            json!({"path": path, "line": line, "column": column})
        );
        assert_eq!(
            diagnostic["location"],
            json!({"line": line, "column": column})
        );
        assert!(
            diagnostic["message"]
                .as_str()
                .is_some_and(|m| !m.is_empty()),
            "§11.3 requires explanatory prose: {diagnostic}"
        );
        assert!(
            diagnostic
                .as_object()
                .expect("a diagnostic is a JSON object")
                .get("target")
                .is_none(),
            "a schema error is about the schema file and omits `target`: {diagnostic}"
        );
    }
}

// ---------------------------------------------------------------------------
// The three Typed Values runtime diagnostics, end to end through real
// validator output. Each group pins the §6 attribution in JSON and the facts
// §11.3 requires human output to make intelligible.
// ---------------------------------------------------------------------------

/// §6.2: an `invalid-value` from a rule capture targets the "`header` whose
/// capture is invalid", anchors on that header's line, and is "attributed to
/// that capture declaration" — which §11.3 renders as a `capture` node
/// carrying its owning rule's `scope` and `index` beside the capture `name`.
///
/// The rule is nested so those coordinates are a real position in the schema
/// rather than the empty scope every top-level rule would produce.
#[test]
fn a_rule_capture_invalid_value_is_attributed_to_its_capture_declaration() {
    let directory = TempDir::new("typed-rule-capture");
    directory.write(
        "schema.yml",
        concat!(
            "version: 1\n",
            "title: null\n",
            "sections:\n",
            "  - id: product\n",
            "    match: Product\n",
            "    sections:\n",
            "      - id: other\n",
            "        match: Other\n",
            "      - id: release\n",
            "        match: \"/Release (?<version>.+)/\"\n",
            "        captures:\n",
            "          version: semver\n",
        ),
    );
    directory.write("doc.md", "## Product\n\n### Release not-a-semver\n");

    let output = run(
        &directory,
        &[
            "check",
            "doc.md",
            "--schema",
            "schema.yml",
            "--format",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let diagnostic = diagnostic(&output);

    assert_eq!(diagnostic["id"], "invalid-value");
    assert_eq!(
        diagnostic["target"],
        json!({"kind": "header", "path": ["Product", "Release not-a-semver"]})
    );
    assert_eq!(diagnostic["location"], json!({"line": 3, "column": 1}));
    // The owning rule is the second entry of the scope opened by the first
    // top-level rule, so the coordinates are `[0]` and `1`, not the empty
    // scope a flattened attribution would report.
    assert_eq!(
        diagnostic["schema_node"],
        json!({"kind": "capture", "scope": [0], "index": 1, "name": "version"})
    );
    assert_eq!(diagnostic["schema_location"]["path"], "schema.yml");
    // §6.2 requires the message to "identify the expected type and the
    // responsible capture"; §11.3 forbids consumers keying on its wording, so
    // this asserts only that both facts are in it.
    let message = diagnostic["message"].as_str().expect("prose");
    assert!(
        message.contains("version") && message.contains("semver"),
        "{message}"
    );

    let human = run(
        &directory,
        &[
            "check",
            "doc.md",
            "--schema",
            "schema.yml",
            "--color",
            "never",
        ],
    );
    let human = stdout(&human);
    // The capture name and expected type both reach the reader, and the
    // schema position is labelled for what it is rather than repeating the
    // word `capture` for a second, unrelated line.
    assert!(human.contains("  capture: \"version\"\n"), "{human}");
    assert!(human.contains("semver"), "{human}");
    assert!(human.contains("  declared: schema.yml:"), "{human}");
    assert!(
        human.contains("  section: \"Product > Release not-a-semver\"\n"),
        "{human}"
    );
}

/// §6.2: an `invalid-value` from a frontmatter capture targets `frontmatter`
/// "with the failing value's pointer" and anchors on the failing entry, and is
/// attributed to that frontmatter capture declaration.
///
/// The capture's `path` is not its name, so the reported pointer can only
/// have come from the query's resolved path components (§4.6) and not from
/// the declaration's spelling.
#[test]
fn a_frontmatter_capture_invalid_value_points_at_the_failing_entry() {
    let directory = TempDir::new("typed-frontmatter-capture");
    directory.write(
        "schema.yml",
        concat!(
            "version: 1\n",
            "title: null\n",
            "frontmatter:\n",
            "  captures:\n",
            "    version:\n",
            "      path: \"$['release-version']\"\n",
            "      type: semver\n",
            "sections: []\n",
        ),
    );
    // An unquoted `1.2` is a YAML float, the mistake §2.4 says diagnostics
    // should suggest quoting.
    directory.write("doc.md", "---\nheader: x\nrelease-version: 1.2\n---\n");

    let output = run(
        &directory,
        &[
            "check",
            "doc.md",
            "--schema",
            "schema.yml",
            "--format",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let diagnostic = diagnostic(&output);

    assert_eq!(diagnostic["id"], "invalid-value");
    assert_eq!(
        diagnostic["target"],
        json!({
            "kind": "frontmatter",
            "line_range": {"start_line": 1, "end_line": 4},
            "pointer": "/release-version"
        })
    );
    assert_eq!(diagnostic["location"], json!({"line": 3, "column": 1}));
    assert_eq!(
        diagnostic["schema_node"],
        json!({"kind": "frontmatter_capture", "name": "version"})
    );
    let message = diagnostic["message"].as_str().expect("prose");
    assert!(
        message.contains("version") && message.contains("semver"),
        "{message}"
    );

    let human = run(
        &directory,
        &[
            "check",
            "doc.md",
            "--schema",
            "schema.yml",
            "--color",
            "never",
        ],
    );
    let human = stdout(&human);
    // §4.6 spells a frontmatter capture `fm.<name>`, which is how an author
    // would refer to it, so that is how the reader is shown it.
    assert!(human.contains("  capture: \"fm.version\"\n"), "{human}");
    assert!(human.contains("  value: \"/release-version\"\n"), "{human}");
    assert!(human.contains("semver"), "{human}");
}

/// §6.1: `missing-value.pointer` "names the intended absent path" rather than
/// the entry it anchors to, and §6.2 anchors it at "the deepest resolving
/// positioned ancestor of the addressed path; block's first line as floor".
///
/// The two are deliberately different here: `/meta/stamp` does not exist,
/// `/meta` does, so the pointer names the absent path while the anchor sits
/// on the line of the ancestor that resolves.
#[test]
fn a_required_frontmatter_capture_missing_value_names_the_absent_path() {
    let directory = TempDir::new("typed-missing-value");
    directory.write(
        "schema.yml",
        concat!(
            "version: 1\n",
            "title: null\n",
            "frontmatter:\n",
            "  captures:\n",
            "    stamp:\n",
            "      path: \"$['meta']['stamp']\"\n",
            "      type: date\n",
            "      required: true\n",
            "sections: []\n",
        ),
    );
    directory.write("doc.md", "---\nheader: x\nmeta:\n  other: y\n---\n");

    let output = run(
        &directory,
        &[
            "check",
            "doc.md",
            "--schema",
            "schema.yml",
            "--format",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let diagnostic = diagnostic(&output);

    assert_eq!(diagnostic["id"], "missing-value");
    assert_eq!(
        diagnostic["target"],
        json!({
            "kind": "frontmatter",
            "line_range": {"start_line": 1, "end_line": 5},
            "pointer": "/meta/stamp"
        })
    );
    // Line 3 is `meta:`, the deepest ancestor that resolves — not line 1, the
    // floor, and not any line of the absent entry, which has none.
    assert_eq!(diagnostic["location"], json!({"line": 3, "column": 1}));
    assert_eq!(
        diagnostic["schema_node"],
        json!({"kind": "frontmatter_capture", "name": "stamp"})
    );

    let human = run(
        &directory,
        &[
            "check",
            "doc.md",
            "--schema",
            "schema.yml",
            "--color",
            "never",
        ],
    );
    let human = stdout(&human);
    assert!(human.contains("  capture: \"fm.stamp\"\n"), "{human}");
    assert!(human.contains("  value: \"/meta/stamp\"\n"), "{human}");
}

/// §4.6: "a bare `fm[...]` is a typed boolean read, not a presence test.
/// [...] Every non-boolean, non-null result node produces `invalid-value`".
/// §6.2 targets `frontmatter` with that value's pointer and attributes the
/// diagnostic to "the constraint containing the query" — not to any capture,
/// because a query is not a declaration.
#[test]
fn a_boolean_query_invalid_value_is_attributed_to_its_containing_constraint() {
    let directory = TempDir::new("typed-boolean-query");
    directory.write(
        "schema.yml",
        concat!(
            "version: 1\n",
            "title: null\n",
            "sections:\n",
            "  - id: a\n",
            "    match: A\n",
            "constraints:\n",
            "  - one_of: [\"fm[$.flags.draft]\", a]\n",
        ),
    );
    // `"yes"` is a YAML string, not a boolean: a presence test would pass and
    // a boolean read must not.
    directory.write("doc.md", "---\nflags:\n  draft: \"yes\"\n---\n\n## A\n");

    let output = run(
        &directory,
        &[
            "check",
            "doc.md",
            "--schema",
            "schema.yml",
            "--format",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let diagnostic = diagnostic(&output);

    assert_eq!(diagnostic["id"], "invalid-value");
    assert_eq!(
        diagnostic["target"],
        json!({
            "kind": "frontmatter",
            "line_range": {"start_line": 1, "end_line": 4},
            "pointer": "/flags/draft"
        })
    );
    // The anchor is the failing entry itself, at the first byte of its key.
    assert_eq!(diagnostic["location"], json!({"line": 3, "column": 3}));
    assert_eq!(
        diagnostic["schema_node"],
        json!({"kind": "constraint", "scope": [], "index": 0})
    );
    // §11.3 makes `references` present "when the corresponding semantic data
    // exists"; the validator attributes this diagnostic to the constraint and
    // supplies no reference, and the renderer will not invent one. The
    // responsible query reaches consumers through the message, which §6.2
    // requires to identify it.
    assert!(
        diagnostic
            .as_object()
            .expect("a diagnostic is a JSON object")
            .get("references")
            .is_none(),
        "no reference is supplied for this diagnostic: {diagnostic}"
    );
    let message = diagnostic["message"].as_str().expect("prose");
    assert!(
        message.contains("$.flags.draft") && message.contains("bool"),
        "{message}"
    );

    let human = run(
        &directory,
        &[
            "check",
            "doc.md",
            "--schema",
            "schema.yml",
            "--color",
            "never",
        ],
    );
    let human = stdout(&human);
    // The query is identified by the preserved core message rather than by
    // the renderer reconstructing it, and the failing value is pointed at.
    assert!(human.contains("$.flags.draft"), "{human}");
    assert!(human.contains("  value: \"/flags/draft\"\n"), "{human}");
    assert!(human.contains("  constraint: schema.yml:"), "{human}");
}

/// §6.2: an `order-violation` targets the "`header` of the violating adjacent
/// pair's second header", anchors on that second header's line, is
/// "attributed to its order entry", and "lists exactly the first and second
/// headers of its violating adjacent pair, in that order".
#[test]
fn a_value_order_violation_names_its_order_entry_and_adjacent_pair() {
    let directory = TempDir::new("typed-order-violation");
    directory.write(
        "schema.yml",
        concat!(
            "version: 1\n",
            "title: null\n",
            "sections:\n",
            "  - id: release\n",
            "    match: \"/Release (?<version>.+)/\"\n",
            "    captures:\n",
            "      version: semver\n",
            "    order:\n",
            "      - by: version\n",
        ),
    );
    directory.write(
        "doc.md",
        "## Release 1.0.0\n\n## Release 3.0.0\n\n## Release 2.0.0\n",
    );

    let output = run(
        &directory,
        &[
            "check",
            "doc.md",
            "--schema",
            "schema.yml",
            "--format",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let diagnostic = diagnostic(&output);

    assert_eq!(diagnostic["id"], "order-violation");
    // The pair that broke is 3.0.0 then 2.0.0; the target and anchor are its
    // second header, the one out of place.
    assert_eq!(
        diagnostic["target"],
        json!({"kind": "header", "path": ["Release 2.0.0"]})
    );
    assert_eq!(diagnostic["location"], json!({"line": 5, "column": 1}));
    assert_eq!(
        diagnostic["schema_node"],
        json!({"kind": "order_entry", "scope": [], "index": 0, "order_index": 0})
    );
    // Exactly two, first then second — not every header the rule matched.
    assert_eq!(
        diagnostic["involved_headers"],
        json!([
            {"header_path": ["Release 3.0.0"], "location": {"line": 3, "column": 1}},
            {"header_path": ["Release 2.0.0"], "location": {"line": 5, "column": 1}}
        ])
    );
    let message = diagnostic["message"].as_str().expect("prose");
    assert!(
        message.contains("3.0.0") && message.contains("2.0.0"),
        "{message}"
    );

    let human = run(
        &directory,
        &[
            "check",
            "doc.md",
            "--schema",
            "schema.yml",
            "--color",
            "never",
        ],
    );
    let human = stdout(&human);
    // The pair is numbered rather than listed as the unordered `involved
    // sections:` a constraint gets, because which of the two is second is the
    // fact that says which one to move.
    assert!(
        human.contains(concat!(
            "  out-of-order pair:\n",
            "    1. doc.md:3:1 \"Release 3.0.0\"\n",
            "    2. doc.md:5:1 \"Release 2.0.0\"\n"
        )),
        "{human}"
    );
    assert!(human.contains("  order entry: 0\n"), "{human}");
    assert!(human.contains("  declared: schema.yml:"), "{human}");
    // The `ordered` constraint's own presentation is untouched by this
    // branch: it still lists an expected order, which `order-violation` has
    // no equivalent of.
    assert!(!human.contains("expected order"), "{human}");
    assert!(!human.contains("involved sections"), "{human}");
}

/// §11.3 requires human output to escape control characters "originating in
/// input paths, documents, schemas, or delegated validator messages", and the
/// typed-value diagnostics opened new routes for such text to reach the
/// terminal: a frontmatter pointer built from a schema-controlled query, and
/// the document-controlled header paths of an out-of-order pair.
#[test]
fn typed_value_presentation_escapes_untrusted_pointers_and_header_paths() {
    let directory = TempDir::new("typed-escaping");
    // The pointer is derived from this member name, so U+202E travels from
    // the schema's query through the resolved path into the human line.
    directory.write(
        "pointer.yml",
        concat!(
            "version: 1\n",
            "title: null\n",
            "frontmatter:\n",
            "  captures:\n",
            "    stamp:\n",
            "      path: \"$['dr\u{202e}aft']\"\n",
            "      type: date\n",
            "sections: []\n",
        ),
    );
    directory.write("pointer.md", "---\n\"dr\\u202eaft\": 12\n---\n");
    // The header text is document-controlled and reaches the pair listing.
    directory.write(
        "pair.yml",
        concat!(
            "version: 1\n",
            "title: null\n",
            "sections:\n",
            "  - id: product\n",
            "    match: \"Product*\"\n",
            "    sections:\n",
            "      - id: release\n",
            "        match: \"/Release (?<version>.+)/\"\n",
            "        captures:\n",
            "          version: semver\n",
            "        order:\n",
            "          - by: version\n",
        ),
    );
    directory.write(
        "pair.md",
        "## Product\u{202e}X\n\n### Release 3.0.0\n\n### Release 2.0.0\n",
    );

    for (document, schema, escaped) in [
        (
            "pointer.md",
            "pointer.yml",
            "  value: \"/dr\\u{202e}aft\"\n",
        ),
        (
            "pair.md",
            "pair.yml",
            "1. pair.md:3:1 \"Product\\u{202e}X > Release 3.0.0\"\n",
        ),
    ] {
        let output = run(
            &directory,
            &["check", document, "--schema", schema, "--color", "never"],
        );
        assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
        let human = stdout(&output);
        assert!(human.contains(escaped), "{human}");
        assert!(!output.stdout.contains(&0x1b));
        assert!(!human.contains('\u{202e}'), "{human}");
    }
}
