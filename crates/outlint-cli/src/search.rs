//! The `search` subcommand (behind the `search` feature): refreshes the
//! repository index under `.outlint/search/` for the files below the search
//! root, runs the query there, and prints the best-matching blocks.

use std::env;

use outlint_search::{render_hits, walk_markdown, Scope, Store};

use crate::{args::SearchOptions, write_stderr, write_stdout};

/// Exit 0 with at least one hit, 1 with none, 2 on a usage or operational
/// error.
pub(crate) fn execute_search(options: &SearchOptions) -> u8 {
    let rendered = match search(options) {
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

fn search(options: &SearchOptions) -> Result<String, String> {
    let current_dir = env::current_dir()
        .map_err(|error| format!("cannot determine the current directory: {error}"))?;
    let search_root = match &options.root {
        Some(root) => {
            let path = current_dir.join(root);
            if !path.is_dir() {
                return Err(format!("--root '{root}' is not a directory"));
            }
            path
        }
        None => current_dir,
    };
    let scope = Scope::locate(&search_root)?;
    let store = Store::open(&scope)?;
    let (files, notes) = walk_markdown(&scope);
    write_notes(&notes);
    write_notes(&store.refresh(&files)?);
    let hits = store.search(&options.words)?;
    Ok(render_hits(&hits))
}

fn write_notes(notes: &[String]) {
    for note in notes {
        write_stderr(&format!("outlint: {note}\n"));
    }
}
