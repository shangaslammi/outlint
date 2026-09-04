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
