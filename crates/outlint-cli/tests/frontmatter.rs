mod common;

use common::*;

#[test]
fn check_validates_yaml_frontmatter_with_a_linked_json_schema() {
    let directory = TempDir::new("linked-frontmatter");
    directory.write(
        "schema.yml",
        "version: 1\nfrontmatter:\n  required: true\n  schema: frontmatter.schema.json\nsections: []\n",
    );
    directory.write(
        "frontmatter.schema.json",
        r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","$ref":"definitions.json#/$defs/frontmatter"}"#,
    );
    directory.write(
        "definitions.json",
        r#"{"$defs":{"frontmatter":{"type":"object","required":["status"],"properties":{"status":{"enum":["draft","final"]}}}}}"#,
    );
    directory.write("valid.md", "---\nstatus: draft\n---\n\n# Document\n");
    directory.write("invalid.md", "---\nstatus: review\n---\n\n# Document\n");

    let valid = run(
        &directory,
        &[
            "check",
            "valid.md",
            "--schema",
            "schema.yml",
            "--format",
            "json",
        ],
    );
    assert_eq!(valid.status.code(), Some(0));

    let invalid = run(
        &directory,
        &[
            "check",
            "invalid.md",
            "--schema",
            "schema.yml",
            "--format",
            "json",
        ],
    );
    assert_eq!(invalid.status.code(), Some(1));
    let diagnostic = &json_output(&invalid)["results"][0]["diagnostics"][0];
    assert_eq!(diagnostic["id"], "frontmatter-schema");
    assert_eq!(
        diagnostic["target"],
        serde_json::json!({
            "kind": "frontmatter",
            "line_range": {"start_line": 1, "end_line": 3},
            "pointer": "/status"
        })
    );
    assert_eq!(
        diagnostic["schema_node"],
        serde_json::json!({"kind": "frontmatter_schema_document"})
    );
    assert!(diagnostic["schema_location"]["path"]
        .as_str()
        .is_some_and(|path| path.ends_with("frontmatter.schema.json")));
}

#[test]
fn frontmatter_schema_messages_preserve_document_number_spellings() {
    let directory = TempDir::new("frontmatter-number-messages");
    directory.write(
        "schema.yml",
        "version: 1\ntitle: null\nfrontmatter:\n  schema: frontmatter.schema.json\nsections: []\n",
    );
    directory.write(
        "frontmatter.schema.json",
        r#"{"type":"object","properties":{"whole":{"maximum":1},"fraction":{"maximum":1},"lower_exponent":{"maximum":1},"upper_exponent":{"maximum":1}}}"#,
    );
    directory.write(
        "document.md",
        "---\nwhole: 100.0\nfraction: 1.5\nlower_exponent: 1e2\nupper_exponent: 1E2\n---\n",
    );

    let output = run(
        &directory,
        &[
            "check",
            "document.md",
            "--schema",
            "schema.yml",
            "--format",
            "json",
        ],
    );

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), "");
    let json = json_output(&output);
    let diagnostics = json["results"][0]["diagnostics"]
        .as_array()
        .expect("diagnostics are an array")
        .iter()
        .map(|diagnostic| {
            diagnostic["message"]
                .as_str()
                .expect("diagnostic message is a string")
        })
        .collect::<Vec<_>>();
    // Each rejected value anchors to its own line, so the diagnostics order by
    // the line their key sits on rather than tying on the block's first line.
    assert_eq!(
        diagnostics,
        [
            "100.0 is greater than the maximum of 1",
            "1.5 is greater than the maximum of 1",
            "1e2 is greater than the maximum of 1",
            "1E2 is greater than the maximum of 1",
        ]
    );
}

#[test]
fn frontmatter_schema_diagnostics_anchor_to_the_failing_entry() {
    let directory = TempDir::new("frontmatter-anchors");
    directory.write(
        "schema.yml",
        "version: 1\nfrontmatter:\n  schema: frontmatter.schema.json\nsections: []\n",
    );
    directory.write(
        "frontmatter.schema.json",
        r#"{"type":"object","required":["absent"],"properties":{
            "tags":{"type":"array","items":{"type":"string"}},
            "count":{"type":"integer"},
            "nested":{"type":"object","properties":{"inner":{"type":"string"}}}
        }}"#,
    );
    // The comment and the blank lines separate the document line of each
    // failing entry from any count of its non-empty predecessors.
    directory.write(
        "document.md",
        concat!(
            "---\n",         // 1
            "# a comment\n", // 2
            "\n",            // 3
            "tags:\n",       // 4
            "  - ok\n",      // 5
            "  - 123\n",     // 6
            "\n",            // 7
            "count: nope\n", // 8
            "nested:\n",     // 9
            "  inner: 1\n",  // 10
            "---\n",         // 11
            "\n",
            "# Document\n",
        ),
    );

    let output = run(
        &directory,
        &[
            "check",
            "document.md",
            "--schema",
            "schema.yml",
            "--format",
            "json",
        ],
    );

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let json = json_output(&output);
    let anchored = json["results"][0]["diagnostics"]
        .as_array()
        .expect("diagnostics are an array")
        .iter()
        .map(|diagnostic| {
            (
                diagnostic["id"].as_str().expect("an id"),
                diagnostic["location"]["line"].as_u64().expect("a line"),
                diagnostic["location"]["column"].as_u64().expect("a column"),
                diagnostic["target"]["pointer"].as_str().expect("a pointer"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        anchored,
        [
            // The root pointer names the mapping, whose extent is the block.
            ("frontmatter-schema", 1, 1, ""),
            ("frontmatter-schema", 6, 5, "/tags/1"),
            ("frontmatter-schema", 8, 1, "/count"),
            ("frontmatter-schema", 10, 3, "/nested/inner"),
        ]
    );
    // The target keeps naming the whole block; only the anchor narrows.
    assert_eq!(
        json["results"][0]["diagnostics"][2]["target"],
        serde_json::json!({
            "kind": "frontmatter",
            "line_range": {"start_line": 1, "end_line": 11},
            "pointer": "/count"
        })
    );
}

#[test]
fn frontmatter_schema_anchors_reach_entries_beside_tags() {
    let directory = TempDir::new("frontmatter-anchors-tagged");
    directory.write(
        "schema.yml",
        "version: 1\nfrontmatter:\n  schema: frontmatter.schema.json\nsections: []\n",
    );
    directory.write(
        "frontmatter.schema.json",
        r#"{"type":"object","properties":{"count":{"type":"integer"}}}"#,
    );
    // An explicit tag used to force a parse path that carried no positions,
    // anchoring every diagnostic of the block at its first line. One spanned
    // reader keeps the entry's own position beside it.
    directory.write(
        "document.md",
        "---\nignored: !!str 5\ncount: nope\n---\n\n# Document\n",
    );

    let output = run(
        &directory,
        &[
            "check",
            "document.md",
            "--schema",
            "schema.yml",
            "--format",
            "json",
        ],
    );

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let diagnostic = &json_output(&output)["results"][0]["diagnostics"][0];
    assert_eq!(diagnostic["id"], "frontmatter-schema");
    assert_eq!(diagnostic["target"]["pointer"], "/count");
    assert_eq!(
        diagnostic["location"],
        serde_json::json!({"line": 3, "column": 1})
    );
}

#[test]
fn block_level_frontmatter_diagnostics_stay_anchored_to_the_block() {
    let directory = TempDir::new("frontmatter-block-anchors");
    directory.write(
        "required.yml",
        "version: 1\nfrontmatter:\n  required: true\nsections: []\n",
    );
    directory.write(
        "forbidden.yml",
        "version: 1\nfrontmatter:\n  allow: false\nsections: []\n",
    );
    directory.write("absent.md", "# Document\n");
    directory.write("unparsable.md", "---\nnot: [a\n---\n\n# Document\n");

    let missing = run(
        &directory,
        &[
            "check",
            "absent.md",
            "--schema",
            "required.yml",
            "--format",
            "json",
        ],
    );
    assert_eq!(missing.status.code(), Some(1), "{}", stderr(&missing));
    let missing = &json_output(&missing)["results"][0]["diagnostics"][0];
    assert_eq!(missing["id"], "missing-frontmatter");
    assert_eq!(
        missing["location"],
        serde_json::json!({"line": 1, "column": 1})
    );

    let present = run(
        &directory,
        &[
            "check",
            "unparsable.md",
            "--schema",
            "forbidden.yml",
            "--format",
            "json",
        ],
    );
    assert_eq!(present.status.code(), Some(1), "{}", stderr(&present));
    let present = json_output(&present);
    let anchored = present["results"][0]["diagnostics"]
        .as_array()
        .expect("diagnostics are an array")
        .iter()
        .map(|diagnostic| {
            (
                diagnostic["id"].as_str().expect("an id"),
                diagnostic["location"]["line"].as_u64().expect("a line"),
                diagnostic["location"]["column"].as_u64().expect("a column"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        anchored,
        [
            ("forbidden-frontmatter", 1, 1),
            ("invalid-frontmatter", 1, 1)
        ]
    );
}

#[test]
fn frontmatter_reference_details_retain_typed_equality() {
    let directory = TempDir::new("frontmatter-reference");
    directory.write(
        "schema.yml",
        "version: 1\ntitle: null\nsections:\n  - id: a\n    match: A\nconstraints:\n  - one_of: [fm.status=true, a]\n",
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
    assert_eq!(output.status.code(), Some(1));
    let reference = &json_output(&output)["results"][0]["diagnostics"][0]["references"][0];
    assert_eq!(reference["kind"], "frontmatter");
    assert_eq!(reference["path"], serde_json::json!(["status"]));
    assert_eq!(
        reference["equals"],
        serde_json::json!({"type": "boolean", "value": true})
    );
}
