mod common;

use common::*;
use serde_json::Value;
use std::fs;

#[test]
fn linked_schema_root_uri_does_not_alias_a_sibling_root_json() {
    let directory = TempDir::new("linked-frontmatter-root-name-collision");
    directory.write(
        "schema.yml",
        "version: 1\nfrontmatter:\n  schema: frontmatter.schema.json\nsections: []\n",
    );
    directory.write("frontmatter.schema.json", r#"{"$ref":"root.json"}"#);
    directory.write("root.json", r#"{"required":["needed"]}"#);
    directory.write("document.md", "---\npresent: true\n---\n");

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
    assert_eq!(
        json_output(&output)["results"][0]["diagnostics"][0]["id"],
        "frontmatter-schema"
    );
}

#[test]
fn linked_schema_parent_refs_from_a_nested_root_remain_distinct() {
    let directory = TempDir::new("linked-frontmatter-parent-ref-collision");
    directory.write(
        "schema.yml",
        "version: 1\nfrontmatter:\n  schema: sub/main.json\nsections: []\n",
    );
    directory.write(
        "sub/main.json",
        r#"{"allOf":[{"$ref":"x.json"},{"$ref":"../x.json"}]}"#,
    );
    directory.write("sub/x.json", r#"{"required":["from_sub"]}"#);
    directory.write("x.json", r#"{"required":["from_root"]}"#);
    directory.write("document.md", "---\nfrom_sub: true\n---\n");

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
    assert_eq!(
        json_output(&output)["results"][0]["diagnostics"][0]["id"],
        "frontmatter-schema"
    );
}

#[test]
fn linked_schema_same_basename_resources_in_different_directories_remain_distinct() {
    let directory = TempDir::new("linked-frontmatter-basename-collision");
    directory.write(
        "schema.yml",
        "version: 1\nfrontmatter:\n  schema: workspace/deep/main.json\nsections: []\n",
    );
    directory.write(
        "workspace/deep/main.json",
        r#"{"allOf":[{"$ref":"left/consumer.json"},{"$ref":"right/consumer.json"}]}"#,
    );
    directory.write(
        "workspace/deep/left/consumer.json",
        r#"{"$ref":"../../target/defs.json"}"#,
    );
    directory.write(
        "workspace/deep/right/consumer.json",
        r#"{"$ref":"../../../target/defs.json"}"#,
    );
    directory.write(
        "workspace/target/defs.json",
        r#"{"required":["from_workspace"]}"#,
    );
    directory.write("target/defs.json", r#"{"required":["from_fixture_root"]}"#);
    directory.write("document.md", "---\nfrom_workspace: true\n---\n");

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
    assert_eq!(
        json_output(&output)["results"][0]["diagnostics"][0]["id"],
        "frontmatter-schema"
    );
}

#[test]
fn linked_schema_protocol_relative_ref_does_not_read_from_current_directory() {
    let directory = TempDir::new("linked-frontmatter-uri-authority");
    directory.write(
        "schema.yml",
        "version: 1\nfrontmatter:\n  schema: frontmatter.schema.json\nsections: []\n",
    );
    directory.write(
        "frontmatter.schema.json",
        r#"{"$ref":"//attacker.invalid/defs.json"}"#,
    );
    directory.write(
        "attacker.invalid/defs.json",
        r#"{"required":["cwd_file_was_loaded"]}"#,
    );
    directory.write("document.md", "---\npresent: true\n---\n");

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
    assert_eq!(stderr(&output), "");
    let json = json_output(&output);
    assert_eq!(
        json["results"][0]["diagnostics"][0]["id"],
        "invalid-frontmatter-schema"
    );
    assert!(
        json["results"][0]["diagnostics"][0]["schema_location"]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("frontmatter.schema.json"))
    );
}

#[test]
fn linked_schema_read_failures_name_the_unreadable_resource() {
    let directory = TempDir::new("linked-frontmatter-read-failures");
    directory.write(
        "root-missing.yml",
        "version: 1\nfrontmatter:\n  schema: missing-root.json\nsections: []\n",
    );
    directory.write(
        "root-directory.yml",
        "version: 1\nfrontmatter:\n  schema: unreadable-root\nsections: []\n",
    );
    fs::create_dir(directory.path().join("unreadable-root"))
        .expect("unreadable root fixture directory is creatable");
    directory.write(
        "nested-missing.yml",
        "version: 1\nfrontmatter:\n  schema: nested-missing-root.json\nsections: []\n",
    );
    directory.write(
        "nested-missing-root.json",
        r#"{"$ref":"missing-nested.json"}"#,
    );
    directory.write(
        "nested-directory.yml",
        "version: 1\nfrontmatter:\n  schema: nested-directory-root.json\nsections: []\n",
    );
    directory.write(
        "nested-directory-root.json",
        r#"{"$ref":"unreadable-nested"}"#,
    );
    fs::create_dir(directory.path().join("unreadable-nested"))
        .expect("unreadable nested fixture directory is creatable");

    let output = run(
        &directory,
        &[
            "schema",
            "check",
            "root-missing.yml",
            "root-directory.yml",
            "nested-missing.yml",
            "nested-directory.yml",
            "--format",
            "json",
        ],
    );

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    let json = json_output(&output);
    let expected_failures = [
        ("missing-root.json", "cannot inspect linked JSON Schema"),
        ("unreadable-root", "is a directory"),
        ("missing-nested.json", "cannot inspect linked JSON Schema"),
        ("unreadable-nested", "is a directory"),
    ];
    let results = json["results"].as_array().expect("results is an array");
    assert_eq!(results.len(), expected_failures.len());
    for (result, (expected_path, expected_error)) in results.iter().zip(expected_failures) {
        let diagnostic = &result["diagnostics"][0];
        assert_eq!(diagnostic["id"], "invalid-frontmatter-schema");
        assert!(diagnostic["message"]
            .as_str()
            .is_some_and(|message| message.contains(expected_path)
                && message.contains(expected_error)
                && !message.contains("https://outlint.invalid")));
        assert!(diagnostic["schema_location"]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with(expected_path)));
        assert_eq!(diagnostic["schema_location"]["line"], 1);
        assert_eq!(diagnostic["schema_location"]["column"], 1);
    }

    directory.write("document.md", "---\npresent: true\n---\n");
    let check = run(
        &directory,
        &[
            "check",
            "document.md",
            "--schema",
            "nested-missing.yml",
            "--format",
            "json",
        ],
    );
    assert_eq!(check.status.code(), Some(1), "{}", stderr(&check));
    assert_eq!(stderr(&check), "");
    let diagnostic = &json_output(&check)["results"][0]["diagnostics"][0];
    assert_eq!(diagnostic["id"], "invalid-frontmatter-schema");
    assert!(diagnostic["schema_location"]["path"]
        .as_str()
        .is_some_and(|path| path.ends_with("missing-nested.json")));
}

#[test]
fn linked_schema_reports_all_invalid_resources_in_reference_order() {
    let directory = TempDir::new("linked-frontmatter-error-collection");
    directory.write(
        "schema.yml",
        "version: 1\nfrontmatter:\n  schema: root.json\nsections: []\n",
    );
    directory.write(
        "root.json",
        r#"{"allOf":[{"$ref":"first.json"},{"$ref":"second.json"}]}"#,
    );
    directory.write("first.json", "{ invalid json }");
    directory.write("second.json", "[]");

    let output = run(
        &directory,
        &["schema", "check", "schema.yml", "--format", "json"],
    );

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    let json = json_output(&output);
    let diagnostics = json["results"][0]["diagnostics"]
        .as_array()
        .expect("diagnostics is an array");
    assert_eq!(diagnostics.len(), 2);
    for (diagnostic, expected_path) in diagnostics.iter().zip(["first.json", "second.json"]) {
        assert_eq!(diagnostic["id"], "invalid-frontmatter-schema");
        assert!(diagnostic["schema_location"]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with(expected_path)));
    }
}

#[cfg(unix)]
#[test]
fn linked_schema_localhost_ref_loads_the_local_absolute_path() {
    let directory = TempDir::new("linked-frontmatter-localhost-authority");
    directory.write(
        "schema.yml",
        "version: 1\nfrontmatter:\n  schema: frontmatter.schema.json\nsections: []\n",
    );
    let target = directory.path().join("defs.json");
    directory.write(
        "frontmatter.schema.json",
        format!(r#"{{"$ref":"file://localhost{}"}}"#, target.display()),
    );
    directory.write("defs.json", r#"{"required":["needed"]}"#);
    directory.write("document.md", "---\npresent: true\n---\n");

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
    assert_eq!(stderr(&output), "");
    assert_eq!(
        json_output(&output)["results"][0]["diagnostics"][0]["id"],
        "frontmatter-schema"
    );
}

#[test]
fn linked_schema_absolute_id_does_not_rebase_physical_sibling_reads() {
    let directory = TempDir::new("linked-frontmatter-absolute-id");
    directory.write(
        "schema.yml",
        "version: 1\nfrontmatter:\n  schema: frontmatter.schema.json\nsections: []\n",
    );
    directory.write(
        "frontmatter.schema.json",
        r#"{"$id":"https://example.com/schemas/frontmatter.json","$ref":"defs.json"}"#,
    );
    directory.write("defs.json", r#"{"required":["needed"]}"#);
    directory.write("document.md", "---\npresent: true\n---\n");

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
    assert_eq!(stderr(&output), "");
    let json = json_output(&output);
    assert_eq!(
        json["results"][0]["diagnostics"][0]["id"],
        "frontmatter-schema"
    );
    // The root pointer is the empty string, and must survive as a present
    // member rather than collapsing into an absent one.
    assert_eq!(
        json["results"][0]["diagnostics"][0]["target"],
        serde_json::json!({
            "kind": "frontmatter",
            "line_range": {"start_line": 1, "end_line": 3},
            "pointer": ""
        })
    );
}

#[test]
fn linked_schema_absolute_id_preserves_same_document_fragment_refs() {
    let directory = TempDir::new("linked-frontmatter-absolute-id-fragment");
    directory.write(
        "schema.yml",
        "version: 1\nfrontmatter:\n  schema: frontmatter.schema.json\nsections: []\n",
    );
    directory.write(
        "frontmatter.schema.json",
        r##"{
            "$id":"https://example.com/schemas/frontmatter.json",
            "$ref":"#/$defs/frontmatter",
            "$defs":{"frontmatter":{"required":["needed"]}}
        }"##,
    );
    directory.write("document.md", "---\npresent: true\n---\n");

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
    assert_eq!(stderr(&output), "");
    assert_eq!(
        json_output(&output)["results"][0]["diagnostics"][0]["id"],
        "frontmatter-schema"
    );
}

#[test]
fn linked_schema_remote_ref_uses_controlled_no_retrieval_diagnostic() {
    let directory = TempDir::new("linked-frontmatter-remote-ref");
    let remote_uri = "https://example.invalid/frontmatter.schema.json";
    directory.write(
        "schema.yml",
        "version: 1\nfrontmatter:\n  schema: frontmatter.schema.json\nsections: []\n",
    );
    directory.write(
        "frontmatter.schema.json",
        format!(r#"{{"$ref":"{remote_uri}"}}"#),
    );

    let output = run(
        &directory,
        &["schema", "check", "schema.yml", "--format", "json"],
    );

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    let json = json_output(&output);
    let diagnostic = &json["results"][0]["diagnostics"][0];
    assert_eq!(diagnostic["id"], "invalid-frontmatter-schema");
    let message = diagnostic["message"]
        .as_str()
        .expect("diagnostic message is a string");
    assert!(
        message.contains(&format!(
            "JSON Schema resource `{remote_uri}` was not preloaded"
        )),
        "unexpected retrieval diagnostic: {message}"
    );
    assert!(
        !message.contains("Default retriever"),
        "unexpected retrieval diagnostic: {message}"
    );
}

#[cfg(unix)]
#[test]
fn linked_schema_refs_use_the_symlink_path_as_their_base() {
    use std::os::unix::fs::symlink;

    let directory = TempDir::new("linked-frontmatter-symlink-base");
    directory.write(
        "schema.yml",
        "version: 1\ntitle: null\nfrontmatter:\n  schema: frontmatter.schema.json\nsections: []\n",
    );
    directory.write("target/root.json", r#"{"$ref":"defs.json"}"#);
    directory.write("target/defs.json", "false");
    directory.write("defs.json", r#"{"type":"object","required":["status"]}"#);
    symlink(
        directory.path().join("target/root.json"),
        directory.path().join("frontmatter.schema.json"),
    )
    .expect("schema symlink is creatable");
    directory.write("document.md", "---\nstatus: draft\n---\n");

    let output = run(
        &directory,
        &["check", "document.md", "--schema", "schema.yml"],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
}

/// Writes a linked JSON Schema whose root reference starts a chain of `links`
/// hops ending at `true`, declaring `links + 1` references in all.
///
/// However long the chain, the document nests three levels: this is the shape
/// no depth bound can see.
fn write_reference_chain_schema(directory: &TempDir, links: usize) {
    let mut definitions = serde_json::Map::new();
    definitions.insert("end".into(), Value::Bool(true));
    for index in 0..links {
        let target = if index + 1 == links {
            "#/$defs/end".to_owned()
        } else {
            format!("#/$defs/{}", index + 1)
        };
        definitions.insert(index.to_string(), serde_json::json!({ "$ref": target }));
    }
    directory.write(
        "frontmatter.schema.json",
        serde_json::json!({ "$ref": "#/$defs/0", "$defs": definitions }).to_string(),
    );
    directory.write(
        "schema.yml",
        "version: 1\nfrontmatter:\n  schema: frontmatter.schema.json\nsections: []\n",
    );
}

#[test]
fn schema_check_refuses_a_reference_chain_instead_of_overrunning_the_stack() {
    // Compiling a `$ref` re-enters the JSON Schema compiler at its target, so
    // a chain costs a stack frame per link; a long enough one aborted the
    // process, which reaches the user as a signal and no diagnostic rather
    // than as a verdict. Every link of it sits at the same JSON depth, so
    // neither the YAML nesting limit nor `serde_json`'s parse limit can refuse
    // it -- the quantity that has to be bounded is the count. Asserting an
    // exit code at all is half the point: an abort has none.
    let directory = TempDir::new("linked-frontmatter-ref-chain");
    write_reference_chain_schema(&directory, 4_000);

    let output = run(
        &directory,
        &["schema", "check", "schema.yml", "--format", "json"],
    );

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    let json = json_output(&output);
    let diagnostic = &json["results"][0]["diagnostics"][0];
    assert_eq!(diagnostic["id"], "invalid-frontmatter-schema");
    assert_eq!(
        diagnostic["message"],
        "frontmatter JSON Schema declares more than 128 `$ref` or `$dynamicRef` members"
    );
    assert!(
        diagnostic["schema_location"]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("frontmatter.schema.json")),
        "the diagnostic names the document holding the chain"
    );
}

#[test]
fn checking_a_document_refuses_a_reference_chain_instead_of_overrunning_the_stack() {
    // The same graph is compiled again when a validator is prepared, so
    // checking a document reached the same recursion by a different route and
    // aborted identically -- no Markdown was needed to trigger it. Both
    // commands are pinned because a bound charged on only one of them would
    // leave the crash reachable from the other.
    let directory = TempDir::new("linked-frontmatter-ref-chain-check");
    write_reference_chain_schema(&directory, 4_000);
    directory.write("document.md", "---\nstatus: draft\n---\n");

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
    assert_eq!(stderr(&output), "");
    let json = json_output(&output);
    let diagnostic = &json["results"][0]["diagnostics"][0];
    assert_eq!(diagnostic["id"], "invalid-frontmatter-schema");
    assert_eq!(
        diagnostic["message"],
        "frontmatter JSON Schema declares more than 128 `$ref` or `$dynamicRef` members"
    );
}
