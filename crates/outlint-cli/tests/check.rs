mod common;

use common::*;

#[test]
fn human_check_is_quiet_on_pass_and_reports_failures() {
    let directory = TempDir::new("human");
    directory.write("schema.yml", VALID_SCHEMA);
    directory.write("pass.md", "## Required\n");
    directory.write("fail.md", "## Other\n");

    let pass = run(&directory, &["check", "pass.md", "-s", "schema.yml"]);
    assert_eq!(pass.status.code(), Some(0));
    assert_eq!(stdout(&pass), "");
    assert_eq!(stderr(&pass), "");

    let fail = run(
        &directory,
        &[
            "check",
            "fail.md",
            "--schema",
            "schema.yml",
            "--color",
            "never",
        ],
    );
    assert_eq!(fail.status.code(), Some(1));
    // Human syntax is intentionally unspecified (§11.3). These checks assert
    // only that the current presentation identifies the source and stable id.
    assert!(stdout(&fail).contains("fail.md:1:1"));
    assert!(stdout(&fail).contains("missing-section"));
    assert_eq!(stderr(&fail), "");
}

#[test]
fn json_check_has_stable_fields_and_order() {
    let directory = TempDir::new("json");
    directory.write("schema.yml", VALID_SCHEMA);
    directory.write("first.md", "text\n");
    directory.write("second.md", "## Required\n");

    let output = run(
        &directory,
        &[
            "check",
            "first.md",
            "second.md",
            "--schema",
            "schema.yml",
            "--format",
            "json",
            "--color",
            "always",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), "");
    // §11.3: JSON output carries no ANSI escapes whatever `--color` says.
    assert!(!output.stdout.contains(&0x1b));
    // The whole envelope, compared as one value rather than field by field:
    // an extra, missing, or renamed member cannot slip past an equality on
    // the complete object the way it can past a member probe, and neither can
    // a reordered *array* — which is what pins the argument order of
    // `results` below. Object member order is not among the things this
    // detects, and could not be: JSON leaves it insignificant and
    // `serde_json::Map` does not retain it here. This is the version 3 shape
    // of §11.3, including all four `summary` counts.
    assert_eq!(
        json_output(&output),
        serde_json::json!({
            "version": 3,
            "results": [
                {
                    "kind": "document",
                    "path": "first.md",
                    "schema": "schema.yml",
                    "diagnostics": [{
                        "id": "missing-section",
                        "message": "matched 0 sections, but at least 1 are required",
                        "location": {"line": 1, "column": 1},
                        "target": {
                            "kind": "missing_header",
                            "parent": [],
                            "matcher": "Required"
                        },
                        "schema_node": {"kind": "rule", "scope": [], "index": 0},
                        "schema_location": {
                            "path": "schema.yml",
                            "line": 4,
                            "column": 5
                        }
                    }]
                },
                {
                    "kind": "document",
                    "path": "second.md",
                    "schema": "schema.yml",
                    "diagnostics": []
                }
            ],
            "summary": {
                "files": 2,
                "documents": 2,
                "schemas": 0,
                "diagnostics": 1
            }
        })
    );
}
