mod common;

use common::*;
use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn schema_check_reports_schema_diagnostics_as_validation_output() {
    let directory = TempDir::new("schema-invalid");
    directory.write("invalid.yml", "version: 2\nsections: []\n");

    let output = run(
        &directory,
        &["schema", "check", "invalid.yml", "--format", "json"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), "");
    let json = json_output(&output);
    assert_eq!(
        json["results"][0]["diagnostics"][0]["id"],
        "unsupported-version"
    );
    assert_eq!(
        json["results"][0]["diagnostics"][0]["schema_location"]["path"],
        "invalid.yml"
    );
    // A schema-load failure is about the schema, not about anything in a
    // document, so it carries no target at all: the member is omitted rather
    // than reported as `document` or as JSON null.
    assert!(
        json["results"][0]["diagnostics"][0]
            .as_object()
            .expect("a schema diagnostic is a JSON object")
            .get("target")
            .is_none(),
        "a schema-load diagnostic must omit `target`: {}",
        json["results"][0]["diagnostics"][0]
    );
}

#[test]
fn schema_syntax_locations_use_one_based_byte_columns_after_non_ascii() {
    let directory = TempDir::new("schema-non-ascii-syntax-location");
    directory.write("invalid.yml", "version: 1\ntitle: å: bad\nsections: []\n");
    directory.write(
        "invalid-bare-cr.yml",
        "version: 1\rtitle: å: bad\rsections: []\r",
    );

    for path in ["invalid.yml", "invalid-bare-cr.yml"] {
        let output = run(&directory, &["schema", "check", path, "--format", "json"]);

        assert_eq!(output.status.code(), Some(1));
        assert_eq!(stderr(&output), "");
        let diagnostic = &json_output(&output)["results"][0]["diagnostics"][0];
        assert_eq!(diagnostic["id"], "syntax");
        assert_eq!(diagnostic["schema_location"]["path"], path);
        assert_eq!(diagnostic["schema_location"]["line"], 2);
        assert_eq!(diagnostic["schema_location"]["column"], 10);
    }
}

#[test]
fn schema_check_and_check_report_oversized_globs_consistently() {
    let directory = TempDir::new("schema-oversized-glob");
    let oversized_glob = format!(
        "version: 1\nsections:\n  - match: \"{}*\"\n",
        "a".repeat(200_000)
    );
    directory.write("oversized.yml", oversized_glob);
    directory.write("valid.yml", VALID_SCHEMA);
    directory.write("document.md", "## Required\n");

    let check = run(
        &directory,
        &[
            "check",
            "document.md",
            "--schema",
            "oversized.yml",
            "--format",
            "json",
        ],
    );
    assert_eq!(check.status.code(), Some(1));
    assert_eq!(stderr(&check), "");
    let check_json = json_output(&check);
    assert_eq!(
        check_json["results"][0]["diagnostics"][0]["id"],
        "invalid-matcher"
    );
    assert_eq!(
        check_json["results"][0]["diagnostics"][0]["schema_location"]["path"],
        "oversized.yml"
    );
    assert_eq!(
        check_json["results"][0]["diagnostics"][0]["schema_location"]["line"],
        3
    );
    assert_eq!(
        check_json["results"][0]["diagnostics"][0]["schema_location"]["column"],
        12
    );

    let schema_check = run(
        &directory,
        &["schema", "check", "oversized.yml", "--format", "json"],
    );
    assert_eq!(schema_check.status.code(), Some(1));
    assert_eq!(stderr(&schema_check), "");
    assert_eq!(json_output(&schema_check), check_json);

    let valid = run(&directory, &["schema", "check", "valid.yml"]);
    assert_eq!(valid.status.code(), Some(0));
    assert_eq!(stdout(&valid), "");
    assert_eq!(stderr(&valid), "");
}

#[test]
fn stdin_requires_schema_and_validates_when_one_is_given() {
    let directory = TempDir::new("stdin");
    directory.write("schema.yml", VALID_SCHEMA);

    let missing_schema = run(&directory, &["check", "-"]);
    assert_eq!(missing_schema.status.code(), Some(2));
    assert!(stderr(&missing_schema).contains("standard input requires"));

    let mut child = Command::new(env!("CARGO_BIN_EXE_outlint"))
        .args(["check", "-", "--schema", "schema.yml", "--format", "json"])
        .current_dir(directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("outlint should spawn");
    child
        .stdin
        .take()
        .expect("stdin should be piped")
        .write_all(b"## Required\n")
        .expect("stdin should be writable");
    let output = child.wait_with_output().expect("outlint should finish");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(json_output(&output)["results"][0]["path"], "-");
}

#[test]
fn operational_error_wins_over_validation_diagnostics() {
    let directory = TempDir::new("precedence");
    directory.write("schema.yml", VALID_SCHEMA);
    directory.write("fail.md", "plain text\n");

    let output = run(
        &directory,
        &["check", "fail.md", "missing.md", "--schema", "schema.yml"],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).contains("missing-section"));
    assert!(stderr(&output).contains("missing.md"));
}

#[test]
fn usage_directories_encoding_bom_help_and_version_follow_the_contract() {
    let directory = TempDir::new("edges");
    directory.write("schema.yml", VALID_SCHEMA);
    directory.write(
        "bom-schema.yml",
        [b"\xef\xbb\xbf".as_slice(), VALID_SCHEMA.as_bytes()].concat(),
    );
    directory.write("bom.md", b"\xef\xbb\xbf## Required\n");
    directory.write("bad.md", [0xff]);
    fs::create_dir(directory.path().join("docs")).expect("directory fixture should be creatable");

    let no_files = run(&directory, &["check"]);
    assert_eq!(no_files.status.code(), Some(2));
    assert!(stderr(&no_files).contains("at least one Markdown input"));

    let directory_input = run(&directory, &["check", "docs", "--schema", "schema.yml"]);
    assert_eq!(directory_input.status.code(), Some(2));
    assert!(stderr(&directory_input).contains("is a directory"));

    let invalid_utf8 = run(&directory, &["check", "bad.md", "--schema", "schema.yml"]);
    assert_eq!(invalid_utf8.status.code(), Some(2));
    assert!(stderr(&invalid_utf8).contains("not valid UTF-8"));

    let bom = run(
        &directory,
        &["check", "bom.md", "--schema", "bom-schema.yml"],
    );
    assert_eq!(bom.status.code(), Some(0));

    let help = run(&directory, &["--help"]);
    assert_eq!(help.status.code(), Some(0));
    assert!(stdout(&help).starts_with("Usage: outlint"));

    let version = run(&directory, &["--version"]);
    assert_eq!(version.status.code(), Some(0));
    assert_eq!(
        stdout(&version),
        format!("outlint {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn invalid_schema_is_grouped_once_and_document_preflight_still_completes() {
    let directory = TempDir::new("invalid-preflight");
    directory.write("invalid.yml", "version: 2\nsections: []\n");
    directory.write("one.md", "## One\n");
    directory.write("two.md", "## Two\n");
    directory.write("bad.md", [0xff]);
    fs::create_dir(directory.path().join("docs")).expect("directory fixture should be creatable");

    let output = run(
        &directory,
        &[
            "check",
            "docs",
            "missing.md",
            "bad.md",
            "one.md",
            "two.md",
            "--schema",
            "invalid.yml",
            "--format",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("docs' is a directory"));
    assert!(stderr(&output).contains("missing.md"));
    assert!(stderr(&output).contains("bad.md': input is not valid UTF-8"));
    let json = json_output(&output);
    assert_eq!(json["results"].as_array().map(Vec::len), Some(1));
    assert_eq!(json["results"][0]["kind"], "schema");
    assert_eq!(json["results"][0]["path"], "invalid.yml");
    assert_eq!(
        json["results"][0]["diagnostics"][0]["id"],
        "unsupported-version"
    );
    assert_eq!(
        json["summary"],
        serde_json::json!({
            "files": 1,
            "documents": 0,
            "schemas": 1,
            "diagnostics": 1
        })
    );
}

#[test]
fn discovered_invalid_schemas_are_grouped_by_resolved_path_in_input_order() {
    let directory = TempDir::new("invalid-discovery-groups");
    directory.write("a/.outlint.yml", "version: 2\nsections: []\n");
    directory.write("a/one.md", "one\n");
    directory.write("a/two.md", "two\n");
    directory.write("a/bad.md", [0xff]);
    directory.write("b/.outlint.yml", "version: 3\nsections: []\n");
    directory.write("b/one.md", "one\n");

    let output = run(
        &directory,
        &[
            "check",
            "a/one.md",
            "a/two.md",
            "a/missing.md",
            "a/bad.md",
            "b/one.md",
            "--format",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("a/missing.md"));
    assert!(stderr(&output).contains("a/bad.md': input is not valid UTF-8"));
    let json = json_output(&output);
    assert_eq!(json["results"].as_array().map(Vec::len), Some(2));
    assert_eq!(json["results"][0]["path"], "a/.outlint.yml");
    assert_eq!(json["results"][1]["path"], "b/.outlint.yml");
    assert_eq!(json["summary"]["schemas"], 2);

    let human = run(
        &directory,
        &["check", "a/one.md", "a/two.md", "--color", "never"],
    );
    assert_eq!(human.status.code(), Some(1));
    assert_eq!(stdout(&human).matches("unsupported-version").count(), 1);
}

#[test]
fn option_delimiter_makes_help_spellings_into_paths() {
    let directory = TempDir::new("delimiter");
    directory.write("schema.yml", VALID_SCHEMA);
    directory.write("--help", "## Required\n");

    let document = run(
        &directory,
        &["check", "--schema", "schema.yml", "--", "--help"],
    );
    assert_eq!(document.status.code(), Some(0));
    assert_eq!(stdout(&document), "");

    let schema = run(&directory, &["schema", "check", "--", "--help"]);
    assert_eq!(schema.status.code(), Some(1));
    assert!(stdout(&schema).contains("invalid-document-shape"));

    let actual_help = run(&directory, &["check", "--help", "--", "missing"]);
    assert_eq!(actual_help.status.code(), Some(0));
    assert!(stdout(&actual_help).starts_with("Usage: outlint check"));
}

#[test]
fn schema_check_handles_directories_bom_and_invalid_utf8_as_specified() {
    let directory = TempDir::new("schema-inputs");
    directory.write(
        "bom.yml",
        [b"\xef\xbb\xbf".as_slice(), VALID_SCHEMA.as_bytes()].concat(),
    );
    directory.write("bad.yml", [0xff]);
    fs::create_dir(directory.path().join("schemas"))
        .expect("directory fixture should be creatable");

    let bom = run(&directory, &["schema", "check", "bom.yml"]);
    assert_eq!(bom.status.code(), Some(0));

    let failures = run(
        &directory,
        &["schema", "check", "schemas", "bad.yml", "missing.yml"],
    );
    assert_eq!(failures.status.code(), Some(2));
    assert!(stderr(&failures).contains("schemas' is a directory"));
    assert!(stderr(&failures).contains("bad.yml': input is not valid UTF-8"));
    assert!(stderr(&failures).contains("missing.yml"));
}

#[cfg(unix)]
#[test]
fn non_utf8_command_line_paths_are_an_explicit_usage_error() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let directory = TempDir::new("non-utf8-arg");
    let output = Command::new(env!("CARGO_BIN_EXE_outlint"))
        .arg("check")
        .arg(OsString::from_vec(b"bad\xff.md".to_vec()))
        .current_dir(directory.path())
        .output()
        .expect("outlint should run");
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("arguments must be valid UTF-8"));
}

/// Version 3 is a hard cut, so there is no `json-v2` escape hatch: §11.3 tells
/// consumers to reject an envelope version they do not know, and a second
/// format name would be exactly the older shape it tells them not to read.
/// Only `human` and `json` are accepted.
#[test]
fn the_replaced_envelope_version_is_not_reachable_through_a_format_name() {
    let directory = TempDir::new("no-json-v2");
    directory.write("schema.yml", VALID_SCHEMA);
    directory.write("doc.md", "## Required\n");

    for rejected in ["json-v2", "json2", "v2"] {
        let output = run(
            &directory,
            &["check", "doc.md", "-s", "schema.yml", "--format", rejected],
        );
        assert_eq!(
            output.status.code(),
            Some(2),
            "`--format {rejected}` must be a usage error"
        );
        assert_eq!(stdout(&output), "");
        assert!(
            stderr(&output).contains("expected human or json"),
            "`--format {rejected}` should name the accepted formats: {}",
            stderr(&output)
        );
    }

    // The two names that are accepted both still work, and `json` is v3.
    let human = run(
        &directory,
        &["check", "doc.md", "-s", "schema.yml", "--format", "human"],
    );
    assert_eq!(human.status.code(), Some(0));
    let json = run(
        &directory,
        &["check", "doc.md", "-s", "schema.yml", "--format", "json"],
    );
    assert_eq!(json.status.code(), Some(0));
    assert_eq!(json_output(&json)["version"], 3);
}

/// §4.6 provides for an implementation-specific limit on evaluating a
/// frontmatter query: "if an implementation-specific resource limit prevents
/// completion, validation has not produced a document verdict and the CLI MUST
/// surface an operational error (§11.5), not a partial diagnostic set."
///
/// Each `[0,0]` segment selects the same node twice, so the intermediate
/// result doubles per segment; thirty of them is a billion nodes, and both the
/// query and the document are things a document author can supply. The
/// refusal is what keeps that from being a way to end the process.
#[test]
fn a_query_that_cannot_be_evaluated_is_an_operational_failure() {
    let directory = TempDir::new("query-limit");
    let query = format!("$.a{}", "[0,0]".repeat(30));
    directory.write(
        "schema.yml",
        format!(
            concat!(
                "version: 1\n",
                "title: null\n",
                "sections:\n",
                "  - id: body\n",
                "    match: Body\n",
                "    required: true\n",
                "constraints:\n",
                "  - requires: {{ if: body, then: \"fm[{}]\" }}\n"
            ),
            query
        ),
    );
    directory.write(
        "doc.md",
        format!(
            "---\na: {}1{}\n---\n\n## Body\n",
            "[".repeat(34),
            "]".repeat(34)
        ),
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

    // §11.5's operational status, not the diagnostic one.
    assert_eq!(output.status.code(), Some(2));
    let reported = stderr(&output);
    assert!(
        reported.contains("cannot validate doc.md against schema.yml"),
        "{reported}"
    );
    assert!(reported.contains(&query), "{reported}");
    assert!(
        reported.contains("the document has no verdict"),
        "{reported}"
    );
    // No verdict means no result: the envelope carries nothing that could be
    // read as this document having been checked and found clean.
    let envelope = json_output(&output);
    assert_eq!(envelope["results"], serde_json::json!([]));
    assert_eq!(envelope["summary"]["documents"], 0);
    assert_eq!(envelope["summary"]["diagnostics"], 0);

    // §4.6 admits the full grammar and forbids rejecting a query "merely for
    // falling outside the guaranteed core", so the same schema is a valid
    // schema: the limit is reached when a document is evaluated against it,
    // never when it is loaded.
    let schema_check = run(
        &directory,
        &["schema", "check", "schema.yml", "--format", "json"],
    );
    assert_eq!(
        schema_check.status.code(),
        Some(0),
        "{}",
        stderr(&schema_check)
    );
}
