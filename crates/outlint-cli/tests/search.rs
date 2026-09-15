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
    assert_eq!(first_line, "docs/alpha.md $.alpha.deployment/p[0] L5");
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
    assert!(text.contains("docs/alpha.md $.alpha.operations.rollback-plan L5"));
    assert!(!text.contains("restores the previous release"));

    // A deleted file disappears from the index; no hits is exit 1.
    fs::remove_file(directory.path().join("beta.md")).expect("fixture removable");
    let output = run(&directory, &["search", "kumquat"]);
    assert_eq!(output.status.code(), Some(1), "stdout: {}", stdout(&output));
    assert_eq!(stdout(&output), "");
}
