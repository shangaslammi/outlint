mod common;

use common::*;

#[test]
fn all_rfc5_ids_are_valid_suppressions() {
    let directory = TempDir::new("rfc5-suppressions");
    let cases = [
        (
            "unexpected-block",
            "version: 1\ncontent: []\noutline: []\n",
            "text\n",
        ),
        (
            "misplaced-block",
            "version: 1\ncontent:\n  - block: p\n  - block: list\noutline: []\n",
            "- item\n\ntext\n",
        ),
        (
            "missing-block",
            "version: 1\ncontent:\n  - block: p\noutline: []\n",
            "",
        ),
        (
            "too-few-blocks",
            "version: 1\ncontent:\n  - block: p\n    repeat: 2..2\noutline: []\n",
            "text\n",
        ),
        (
            "too-many-blocks",
            "version: 1\ncontent:\n  - block: p\n    repeat: 0..1\noutline: []\n",
            "one\n\ntwo\n",
        ),
        (
            "unexpected-item",
            "version: 1\ncontent:\n  - block: list\n    items: []\noutline: []\n",
            "- A\n",
        ),
        (
            "misplaced-item",
            "version: 1\ncontent:\n  - block: list\n    items:\n      - match: A\n      - match: B\noutline: []\n",
            "- B\n- A\n",
        ),
        (
            "missing-item",
            "version: 1\ncontent:\n  - block: list\n    items:\n      - match: A\noutline: []\n",
            "- B\n",
        ),
        (
            "too-few-items",
            "version: 1\ncontent:\n  - block: list\n    items:\n      - match: A\n        repeat: 2..2\noutline: []\n",
            "- A\n",
        ),
        (
            "too-many-items",
            "version: 1\ncontent:\n  - block: list\n    items:\n      - match: A\n        repeat: 0..1\noutline: []\n",
            "- A\n- A\n",
        ),
    ];

    for (index, (id, schema, markdown)) in cases.iter().enumerate() {
        let schema_path = format!("schema-{index}.yml");
        let plain_path = format!("plain-{index}.md");
        let suppressed_path = format!("suppressed-{index}.md");
        directory.write(&schema_path, schema);
        directory.write(&plain_path, markdown);
        directory.write(
            &suppressed_path,
            format!("<!-- outlint-disable-file {id} -->\n{markdown}"),
        );

        let plain = run(
            &directory,
            &[
                "check",
                &plain_path,
                "--schema",
                &schema_path,
                "--format",
                "json",
            ],
        );
        let plain_envelope = json_output(&plain);
        let plain_ids = plain_envelope["results"][0]["diagnostics"]
            .as_array()
            .expect("diagnostics is an array")
            .iter()
            .filter_map(|diagnostic| diagnostic["id"].as_str())
            .collect::<Vec<_>>();
        assert!(
            plain_ids.contains(id),
            "fixture did not produce {id}: {plain_ids:?}"
        );

        let suppressed = run(
            &directory,
            &[
                "check",
                &suppressed_path,
                "--schema",
                &schema_path,
                "--format",
                "json",
            ],
        );
        let suppressed_envelope = json_output(&suppressed);
        let suppressed_ids = suppressed_envelope["results"][0]["diagnostics"]
            .as_array()
            .expect("diagnostics is an array")
            .iter()
            .filter_map(|diagnostic| diagnostic["id"].as_str())
            .collect::<Vec<_>>();
        assert!(
            !suppressed_ids.contains(id),
            "{id} was not suppressed: {suppressed_ids:?}"
        );
    }
}

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
    // `serde_json::Map` does not retain it here. This is the envelope shape
    // of §11.3, including all four `summary` counts.
    assert_eq!(
        json_output(&output),
        serde_json::json!({
            "version": 2,
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
