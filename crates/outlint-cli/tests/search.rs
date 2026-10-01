#![cfg(feature = "search")]

mod common;

use common::*;
use serde_json::json;
use std::{
    fs,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::{ffi::OsString, os::unix::ffi::OsStringExt};

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

    let output = run(
        &directory,
        &["search", "--format", "json", "rare", "needle"],
    );
    assert_eq!(output.status.code(), Some(0));
    let value = json_output(&output);
    assert_eq!(value["total"], 1, "the matching item replaces its list");
    assert_eq!(value["hits"][0]["mdpath"], "$/list[0]/item[1]");

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
fn search_preserves_canonical_paths_for_merged_and_repeated_sections() {
    let directory = TempDir::new("search-canonical-paths");
    fs::create_dir(directory.path().join(".git")).expect("fake repository marker");
    directory.write(
        "docs/paths.md",
        "Root apricot.\n\n# Guide\n\nRoot banana.\n\n## 🎉\n\nCedar content.\n\n## Notes\n\nDahlia content.\n\n## Notes\n\nElm content.\n",
    );

    for (term, expected) in [
        ("apricot", "$/p[0]"),
        ("banana", "$/p[1]"),
        ("cedar", "$.[0]/p[0]"),
        ("dahlia", "$.notes[0]/p[0]"),
        ("elm", "$.notes[1]/p[0]"),
    ] {
        let output = run(&directory, &["search", "--format", "json", term]);
        assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
        let value = json_output(&output);
        assert_eq!(value["hits"][0]["path"], "docs/paths.md", "{term}");
        assert_eq!(value["hits"][0]["mdpath"], expected, "{term}");
    }
}

#[test]
fn matching_items_replace_only_their_list_and_share_the_exact_total() {
    let directory = TempDir::new("search-list-items");
    fs::create_dir(directory.path().join(".git")).expect("fake repository marker");
    directory.write(
        "items.md",
        "# Items\n\n- needle first\n- needle second\n\nSeparator.\n\n- ordinary preparation\n- rare detail\n",
    );

    let output = run(&directory, &["search", "--format", "json", "needle"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let value = json_output(&output);
    assert_eq!(value["total"], 2);
    let paths: Vec<_> = value["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .filter_map(|hit| hit["mdpath"].as_str())
        .collect();
    assert_eq!(
        paths,
        ["$/list[0]/item[0]", "$/list[0]/item[1]"],
        "both matching items remain and their aggregate list is absent"
    );
    assert!(value["total"].as_u64().unwrap_or(0) >= paths.len() as u64);

    let output = run(
        &directory,
        &["search", "--format", "json", "ordinary", "detail"],
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let value = json_output(&output);
    assert_eq!(value["total"], 1);
    assert_eq!(value["hits"][0]["mdpath"], "$/list[1]");
}

#[test]
fn aggregate_lists_cannot_crowd_better_final_hits_out_of_the_limit() {
    let directory = TempDir::new("search-list-cut");
    fs::create_dir(directory.path().join(".git")).expect("fake repository marker");
    let mut source = "# Ranking\n\n".to_owned();
    for index in 0..10 {
        source.push_str(&format!(
            "Needle orchard needle orchard competitor {index}.\n\n"
        ));
    }
    for index in 0..40 {
        source.push_str(&format!(
            "Cluster {index}:\n\n\
             - needle orchard target {index}\n\
             - needle needle needle needle needle\n\
             - orchard orchard orchard orchard orchard\n\n"
        ));
    }
    directory.write("ranking.md", source);

    let output = run(
        &directory,
        &["search", "--format", "json", "needle", "orchard"],
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let value = json_output(&output);
    assert_eq!(value["total"], 50);
    let paths: Vec<_> = value["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .filter_map(|hit| hit["mdpath"].as_str())
        .collect();
    assert_eq!(paths.len(), 10);
    assert!(
        paths.iter().all(|path| path.contains("/p[")),
        "forty high-scoring aggregate lists must not crowd the better paragraphs out after those lists are excluded: {paths:?}"
    );
}

#[test]
fn search_limit_and_total_render_in_every_format() {
    let directory = TempDir::new("search-limit");
    fs::create_dir(directory.path().join(".git")).expect("fake repository marker");
    let mut source = "# Entries\n\n".to_owned();
    for index in 0..12 {
        source.push_str(&format!("Needle entry {index}.\n\n"));
    }
    directory.write("entries.md", source);

    let output = run(&directory, &["search", "--limit", "2", "needle"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(stdout(&output).ends_with("(2 of 12 matching blocks; use --limit to see more)\n"));

    let output = run(
        &directory,
        &["search", "--format", "json", "--limit", "4", "needle"],
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(stderr(&output), "");
    let value = json_output(&output);
    assert_eq!(value["total"], 12);
    assert_eq!(value["hits"].as_array().map(Vec::len), Some(4));
    assert!(value["total"].as_u64().unwrap_or(0) >= 4);
    let rendered = stdout(&output);
    assert!(
        rendered.find("\"query\":").expect("query member")
            < rendered.find("\"total\":").expect("total member")
    );

    let output = run(
        &directory,
        &["search", "--format", "compact", "--limit", "3", "needle"],
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout(&output).lines().count(), 3);
    assert_eq!(
        stderr(&output),
        "(3 of 12 matching blocks; use --limit to see more)\n"
    );

    let output = run(&directory, &["search", "--limit", "12", "needle"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(
        stdout(&output)
            .lines()
            .filter(|line| line.starts_with("entries.md "))
            .count(),
        12
    );
    assert!(!stdout(&output).contains("use --limit"));

    for value in ["0", "1001", "18446744073709551615", "many"] {
        let output = run(&directory, &["search", "--limit", value, "needle"]);
        assert_eq!(output.status.code(), Some(2), "{value}");
        assert_eq!(stdout(&output), "");
        assert!(stderr(&output).contains("expected an integer from 1 to 1000"));
    }

    let output = run(&directory, &["search", "--help"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output).contains("--limit <N>             Print 1..=1000 hits"));
}

#[test]
fn search_reports_index_creation_and_rebuilds_only_when_they_happen() {
    let directory = TempDir::new("search-index-notes");
    fs::create_dir(directory.path().join(".git")).expect("fake repository marker");
    directory.write("guide.md", "# Guide\n\nNeedle.\n");
    let index = directory.path().join(".outlint/search");

    let output = run(&directory, &["search", "needle"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        stderr(&output),
        format!("outlint: created search index at {}\n", index.display())
    );

    let output = run(&directory, &["search", "needle"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stderr(&output), "");

    directory.write(
        ".outlint/search/outlint-index.json",
        "{\"format\": 1, \"outlint\": \"0.1.0\"}\n",
    );
    let output = run(&directory, &["search", "needle"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        stderr(&output),
        format!(
            "outlint: rebuilt search index at {} (format changed)\n",
            index.display()
        )
    );

    let marker = fs::read_to_string(index.join("outlint-index.json")).expect("marker readable");
    let old_version = marker.replace(env!("CARGO_PKG_VERSION"), "0.0.0");
    directory.write(".outlint/search/outlint-index.json", &old_version);
    let output = run(&directory, &["search", "needle"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        stderr(&output),
        format!(
            "outlint: rebuilt search index at {} (outlint version changed)\n",
            index.display()
        )
    );

    directory.write(".outlint/search/meta.json", "not an index");
    let output = run(&directory, &["search", "needle"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        stderr(&output),
        format!(
            "outlint: rebuilt search index at {} (index could not be opened)\n",
            index.display()
        )
    );
}

#[test]
fn search_snippets_keep_inline_code_and_adjacent_punctuation() {
    let directory = TempDir::new("search-snippet-fidelity");
    fs::create_dir(directory.path().join(".git")).expect("fake repository marker");
    directory.write(
        "guide.md",
        "# Guide\n\nUse micro*service*, inter**operate**, and `conflicting-frontmatter`. Then continue.\n",
    );

    let output = run(&directory, &["search", "frontmatter"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains(
            "  Use microservice, interoperate, and `conflicting-frontmatter`. Then continue.\n"
        ),
        "stdout: {}",
        stdout(&output)
    );

    for words in [["micro", "service"], ["inter", "operate"]] {
        let output = run(&directory, &["search", words[0], words[1]]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{words:?}: {}",
            stderr(&output)
        );
        assert!(stdout(&output).contains("guide.md $/p[0]"));
    }
    for word in ["microservice", "interoperate"] {
        let output = run(&directory, &["search", word]);
        assert_eq!(output.status.code(), Some(1), "{word}: {}", stdout(&output));
    }
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

#[test]
fn search_supports_json_compact_and_environment_defaults() {
    let directory = TempDir::new("search-formats");
    fs::create_dir(directory.path().join(".git")).expect("fake repository marker");
    directory.write(
        "docs/guide.md",
        "# Guide\n\n## Setup\n\nExit codes explain command outcomes.\n",
    );

    let output = run(&directory, &["search", "--format", "json", "exit", "codes"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let value = json_output(&output);
    assert_eq!(value["version"], 1);
    assert_eq!(value["query"], "exit codes");
    assert_eq!(value["hits"][0]["path"], "docs/guide.md");
    assert_eq!(value["hits"][0]["mdpath"], "$.setup/p[0]");
    assert!(value["hits"][0]["bytes"].is_u64());
    assert!(value["hits"][0]["section_bytes"].is_u64());
    assert!(value["hits"][0]["score"].is_number());
    assert!(value.get("term_counts").is_none());

    let output = run(
        &directory,
        &["search", "--format", "compact", "exit", "codes"],
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let fields: Vec<_> = stdout(&output).trim_end().split('\t').collect();
    assert_eq!(fields.len(), 4);
    assert_eq!(fields[0], "docs/guide.md");
    assert_eq!(fields[1], "$.setup/p[0]");
    assert!(fields[2].contains('/'));
    assert_eq!(fields[3], "Exit codes explain command outcomes.");

    let output = run(
        &directory,
        &["search", "--format", "json", "exit", "missing"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), "");
    let value = json_output(&output);
    assert_eq!(value["hits"], json!([]));
    assert_eq!(value["term_counts"][0]["term"], "exit");
    assert_eq!(value["term_counts"][1]["term"], "missing");

    let output = run(
        &directory,
        &["search", "--format", "compact", "exit", "missing"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).starts_with("outlint: no block contains all words; exit "));
    assert!(stderr(&output).contains(", missing 0\n"));

    let output = run(
        &directory,
        &["search", "--format", "json", "\"missing phrase\""],
    );
    assert_eq!(output.status.code(), Some(1));
    let value = json_output(&output);
    assert!(value["term_counts"].is_null());

    let output = run(
        &directory,
        &["search", "--format", "compact", "\"missing phrase\""],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), "outlint: no search hits\n");

    let output = run_with_format_env(&directory, &["search", "exit", "codes"], "compact");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout(&output).trim_end().split('\t').count(), 4);

    let output = run_with_format_env(
        &directory,
        &["search", "--format", "human", "exit", "codes"],
        "bogus",
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains(" (section "));

    let output = run_with_format_env(&directory, &["search", "exit", "codes"], "");
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains(" (section "));
    assert!(!stdout(&output).contains('\t'));

    let output = run_with_format_env(&directory, &["search", "exit", "codes"], "bogus");
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("invalid OUTLINT_FORMAT value 'bogus'"));
}

#[cfg(unix)]
#[test]
fn search_rejects_a_non_unicode_environment_format() {
    let directory = TempDir::new("search-non-unicode-format");
    fs::create_dir(directory.path().join(".git")).expect("fake repository marker");
    directory.write("docs/guide.md", "# Guide\n\nExit codes.\n");

    let output = run_with_format_env(
        &directory,
        &["search", "exit", "codes"],
        OsString::from_vec(vec![0xff]),
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("invalid OUTLINT_FORMAT value"));
}
