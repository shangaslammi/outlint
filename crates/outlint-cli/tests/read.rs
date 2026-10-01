#![cfg(feature = "read")]

mod common;

use common::*;

#[cfg(unix)]
use std::{ffi::OsString, os::unix::ffi::OsStringExt};

const FIXTURE: &str = "---\ntitle: Guide\n---\n\nRoot preamble paragraph.\n\n# Guide\n\nGuide intro.\n\n## Setup\n\nInstall the tool first.\n\n- first item\n- second item\n- third item\n\n```sh\noutlint check README.md\n```\n\n## FAQ\n\n### Question\n\nWhy?\n\n### Question\n\nHow?\n";

#[test]
fn read_prints_nodes_lists_trees_and_reports_path_errors() {
    let directory = TempDir::new("read");
    directory.write("guide.md", FIXTURE);

    let output = run(&directory, &["read", "guide.md", "$.setup"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "## Setup\n\nInstall the tool first.\n\n- first item\n- second item\n- third item\n\n```sh\noutlint check README.md\n```\n"
    );

    let output = run(&directory, &["read", "--tree", "guide.md"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).starts_with("$ "),
        "stdout: {}",
        stdout(&output)
    );

    let output = run(&directory, &["read", "guide.md", ".setp"]);
    assert_eq!(output.status.code(), Some(1), "stdout: {}", stdout(&output));
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("cannot resolve"));

    let output = run(&directory, &["read", "--blocks", "guide.md"]);
    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
}

#[test]
fn read_supports_json_compact_and_structured_path_errors() {
    let directory = TempDir::new("read-formats");
    directory.write("docs/guide.md", FIXTURE);

    let output = run(
        &directory,
        &[
            "read",
            "--format",
            "json",
            "docs/guide.md",
            "$..question[1]",
        ],
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let value = json_output(&output);
    assert_eq!(value["version"], 1);
    assert_eq!(value["file"], "docs/guide.md");
    assert_eq!(value["mdpath"], "$.faq.question[1]");
    assert_eq!(
        value["bytes"].as_u64(),
        Some(value["content"].as_str().unwrap().len() as u64)
    );

    let output = run(
        &directory,
        &[
            "read",
            "--tree",
            "--blocks",
            "--format",
            "json",
            "docs/guide.md",
            "$.setup[0]",
        ],
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let value = json_output(&output);
    assert_eq!(value["mdpath"], "$.setup");
    assert_eq!(value["nodes"][0]["kind"], "section");
    assert_eq!(value["nodes"][0]["level"], 2);
    assert_eq!(value["nodes"][1]["kind"], "p");
    assert_eq!(value["nodes"][1]["preview"], "Install the tool first.");
    assert_eq!(value["nodes"][2]["items"], 3);

    let output = run(
        &directory,
        &[
            "read",
            "--tree",
            "--blocks",
            "--format",
            "compact",
            "docs/guide.md",
            "$.setup",
        ],
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output).contains("$.setup/list[0]/item[1]\t14B\titem: second item\n"));
    assert!(!stdout(&output).contains("  "));

    let output = run(
        &directory,
        &["read", "--format", "json", "docs/guide.md", ".setp"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), "");
    let value = json_output(&output);
    assert_eq!(value["error"]["kind"], "unresolved");
    assert_eq!(value["error"]["path"], "$.setp");
    assert_eq!(value["error"]["resolved"], "$");
    assert_eq!(value["error"]["nodes"][1]["mdpath"], "$.setup");

    let output = run(
        &directory,
        &["read", "--format", "json", "docs/guide.md", ".Setup"],
    );
    assert_eq!(output.status.code(), Some(1));
    let value = json_output(&output);
    assert_eq!(value["error"]["kind"], "syntax");
    assert_eq!(value["error"]["path"], "$.Setup");
    assert!(value["error"]["message"].is_string());
    assert!(value["error"]["offset"].is_u64());

    let output = run(
        &directory,
        &[
            "read",
            "--format",
            "json",
            "docs/guide.md",
            "$.faq.question",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let value = json_output(&output);
    assert_eq!(value["error"]["kind"], "ambiguous");
    assert_eq!(value["error"]["resolved"], "$.faq");
    assert_eq!(
        value["error"]["candidates"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(
        value["error"]["candidates"][0]["mdpath"],
        "$.faq.question[0]"
    );

    let output = run(
        &directory,
        &[
            "read",
            "--format",
            "compact",
            "docs/guide.md",
            "$.faq.question",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("$.faq.question[0]\t19B\tQuestion\n"));

    let output = run_with_format_env(
        &directory,
        &["read", "--tree", "docs/guide.md", "$.setup"],
        "compact",
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output).starts_with("$.setup\t109B\tSetup\n"));

    let output = run_with_format_env(
        &directory,
        &[
            "read",
            "--tree",
            "--format",
            "human",
            "docs/guide.md",
            "$.setup",
        ],
        "bogus",
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output).starts_with("$.setup  109B  Setup\n"));

    let output = run_with_format_env(
        &directory,
        &["read", "--tree", "docs/guide.md", "$.setup"],
        "",
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output).starts_with("$.setup  109B  Setup\n"));
    assert!(!stdout(&output).contains('\t'));

    let output = run_with_format_env(&directory, &["read", "docs/guide.md"], "bogus");
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("invalid OUTLINT_FORMAT value 'bogus'"));
}

#[cfg(unix)]
#[test]
fn read_rejects_a_non_unicode_environment_format() {
    let directory = TempDir::new("read-non-unicode-format");
    directory.write("docs/guide.md", FIXTURE);

    let output = run_with_format_env(
        &directory,
        &["read", "docs/guide.md"],
        OsString::from_vec(vec![0xff]),
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("invalid OUTLINT_FORMAT value"));
}

#[test]
fn human_tree_preserves_empty_item_labels() {
    let directory = TempDir::new("read-empty-item-labels");
    directory.write(
        "docs/guide.md",
        "# Guide\n\n## Items\n\n-\n- [](/destination)\n",
    );

    let output = run(
        &directory,
        &["read", "--tree", "--blocks", "docs/guide.md", "$.items"],
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "$.items                  31B  Items\n\
         $.items/list[0]          21B  list (2 items)\n\
         $.items/list[0]/item[0]  2B  item: \n\
         $.items/list[0]/item[1]  19B  item: \n"
    );
}

#[test]
fn list_and_item_tree_bases_have_stable_block_membership() {
    let directory = TempDir::new("read-list-item-trees");
    directory.write("docs/guide.md", FIXTURE);

    for (extra, expected) in [
        (&[][..], "$.setup/list[0]\t40B\tlist (3 items)\n"),
        (
            &["--blocks"][..],
            "$.setup/list[0]\t40B\tlist (3 items)\n\
             $.setup/list[0]/item[0]\t13B\titem: first item\n\
             $.setup/list[0]/item[1]\t14B\titem: second item\n\
             $.setup/list[0]/item[2]\t13B\titem: third item\n",
        ),
    ] {
        let mut arguments = vec!["read", "--tree", "--format", "compact"];
        arguments.extend_from_slice(extra);
        arguments.extend_from_slice(&["docs/guide.md", "$.setup/list[0]"]);
        let output = run(&directory, &arguments);
        assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
        assert_eq!(stdout(&output), expected);
    }

    let expected = "$.setup/list[0]/item[1]\t14B\titem: second item\n";
    for extra in [&[][..], &["--blocks"][..]] {
        let mut arguments = vec!["read", "--tree", "--format", "compact"];
        arguments.extend_from_slice(extra);
        arguments.extend_from_slice(&["docs/guide.md", "$.setup/list[0]/item[1]"]);
        let output = run(&directory, &arguments);
        assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
        assert_eq!(stdout(&output), expected);
    }
}

#[test]
fn read_rejects_non_document_terminals_and_impossible_block_chains() {
    let directory = TempDir::new("read-invalid-terminals");
    directory.write("docs/guide.md", FIXTURE);

    for path in [
        "$/table[0]",
        "$/row[0]",
        "$/cell[0]",
        "$/col[0]",
        "$/p[0]/p[0]",
        "$/list[0]/list[0]",
        "$/p[0]/list[0]/item[0]",
        "$.setup/list[0]/item[0]/p[0]",
        "$.setup/list[0]/item[0]/list[0]/item[0]",
    ] {
        let output = run(
            &directory,
            &["read", "--format", "json", "docs/guide.md", path],
        );
        assert_eq!(output.status.code(), Some(1), "stderr: {}", stderr(&output));
        assert_eq!(json_output(&output)["error"]["kind"], "syntax", "{path}");
    }
}

#[test]
fn resolution_errors_keep_the_deepest_canonical_prefix() {
    let directory = TempDir::new("read-resolution-prefixes");
    directory.write("docs/guide.md", FIXTURE);

    for (path, step, resolved, nodes) in [
        (
            "$.setup/list[1]",
            "list[1]",
            "$.setup",
            vec![
                "$.setup",
                "$.setup/p[0]",
                "$.setup/list[0]",
                "$.setup/list[0]/item[0]",
                "$.setup/list[0]/item[1]",
                "$.setup/list[0]/item[2]",
                "$.setup/code[0]",
            ],
        ),
        (
            "$.setup/list[0]/item[3]",
            "item[3]",
            "$.setup/list[0]",
            vec![
                "$.setup/list[0]",
                "$.setup/list[0]/item[0]",
                "$.setup/list[0]/item[1]",
                "$.setup/list[0]/item[2]",
            ],
        ),
        (
            "$..faq.missing",
            "missing",
            "$.faq",
            vec!["$.faq", "$.faq.question[0]", "$.faq.question[1]"],
        ),
    ] {
        let output = run(
            &directory,
            &["read", "--format", "json", "docs/guide.md", path],
        );
        assert_eq!(output.status.code(), Some(1), "stderr: {}", stderr(&output));
        let value = json_output(&output);
        assert_eq!(value["error"]["kind"], "unresolved", "{path}");
        assert_eq!(value["error"]["step"], step, "{path}");
        assert_eq!(value["error"]["resolved"], resolved, "{path}");
        let actual: Vec<&str> = value["error"]["nodes"]
            .as_array()
            .expect("error nodes are an array")
            .iter()
            .filter_map(|node| node["mdpath"].as_str())
            .collect();
        assert_eq!(actual, nodes, "{path}");
    }
}

#[test]
fn positional_duplicate_and_merged_root_paths_render_canonically() {
    const PATHS: &str = "\
Root before.

# Guide

Root after.

## 🎉

Celebration.

## Notes

First.

## Notes

Second.
";
    let directory = TempDir::new("read-canonical-paths");
    directory.write("docs/paths.md", PATHS);

    let output = run(
        &directory,
        &[
            "read",
            "--tree",
            "--blocks",
            "--format",
            "json",
            "docs/paths.md",
            "$",
        ],
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let value = json_output(&output);
    let actual: Vec<&str> = value["nodes"]
        .as_array()
        .expect("tree nodes are an array")
        .iter()
        .filter_map(|node| node["mdpath"].as_str())
        .collect();
    assert_eq!(
        actual,
        [
            "$",
            "$/p[0]",
            "$/p[1]",
            "$.[0]",
            "$.[0]/p[0]",
            "$.notes[0]",
            "$.notes[0]/p[0]",
            "$.notes[1]",
            "$.notes[1]/p[0]",
        ]
    );

    let output = run(
        &directory,
        &["read", "--format", "json", "docs/paths.md", "$.[0]"],
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(json_output(&output)["mdpath"], "$.[0]");

    for path in ["$.notes", "$..notes"] {
        let output = run(
            &directory,
            &["read", "--format", "json", "docs/paths.md", path],
        );
        assert_eq!(output.status.code(), Some(1), "stderr: {}", stderr(&output));
        let value = json_output(&output);
        assert_eq!(value["error"]["kind"], "ambiguous", "{path}");
        assert_eq!(value["error"]["resolved"], "$", "{path}");
        let candidates: Vec<&str> = value["error"]["candidates"]
            .as_array()
            .expect("candidates are an array")
            .iter()
            .filter_map(|node| node["mdpath"].as_str())
            .collect();
        assert_eq!(candidates, ["$.notes[0]", "$.notes[1]"], "{path}");
    }
}
