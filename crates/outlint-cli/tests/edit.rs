#![cfg(feature = "edit")]
mod common;
use common::{run, stdout, TempDir};
use std::{fs, io::ErrorKind, process::Command};
#[test]
fn preview_apply_and_rejection() {
    let dir = TempDir::new("edit");
    dir.write("doc with spaces.md", "old");
    dir.write("payload", "new\n");
    let args = [
        "edit",
        "doc with spaces.md",
        "replace",
        "$/p",
        "--payload-file",
        "payload",
    ];
    let preview = run(&dir, &[&args[..], &["--dry-run"]].concat());
    assert!(preview.status.success());
    assert_eq!(
        stdout(&preview),
        concat!(
            "--- a/doc with spaces.md\t\n+++ b/doc with spaces.md\t\n@@ -1 +1 @@\n",
            "-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n"
        )
    );
    let file = dir.path().join("doc with spaces.md");
    assert_eq!(fs::read_to_string(&file).unwrap(), "old");
    assert!(run(&dir, &args).status.success());
    assert_eq!(
        run(
            &dir,
            &[
                "edit",
                "doc with spaces.md",
                "replace",
                "$/p",
                "--payload",
                "# heading"
            ]
        )
        .status
        .code(),
        Some(1)
    );
    assert_eq!(fs::read_to_string(file).unwrap(), "new");
}

#[test]
fn changelog_release_cut_preview_round_trips() {
    let dir = TempDir::new("changelog-cut-preview");
    let initial = include_str!("../../../CHANGELOG.md");
    dir.write("CHANGELOG.md", initial);
    dir.write(
        "CHANGELOG.outlint.yml",
        include_str!("../../../CHANGELOG.outlint.yml"),
    );
    dir.write(
        "cut.edit",
        r#"rename $.unreleased "[0.3.0] - 2026-12-01"
insert-section $.unreleased before "[Unreleased]"
replace-text $ <<OLD <<NEW
[Unreleased]: https://github.com/shangaslammi/outlint/compare/v0.2.0...HEAD
OLD
[Unreleased]: https://github.com/shangaslammi/outlint/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/shangaslammi/outlint/compare/v0.2.0...v0.3.0
NEW
"#,
    );
    let args = [
        "edit",
        "CHANGELOG.md",
        "--script",
        "cut.edit",
        "--format",
        "human",
    ];
    let output = run(&dir, &[&args[..], &["--dry-run"]].concat());
    assert!(output.status.success(), "{}", common::stderr(&output));
    let preview = stdout(&output);
    let old_start = initial.lines().count() - 5;
    let new_start = old_start + 2;
    assert_eq!(
        preview,
        format!(
            concat!(
                "--- a/CHANGELOG.md\n",
                "+++ b/CHANGELOG.md\n",
                "@@ -11,6 +11,8 @@\n",
                " \n",
                " ## [Unreleased]\n",
                " \n",
                "+## [0.3.0] - 2026-12-01\n",
                "+\n",
                " ### Added\n",
                " \n",
                " - **Experimental edit extension.** `outlint edit` previews and applies Markdown\n",
                "@@ -{},6 +{},7 @@\n",
                " - **Dual licensing** under [MIT](LICENSE-MIT) or\n",
                "   [Apache-2.0](LICENSE-APACHE), and a declared MSRV of Rust 1.86 tested in CI.\n",
                " \n",
                "-[Unreleased]: https://github.com/shangaslammi/outlint/compare/v0.2.0...HEAD\n",
                "+[Unreleased]: https://github.com/shangaslammi/outlint/compare/v0.3.0...HEAD\n",
                "+[0.3.0]: https://github.com/shangaslammi/outlint/compare/v0.2.0...v0.3.0\n",
                " [0.2.0]: https://github.com/shangaslammi/outlint/compare/v0.1.0...v0.2.0\n",
                " [0.1.0]: https://github.com/shangaslammi/outlint/releases/tag/v0.1.0\n",
            ),
            old_start, new_start,
        )
    );

    let file = dir.path().join("CHANGELOG.md");
    assert_eq!(fs::read(&file).unwrap(), initial.as_bytes());
    let applied = run(&dir, &args);
    assert!(applied.status.success(), "{}", common::stderr(&applied));
    let written = fs::read(&file).unwrap();

    dir.write("CHANGELOG.md", initial);
    dir.write("release.patch", preview);
    // Use Git's patch parser when installed. The fallback only needs to
    // interpret the two LF-terminated, nonempty ranges asserted above.
    let patched = match Command::new("git")
        .args(["apply", "--whitespace=nowarn", "release.patch"])
        .current_dir(dir.path())
        .output()
    {
        Ok(git) => {
            assert!(git.status.success(), "{}", common::stderr(&git));
            fs::read(&file).unwrap()
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            apply_release_hunks(initial, preview).into_bytes()
        }
        Err(error) => panic!("cannot run git apply: {error}"),
    };
    assert_eq!(patched, written);
}

fn apply_release_hunks(original: &str, preview: &str) -> String {
    let mut remaining = original.split_inclusive('\n');
    let mut consumed = 0;
    let mut patched = String::new();
    for record in preview.split_inclusive('\n').skip(2) {
        if let Some(range) = record.strip_prefix("@@ -") {
            let (start, _) = range.split_once(',').unwrap();
            let offset = start.parse::<usize>().unwrap() - 1;
            patched.extend(remaining.by_ref().take(offset - consumed));
            consumed = offset;
        } else {
            let (prefix, line) = record.split_at(1);
            if prefix != "+" {
                assert_eq!(remaining.next(), Some(line));
                consumed += 1;
            }
            if prefix != "-" {
                patched.push_str(line);
            }
        }
    }
    patched.extend(remaining);
    patched
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
        b"new\r\ncontinued\r\n> a\r\n"
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
        assert!(
            output.stderr.is_empty(),
            "{name}: {}",
            common::stderr(&output)
        );
        let value = common::json_output(&output);
        if case.join("expected.json").is_file() {
            let expected: serde_json::Value =
                serde_json::from_slice(&fs::read(case.join("expected.json")).unwrap()).unwrap();
            assert_eq!(output.status.code(), Some(1), "{name}: {value}");
            for field in ["id", "line", "other_line"] {
                if let Some(expected) = expected.get(field) {
                    assert_eq!(&value["error"][field], expected, "{name}: {value}");
                }
            }
            if let Some(expected) = expected.get("diagnostics") {
                let ids: Vec<_> = value["error"]["diagnostics"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|diagnostic| &diagnostic["id"])
                    .collect();
                assert_eq!(serde_json::json!(ids), *expected, "{name}: {value}");
            }
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
    assert_eq!(
        common::stderr(&output),
        "outlint: destination doc.md is read-only\n"
    );
    assert_eq!(fs::read(&path).unwrap(), b"old\n");

    let alias = dir.path().join("alias.md");
    fs::hard_link(&path, &alias).unwrap();
    for options in [&args[..], &[&args[..], &["--dry-run"]].concat()] {
        let output = run(&dir, options);
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(
            common::stderr(&output),
            "outlint: destination doc.md has more than one hard link\n"
        );
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

#[test]
fn schema_selection_flags_and_json_member_order() {
    let dir = TempDir::new("edit-schema-selection");
    dir.write("doc.md", "old\n");
    dir.write("doc.outlint.yml", "invalid schema");
    let args = ["edit", "doc.md", "replace", "$/p", "--payload", "new"];
    let ignored = run(
        &dir,
        &[
            &args[..],
            &[
                "--ignore-schema",
                "--dry-run",
                "--format",
                "compact",
                "--format",
                "json",
            ],
        ]
        .concat(),
    );
    assert!(ignored.status.success(), "{}", common::stderr(&ignored));
    assert!(stdout(&ignored).starts_with(
        "{\"version\":1,\"file\":\"doc.md\",\"result\":\"changed\",\"paths\":[\"$/p[0]\"],\"schema\":null,\"diff\":"
    ));
    let conflict = run(
        &dir,
        &[
            &args[..],
            &["--ignore-schema", "--schema", "doc.outlint.yml"],
        ]
        .concat(),
    );
    assert_eq!(conflict.status.code(), Some(2));
    assert!(common::stderr(&conflict).contains("choose --schema or --ignore-schema"));
}

fn with_input(dir: &TempDir, arguments: &[&str], input: &[u8]) -> std::process::Output {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_outlint"))
        .current_dir(dir.path())
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn flag_payloads_read_stdin_once() {
    let dir = TempDir::new("edit-stdin");
    dir.write("doc.md", "old\n");
    let result = with_input(
        &dir,
        &["edit", "doc.md", "replace", "$/p", "--payload", "-"],
        b"\xef\xbb\xbfnews\n",
    );
    assert!(result.status.success(), "{}", common::stderr(&result));
    let result = with_input(
        &dir,
        &[
            "edit",
            "doc.md",
            "replace-text",
            "$/p",
            "--old",
            "-",
            "--payload",
            "good",
        ],
        b"news",
    );
    assert!(result.status.success(), "{}", common::stderr(&result));
    let result = run(
        &dir,
        &[
            "edit",
            "doc.md",
            "replace-text",
            "$/p",
            "--old",
            "-",
            "--payload",
            "-",
        ],
    );
    assert_eq!(result.status.code(), Some(2));
    assert!(common::stderr(&result).contains("stdin already used"));
    assert_eq!(fs::read(dir.path().join("doc.md")).unwrap(), b"good\n");
}

#[test]
fn edit_options_reject_repetition_and_script_mixing() {
    let dir = TempDir::new("edit-exclusive");
    dir.write("doc.md", "old\n");
    dir.write("script", "replace $/p new\n");
    for (arguments, message) in [
        (
            &[
                "replace",
                "$/p",
                "--payload",
                "new",
                "--payload-file",
                "other",
            ][..],
            "repeated payload source",
        ),
        (
            &["--script", "script", "--force", "--force"],
            "repeated option --force",
        ),
        (
            &["--script", "script", "--script", "script"],
            "repeated option --script",
        ),
        (
            &["--script", "script", "delete", "$/p"],
            "--script cannot be combined",
        ),
        (
            &["--script", "script", "--old", "old"],
            "--script cannot be combined",
        ),
    ] {
        let result = run(&dir, &[&["edit", "doc.md"][..], arguments].concat());
        assert_eq!(result.status.code(), Some(2));
        assert!(stdout(&result).is_empty());
        assert!(common::stderr(&result).contains(message));
    }
    assert_eq!(fs::read(dir.path().join("doc.md")).unwrap(), b"old\n");
}

#[test]
fn invalid_targets_and_missing_text_have_distinct_refusals() {
    let dir = TempDir::new("edit-target-errors");
    dir.write("doc.md", "old\n");
    let invalid = run(
        &dir,
        &["edit", "doc.md", "rename", "$/p", "bad", "--format", "json"],
    );
    assert_eq!(invalid.status.code(), Some(1));
    assert_eq!(
        common::json_output(&invalid)["error"]["id"],
        "invalid-target"
    );
    assert!(stdout(&invalid).starts_with(
        "{\"version\":1,\"file\":\"doc.md\",\"error\":{\"id\":\"invalid-target\",\"message\":"
    ));
    let missing = run(
        &dir,
        &[
            "edit",
            "doc.md",
            "replace-text",
            "$/p",
            "--old",
            "else",
            "--payload",
            "new",
            "--format",
            "json",
        ],
    );
    assert_eq!(missing.status.code(), Some(1));
    assert_eq!(
        common::json_output(&missing)["error"]["id"],
        "text-not-found"
    );
    dir.write("script", "# comment\n\nrename $/p bad\n");
    let script = run(
        &dir,
        &["edit", "doc.md", "--script", "script", "--format", "human"],
    );
    assert_eq!(script.status.code(), Some(1));
    assert!(common::stderr(&script).starts_with("invalid-target\tline 3: $/p[0]: "));
    assert!(stdout(&script).is_empty());
}

#[test]
fn suppression_occurrences_are_reported_and_force_can_admit_them() {
    let dir = TempDir::new("edit-suppression-gate");
    let original = "old\n\n<!-- outlint-disable-file unexpected-section -->\n";
    dir.write("doc.md", original);
    dir.write("doc.outlint.yml", "version: 1\ntitle: null\n");
    let payload = "<!-- outlint-disable unexpected-block -->\nnew\n\n<!-- outlint-disable-file unexpected-section -->";
    for format in ["human", "compact", "json"] {
        for force in [false, true] {
            let mut args = vec![
                "edit",
                "doc.md",
                "append-block",
                "$",
                "--payload",
                payload,
                "--format",
                format,
                "--dry-run",
            ];
            if force {
                args.push("--force");
            }
            let result = run(&dir, &args);
            assert_eq!(
                result.status.code(),
                Some(if force { 0 } else { 1 }),
                "{}",
                common::stderr(&result)
            );
            if format == "json" {
                let value = common::json_output(&result);
                let gate = if force { &value } else { &value["error"] };
                assert_eq!(gate["diagnostic_source"], "candidate");
                let diagnostics = gate["diagnostics"].as_array().unwrap();
                assert_eq!(diagnostics.len(), 2);
                assert!(diagnostics
                    .iter()
                    .all(|d| d["id"] == "added-suppression" && d.get("schema_location").is_none()));
                assert!(result.stderr.is_empty());
            } else {
                let stderr = common::stderr(&result);
                assert_eq!(stderr.matches("added-suppression").count(), 2);
                assert!(stderr.contains("the edit adds an inline suppression comment"));
                assert_eq!(stderr.starts_with("new-diagnostics\t"), !force);
                assert!(stderr.contains(if format == "compact" {
                    "candidate\tdoc.md\t"
                } else {
                    " (candidate) added-suppression "
                }));
                assert_eq!(stdout(&result).starts_with("--- a/doc.md\n"), force);
            }
            assert_eq!(
                fs::read(dir.path().join("doc.md")).unwrap(),
                original.as_bytes()
            );
        }
    }
    let forced = run(
        &dir,
        &[
            "edit",
            "doc.md",
            "append-block",
            "$",
            "--payload",
            payload,
            "--force",
            "--format",
            "json",
        ],
    );
    assert!(forced.status.success());
    let written = fs::read_to_string(dir.path().join("doc.md")).unwrap();
    assert!(written.contains("<!-- outlint-disable unexpected-block -->\nnew\n"));
    assert_eq!(
        written
            .matches("outlint-disable-file unexpected-section")
            .count(),
        2
    );
}

#[test]
fn added_suppressions_are_allowed_when_the_schema_gate_is_skipped() {
    let dir = TempDir::new("edit-suppression-without-schema");
    let original = "old\n";
    let payload = "<!-- outlint-disable unexpected-block -->\nnew";
    let args = [
        "edit",
        "doc.md",
        "append-block",
        "$",
        "--payload",
        payload,
        "--format",
        "json",
    ];
    for ignore in [false, true] {
        dir.write("doc.md", original);
        let mut args = args.to_vec();
        if ignore {
            dir.write("doc.outlint.yml", "version: 1\ntitle: null\n");
            args.push("--ignore-schema");
        }
        let output = run(&dir, &args);
        assert!(output.status.success(), "{}", common::stderr(&output));
        let value = common::json_output(&output);
        assert_eq!(value["result"], "changed");
        assert!(value["schema"].is_null());
        assert!(value.get("diagnostics").is_none());
        assert_eq!(
            fs::read_to_string(dir.path().join("doc.md")).unwrap(),
            format!("{original}\n{payload}\n")
        );
    }
}
