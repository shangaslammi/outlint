mod common;

use common::*;
use std::{fs, process::Command};

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
fn discovery_prefers_the_stem_schema_within_a_directory() {
    let directory = TempDir::new("stem-discovery");
    directory.write(
        ".outlint.yml",
        "version: 1\ntitle: null\nsections:\n  - match: Root\n    required: true\n",
    );
    directory.write(
        "CHANGELOG.outlint.yml",
        "version: 1\ntitle: null\nsections:\n  - match: Log\n    required: true\n",
    );
    directory.write("CHANGELOG.md", "## Log\n");
    directory.write("other.md", "## Root\n");

    let output = run(
        &directory,
        &["check", "CHANGELOG.md", "other.md", "--format", "json"],
    );
    assert_eq!(output.status.code(), Some(0));
    let json = json_output(&output);
    assert_eq!(json["results"][0]["schema"], "CHANGELOG.outlint.yml");
    assert_eq!(json["results"][1]["schema"], ".outlint.yml");
}

#[test]
fn discovery_finds_a_stem_schema_in_an_ancestor_directory() {
    let directory = TempDir::new("stem-ancestor");
    directory.write(
        "doc.outlint.yml",
        "version: 1\ntitle: null\nsections:\n  - match: Far\n    required: true\n",
    );
    directory.write("nested/doc.md", "## Far\n");

    let output = run(&directory, &["check", "nested/doc.md", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        json_output(&output)["results"][0]["schema"],
        "doc.outlint.yml"
    );
}

#[test]
fn discovery_prefers_a_nearer_directory_over_an_ancestor_stem_schema() {
    let directory = TempDir::new("stem-precedence");
    directory.write(
        "doc.outlint.yml",
        "version: 1\ntitle: null\nsections:\n  - match: Far\n    required: true\n",
    );
    directory.write(
        "nested/.outlint.yml",
        "version: 1\ntitle: null\nsections:\n  - match: Near\n    required: true\n",
    );
    directory.write("nested/doc.md", "## Near\n");

    let output = run(&directory, &["check", "nested/doc.md", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        json_output(&output)["results"][0]["schema"],
        "nested/.outlint.yml"
    );
}

#[test]
fn discovery_treats_an_extensionless_file_name_as_its_own_stem() {
    let directory = TempDir::new("stem-extensionless");
    directory.write(
        "NOTES.outlint.yml",
        "version: 1\ntitle: null\nsections:\n  - match: Notes\n    required: true\n",
    );
    directory.write("NOTES", "## Notes\n");

    let output = run(&directory, &["check", "NOTES", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        json_output(&output)["results"][0]["schema"],
        "NOTES.outlint.yml"
    );
}

#[test]
fn discovery_skips_directories_named_like_schemas() {
    let directory = TempDir::new("stem-directory-candidates");
    directory.write(
        ".outlint.yml",
        "version: 1\ntitle: null\nsections:\n  - match: Root\n    required: true\n",
    );
    fs::create_dir_all(directory.path().join("nested/.outlint.yml"))
        .expect("directory candidate should be creatable");
    fs::create_dir_all(directory.path().join("nested/doc.outlint.yml"))
        .expect("directory candidate should be creatable");
    directory.write("nested/doc.md", "## Root\n");

    let output = run(&directory, &["check", "nested/doc.md", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(json_output(&output)["results"][0]["schema"], ".outlint.yml");
}

#[test]
fn the_repository_changelog_passes_its_discovered_schema() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    assert!(
        root.join("CHANGELOG.outlint.yml").is_file(),
        "the dogfood schema should sit at the repository root"
    );
    let output = Command::new(env!("CARGO_BIN_EXE_outlint"))
        .args(["check", "CHANGELOG.md", "--format", "json"])
        .current_dir(&root)
        .output()
        .expect("outlint should run");
    assert_eq!(
        output.status.code(),
        Some(0),
        "CHANGELOG.md should satisfy CHANGELOG.outlint.yml:\n{}\n{}",
        stdout(&output),
        stderr(&output)
    );
    assert_eq!(
        json_output(&output)["results"][0]["schema"],
        "CHANGELOG.outlint.yml"
    );
}
