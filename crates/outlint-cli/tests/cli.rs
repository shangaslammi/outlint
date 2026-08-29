//! CLI surface regression tests.
//!
//! **This suite mixes contract tests and presentation regression tests.** The
//! command grammar, discovery, stdin, streams, JSON data model, ordering, exit
//! status, no-mutation rule, and offline behavior are normative in
//! `spec/outlint-spec.md` §11. Exact message wording, help layout, and other
//! human presentation remain implementation details.
//!
//! Two consequences worth knowing before you change anything here:
//!
//! - A failure in this file is always a **regression signal**, but is evidence of
//!   a specification violation only when the asserted behavior is stated in the
//!   specification. Check §11 before treating a presentation assertion as a
//!   portable requirement.
//! - Conversely, do not treat a passing test here as proof that behavior is
//!   specified. Much of what these tests assert is incidental: exact wording of
//!   messages, punctuation and escaping choices, and help-text layout.
//!
//! Ordering in particular: the conformance corpus (`testdata/`) deliberately does
//! **not** constrain validator order — its format is shared by independent
//! implementations. The reference CLI nevertheless promises its own total
//! presentation order in §11.4; assertions of that order belong to the CLI
//! contract, not the corpus.
//!
//! TODO(cleanup): this suite needs a pass to separate the two kinds of assertion
//! it currently mixes — normative behavioral contracts from implementation
//! detail (message wording, help layout, and formatting). Contract tests should
//! cite §11; incidental assertions should be visibly marked so a deliberate
//! change to presentation does not read as a contract break. Until that pass
//! happens, expect this file to over-constrain the implementation.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::Value;

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

const VALID_SCHEMA: &str =
    "version: 1\ntitle: null\nsections:\n  - match: Required\n    required: true\n";

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        for _ in 0..100 {
            let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "outlint-cli-{name}-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("temporary test directory should be creatable: {error}"),
            }
        }
        panic!("could not allocate a unique temporary test directory")
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, relative: &str, contents: impl AsRef<[u8]>) {
        let path = self.0.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("fixture parent should be creatable");
        }
        fs::write(path, contents).expect("fixture should be writable");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(directory: &TempDir, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_outlint"))
        .args(arguments)
        .current_dir(directory.path())
        .output()
        .expect("outlint should run")
}

fn stdout(output: &Output) -> &str {
    std::str::from_utf8(&output.stdout).expect("stdout should be UTF-8")
}

fn stderr(output: &Output) -> &str {
    std::str::from_utf8(&output.stderr).expect("stderr should be UTF-8")
}

fn json_output(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("stdout should be one JSON document")
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
    assert!(stdout(&fail).contains("fail.md:1:1 [missing-section]"));
    assert!(stdout(&fail).ends_with("1 diagnostic in 1 file\n"));
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
fn discovery_uses_the_nearest_schema_for_each_file() {
    let directory = TempDir::new("discovery");
    directory.write(
        ".outlint.yml",
        "version: 1\ntitle: null\nsections:\n  - match: Root\n    required: true\n",
    );
    directory.write(
        "nested/.outlint.yml",
        "version: 1\ntitle: null\nsections:\n  - match: Nested\n    required: true\n",
    );
    directory.write("root.md", "## Root\n");
    directory.write("nested/doc.md", "## Nested\n");

    let output = run(
        &directory,
        &["check", "root.md", "nested/doc.md", "--format", "json"],
    );
    assert_eq!(output.status.code(), Some(0));
    let json = json_output(&output);
    assert_eq!(json["results"][0]["schema"], ".outlint.yml");
    assert_eq!(json["results"][1]["schema"], "nested/.outlint.yml");
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
    assert!(stdout(&output).contains("[missing-section]"));
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
    assert_eq!(stdout(&human).matches("[unsupported-version]").count(), 1);
    assert!(stdout(&human).ends_with("1 diagnostic in 1 file\n"));
}

#[test]
fn constraint_details_are_preserved_in_json_and_human_output() {
    let directory = TempDir::new("constraint-details");
    directory.write(
        "schema.yml",
        "version: 1\ntitle: null\nsections:\n  - id: a\n    match: A\n  - id: b\n    match: B\nconstraints:\n  - one_of: [a, b]\n",
    );
    directory.write("doc.md", "## A\n## B\n");

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
    let diagnostic = &json_output(&output)["results"][0]["diagnostics"][0];
    assert_eq!(
        diagnostic["target"],
        serde_json::json!({"kind": "document"})
    );
    assert_eq!(
        diagnostic["schema_node"],
        serde_json::json!({"kind": "constraint", "scope": [], "index": 0})
    );
    assert_eq!(diagnostic["schema_location"]["path"], "schema.yml");
    assert_eq!(
        diagnostic["involved_headers"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(diagnostic["references"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        diagnostic["references"][0]["path"],
        serde_json::json!(["a"])
    );
    assert_eq!(
        diagnostic["references"][0]["matcher"],
        serde_json::json!({"kind": "exact", "value": "A"})
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
    assert!(human.contains("target=document"));
    assert!(human.contains("schema_node=constraint(scope=[],index=0)"));
    assert!(human.contains("schema_location=\"schema.yml\":"));
    assert!(human.contains("involved_headers=[\"A\"@1:1, \"B\"@2:1]"));
    assert!(human.contains("references=[a=>exact:\"A\", b=>exact:\"B\"]"));
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

// Windows refuses control characters in filenames (`fs::write` fails with
// ERROR_INVALID_NAME before outlint runs), so this on-disk fixture can only
// exist on Unix. The escaping under test is platform-independent.
#[cfg(unix)]
#[test]
fn human_output_escapes_untrusted_control_characters() {
    let directory = TempDir::new("human-escape");
    directory.write(
        "schema.yml",
        "version: 1\ntitle: null\nsections:\n  - match: \"Required\\nHeading\"\n    required: true\n",
    );
    let document = "evil\u{1b}\n.md";
    directory.write(document, "plain text\n");

    let output = run(
        &directory,
        &[
            "check",
            document,
            "--schema",
            "schema.yml",
            "--color",
            "never",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(!output.stdout.contains(&0x1b));
    assert!(stdout(&output).contains("evil\\x1b\\n.md:1:1"));
    assert!(stdout(&output).contains("Required\\nHeading"));
    assert_eq!(stdout(&output).lines().count(), 2);
}

#[test]
fn human_output_prints_message_quotes_verbatim() {
    // jsonschema's messages quote the property they talk about; the human
    // renderer must print those quotes as-is (`"title" is a required
    // property`), not as the JSON-escaped `\"title\"`. JSON-style escaping
    // belongs only to `--format json`.
    let directory = TempDir::new("human-quotes");
    directory.write(
        "schema.yml",
        "version: 1\nfrontmatter:\n  schema: frontmatter.schema.json\nsections: []\n",
    );
    directory.write(
        "frontmatter.schema.json",
        r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","required":["title"]}"#,
    );
    directory.write("doc.md", "---\nstatus: draft\n---\n");

    let output = run(
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
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).contains("\"title\" is a required property"));
    assert!(!stdout(&output).contains("\\\""));
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
    assert!(stdout(&schema).contains("[invalid-document-shape]"));

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
