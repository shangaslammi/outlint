#![cfg(feature = "search")]

mod common;

use common::*;
use std::{
    fs,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

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
        "docs/alpha.md $.deployment/p[0] 49B (section 64B)"
    );
    // A block hit is its header and one snippet line holding the match.
    assert!(stdout(&output).contains(
        "docs/alpha.md $.deployment/p[0] 49B (section 64B)\n  The rollback plan restores the previous release.\n\n"
    ));
    assert!(directory.path().join(".outlint/.gitignore").is_file());

    // A word found only in a heading hits the section, whose snippet is its
    // own first paragraph rather than the heading again.
    let output = run(&directory, &["search", "deployment"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains(
        "docs/alpha.md $.deployment 64B\n  The rollback plan restores the previous release.\n\n"
    ));
    assert!(!stdout(&output).contains("  ## Deployment"));

    // A rewrite with a different size is picked up on the next invocation.
    directory.write(
        "docs/alpha.md",
        "# Alpha\n\n## Operations\n\n### Rollback plan\n\nRestore the previous release, then verify.\n",
    );
    let output = run(&directory, &["search", "rollback", "plan"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains(
        "docs/alpha.md $.operations.rollback-plan 62B\n  Restore the previous release, then verify.\n\n"
    ));
    assert!(!text.contains("restores the previous release"));
    // A section with no content of its own prints no snippet line.
    let output = run(&directory, &["search", "operations"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("docs/alpha.md $.operations 77B\n\n"));

    // No conjunction hit explains each simple term using the same stemmed
    // matching as the search itself.
    let output = run(&directory, &["search", "rollbacks", "kumquats"]);
    assert_eq!(output.status.code(), Some(1), "stdout: {}", stdout(&output));
    assert_eq!(stdout(&output), "");
    assert_eq!(
        stderr(&output),
        "outlint: no block contains all of: rollbacks kumquats\n  \
         rollbacks 1, kumquats 1\n  \
         (a block must contain every word; drop or change the rarest words)\n"
    );

    // A deleted file disappears from the index; no hits is exit 1.
    fs::remove_file(directory.path().join("beta.md")).expect("fixture removable");
    let output = run(&directory, &["search", "kumquat"]);
    assert_eq!(output.status.code(), Some(1), "stdout: {}", stdout(&output));
    assert_eq!(stdout(&output), "");
    assert_eq!(
        stderr(&output),
        "outlint: no block contains all of: kumquat\n  kumquat 0\n  \
         (a block must contain every word; drop or change the rarest words)\n"
    );
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
    assert_eq!(headers(&output), ["other/b.md"]);
    #[cfg(feature = "read")]
    {
        let header = stdout(&output).lines().next().unwrap_or("");
        let mut fields = header.split_whitespace();
        let file = fields.next().unwrap_or("");
        let mdpath = fields.next().unwrap_or("");
        let read = run(&directory, &["read", file, mdpath]);
        assert_eq!(read.status.code(), Some(0), "stderr: {}", stderr(&read));
        assert!(stdout(&read).contains("Kumquat jam recipe."));
    }

    // Searching a parent from a subdirectory still prints paths usable from
    // that subdirectory: a local path for its own file and `..` for a sibling.
    let output = run_in(
        &directory.path().join("docs"),
        &["search", "--root", "..", "kumquat"],
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(headers(&output), ["../other/b.md", "a.md"]);
    let output = run(&directory, &["search", "--root", "missing", "kumquat"]);
    assert_eq!(output.status.code(), Some(2), "stdout: {}", stdout(&output));
    assert!(stderr(&output).contains("--root 'missing' is not a directory"));
}

#[test]
fn search_returns_item_paths_and_folds_lead_ins() {
    let directory = TempDir::new("search-units");
    fs::create_dir(directory.path().join(".git")).expect("fake repository marker");
    directory.write(
        "guide.md",
        "# Guide\n\n**Exit codes.**\n\n| Code | Meaning |\n| --- | --- |\n| 0 | success |\n\nSteps:\n\n- ordinary preparation\n- rare needle detail\n",
    );

    let output = run(&directory, &["search", "exit", "codes"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains(
            "guide.md $/p[1] 49B\n  Exit codes. | Code | Meaning | | --- | --- | | 0 | success"
        ),
        "stdout: {}",
        stdout(&output)
    );
    assert!(!stdout(&output).contains("$/p[0]"));

    let output = run(&directory, &["search", "rare", "needle"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("guide.md $/list[0]/item[1] 21B\n  rare needle detail\n"));
    assert!(!stdout(&output).contains("guide.md $/list[0] "));

    let output = run(&directory, &["search", "ordinary", "detail"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("guide.md $/list[0] 44B\n"));
    assert!(!stdout(&output).contains("/item["));

    let output = run(&directory, &["search", "needle", "absentword"]);
    assert_eq!(output.status.code(), Some(1), "stdout: {}", stdout(&output));
    assert!(
        stderr(&output).contains("needle 1, absentword 0"),
        "item text must not double-count its containing list: {}",
        stderr(&output)
    );
}

#[test]
fn a_matching_item_below_the_display_cut_still_suppresses_its_list() {
    let directory = TempDir::new("search-list-cut");
    fs::create_dir(directory.path().join(".git")).expect("fake repository marker");
    let mut source = "# Ranking\n\n".to_owned();
    for index in 0..10 {
        source.push_str(&format!(
            "Needle orchard needle orchard competitor {index}.\n\n"
        ));
    }
    source.push_str(
        "Needle orchard needle orchard needle orchard needle orchard needle orchard:\n\n\
         - needle orchard target\n\
         - unrelated filler\n",
    );
    directory.write("ranking.md", source);

    let output = run(&directory, &["search", "needle", "orchard"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let headers: Vec<_> = stdout(&output)
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with("  "))
        .collect();
    assert_eq!(headers.len(), 10, "stdout: {}", stdout(&output));
    assert!(
        headers.iter().all(|header| header.contains("/p[")),
        "the high-scoring list must be suppressed even when its narrower item falls below the ten displayed hits: {}",
        stdout(&output)
    );
}

#[test]
fn search_notices_a_same_size_rewrite_within_one_second() {
    let directory = TempDir::new("search-mtime");
    fs::create_dir(directory.path().join(".git")).expect("fake repository marker");
    let set_modified = |relative: &str, time: SystemTime| {
        fs::File::options()
            .write(true)
            .open(directory.path().join(relative))
            .expect("fixture opens for writing")
            .set_modified(time)
            .expect("fixture mtime is settable");
    };
    let base = UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    directory.write("note.md", "# Note\n\nKumquat.\n");
    set_modified("note.md", base);
    let output = run(&directory, &["search", "kumquat"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));

    // Same length, same second, different content: still picked up.
    directory.write("note.md", "# Note\n\nLoquats.\n");
    set_modified("note.md", base + Duration::from_millis(100));
    let output = run(&directory, &["search", "loquats"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("  Loquats.\n"));
}
