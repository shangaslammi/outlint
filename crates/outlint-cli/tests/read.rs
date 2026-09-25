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
