//! The filesystem shell: search-scope discovery, the Markdown walk, and the
//! on-disk tantivy index with its refresh and search operations.

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, Read},
    ops::Range,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use ignore::{Error as WalkError, WalkBuilder};
use tantivy::{
    collector::{Count, TopDocs},
    directory::error::LockError,
    doc,
    schema::Value,
    snippet::SnippetGenerator,
    DocId, Index, IndexReader, ReloadPolicy, Searcher, SegmentReader, TantivyDocument,
    TantivyError, Term,
};

use crate::{
    index::{
        build_count_query, build_item_query, build_query, build_schema, snippet_query, Fields,
        INDEX_FORMAT_VERSION, KIND_TOMBSTONE, KIND_UNIT, MTIME, PATH, SIZE,
    },
    result::{select_smallest_hits, CandidateHit, Hit, SearchResults, TermCount},
    units::{collapse_whitespace, index_units, IndexUnit},
};

/// Markdown files larger than this are walked but not indexed: the refresh
/// notes them once and records a tombstone in place of their units.
const MAX_FILE_SIZE: u64 = 4 * 1024 * 1024;
/// Memory budget handed to the tantivy writer.
const WRITER_HEAP_BYTES: usize = 50_000_000;
/// Broad candidates fetched per requested result before list-to-item
/// replacement and de-duplication.
const CANDIDATES_PER_HIT: usize = 4;
/// Upper bound on a hit's snippet, in characters, before the ellipsis.
const SNIPPET_CHARS: usize = 160;
const INDEX_MARKER: &str = "outlint-index.json";

/// A failed search-library operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchError {
    /// The operation or invariant that failed.
    pub kind: SearchErrorKind,
    /// The affected filesystem or indexed path, when one is known.
    pub path: Option<PathBuf>,
    /// The underlying provider or operating-system message.
    pub cause: String,
}

/// The operation or invariant represented by a [`SearchError`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SearchErrorKind {
    /// The search root could not be canonicalized.
    ResolveSearchRoot,
    /// The search root's repository-relative spelling is not UTF-8.
    NonUtf8SearchRoot,
    /// An index directory could not be created.
    CreateDirectory,
    /// An index metadata file could not be written.
    WriteFile,
    /// An unusable index directory could not be removed before rebuilding.
    RemoveDirectory,
    /// A new Tantivy index could not be created.
    CreateIndex,
    /// The index writer could not be opened.
    OpenIndexWriter,
    /// One Markdown file could not be added to the index.
    IndexFile,
    /// The index changes could not be committed.
    CommitIndex,
    /// Tantivy's merge workers did not finish successfully.
    FinishIndexMerge,
    /// The index reader could not be opened.
    OpenIndexReader,
    /// Indexed file signatures could not be read.
    ReadIndex,
    /// A search query could not be executed.
    ExecuteSearch,
    /// Snippet generation could not be prepared.
    PrepareSnippets,
    /// A ranked hit could not be loaded.
    LoadSearchHit,
    /// A query for matching list items could not be executed.
    ExecuteItemSearch,
    /// A matching list-item hit could not be loaded.
    LoadItemHit,
    /// A query term could not be counted.
    CountTerm {
        /// The user-spelled term being counted.
        term: String,
    },
    /// A required field was absent or had the wrong stored type.
    CorruptIndexRecord {
        /// The required field that was unusable.
        field: StoredField,
    },
}

/// A required field stored in each searchable index record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StoredField {
    /// Repository-relative Markdown file path.
    Path,
    /// Canonical document path inside the Markdown file.
    DocumentPath,
    /// Byte length of the indexed unit.
    Bytes,
    /// Source text used to produce the displayed snippet.
    Snippet,
}

/// A noteworthy index lifecycle event or recoverable issue that leaves search
/// able to continue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchNote {
    /// The affected path, when the operation identified one.
    pub path: Option<PathBuf>,
    /// The operation that occurred, was skipped, or was degraded.
    pub operation: SearchNoteOperation,
    /// The underlying reason without presentation prefixes.
    pub cause: String,
}

/// The operation represented by a [`SearchNote`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SearchNoteOperation {
    /// The search index directory was created for the first time.
    CreateIndex,
    /// An existing search index directory was replaced.
    RebuildIndex,
    /// Walking an entry or ignore file failed.
    Walk,
    /// Reading file metadata failed.
    Stat,
    /// A path could not be represented as a lossless index key.
    EncodePath,
    /// A Markdown file could not be indexed from its content or bytes.
    IndexFile,
    /// Another process owns the index writer lock.
    RefreshIndex,
}

fn search_error(
    kind: SearchErrorKind,
    path: Option<PathBuf>,
    cause: impl std::fmt::Display,
) -> SearchError {
    SearchError {
        kind,
        path,
        cause: cause.to_string(),
    }
}

/// One Markdown file found by [`walk_markdown`], with the change signature the
/// refresh compares against the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkedFile {
    /// Path relative to the repository root, with forward slashes.
    pub path: String,
    /// Modification time in nanoseconds since the Unix epoch: `0` when unknown
    /// or before the epoch, saturated at `u64::MAX` beyond its range.
    pub mtime: u64,
    /// File size in bytes.
    pub size: u64,
}

/// The outcome of [`walk_markdown`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Walk {
    /// The Markdown files found, sorted by path.
    pub files: Vec<WalkedFile>,
    /// One note per entry that could not be examined or was skipped.
    pub notes: Vec<SearchNote>,
    /// Whether every entry under the search root was examined. When `false`,
    /// a file absent from `files` may still exist, so a refresh must not
    /// treat indexed files it did not see as deleted.
    pub complete: bool,
}

/// Where a search runs: the repository whose `.outlint/search/` holds the
/// index, and the directory inside it — the search root — whose files are
/// walked and matched. Indexed paths are relative to the repository, so one
/// index serves every search root under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    repo: PathBuf,
    search_root: PathBuf,
    /// The search root relative to `repo` with a trailing `/`, or empty when
    /// the two coincide; an indexed path lies under the search root exactly
    /// when it starts with this.
    prefix: String,
}

impl Scope {
    /// Resolves `search_root` and finds its repository: the nearest ancestor
    /// (inclusive) containing a `.git` entry, or the search root itself when
    /// there is none. The search root is canonicalized first, so a symlinked
    /// or `..`-containing directory is scoped by where it really lives.
    ///
    /// # Errors
    ///
    /// Returns a structured error when `search_root` cannot be canonicalized
    /// or its path inside the repository is not valid UTF-8.
    pub fn locate(search_root: &Path) -> Result<Self, SearchError> {
        let search_root = search_root.canonicalize().map_err(|error| {
            search_error(
                SearchErrorKind::ResolveSearchRoot,
                Some(search_root.to_path_buf()),
                error,
            )
        })?;
        let repo = search_root
            .ancestors()
            .find(|directory| directory.join(".git").exists())
            .unwrap_or(&search_root)
            .to_path_buf();
        let prefix = match search_root
            .strip_prefix(&repo)
            .ok()
            .filter(|relative| !relative.as_os_str().is_empty())
        {
            Some(relative) => match slash_path(relative) {
                Some(relative) => format!("{relative}/"),
                None => {
                    return Err(search_error(
                        SearchErrorKind::NonUtf8SearchRoot,
                        Some(search_root.clone()),
                        "path is not valid UTF-8",
                    ))
                }
            },
            None => String::new(),
        };
        Ok(Self {
            repo,
            search_root,
            prefix,
        })
    }

    /// The canonical directory whose Markdown files this scope searches.
    pub fn search_root(&self) -> &Path {
        &self.search_root
    }
}

/// A relative path as forward-slash-separated components, or `None` when a
/// component is not valid UTF-8 — such a path is never lossily converted
/// into an index key, because the key could then name a different file.
fn slash_path(relative: &Path) -> Option<String> {
    relative
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()
        .map(|components| components.join("/"))
}

/// Whether a file extension marks a Markdown file, ignoring ASCII case.
fn is_markdown_extension(extension: &str) -> bool {
    extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
}

/// The path tagged onto a recursive-walk error, when the walker supplied one.
fn walk_error_path(error: &WalkError) -> Option<&Path> {
    match error {
        WalkError::Partial(errors) => errors.iter().find_map(walk_error_path),
        WalkError::WithLineNumber { err, .. } | WalkError::WithDepth { err, .. } => {
            walk_error_path(err)
        }
        WalkError::WithPath { path, .. } => Some(path),
        WalkError::Loop { child, .. } => Some(child),
        WalkError::Io(_)
        | WalkError::Glob { .. }
        | WalkError::UnrecognizedFileType(_)
        | WalkError::InvalidDefinition => None,
    }
}

/// Lists the Markdown files under the scope's search root, respecting ignore
/// files and skipping hidden entries, symlinks, and files whose name is not
/// valid UTF-8. Oversized files are listed like any other, so the refresh
/// can note them and stop re-examining them.
pub fn walk_markdown(scope: &Scope) -> Walk {
    let mut files = Vec::new();
    let mut notes = Vec::new();
    let mut complete = true;
    let walker = WalkBuilder::new(&scope.search_root)
        .hidden(true)
        .follow_links(false)
        .filter_entry(|entry| entry.file_name() != ".outlint")
        .build();
    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                notes.push(SearchNote {
                    path: walk_error_path(&error).map(Path::to_path_buf),
                    operation: SearchNoteOperation::Walk,
                    cause: error.to_string(),
                });
                complete = false;
                continue;
            }
        };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let extension = entry
            .path()
            .extension()
            .and_then(|extension| extension.to_str());
        if !extension.is_some_and(is_markdown_extension) {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                notes.push(SearchNote {
                    path: Some(entry.path().to_path_buf()),
                    operation: SearchNoteOperation::Stat,
                    cause: error.to_string(),
                });
                complete = false;
                continue;
            }
        };
        let Ok(relative) = entry.path().strip_prefix(&scope.repo) else {
            continue;
        };
        let Some(path) = slash_path(relative) else {
            notes.push(SearchNote {
                path: Some(entry.path().to_path_buf()),
                operation: SearchNoteOperation::EncodePath,
                cause: "file name is not valid UTF-8".to_owned(),
            });
            continue;
        };
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
            });
        files.push(WalkedFile {
            path,
            mtime,
            size: metadata.len(),
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Walk {
        files,
        notes,
        complete,
    }
}

/// An opened on-disk search index, scoped to one search root inside its
/// repository.
pub struct Store {
    scope: Scope,
    index: Index,
    fields: Fields,
}

/// Filesystem changes needed to reconcile one walk with indexed signatures.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RefreshPlan {
    reindex: Vec<WalkedFile>,
    remove: Vec<String>,
}

/// Compares signatures without reading files or touching the index. Removal
/// is suppressed after an incomplete walk because an absent entry may still
/// exist on disk.
fn plan_refresh(
    walked: &[WalkedFile],
    indexed: &HashMap<String, (u64, u64)>,
    search_root_prefix: &str,
    walk_complete: bool,
) -> RefreshPlan {
    let walked_paths: HashSet<&str> = walked.iter().map(|file| file.path.as_str()).collect();
    let reindex = walked
        .iter()
        .filter(|file| indexed.get(&file.path) != Some(&(file.mtime, file.size)))
        .cloned()
        .collect();
    let mut remove = if walk_complete {
        indexed
            .keys()
            .filter(|path| {
                let path: &str = path;
                path.starts_with(search_root_prefix) && !walked_paths.contains(path)
            })
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    remove.sort();
    RefreshPlan { reindex, remove }
}

/// Reads only the leading numeric format member written by this crate. Other
/// marker content remains opaque and causes the safer open-failure rebuild
/// note rather than being treated as a known format transition.
fn marker_format(marker: Option<&str>) -> Option<u32> {
    let digits = marker?.strip_prefix("{\"format\": ")?;
    let end = digits.find(|character: char| !character.is_ascii_digit())?;
    digits.get(..end)?.parse().ok()
}

/// Describes the successful creation or rebuild selected from already-read
/// filesystem state. Keeping this choice pure makes the user-visible reason
/// independent of the I/O that performs the rebuild.
fn rebuilt_index_note(
    directory: &Path,
    directory_existed: bool,
    actual_marker: Option<&str>,
) -> SearchNote {
    if !directory_existed {
        return SearchNote {
            path: Some(directory.to_path_buf()),
            operation: SearchNoteOperation::CreateIndex,
            cause: "index directory did not exist".to_owned(),
        };
    }
    SearchNote {
        path: Some(directory.to_path_buf()),
        operation: SearchNoteOperation::RebuildIndex,
        cause: if marker_format(actual_marker).is_some_and(|format| format != INDEX_FORMAT_VERSION)
        {
            "format changed".to_owned()
        } else {
            "index could not be opened".to_owned()
        },
    }
}

impl Store {
    /// Opens the index under `<repo>/.outlint/search/`, or creates it when it
    /// is missing.
    ///
    /// This operation may delete and rebuild the entire search index
    /// directory. It does so when the format marker differs from the current
    /// format, the index cannot be opened, or its schema is unusable. It also
    /// creates `.outlint/.gitignore` when missing.
    ///
    /// # Errors
    ///
    /// Returns a structured error when a directory, metadata file, or index
    /// cannot be created, removed, or written. On success, notes identify a
    /// newly created or rebuilt directory; an ordinary open returns none.
    pub fn open_or_rebuild(scope: &Scope) -> Result<(Self, Vec<SearchNote>), SearchError> {
        let outlint_dir = scope.repo.join(".outlint");
        fs::create_dir_all(&outlint_dir).map_err(|error| {
            search_error(
                SearchErrorKind::CreateDirectory,
                Some(outlint_dir.clone()),
                error,
            )
        })?;
        let gitignore = outlint_dir.join(".gitignore");
        if !gitignore.exists() {
            fs::write(&gitignore, "*\n").map_err(|error| {
                search_error(SearchErrorKind::WriteFile, Some(gitignore.clone()), error)
            })?;
        }
        let directory = outlint_dir.join("search");
        let directory_existed = directory.exists();
        let marker = directory.join(INDEX_MARKER);
        let expected_marker = format!(
            "{{\"format\": {INDEX_FORMAT_VERSION}, \"outlint\": \"{}\"}}\n",
            env!("CARGO_PKG_VERSION")
        );
        let actual_marker = fs::read_to_string(&marker).ok();
        if actual_marker.as_deref() == Some(expected_marker.as_str()) {
            if let Ok(index) = Index::open_in_dir(&directory) {
                if let Ok(fields) = Fields::of(&index.schema()) {
                    return Ok((
                        Self {
                            scope: scope.clone(),
                            index,
                            fields,
                        },
                        Vec::new(),
                    ));
                }
            }
        }
        let note = rebuilt_index_note(&directory, directory_existed, actual_marker.as_deref());
        if directory.exists() {
            fs::remove_dir_all(&directory).map_err(|error| {
                search_error(
                    SearchErrorKind::RemoveDirectory,
                    Some(directory.clone()),
                    error,
                )
            })?;
        }
        fs::create_dir_all(&directory).map_err(|error| {
            search_error(
                SearchErrorKind::CreateDirectory,
                Some(directory.clone()),
                error,
            )
        })?;
        let (schema, fields) = build_schema();
        let index = Index::create_in_dir(&directory, schema).map_err(|error| {
            search_error(SearchErrorKind::CreateIndex, Some(directory.clone()), error)
        })?;
        fs::write(&marker, expected_marker).map_err(|error| {
            search_error(SearchErrorKind::WriteFile, Some(marker.clone()), error)
        })?;
        Ok((
            Self {
                scope: scope.clone(),
                index,
                fields,
            },
            vec![note],
        ))
    }

    /// Brings the index in line with `walk`, the walk of the search root:
    /// files whose `(mtime, size)` changed or are new are re-indexed, indexed
    /// files under the search root that were not walked are removed — unless
    /// the walk was incomplete, when nothing is removed — and unchanged files
    /// are untouched. Indexed files outside the search root are never
    /// touched, so searches from different roots share the index safely.
    ///
    /// A walked file that yields no units for a reason fixed by its content
    /// (not UTF-8, unparseable, over the size cap, or simply empty) is
    /// recorded as a tombstone carrying only its signature, so it is not
    /// re-read on every run. A file that cannot be read at all (permission
    /// denied, an I/O error) is noted and left out of the index instead: its
    /// signature does not change when the cause goes away, so a tombstone
    /// would never be retried, whereas an unindexed file is retried on the
    /// next run.
    ///
    /// Returns notes for the caller to print: files skipped because they are
    /// not UTF-8, do not parse, are over the size cap, or cannot be read, or
    /// the fact that another process holds the writer lock, in which case the
    /// index is left as it is.
    ///
    /// # Errors
    ///
    /// Returns a structured error when the index cannot be read or written.
    pub fn refresh(&self, walk: &Walk) -> Result<Vec<SearchNote>, SearchError> {
        let known = self.indexed_files()?;
        let plan = plan_refresh(&walk.files, &known, &self.scope.prefix, walk.complete);
        if plan.reindex.is_empty() && plan.remove.is_empty() {
            return Ok(Vec::new());
        }

        let mut notes = Vec::new();
        let mut writer = match self.index.writer::<TantivyDocument>(WRITER_HEAP_BYTES) {
            Ok(writer) => writer,
            Err(TantivyError::LockFailure(LockError::LockBusy, _)) => {
                notes.push(SearchNote {
                    path: Some(self.scope.repo.join(".outlint/search")),
                    operation: SearchNoteOperation::RefreshIndex,
                    cause: "another process holds the writer lock".to_owned(),
                });
                return Ok(notes);
            }
            Err(error) => {
                return Err(search_error(
                    SearchErrorKind::OpenIndexWriter,
                    Some(self.scope.repo.join(".outlint/search")),
                    error,
                ))
            }
        };
        for path in &plan.remove {
            writer.delete_term(Term::from_field_text(self.fields.path, path));
        }
        for file in &plan.reindex {
            writer.delete_term(Term::from_field_text(self.fields.path, &file.path));
            let units = match load_units(&self.scope.repo, file) {
                Load::Units(units) => units,
                Load::Skipped(note) => {
                    notes.push(note);
                    Vec::new()
                }
                Load::Unreadable(note) => {
                    notes.push(note);
                    continue;
                }
            };
            let fields = &self.fields;
            if units.is_empty() {
                // A tombstone: the signature alone, with no `mdpath` and no
                // indexed text, so the file counts as up to date next time
                // and can never be a hit.
                writer
                    .add_document(doc!(
                        fields.path => file.path.as_str(),
                        fields.mtime => file.mtime,
                        fields.size => file.size,
                        fields.kind => KIND_TOMBSTONE,
                    ))
                    .map_err(|error| {
                        search_error(
                            SearchErrorKind::IndexFile,
                            Some(PathBuf::from(&file.path)),
                            error,
                        )
                    })?;
                continue;
            }
            for unit in units {
                let mut document = doc!(
                    fields.path => file.path.as_str(),
                    fields.mdpath => unit.mdpath,
                    fields.bytes => unit.bytes,
                    fields.context => unit.context,
                    fields.body => unit.body_text,
                    fields.snippet => unit.snippet_text,
                    fields.mtime => file.mtime,
                    fields.size => file.size,
                    fields.kind => KIND_UNIT,
                );
                if let Some(section_bytes) = unit.section_bytes {
                    document.add_u64(fields.section_bytes, section_bytes);
                }
                if let Some(parent_list) = unit.parent_list {
                    document.add_text(fields.parent_list, parent_list);
                }
                writer.add_document(document).map_err(|error| {
                    search_error(
                        SearchErrorKind::IndexFile,
                        Some(PathBuf::from(&file.path)),
                        error,
                    )
                })?;
            }
        }
        writer.commit().map_err(|error| {
            search_error(
                SearchErrorKind::CommitIndex,
                Some(self.scope.repo.join(".outlint/search")),
                error,
            )
        })?;
        writer.wait_merging_threads().map_err(|error| {
            search_error(
                SearchErrorKind::FinishIndexMerge,
                Some(self.scope.repo.join(".outlint/search")),
                error,
            )
        })?;
        Ok(notes)
    }

    /// Returns up to `limit` best-scoring smallest matching units for `words`
    /// under the search root, ordered by score, path, and document path. A
    /// matching list is replaced by its best matching item when one item
    /// satisfies the whole query; otherwise the list represents a match whose
    /// words are spread across items. Paths are relative to the search root,
    /// and each hit carries a snippet chosen around the query words.
    ///
    /// # Errors
    ///
    /// Returns a structured error when the index cannot be read or a stored
    /// hit is corrupt.
    pub fn search(&self, words: &str, limit: usize) -> Result<SearchResults, SearchError> {
        let searcher = self.reader()?.searcher();
        let query = build_query(&self.index, &self.fields, words, &self.scope.prefix);
        let count_query = build_count_query(&self.index, &self.fields, words, &self.scope.prefix);
        let total = searcher
            .search(&*count_query, &Count)
            .map_err(|error| search_error(SearchErrorKind::ExecuteSearch, None, error))?;
        let candidate_limit = limit.saturating_mul(CANDIDATES_PER_HIT);
        let top = if candidate_limit == 0 {
            Vec::new()
        } else {
            searcher
                .search(
                    &*query,
                    &TopDocs::with_limit(candidate_limit).order_by_score(),
                )
                .map_err(|error| search_error(SearchErrorKind::ExecuteSearch, None, error))?
        };
        let highlight = snippet_query(&self.index, &self.fields, words);
        let mut generator =
            SnippetGenerator::create(&searcher, &*highlight, self.fields.snippet)
                .map_err(|error| search_error(SearchErrorKind::PrepareSnippets, None, error))?;
        generator.set_max_num_chars(SNIPPET_CHARS);
        let mut candidates = Vec::with_capacity(top.len());
        for (score, address) in top {
            let document: TantivyDocument = searcher
                .doc(address)
                .map_err(|error| search_error(SearchErrorKind::LoadSearchHit, None, error))?;
            let Some(hit) = hit_from_document(&self.fields, &generator, &document, score)? else {
                continue;
            };
            let narrower = if is_list_path(&hit.mdpath) {
                self.best_matching_item(&searcher, &generator, words, &hit.path, &hit.mdpath)?
            } else {
                None
            };
            candidates.push(CandidateHit { hit, narrower });
        }
        let mut hits = select_smallest_hits(candidates, limit);
        for hit in &mut hits {
            if hit.path.starts_with(&self.scope.prefix) {
                hit.path.drain(..self.scope.prefix.len());
            }
        }
        Ok(SearchResults { hits, total })
    }

    /// Finds the best item of the list at `mdpath` in `path` that satisfies
    /// the whole query. Only called for list candidates: for any other node
    /// the parent restriction would match nothing, at the cost of a query.
    fn best_matching_item(
        &self,
        searcher: &Searcher,
        generator: &SnippetGenerator,
        words: &str,
        path: &str,
        mdpath: &str,
    ) -> Result<Option<Hit>, SearchError> {
        let query = build_item_query(
            &self.index,
            &self.fields,
            words,
            &self.scope.prefix,
            path,
            mdpath,
        );
        let Some((score, address)) = searcher
            .search(&*query, &TopDocs::with_limit(1).order_by_score())
            .map_err(|error| search_error(SearchErrorKind::ExecuteItemSearch, None, error))?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let document: TantivyDocument = searcher
            .doc(address)
            .map_err(|error| search_error(SearchErrorKind::LoadItemHit, None, error))?;
        hit_from_document(&self.fields, generator, &document, score)
    }

    /// Counts, for a simple whitespace-separated word query, how many
    /// non-item units under the search root match each user-spelled term in
    /// their own body. Lists stand for their items so each piece of text is
    /// counted once. The same query parser and stemmed field as
    /// [`Self::search`] determine each count. Returns `None` when `words` uses
    /// phrase, field, Boolean, or other query syntax for which independent
    /// term counts would be misleading.
    ///
    /// # Errors
    ///
    /// Returns a structured error when the index cannot be read or searched.
    pub fn term_counts(&self, words: &str) -> Result<Option<Vec<TermCount>>, SearchError> {
        let Some(terms) = simple_query_terms(words) else {
            return Ok(None);
        };
        let searcher = self.reader()?.searcher();
        terms
            .into_iter()
            .map(|term| {
                let query = build_count_query(&self.index, &self.fields, term, &self.scope.prefix);
                let blocks = searcher.search(&*query, &Count).map_err(|error| {
                    search_error(
                        SearchErrorKind::CountTerm {
                            term: term.to_owned(),
                        },
                        None,
                        error,
                    )
                })?;
                Ok(TermCount {
                    term: term.to_owned(),
                    blocks,
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    fn reader(&self) -> Result<IndexReader, SearchError> {
        self.index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|error| search_error(SearchErrorKind::OpenIndexReader, None, error))
    }

    /// The `(mtime, size)` signature of every indexed file, read from fast
    /// fields without loading stored documents.
    fn indexed_files(&self) -> Result<HashMap<String, (u64, u64)>, SearchError> {
        let searcher = self.reader()?.searcher();
        let mut known = HashMap::new();
        for segment in searcher.segment_readers() {
            collect_segment_files(segment, &mut known)
                .map_err(|error| search_error(SearchErrorKind::ReadIndex, None, error))?;
        }
        Ok(known)
    }
}

/// Loads the rendering fields of one indexed unit. A non-finite score (which
/// JSON cannot carry) is skipped; a missing or mistyped required field is a
/// typed corruption error.
fn hit_from_document(
    fields: &Fields,
    generator: &SnippetGenerator,
    document: &TantivyDocument,
    score: f32,
) -> Result<Option<Hit>, SearchError> {
    if !score.is_finite() {
        return Ok(None);
    }
    let corrupt = |field, path: Option<&str>| {
        search_error(
            SearchErrorKind::CorruptIndexRecord { field },
            path.map(PathBuf::from),
            "required stored field is missing or has the wrong type",
        )
    };
    let path = document
        .get_first(fields.path)
        .and_then(|value| value.as_str())
        .ok_or_else(|| corrupt(StoredField::Path, None))?
        .to_owned();
    let mdpath = document
        .get_first(fields.mdpath)
        .and_then(|value| value.as_str())
        .ok_or_else(|| corrupt(StoredField::DocumentPath, Some(&path)))?
        .to_owned();
    let bytes = document
        .get_first(fields.bytes)
        .and_then(|value| value.as_u64())
        .ok_or_else(|| corrupt(StoredField::Bytes, Some(&path)))?;
    let snippet_text = document
        .get_first(fields.snippet)
        .and_then(|value| value.as_str())
        .ok_or_else(|| corrupt(StoredField::Snippet, Some(&path)))?;
    let section_bytes = document
        .get_first(fields.section_bytes)
        .and_then(|value| value.as_u64());
    Ok(Some(Hit {
        path,
        mdpath: mdpath.to_owned(),
        bytes,
        section_bytes,
        score,
        snippet: snippet_of(generator, document, snippet_text),
    }))
}

/// Whether a rendered document path addresses a list block. Section slugs
/// never contain `/`, so the last `/` step of a canonical path is its block
/// or item step, and only a list's is spelled `list[i]`.
fn is_list_path(mdpath: &str) -> bool {
    mdpath
        .rsplit_once('/')
        .is_some_and(|(_, step)| step.starts_with("list["))
}

/// User-spelled terms of a plain keyword query. Being conservative is
/// intentional: a generic no-hits note is more useful than incorrect counts
/// when tantivy could interpret punctuation or an uppercase operator as
/// syntax.
fn simple_query_terms(words: &str) -> Option<Vec<&str>> {
    let terms: Vec<_> = words.split_whitespace().collect();
    (!terms.is_empty()
        && terms.iter().all(|term| {
            term.chars().all(char::is_alphanumeric) && !matches!(*term, "AND" | "OR" | "NOT")
        }))
    .then_some(terms)
}

/// The excerpt of `text` — the hit's stored snippet text — shown under its
/// header: the generator's fragment around the query words when any of them
/// occurs in the text, else the leading words that fit [`SNIPPET_CHARS`].
/// (The fallback is not tantivy's: its generator yields an empty snippet when
/// no term matches.) The fragment is widened over punctuation stuck to its
/// ends, which the generator's token bounds leave out, so `release` becomes
/// `release.`; whitespace is collapsed, since the generator keeps newlines;
/// and `…` marks either end at which the fragment cuts the text.
fn snippet_of(generator: &SnippetGenerator, document: &TantivyDocument, text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let snippet = generator.snippet_from_doc(document);
    let fragment = snippet.fragment();
    let range = match text.find(fragment) {
        Some(start) if !fragment.is_empty() => widen(text, start..start + fragment.len()),
        _ => 0..leading_fragment(text, SNIPPET_CHARS).len(),
    };
    let mut excerpt = collapse_whitespace(&text[range.clone()]);
    if range.start > 0 {
        excerpt.insert(0, '…');
    }
    if range.end < text.len() {
        excerpt.push('…');
    }
    excerpt
}

/// How many characters [`widen`] may add at each end of a fragment: enough
/// for a closing quote and a period, and a bound that keeps a long run of
/// symbols (which the tokenizer skips, so they never end a fragment) from
/// stretching the excerpt past [`SNIPPET_CHARS`].
const WIDEN_CHARS: usize = 3;

/// `range` extended at both ends over up to [`WIDEN_CHARS`] characters that
/// are neither whitespace nor alphanumeric, so a fragment cut at token bounds
/// keeps the quote before its first word and the period after its last. Both
/// ends stay on character boundaries.
fn widen(text: &str, range: Range<usize>) -> Range<usize> {
    let sticks = |character: &char| !character.is_whitespace() && !character.is_alphanumeric();
    let before: usize = text[..range.start]
        .chars()
        .rev()
        .take_while(sticks)
        .take(WIDEN_CHARS)
        .map(char::len_utf8)
        .sum();
    let after: usize = text[range.end..]
        .chars()
        .take_while(sticks)
        .take(WIDEN_CHARS)
        .map(char::len_utf8)
        .sum();
    range.start - before..range.end + after
}

/// The whole of `text` when it has at most `max_chars` characters, else its
/// longest prefix within that bound that ends on a word boundary — or the
/// bare prefix when the first word alone exceeds it.
fn leading_fragment(text: &str, max_chars: usize) -> &str {
    let Some((end, next)) = text.char_indices().nth(max_chars) else {
        return text;
    };
    let head = &text[..end];
    if next.is_whitespace() {
        return head.trim_end();
    }
    match head.rfind(char::is_whitespace) {
        Some(space) if space > 0 => head[..space].trim_end(),
        _ => head,
    }
}

/// Records the signature of every file with a live document in `segment`.
/// All documents of one file in a segment were written together and share
/// its signature, so only the first document per path ordinal is decoded:
/// a file contributes dozens of units, and decoding the path of each one
/// dominated a refresh that found nothing to do.
fn collect_segment_files(
    segment: &SegmentReader,
    known: &mut HashMap<String, (u64, u64)>,
) -> tantivy::Result<()> {
    let fast_fields = segment.fast_fields();
    let Some(paths) = fast_fields.str(PATH)? else {
        return Ok(());
    };
    let mtimes = fast_fields.u64(MTIME)?;
    let sizes = fast_fields.u64(SIZE)?;
    let mut seen = vec![false; paths.num_terms()];
    let mut path = String::new();
    for doc in segment.doc_ids_alive() {
        let doc: DocId = doc;
        let Some(ord) = paths.term_ords(doc).next() else {
            continue;
        };
        match usize::try_from(ord)
            .ok()
            .and_then(|index| seen.get_mut(index))
        {
            Some(true) => continue,
            Some(slot) => *slot = true,
            None => {}
        }
        path.clear();
        if !paths.ord_to_str(ord, &mut path)? {
            continue;
        }
        let (Some(mtime), Some(size)) = (mtimes.first(doc), sizes.first(doc)) else {
            continue;
        };
        if !known.contains_key(path.as_str()) {
            known.insert(path.clone(), (mtime, size));
        }
    }
    Ok(())
}

/// The outcome of [`load_units`]: the two failure cases are kept apart
/// because only one of them is worth remembering under the file's signature.
enum Load {
    /// The file was read and split; the list may be empty.
    Units(Vec<IndexUnit>),
    /// The file was read but cannot be indexed for a reason determined by
    /// its content — over the size cap, not UTF-8, or unparseable — which
    /// cannot change without changing its `(mtime, size)` signature. The
    /// note explains why; a tombstone records the signature.
    Skipped(SearchNote),
    /// The file could not be opened or read. The cause (permissions, a
    /// device error) can go away without touching the signature, so nothing
    /// is recorded and the file is retried on the next refresh.
    Unreadable(SearchNote),
}

/// Reads and splits one walked file into units, or explains in one note why
/// it cannot be indexed.
fn load_units(repo: &Path, file: &WalkedFile) -> Load {
    let bytes = match read_capped(&repo.join(&file.path)) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            return Load::Skipped(SearchNote {
                path: Some(PathBuf::from(&file.path)),
                operation: SearchNoteOperation::IndexFile,
                cause: format!("larger than {} MiB", MAX_FILE_SIZE >> 20),
            })
        }
        Err(error) => {
            return Load::Unreadable(SearchNote {
                path: Some(PathBuf::from(&file.path)),
                operation: SearchNoteOperation::IndexFile,
                cause: error.to_string(),
            })
        }
    };
    let Ok(source) = String::from_utf8(bytes) else {
        return Load::Skipped(SearchNote {
            path: Some(PathBuf::from(&file.path)),
            operation: SearchNoteOperation::IndexFile,
            cause: "not valid UTF-8".to_owned(),
        });
    };
    match index_units(&file.path, &source) {
        Ok(units) => Load::Units(units),
        Err(error) => Load::Skipped(SearchNote {
            path: Some(PathBuf::from(&file.path)),
            operation: SearchNoteOperation::IndexFile,
            cause: error.to_string(),
        }),
    }
}

/// Reads a whole file of at most [`MAX_FILE_SIZE`] bytes, or `None` when it
/// is larger. The cap is checked on the open handle and enforced on the bytes
/// actually read, because the file may have grown since the walk measured it.
fn read_capped(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let file = fs::File::open(path)?;
    if file.metadata()?.len() > MAX_FILE_SIZE {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_SIZE + 1).read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= MAX_FILE_SIZE).then_some(bytes))
}

#[cfg(test)]
mod tests {
    use tantivy::{collector::TopDocs, doc, Index};

    use super::*;

    #[test]
    fn index_lifecycle_notes_distinguish_creation_format_and_open_failures() {
        let directory = Path::new("workspace/.outlint/search");
        let created = rebuilt_index_note(directory, false, None);
        assert_eq!(created.operation, SearchNoteOperation::CreateIndex);
        assert_eq!(created.path.as_deref(), Some(directory));

        let old_marker = format!(
            "{{\"format\": {}, \"outlint\": \"0.1.0\"}}\n",
            INDEX_FORMAT_VERSION.saturating_sub(1)
        );
        let changed = rebuilt_index_note(directory, true, Some(&old_marker));
        assert_eq!(changed.operation, SearchNoteOperation::RebuildIndex);
        assert_eq!(changed.cause, "format changed");

        let unreadable = rebuilt_index_note(directory, true, Some("not a marker"));
        assert_eq!(unreadable.operation, SearchNoteOperation::RebuildIndex);
        assert_eq!(unreadable.cause, "index could not be opened");
    }

    #[test]
    fn is_list_path_matches_only_a_final_list_step() {
        assert!(is_list_path("$/list[0]"));
        assert!(is_list_path("$.a.b/list[3]"));
        assert!(!is_list_path("$.a/list[0]/item[1]"));
        assert!(!is_list_path("$.list[0]"));
        assert!(!is_list_path("$.list"));
        assert!(!is_list_path("$.a/p[0]"));
    }

    #[test]
    fn refresh_plan_reindexes_changed_files_and_removes_only_complete_scope_entries() {
        let walked = vec![
            WalkedFile {
                path: "docs/a.md".into(),
                mtime: 1,
                size: 10,
            },
            WalkedFile {
                path: "docs/b.md".into(),
                mtime: 3,
                size: 20,
            },
            WalkedFile {
                path: "docs/c.md".into(),
                mtime: 4,
                size: 30,
            },
        ];
        let indexed = HashMap::from([
            ("docs/a.md".to_owned(), (1, 10)),
            ("docs/b.md".to_owned(), (2, 20)),
            ("docs/gone.md".to_owned(), (5, 40)),
            ("notes/kept.md".to_owned(), (6, 50)),
        ]);

        let plan = plan_refresh(&walked, &indexed, "docs/", true);
        assert_eq!(
            plan.reindex
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["docs/b.md", "docs/c.md"]
        );
        assert_eq!(plan.remove, ["docs/gone.md"]);

        let incomplete = plan_refresh(&walked, &indexed, "docs/", false);
        assert!(incomplete.remove.is_empty());
    }

    #[test]
    fn malformed_hit_reports_the_required_stored_field() {
        let (schema, fields) = build_schema();
        let index = Index::create_in_ram(schema);
        let mut writer = index.writer(15_000_000).expect("writer");
        writer
            .add_document(doc!(
                fields.path => "docs/a.md",
                fields.mdpath => "$/p[0]",
                fields.context => "a",
                fields.body => "text",
                fields.snippet => "text",
                fields.kind => KIND_UNIT,
            ))
            .expect("add");
        writer.commit().expect("commit");
        let searcher = index.reader().expect("reader").searcher();
        let query = build_query(&index, &fields, "*", "");
        let top = searcher
            .search(&*query, &TopDocs::with_limit(1).order_by_score())
            .expect("search");
        let document: TantivyDocument = searcher.doc(top[0].1).expect("doc");
        let highlight = snippet_query(&index, &fields, "text");
        let generator =
            SnippetGenerator::create(&searcher, &*highlight, fields.snippet).expect("generator");

        let error = hit_from_document(&fields, &generator, &document, 1.0)
            .expect_err("missing bytes is corrupt");
        assert_eq!(
            error.kind,
            SearchErrorKind::CorruptIndexRecord {
                field: StoredField::Bytes
            }
        );
        assert_eq!(error.path.as_deref(), Some(Path::new("docs/a.md")));
    }

    #[test]
    fn collect_segment_files_reports_each_file_once_with_its_signature() {
        let (schema, fields) = build_schema();
        let index = Index::create_in_ram(schema);
        let mut writer = index.writer(15_000_000).expect("writer");
        for (path, mtime, size, units) in [("a.md", 10_u64, 100_u64, 3), ("b.md", 20, 200, 2)] {
            for unit in 0..units {
                writer
                    .add_document(doc!(
                        fields.path => path,
                        fields.mdpath => format!("$/p[{unit}]"),
                        fields.mtime => mtime,
                        fields.size => size,
                        fields.kind => KIND_UNIT,
                    ))
                    .expect("add");
            }
        }
        writer
            .add_document(doc!(
                fields.path => "c.md",
                fields.mtime => 30_u64,
                fields.size => 0_u64,
                fields.kind => KIND_TOMBSTONE,
            ))
            .expect("add");
        writer.commit().expect("commit");
        let searcher = index.reader().expect("reader").searcher();
        let mut known = HashMap::new();
        for segment in searcher.segment_readers() {
            collect_segment_files(segment, &mut known).expect("collect");
        }
        assert_eq!(
            known,
            HashMap::from([
                ("a.md".to_owned(), (10, 100)),
                ("b.md".to_owned(), (20, 200)),
                ("c.md".to_owned(), (30, 0)),
            ])
        );
    }

    /// Indexes one unit whose snippet text is `text` and excerpts it for
    /// `words`, the way [`Store::search`] does.
    fn excerpt(text: &str, words: &str) -> String {
        let (schema, fields) = build_schema();
        let index = Index::create_in_ram(schema);
        let mut writer = index.writer(15_000_000).expect("writer");
        writer
            .add_document(doc!(
                fields.path => "a.md",
                fields.mdpath => "$/p[0]",
                fields.context => "a",
                fields.body => text,
                fields.snippet => text,
                fields.kind => KIND_UNIT,
            ))
            .expect("add");
        writer.commit().expect("commit");
        let searcher = index.reader().expect("reader").searcher();
        let highlight = snippet_query(&index, &fields, words);
        let mut generator =
            SnippetGenerator::create(&searcher, &*highlight, fields.snippet).expect("generator");
        generator.set_max_num_chars(SNIPPET_CHARS);
        let query = build_query(&index, &fields, "*", "");
        let top = searcher
            .search(&*query, &TopDocs::with_limit(1).order_by_score())
            .expect("search");
        let document: TantivyDocument = searcher.doc(top[0].1).expect("doc");
        let stored = document
            .get_first(fields.snippet)
            .and_then(|value| value.as_str())
            .unwrap_or("");
        snippet_of(&generator, &document, stored)
    }

    #[test]
    fn snippet_centres_on_query_words_and_marks_a_cut() {
        let filler = "Nothing to see here. ".repeat(12);
        let text = format!("{filler}The kumquat orchard thrives. {filler}");
        let excerpt = excerpt(&text, "kumquats");
        assert!(excerpt.contains("kumquat orchard"), "{excerpt}");
        assert!(excerpt.starts_with('…'), "{excerpt}");
        assert!(excerpt.ends_with('…'), "{excerpt}");
        assert!(excerpt.chars().count() <= SNIPPET_CHARS + 2, "{excerpt}");
        assert!(!excerpt.contains("  "), "{excerpt}");
    }

    #[test]
    fn snippet_keeps_the_punctuation_around_the_fragment() {
        assert_eq!(excerpt("Loquats.", "loquats"), "Loquats.");
        assert_eq!(
            excerpt("Say \"kumquat\" twice. Then stop.", "kumquat"),
            "Say \"kumquat\" twice. Then stop."
        );
        assert_eq!(
            widen("a \"b\" c", 3..4),
            2..5,
            "widening stops at whitespace"
        );
    }

    #[test]
    fn snippet_widening_is_bounded() {
        let text = format!("needle{}", "\u{1F600}".repeat(10_000));
        let bounded = excerpt(&text, "needle");
        assert!(bounded.starts_with("needle"), "{bounded}");
        assert!(bounded.ends_with('…'), "{bounded}");
        assert!(bounded.chars().count() <= SNIPPET_CHARS + 1, "{bounded}");
        assert_eq!(excerpt("Tag the release.", "release"), "Tag the release.");
        assert_eq!(widen("x!!!", 0..1), 0..4, "three characters fit the cap");
        assert_eq!(widen("!!!!x!!!!", 4..5), 1..8, "widening stops at the cap");
    }

    #[test]
    fn snippet_falls_back_to_the_leading_words() {
        assert_eq!(excerpt("Short and sweet.", "loquat"), "Short and sweet.");
        let text = "word ".repeat(40);
        let leading = excerpt(text.trim(), "loquat");
        assert!(leading.ends_with("word…"), "{leading}");
        assert!(leading.chars().count() <= SNIPPET_CHARS + 1, "{leading}");
        assert_eq!(excerpt("", "loquat"), "");
    }

    #[test]
    fn leading_fragment_cuts_on_a_word_boundary() {
        assert_eq!(leading_fragment("one two three", 20), "one two three");
        assert_eq!(leading_fragment("one two three", 8), "one two");
        assert_eq!(leading_fragment("one two three", 7), "one two");
        assert_eq!(leading_fragment("supercalifragilistic", 5), "super");
        assert_eq!(leading_fragment("ééé ààà", 5), "ééé");
    }
}
