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
    assert!(!output.stdout.contains(&0x1b));
    let json = json_output(&output);
    assert_eq!(json["version"], 2);
    assert_eq!(json["results"][0]["path"], "first.md");
    assert_eq!(json["results"][1]["path"], "second.md");
    assert_eq!(json["results"][0]["schema"], "schema.yml");
    assert_eq!(
        json["results"][0]["diagnostics"][0]["id"],
        "missing-section"
    );
    assert_eq!(
        json["results"][0]["diagnostics"][0]["target"],
        serde_json::json!({
            "kind": "missing_header",
            "parent": [],
            "matcher": "Required"
        })
    );
    assert_eq!(
        json["results"][0]["diagnostics"][0]["location"],
        serde_json::json!({"line": 1, "column": 1})
    );
    assert_eq!(
        json["results"][0]["diagnostics"][0]["schema_location"]["path"],
        "schema.yml"
    );
    assert_eq!(
        json["summary"],
        serde_json::json!({
            "files": 2,
            "documents": 2,
            "schemas": 0,
            "diagnostics": 1
        })
    );
}
