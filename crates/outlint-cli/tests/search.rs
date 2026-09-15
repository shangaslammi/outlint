#![cfg(feature = "search")]

mod common;

use common::*;
use std::fs;

#[test]
fn search_indexes_refreshes_and_forgets_workspace_markdown() {
    let directory = TempDir::new("search");
    fs::create_dir(directory.path().join(".git")).expect("fake repository marker");
    directory.write(
        "docs/alpha.md",
        "# Alpha\n\n## Deployment\n\nThe rollback plan restores the previous release.\n",
    );
    directory.write(
        "beta.md",
        "# Beta\n\nA glossary of kumquat cultivation terms.\n",
    );

    let output = run(&directory, &["search", "rollback", "plan"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let first_line = stdout(&output).lines().next().unwrap_or("");
    assert_eq!(
        first_line,
        "docs/alpha.md $.alpha.deployment/p[0] 49B (section 64B)"
    );
    assert!(stdout(&output).contains("  The rollback plan restores"));
    assert!(directory.path().join(".outlint/.gitignore").is_file());

    // A rewrite with a different size is picked up on the next invocation.
    directory.write(
        "docs/alpha.md",
        "# Alpha\n\n## Operations\n\n### Rollback plan\n\nRestore the previous release, then verify.\n",
    );
    let output = run(&directory, &["search", "rollback", "plan"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("docs/alpha.md $.alpha.operations.rollback-plan 62B\n"));
    assert!(!text.contains("restores the previous release"));

    // A deleted file disappears from the index; no hits is exit 1.
    fs::remove_file(directory.path().join("beta.md")).expect("fixture removable");
    let output = run(&directory, &["search", "kumquat"]);
    assert_eq!(output.status.code(), Some(1), "stdout: {}", stdout(&output));
    assert_eq!(stdout(&output), "");
}

#[test]
fn search_scopes_to_the_search_root_and_shares_the_repository_index() {
    let directory = TempDir::new("search-root");
    fs::create_dir(directory.path().join(".git")).expect("fake repository marker");
    directory.write("docs/a.md", "# A\n\nKumquat orchards thrive here.\n");
    directory.write("other/b.md", "# B\n\nKumquat jam recipe.\n");
    // The paths of the hit headers, sorted: relevance order is presentation.
    let headers = |output: &std::process::Output| -> Vec<String> {
        let mut paths: Vec<String> = stdout(output)
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with("  "))
            .map(|line| line.split(' ').next().unwrap_or("").to_owned())
            .collect();
        paths.sort();
        paths
    };

    // From the repository root every file is searched.
    let output = run(&directory, &["search", "kumquat"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(headers(&output), ["docs/a.md", "other/b.md"]);

    // From a subdirectory only its files are searched, printed relative to
    // it, and the repository index is reused rather than a new one created.
    let output = run_in(&directory.path().join("docs"), &["search", "kumquat"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(headers(&output), ["a.md"]);
    assert!(directory.path().join(".outlint/search").is_dir());
    assert!(!directory.path().join("docs/.outlint").exists());

    // --root scopes the same way from anywhere; a missing root is a usage error.
    let output = run(&directory, &["search", "--root", "other", "kumquat"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(headers(&output), ["b.md"]);
    let output = run(&directory, &["search", "--root", "missing", "kumquat"]);
    assert_eq!(output.status.code(), Some(2), "stdout: {}", stdout(&output));
    assert!(stderr(&output).contains("--root 'missing' is not a directory"));
}
