//! The `search` subcommand (behind the `search` feature): refreshes the
//! workspace index under `.outlint/search/`, runs the query, and prints the
//! best-matching blocks.

use std::env;

use outlint_search::{render_hits, walk_markdown, workspace_root, Store};

use crate::{write_stderr, write_stdout};

/// Exit 0 with at least one hit, 1 with none, 2 on an operational error.
pub(crate) fn execute_search(words: &str) -> u8 {
    let rendered = match search(words) {
        Ok(rendered) => rendered,
        Err(message) => {
            write_stderr(&format!("outlint: {message}\n"));
            return 2;
        }
    };
    if rendered.is_empty() {
        1
    } else {
        write_stdout(&rendered)
    }
}

fn search(words: &str) -> Result<String, String> {
    let current_dir = env::current_dir()
        .map_err(|error| format!("cannot determine the current directory: {error}"))?;
    let root = workspace_root(&current_dir);
    let store = Store::open(&root)?;
    let (files, notes) = walk_markdown(&root);
    write_notes(&notes);
    write_notes(&store.refresh(&files)?);
    let hits = store.search(words)?;
    Ok(render_hits(&hits))
}

fn write_notes(notes: &[String]) {
    for note in notes {
        write_stderr(&format!("outlint: {note}\n"));
    }
}
