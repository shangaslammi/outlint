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
        (vec!["insert-section",".unreleased","append-child","Changed","--payload","- First entry\n  wrapped."],"\n## [0.2.0]","\n### Changed\n\n- First entry\n  wrapped.\n\n## [0.2.0]"),
        (vec!["append-item",".unreleased.changed/list","--payload","Second entry."],"  wrapped.\n","  wrapped.\n- Second entry.\n"),
        (vec!["insert-section",".unreleased.changed","after","Fixed","--payload","- A fix."],"- Second entry.\n","- Second entry.\n\n### Fixed\n\n- A fix.\n"),
        (vec!["--script","cut.edit"],"## [Unreleased]\n","## [Unreleased]\n\n## [0.3.0] - 2026-12-01\n"),
        (vec!["replace-text","$","--old","v0.2.0...HEAD","--payload","v0.3.0...HEAD\n[0.3.0]: https://github.com/shangaslammi/outlint/compare/v0.2.0...v0.3.0"],"v0.2.0...HEAD","v0.3.0...HEAD\n[0.3.0]: https://github.com/shangaslammi/outlint/compare/v0.2.0...v0.3.0")
    ];
    dir.write("cut.edit","rename $.unreleased \"[0.3.0] - 2026-12-01\"\ninsert-section $.unreleased before \"[Unreleased]\"\n");
    for (args, old, new) in steps {
        let out = command(&args, 0);
        assert_eq!(out["schema"], "CHANGELOG.outlint.yml");
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
    assert!(err["message"].as_str().unwrap().contains("cannot resolve"));
    assert!(err["suggestions"].is_array());
}

#[test]
fn refusals_report_candidate_coordinates_in_all_formats() {
    let dir = TempDir::new("edit-candidate-location");
    dir.write("doc.md", "# T\n\n## Unreleased\n");
    dir.write(
        "doc.outlint.yml",
        "version: 1\ntitle: T\nsections: [{match: Unreleased, sections: []}]\n",
    );
    for format in ["human", "compact", "json"] {
        let out = run(
            &dir,
            &[
                "edit",
                "doc.md",
                "insert-section",
                "$.unreleased",
                "append-child",
                "Addded",
                "--payload",
                "- **X.** y",
                "--dry-run",
                "--format",
                format,
            ],
        );
        assert_eq!(out.status.code(), Some(1));
        let stderr = common::stderr(&out);
        match format {
            "json" => {
                assert!(stderr.is_empty());
                let json = common::json_output(&out);
                let error = &json["error"];
                assert_eq!(error["id"], "new-diagnostics");
                assert_eq!(error["diagnostic_source"], "candidate");
                let diagnostic = &error["diagnostics"][0];
                assert_eq!(diagnostic["id"], "unexpected-section");
                let checked = run(&dir, &["check", "doc.md", "--format", "json"]);
                let checked = common::json_output(&checked);
                assert_eq!(
                    checked["results"][0]["schema"], "doc.outlint.yml",
                    "{checked}"
                );
                assert_eq!(diagnostic["schema_location"]["path"], "doc.outlint.yml");
                assert_eq!(
                    diagnostic["location"],
                    serde_json::json!({"line": 5, "column": 1})
                );
            }
            "compact" => {
                assert!(stdout(&out).is_empty());
                assert!(
                    stderr.contains("candidate\tdoc.md\t5\t1\tunexpected-section\t"),
                    "{stderr}"
                );
            }
            _ => {
                assert!(stdout(&out).is_empty());
                assert!(
                    stderr.contains("doc.md:5:1 (candidate) unexpected-section "),
                    "{stderr}"
                );
            }
        }
    }
    assert_eq!(
        fs::read(dir.path().join("doc.md")).unwrap(),
        b"# T\n\n## Unreleased\n"
    );
}

#[test]
fn overlapping_literal_matches_have_a_written_refusal() {
    let dir = TempDir::new("edit-literal-refusal");
    dir.write("doc.md", "ababa\n");
    for format in ["human", "compact", "json"] {
        let out = run(
            &dir,
            &[
                "edit",
                "doc.md",
                "replace-text",
                "$",
                "--old",
                "aba",
                "--payload",
                "x",
                "--format",
                format,
            ],
        );
        assert_eq!(out.status.code(), Some(1));
        let message = "old text occurs 2 times in `$`";
        if format == "json" {
            assert!(common::stderr(&out).is_empty());
            let json = common::json_output(&out);
            assert_eq!(json["error"]["id"], "ambiguous-text");
            assert_eq!(json["error"]["message"], message);
            assert_eq!(json["error"]["count"], 2);
        } else {
            assert!(stdout(&out).is_empty());
            assert_eq!(common::stderr(&out), format!("ambiguous-text\t{message}\n"));
        }
    }
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
    let json = common::json_output(&out);
    assert_eq!(json["diagnostic_source"], "candidate");
    assert_eq!(json["diagnostics"][0]["id"], "unexpected-section");
    dir.write("other.yml", "[");
    assert_eq!(run(&dir, &args).status.code(), Some(2));
    assert_eq!(
        fs::read_to_string(dir.path().join("doc.md")).unwrap(),
        "# T\n"
    );
}

#[test]
fn edit_fixtures_match_exact_bytes_or_refuse_without_writing() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/edit");
    for case in fs::read_dir(root).unwrap() {
        let case = case.unwrap().path();
        let name = case.file_name().unwrap().to_str().unwrap();
        let dir = TempDir::new(name);
        let original = fs::read(case.join("input.md")).unwrap();
        for file in ["input.md", "script.edit", "input.outlint.yml"] {
            let source = case.join(file);
            if source.is_file() {
                fs::copy(source, dir.path().join(file)).unwrap();
            }
        }
        let output = run(
            &dir,
            &[
                "edit",
                "input.md",
                "--script",
                "script.edit",
                "--format",
                "json",
            ],
        );
        let written = fs::read(dir.path().join("input.md")).unwrap();
        let value = common::json_output(&output);
        if case.join("expected.json").is_file() {
            let expected: serde_json::Value =
                serde_json::from_slice(&fs::read(case.join("expected.json")).unwrap()).unwrap();
            assert_eq!(output.status.code(), Some(1), "{name}: {value}");
            assert_eq!(value["error"]["id"], expected["id"], "{name}: {value}");
            assert_eq!(written, original, "{name}");
        } else {
            assert!(output.status.success(), "{name}: {value}");
            assert_eq!(
                written,
                fs::read(case.join("expected.md")).unwrap(),
                "{name}"
            );
        }
    }
}

#[test]
fn edit_strips_boms_from_external_inputs_and_preserves_the_destination_bom() {
    let dir = TempDir::new("edit-bom");
    dir.write("doc.md", "\u{feff}old word\n");
    dir.write("old.txt", "\u{feff}old");
    dir.write("new.txt", "\u{feff}new");
    dir.write("swap.edit", "\u{feff}replace-text $/p @old.txt @new.txt\n");
    let output = run(&dir, &["edit", "doc.md", "--script", "swap.edit"]);
    assert!(output.status.success(), "{}", common::stderr(&output));
    assert_eq!(
        fs::read(dir.path().join("doc.md")).unwrap(),
        b"\xef\xbb\xbfnew word\n"
    );

    dir.write("old.txt", "\u{feff}new");
    dir.write("-", "\u{feff}other");
    let output = run(
        &dir,
        &[
            "edit",
            "doc.md",
            "replace-text",
            "$/p",
            "--old-file",
            "old.txt",
            "--payload-file",
            "-",
        ],
    );
    assert!(output.status.success(), "{}", common::stderr(&output));
    assert_eq!(
        fs::read(dir.path().join("doc.md")).unwrap(),
        b"\xef\xbb\xbfother word\n"
    );
}

#[test]
fn edit_refusals_locate_the_instruction_and_distinguish_payload_from_structure() {
    let dir = TempDir::new("edit-instruction");
    dir.write("doc.md", "lead\n\n- one\n");
    let args = ["edit", "doc.md", "--script", "batch", "--format", "json"];
    for (prefix, payload, reason) in [
        ("replace $/p heading\n", "# heading", "payload-form"),
        ("# comment\n", "- another", "candidate-structure"),
    ] {
        dir.write(
            "batch",
            format!("{prefix}\ninsert-after $/list \"{payload}\"\n"),
        );
        let output = run(&dir, &args);
        let error = &common::json_output(&output)["error"];
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(error["id"], "invalid-payload");
        assert_eq!(error["line"], 3);
        assert_eq!(error["path"], "$/list[0]");
        assert_eq!(error["reason"], reason);
    }
    dir.write(
        "batch",
        "replace $/list \"- new\"\n\nappend-item $/list x\n",
    );
    let output = run(&dir, &args);
    let error = &common::json_output(&output)["error"];
    assert_eq!(error["id"], "conflicting-edits");
    assert_eq!(error["line"], 3);
    assert_eq!(error["other_line"], 1);
    assert_eq!(error["path"], "$/list[0]");
    assert_eq!(
        fs::read(dir.path().join("doc.md")).unwrap(),
        b"lead\n\n- one\n"
    );
}

#[test]
fn edit_refuses_read_only_and_hard_linked_destinations() {
    let dir = TempDir::new("edit-ineligible-destination");
    dir.write("doc.md", "old\n");
    let path = dir.path().join("doc.md");
    let original = fs::metadata(&path).unwrap().permissions();
    let mut read_only = original.clone();
    read_only.set_readonly(true);
    fs::set_permissions(&path, read_only).unwrap();
    let args = [
        "edit",
        "doc.md",
        "replace",
        "$/p",
        "--payload",
        "new",
        "--force",
    ];
    let output = run(&dir, &args);
    fs::set_permissions(&path, original).unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(common::stderr(&output).contains("read-only"));
    assert_eq!(fs::read(&path).unwrap(), b"old\n");

    let alias = dir.path().join("alias.md");
    fs::hard_link(&path, &alias).unwrap();
    for options in [&args[..], &[&args[..], &["--dry-run"]].concat()] {
        let output = run(&dir, options);
        assert_eq!(output.status.code(), Some(2));
        assert!(common::stderr(&output).contains("hard link"));
    }
    assert_eq!(fs::read(&path).unwrap(), b"old\n");
    assert_eq!(fs::read(&alias).unwrap(), b"old\n");
}

#[cfg(unix)]
#[test]
fn edit_replaces_the_inode_atomically_and_keeps_permissions() {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let dir = TempDir::new("edit-atomic");
    dir.write("doc.md", "old\n");
    let path = dir.path().join("doc.md");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let mut original = fs::File::open(&path).unwrap();
    let inode = original.metadata().unwrap().ino();
    let args = ["edit", "doc.md", "replace", "$/p", "--payload", "new"];
    let output = run(&dir, &args);
    assert!(output.status.success(), "{}", common::stderr(&output));
    assert_eq!(stdout(&output), "doc.md changed\n");
    let mut old_bytes = Vec::new();
    original.read_to_end(&mut old_bytes).unwrap();
    assert_eq!(old_bytes, b"old\n");
    let metadata = fs::metadata(&path).unwrap();
    assert_ne!(metadata.ino(), inode);
    assert_eq!(metadata.permissions().mode() & 0o777, 0o640);
    assert_eq!(fs::read(&path).unwrap(), b"new\n");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);

    let output = run(&dir, &[&args[..], &["--dry-run"]].concat());
    assert!(output.status.success());
    assert!(stdout(&output).is_empty());
    let output = run(&dir, &[&args[..], &["--format", "compact"]].concat());
    assert_eq!(stdout(&output), "doc.md\tunchanged\n");
    assert_eq!(fs::metadata(&path).unwrap().ino(), metadata.ino());
}

#[test]
fn edit_rejects_invalid_source_and_non_regular_destinations() {
    let dir = TempDir::new("edit-invalid-source");
    for (name, source, message) in [
        ("invalid.md", &b"\xff"[..], "UTF-8"),
        ("bare-cr.md", &b"old\r"[..], "carriage return"),
    ] {
        dir.write(name, source);
        let output = run(&dir, &["edit", name, "delete", "$/p"]);
        assert_eq!(output.status.code(), Some(2));
        assert!(common::stderr(&output).contains(message));
        assert_eq!(fs::read(dir.path().join(name)).unwrap(), source);
    }
    fs::create_dir(dir.path().join("directory")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("bare-cr.md", dir.path().join("symlink")).unwrap();
    for name in [
        "directory",
        #[cfg(unix)]
        "symlink",
    ] {
        let output = run(&dir, &["edit", name, "delete", "$/p"]);
        assert_eq!(output.status.code(), Some(2));
        assert!(common::stderr(&output).contains("regular file"));
    }
    assert!(run(&dir, &["edit", "missing", "--help"]).status.success());
}
