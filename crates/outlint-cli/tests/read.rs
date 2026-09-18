#![cfg(feature = "read")]

mod common;

use common::*;

const FIXTURE: &str = "---\ntitle: Guide\n---\n\nRoot preamble paragraph.\n\n# Guide\n\nGuide intro.\n\n## Setup\n\nInstall the tool first.\n\n- first item\n- second item\n- third item\n\n```sh\noutlint check README.md\n```\n\n## FAQ\n\n### Question\n\nWhy?\n\n### Question\n\nHow?\n";

#[test]
fn read_prints_nodes_lists_trees_and_reports_path_errors() {
    let directory = TempDir::new("read");
    directory.write("guide.md", FIXTURE);

    let output = run(&directory, &["read", "guide.md", "$.guide.setup"]);
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

    let output = run(&directory, &["read", "guide.md", ".guide.setp"]);
    assert_eq!(output.status.code(), Some(1), "stdout: {}", stdout(&output));
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("cannot resolve"));

    let output = run(&directory, &["read", "--blocks", "guide.md"]);
    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
}
