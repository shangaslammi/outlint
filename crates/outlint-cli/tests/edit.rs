#![cfg(feature = "edit")]
mod common;
use common::{run, stdout, TempDir};
use std::fs;
#[test]
fn preview_apply_and_rejection() {
    let dir = TempDir::new("edit");
    dir.write("doc.md", "old");
    dir.write("payload", "new\n");
    let args = [
        "edit",
        "doc.md",
        "replace",
        "$/p",
        "--payload-file",
        "payload",
    ];
    let preview = run(&dir, &[&args[..], &["--dry-run"]].concat());
    assert!(preview.status.success());
    assert_eq!(stdout(&preview),"--- doc.md\n+++ doc.md\n@@ -1,1 +1,1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n");
    let file = dir.path().join("doc.md");
    assert_eq!(fs::read_to_string(&file).unwrap(), "old");
    assert!(run(&dir, &args).status.success());
    assert_eq!(
        run(
            &dir,
            &["edit", "doc.md", "replace", "$/p", "--payload", "# heading"]
        )
        .status
        .code(),
        Some(1)
    );
    assert_eq!(fs::read_to_string(file).unwrap(), "new");
}

#[test]
fn changelog_release_cycle() {
    let dir = TempDir::new("changelog-cycle");
    let initial = include_str!("../../../CHANGELOG.md");
    dir.write("CHANGELOG.md", initial);
    dir.write(
        "CHANGELOG.outlint.yml",
        include_str!("../../../CHANGELOG.outlint.yml"),
    );
    let file = dir.path().join("CHANGELOG.md");
    let command = |args: &[&str], code| {
        let args = [&["edit", "CHANGELOG.md", "--format", "json"][..], args].concat();
        let out = run(&dir, &args);
        assert_eq!(
            out.status.code(),
            Some(code),
            "{args:?}: {} {}",
            stdout(&out),
            common::stderr(&out)
        );
        common::json_output(&out)
    };
    let mut expected = initial.to_string();
    let steps=[
        (vec!["insert-section",".unreleased","append-child","Added","--payload","- First entry\n  wrapped."],"## [Unreleased]\n","## [Unreleased]\n\n### Added\n\n- First entry\n  wrapped.\n"),
        (vec!["append-item",".unreleased.added/list","--payload","Second entry."],"  wrapped.\n","  wrapped.\n- Second entry.\n"),
        (vec!["insert-section",".unreleased.added","after","Fixed","--payload","- A fix."],"- Second entry.\n","- Second entry.\n\n### Fixed\n\n- A fix.\n"),
        (vec!["--script","cut.edit"],"## [Unreleased]\n","## [Unreleased]\n\n## [0.3.0] - 2026-12-01\n"),
        (vec!["replace-text","$","--old","v0.2.0...HEAD","--payload","v0.3.0...HEAD\n[0.3.0]: https://github.com/shangaslammi/outlint/compare/v0.2.0...v0.3.0"],"v0.2.0...HEAD","v0.3.0...HEAD\n[0.3.0]: https://github.com/shangaslammi/outlint/compare/v0.2.0...v0.3.0")
    ];
    dir.write("cut.edit","rename $.unreleased \"[0.3.0] - 2026-12-01\"\ninsert-section $.unreleased before \"[Unreleased]\"\n");
    for (args, old, new) in steps {
        let out = command(&args, 0);
        assert!(out["schema"]
            .as_str()
            .unwrap()
            .ends_with("CHANGELOG.outlint.yml"));
        expected = expected.replacen(old, new, 1);
        assert_eq!(fs::read(&file).unwrap(), expected.as_bytes(), "{args:?}");
    }
    for (args, id) in [
        (
            vec![
                "insert-section",
                ".unreleased",
                "append-child",
                "Fixedd",
                "--payload",
                "- typo",
            ],
            "unexpected-section",
        ),
        (
            vec![
                "insert-section",
                ".unreleased",
                "after",
                "[0.1.5] - 2026-12-01",
            ],
            "order-violation",
        ),
    ] {
        let out = command(&args, 1);
        assert_eq!(out["error"]["id"], "new-diagnostics");
        assert!(
            out["error"]["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["id"] == id),
            "{out}"
        );
        assert_eq!(fs::read(&file).unwrap(), expected.as_bytes());
    }
}

#[test]
fn scripts_resolve_inputs_locally_and_preserve_error_detail() {
    let dir = TempDir::new("edit-scripts");
    dir.write("doc", "old\r\n");
    dir.write("scripts/body", "new\ncontinued");
    dir.write(
        "scripts/change",
        "# body\nreplace $/p @body\ninsert-after $/p <<END\n> a\r\nEND\n",
    );
    let args = [
        "edit",
        "doc",
        "--script",
        "scripts/change",
        "--format",
        "json",
    ];
    assert!(run(&dir, &args).status.success());
    assert_eq!(
        fs::read(dir.path().join("doc")).unwrap(),
        b"new\r\ncontinued\r\n\r\n> a\r\n"
    );
    dir.write("scripts/change", "\nreplace $/p <<END\nmissing end");
    let out = run(&dir, &args);
    let err = &common::json_output(&out)["error"];
    assert_eq!(
        (&err["id"], err["line"].as_u64(), err["column"].as_u64()),
        (&serde_json::json!("invalid-script"), Some(2), Some(13))
    );
    dir.write("scripts/change", "\ndelete $/p[9]\n");
    let out = run(&dir, &args);
    let err = &common::json_output(&out)["error"];
    assert_eq!(err["id"], "unresolved-path");
    assert_eq!(err["kind"], "unresolved");
    assert_eq!(err["line"], 2);
    assert!(err["suggestions"].is_array());
}
#[test]
fn forcing_still_validates_and_reports() {
    let dir = TempDir::new("edit-gate");
    dir.write("doc.md", "# T\n");
    dir.write("other.yml", "version: 1\ntitle: T\nsections: []\n");
    let args = [
        "edit",
        "doc.md",
        "insert-section",
        "$",
        "append-child",
        "X",
        "--schema",
        "other.yml",
        "--format",
        "json",
        "--dry-run",
        "--force",
    ];
    let out = run(&dir, &args);
    assert!(out.status.success());
    assert_eq!(
        common::json_output(&out)["diagnostics"][0]["id"],
        "unexpected-section"
    );
    dir.write("other.yml", "[");
    assert_eq!(run(&dir, &args).status.code(), Some(2));
    assert_eq!(
        fs::read_to_string(dir.path().join("doc.md")).unwrap(),
        "# T\n"
    );
}
