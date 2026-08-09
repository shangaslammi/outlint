use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::Value;

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

const VALID_SCHEMA: &str = "version: 1\nsections:\n  - match: Required\n    required: true\n";

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
    assert_eq!(json["version"], 1);
    assert_eq!(json["results"][0]["path"], "first.md");
    assert_eq!(json["results"][1]["path"], "second.md");
    assert_eq!(json["results"][0]["schema"], "schema.yml");
    assert_eq!(
        json["results"][0]["diagnostics"][0]["id"],
        "missing-section"
    );
    assert_eq!(
        json["results"][0]["diagnostics"][0]["header_path"],
        serde_json::json!(["Required"])
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
}

#[test]
fn discovery_uses_the_nearest_schema_for_each_file() {
    let directory = TempDir::new("discovery");
    directory.write(
        ".outlint.yml",
        "version: 1\nsections:\n  - match: Root\n    required: true\n",
    );
    directory.write(
        "nested/.outlint.yml",
        "version: 1\nsections:\n  - match: Nested\n    required: true\n",
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
        "version: 1\nsections:\n  - id: a\n    match: A\n  - id: b\n    match: B\nconstraints:\n  - one_of: [a, b]\n",
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
    assert_eq!(diagnostic["header_path"], serde_json::json!([]));
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
    assert!(human.contains("header_path=\"\""));
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
        "version: 1\nsections:\n  - id: a\n    match: A\nconstraints:\n  - one_of: [fm.status=true, a]\n",
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

#[test]
fn human_output_escapes_untrusted_control_characters() {
    let directory = TempDir::new("human-escape");
    directory.write(
        "schema.yml",
        "version: 1\nsections:\n  - match: \"Required\\nHeading\"\n    required: true\n",
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
