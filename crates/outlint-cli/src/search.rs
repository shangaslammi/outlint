//! The `search` subcommand (behind the `search` feature): refreshes the
//! repository index under `.outlint/search/` for the files below the search
//! root, runs the query there, and prints the best-matching blocks.

use std::{
    env,
    path::{Component, Path, PathBuf},
};

use outlint_search::{
    walk_markdown, Hit, Scope, SearchError, SearchErrorKind, SearchNote, SearchNoteOperation,
    Store, StoredField, TermCount,
};

use crate::{
    args::ReadSearchFormat,
    args::SearchOptions,
    render::{self, escape_compact, escape_human},
    write_stderr, write_stdout,
};

/// Exit 0 with at least one hit, 1 with none, 2 on a usage or operational
/// error.
pub(crate) fn execute_search(options: &SearchOptions) -> u8 {
    let run = search(options);
    write_notes(&run.notes, options.format);
    let result = match run.result {
        Ok(result) => result,
        Err(error) => {
            write_stderr(&message_line(&search_error_message(&error), options.format));
            return 2;
        }
    };
    if result.hits.is_empty() {
        let rendered = render::search::no_hits(
            &options.words,
            result.total,
            result.term_counts.as_deref(),
            options.format,
        );
        if options.format == ReadSearchFormat::Json {
            if write_stdout(&rendered) == 2 {
                2
            } else {
                1
            }
        } else {
            write_stderr(&rendered);
            1
        }
    } else {
        let status = write_stdout(&render::search::hits(
            &options.words,
            &result.hits,
            result.total,
            options.format,
        ));
        if options.format == ReadSearchFormat::Compact {
            if let Some(note) =
                render::search::compact_truncation_note(result.hits.len(), result.total)
            {
                write_stderr(&note);
            }
        }
        status
    }
}

struct SearchResult {
    hits: Vec<Hit>,
    total: usize,
    term_counts: Option<Vec<TermCount>>,
}

struct SearchRun {
    notes: Vec<SearchNote>,
    result: Result<SearchResult, SearchFailure>,
}

enum SearchFailure {
    Shell(String),
    Library(SearchError),
}

impl From<SearchError> for SearchFailure {
    fn from(error: SearchError) -> Self {
        Self::Library(error)
    }
}

fn search(options: &SearchOptions) -> SearchRun {
    let mut notes = Vec::new();
    let result = search_with_notes(options, &mut notes);
    SearchRun { notes, result }
}

fn search_with_notes(
    options: &SearchOptions,
    notes: &mut Vec<SearchNote>,
) -> Result<SearchResult, SearchFailure> {
    let current_dir = env::current_dir().map_err(|error| {
        SearchFailure::Shell(format!("cannot determine the current directory: {error}"))
    })?;
    let current_dir = current_dir.canonicalize().map_err(|error| {
        SearchFailure::Shell(format!("cannot resolve the current directory: {error}"))
    })?;
    let search_root = match &options.root {
        Some(root) => {
            let path = current_dir.join(root);
            if !path.is_dir() {
                return Err(SearchFailure::Shell(format!(
                    "--root '{root}' is not a directory"
                )));
            }
            path
        }
        None => current_dir.clone(),
    };
    let scope = Scope::locate(&search_root)?;
    let (store, open_notes) = Store::open_or_rebuild(&scope)?;
    notes.extend(open_notes);
    let walk = walk_markdown(&scope);
    notes.extend(walk.notes.iter().cloned());
    notes.extend(store.refresh(&walk)?);
    let mut matches = store.search(&options.words, options.limit)?;
    make_paths_relative(&mut matches.hits, &current_dir, scope.search_root())
        .map_err(SearchFailure::Shell)?;
    let term_counts = if matches.hits.is_empty() {
        store.term_counts(&options.words)?
    } else {
        None
    };
    Ok(SearchResult {
        hits: matches.hits,
        total: matches.total,
        term_counts,
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

fn message_line(message: &str, format: ReadSearchFormat) -> String {
    let message = match format {
        ReadSearchFormat::Compact => escape_compact(message),
        ReadSearchFormat::Human => escape_human(message),
        ReadSearchFormat::Json => message.to_owned(),
    };
    format!("outlint: {message}\n")
}

fn write_notes(notes: &[SearchNote], format: ReadSearchFormat) {
    for note in notes {
        write_stderr(&message_line(&search_note_message(note), format));
    }
}

fn display_path(path: Option<&Path>) -> String {
    path.map_or_else(|| "<unknown>".to_owned(), |path| path.display().to_string())
}

fn search_note_message(note: &SearchNote) -> String {
    match note.operation {
        SearchNoteOperation::CreateIndex => format!(
            "created search index at {}",
            display_path(note.path.as_deref())
        ),
        SearchNoteOperation::RebuildIndex => format!(
            "rebuilt search index at {} ({})",
            display_path(note.path.as_deref()),
            note.cause
        ),
        SearchNoteOperation::Walk => format!("cannot walk: {}", note.cause),
        SearchNoteOperation::Stat => format!(
            "cannot stat {}: {}",
            display_path(note.path.as_deref()),
            note.cause
        ),
        SearchNoteOperation::EncodePath | SearchNoteOperation::IndexFile => format!(
            "skipping {}: {}",
            display_path(note.path.as_deref()),
            note.cause
        ),
        SearchNoteOperation::RefreshIndex => {
            "search index is being refreshed by another process; searching the existing index"
                .to_owned()
        }
        _ => format!("search note: {}", note.cause),
    }
}

fn stored_field_name(field: StoredField) -> &'static str {
    match field {
        StoredField::Path => "path",
        StoredField::DocumentPath => "mdpath",
        StoredField::Bytes => "bytes",
        StoredField::Snippet => "snippet",
        _ => "unknown",
    }
}

fn search_error_message(error: &SearchFailure) -> String {
    match error {
        SearchFailure::Shell(message) => message.clone(),
        SearchFailure::Library(error) => library_error_message(error),
    }
}

fn library_error_message(error: &SearchError) -> String {
    let path = || display_path(error.path.as_deref());
    match &error.kind {
        SearchErrorKind::ResolveSearchRoot => {
            format!("cannot resolve {}: {}", path(), error.cause)
        }
        SearchErrorKind::NonUtf8SearchRoot => {
            format!("search root {} is not valid UTF-8", path())
        }
        SearchErrorKind::CreateDirectory => {
            format!("cannot create {}: {}", path(), error.cause)
        }
        SearchErrorKind::WriteFile => format!("cannot write {}: {}", path(), error.cause),
        SearchErrorKind::RemoveDirectory => {
            format!("cannot remove {}: {}", path(), error.cause)
        }
        SearchErrorKind::CreateIndex => format!("cannot create search index: {}", error.cause),
        SearchErrorKind::OpenIndexWriter => {
            format!("cannot open search index for writing: {}", error.cause)
        }
        SearchErrorKind::IndexFile => {
            format!("cannot index {}: {}", path(), error.cause)
        }
        SearchErrorKind::CommitIndex => format!("cannot commit search index: {}", error.cause),
        SearchErrorKind::FinishIndexMerge => {
            format!("cannot finish search index merge: {}", error.cause)
        }
        SearchErrorKind::OpenIndexReader => {
            format!("cannot open search index: {}", error.cause)
        }
        SearchErrorKind::ReadIndex => format!("cannot read search index: {}", error.cause),
        SearchErrorKind::ExecuteSearch => format!("cannot search: {}", error.cause),
        SearchErrorKind::PrepareSnippets => {
            format!("cannot prepare snippets: {}", error.cause)
        }
        SearchErrorKind::LoadSearchHit => {
            format!("cannot load search hit: {}", error.cause)
        }
        SearchErrorKind::ExecuteItemSearch => {
            format!("cannot search list items: {}", error.cause)
        }
        SearchErrorKind::LoadItemHit => {
            format!("cannot load list item hit: {}", error.cause)
        }
        SearchErrorKind::CountTerm { term } => {
            format!("cannot count search term '{term}': {}", error.cause)
        }
        SearchErrorKind::CorruptIndexRecord { field } => format!(
            "corrupt search index record{}: required field '{}' is missing or invalid",
            error
                .path
                .as_deref()
                .map(|path| format!(" for {}", path.display()))
                .unwrap_or_default(),
            stored_field_name(*field)
        ),
        _ => format!("search failed: {}", error.cause),
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
                Path::new("/repo/docs/guides"),
                "setup.md"
            ),
            Some("docs/guides/setup.md".into())
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

    #[test]
    fn compact_notes_escape_control_characters_and_backslashes() {
        assert_eq!(
            message_line(
                "docs/tab\tcr\rlf\nesc\u{1b}slash\\.md refreshed",
                ReadSearchFormat::Compact
            ),
            "outlint: docs/tab\\tcr\\u{d}lf\\nesc\\u{1b}slash\\\\.md refreshed\n"
        );
        assert_eq!(
            message_line("docs/bad\nfile.md refreshed", ReadSearchFormat::Human),
            "outlint: docs/bad\\nfile.md refreshed\n"
        );
    }
}
