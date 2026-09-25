//! The `search` subcommand (behind the `search` feature): refreshes the
//! repository index under `.outlint/search/` for the files below the search
//! root, runs the query there, and prints the best-matching blocks.

use std::{
    env,
    path::{Component, Path, PathBuf},
};

use outlint_search::{render_hits, render_no_hits, walk_markdown, Hit, Scope, Store};

use crate::{args::SearchOptions, write_stderr, write_stdout};

/// Exit 0 with at least one hit, 1 with none, 2 on a usage or operational
/// error.
pub(crate) fn execute_search(options: &SearchOptions) -> u8 {
    let result = match search(options) {
        Ok(result) => result,
        Err(message) => {
            write_stderr(&format!("outlint: {message}\n"));
            return 2;
        }
    };
    if result.hits.is_empty() {
        write_stderr(&result.no_hits);
        1
    } else {
        write_stdout(&result.hits)
    }
}

struct SearchResult {
    hits: String,
    no_hits: String,
}

fn search(options: &SearchOptions) -> Result<SearchResult, String> {
    let current_dir = env::current_dir()
        .map_err(|error| format!("cannot determine the current directory: {error}"))?;
    let current_dir = current_dir
        .canonicalize()
        .map_err(|error| format!("cannot resolve the current directory: {error}"))?;
    let search_root = match &options.root {
        Some(root) => {
            let path = current_dir.join(root);
            if !path.is_dir() {
                return Err(format!("--root '{root}' is not a directory"));
            }
            path
        }
        None => current_dir.clone(),
    };
    let scope = Scope::locate(&search_root)?;
    let store = Store::open(&scope)?;
    let walk = walk_markdown(&scope);
    write_notes(&walk.notes);
    write_notes(&store.refresh(&walk)?);
    let mut hits = store.search(&options.words)?;
    make_paths_relative(&mut hits, &current_dir, scope.search_root())?;
    let no_hits = if hits.is_empty() {
        let counts = store.term_counts(&options.words)?;
        render_no_hits(&options.words, counts.as_deref())
    } else {
        String::new()
    };
    Ok(SearchResult {
        hits: render_hits(&hits),
        no_hits,
    })
}

/// Rebases search-root-relative hit paths onto the invocation directory. Both
/// base directories are resolved by the IO shell before this pure transform.
fn make_paths_relative(
    hits: &mut [Hit],
    current_dir: &Path,
    search_root: &Path,
) -> Result<(), String> {
    for hit in hits {
        hit.path = output_path(current_dir, search_root, &hit.path).ok_or_else(|| {
            format!(
                "cannot express search hit '{}' relative to the current directory",
                hit.path
            )
        })?;
    }
    Ok(())
}

fn output_path(current_dir: &Path, search_root: &Path, indexed_path: &str) -> Option<String> {
    let indexed_path = Path::new(indexed_path);
    if indexed_path.is_absolute() {
        return None;
    }
    let target = search_root.join(indexed_path);
    relative_path(current_dir, &target)
        .and_then(|path| slash_path(&path))
        .or_else(|| target.to_str().map(|path| path.replace('\\', "/")))
}

fn relative_path(base: &Path, target: &Path) -> Option<PathBuf> {
    let base: Vec<_> = base.components().collect();
    let target: Vec<_> = target.components().collect();
    let common = base
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return None;
    }
    let mut relative = PathBuf::new();
    for component in &base[common..] {
        match component {
            Component::Normal(_) => relative.push(".."),
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => return None,
        }
    }
    for component in &target[common..] {
        match component {
            Component::Normal(component) => relative.push(component),
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => return None,
        }
    }
    Some(relative)
}

fn slash_path(path: &Path) -> Option<String> {
    path.iter()
        .map(|component| component.to_str())
        .collect::<Option<Vec<_>>>()
        .map(|components| components.join("/"))
}

fn write_notes(notes: &[String]) {
    for note in notes {
        write_stderr(&format!("outlint: {note}\n"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_paths_are_relative_to_the_invocation_directory() {
        assert_eq!(
            output_path(
                Path::new("/repo"),
                Path::new("/repo/design-docs/search"),
                "read-spec.md"
            ),
            Some("design-docs/search/read-spec.md".into())
        );
        assert_eq!(
            output_path(Path::new("/repo"), Path::new("/repo"), "spec/a.md"),
            Some("spec/a.md".into())
        );
        assert_eq!(
            output_path(Path::new("/repo/spec"), Path::new("/repo"), "README.md"),
            Some("../README.md".into())
        );
        assert_eq!(
            output_path(
                Path::new("/repo/spec"),
                Path::new("/repo"),
                "spec/outlint-spec.md"
            ),
            Some("outlint-spec.md".into())
        );
    }
}
