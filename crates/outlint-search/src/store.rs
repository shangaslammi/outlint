//! The filesystem shell: search-scope discovery, the Markdown walk, and the
//! on-disk tantivy index with its refresh and search operations.

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use ignore::WalkBuilder;
use tantivy::{
    collector::TopDocs, directory::error::LockError, doc, schema::Value, DocId, Index, IndexReader,
    ReloadPolicy, SegmentReader, TantivyDocument, TantivyError, Term,
};

use crate::{
    index::{
        build_query, build_schema, Fields, INDEX_FORMAT_VERSION, KIND_TOMBSTONE, KIND_UNIT, MTIME,
        PATH, SIZE,
    },
    render::{sort_hits, Hit},
    units::{index_units, IndexUnit},
};

/// Markdown files larger than this are walked but not indexed: the refresh
/// notes them once and records a tombstone in place of their units.
const MAX_FILE_SIZE: u64 = 4 * 1024 * 1024;
/// Memory budget handed to the tantivy writer.
const WRITER_HEAP_BYTES: usize = 50_000_000;
/// Hits returned per search.
const HIT_LIMIT: usize = 10;
const INDEX_MARKER: &str = "outlint-index.json";

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
    pub notes: Vec<String>,
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
    /// Returns a message when `search_root` cannot be canonicalized or its
    /// path inside the repository is not valid UTF-8.
    pub fn locate(search_root: &Path) -> Result<Self, String> {
        let search_root = search_root
            .canonicalize()
            .map_err(|error| format!("cannot resolve {}: {error}", search_root.display()))?;
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
                    return Err(format!(
                        "search root {} is not valid UTF-8",
                        search_root.display()
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
                notes.push(format!("cannot walk: {error}"));
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
                notes.push(format!("cannot stat {}: {error}", entry.path().display()));
                complete = false;
                continue;
            }
        };
        let Ok(relative) = entry.path().strip_prefix(&scope.repo) else {
            continue;
        };
        let Some(path) = slash_path(relative) else {
            notes.push(format!(
                "skipping {}: file name is not valid UTF-8",
                entry.path().display()
            ));
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

impl Store {
    /// Opens the index under `<repo>/.outlint/search/`, creating it — and the
    /// `.outlint/.gitignore` that keeps it out of version control — when
    /// missing, and rebuilding it when its format marker or its files are
    /// unusable.
    ///
    /// # Errors
    ///
    /// Returns a message when the directory or the index cannot be created.
    pub fn open(scope: &Scope) -> Result<Self, String> {
        let outlint_dir = scope.repo.join(".outlint");
        fs::create_dir_all(&outlint_dir)
            .map_err(|error| format!("cannot create {}: {error}", outlint_dir.display()))?;
        let gitignore = outlint_dir.join(".gitignore");
        if !gitignore.exists() {
            fs::write(&gitignore, "*\n")
                .map_err(|error| format!("cannot write {}: {error}", gitignore.display()))?;
        }
        let directory = outlint_dir.join("search");
        let marker = directory.join(INDEX_MARKER);
        let expected_marker = format!(
            "{{\"format\": {INDEX_FORMAT_VERSION}, \"outlint\": \"{}\"}}\n",
            env!("CARGO_PKG_VERSION")
        );
        if fs::read_to_string(&marker).ok().as_deref() == Some(expected_marker.as_str()) {
            if let Ok(index) = Index::open_in_dir(&directory) {
                if let Ok(fields) = Fields::of(&index.schema()) {
                    return Ok(Self {
                        scope: scope.clone(),
                        index,
                        fields,
                    });
                }
            }
        }
        if directory.exists() {
            fs::remove_dir_all(&directory)
                .map_err(|error| format!("cannot remove {}: {error}", directory.display()))?;
        }
        fs::create_dir_all(&directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
        let (schema, fields) = build_schema();
        let index = Index::create_in_dir(&directory, schema)
            .map_err(|error| format!("cannot create search index: {error}"))?;
        fs::write(&marker, expected_marker)
            .map_err(|error| format!("cannot write {}: {error}", marker.display()))?;
        Ok(Self {
            scope: scope.clone(),
            index,
            fields,
        })
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
    /// Returns a message when the index cannot be read or written.
    pub fn refresh(&self, walk: &Walk) -> Result<Vec<String>, String> {
        let known = self.indexed_files()?;
        let walked: HashSet<&String> = walk.files.iter().map(|file| &file.path).collect();
        let stale: Vec<&WalkedFile> = walk
            .files
            .iter()
            .filter(|file| known.get(&file.path) != Some(&(file.mtime, file.size)))
            .collect();
        let removed: Vec<&String> = if walk.complete {
            known
                .keys()
                .filter(|path| path.starts_with(&self.scope.prefix) && !walked.contains(path))
                .collect()
        } else {
            Vec::new()
        };
        if stale.is_empty() && removed.is_empty() {
            return Ok(Vec::new());
        }

        let mut notes = Vec::new();
        let mut writer = match self.index.writer::<TantivyDocument>(WRITER_HEAP_BYTES) {
            Ok(writer) => writer,
            Err(TantivyError::LockFailure(LockError::LockBusy, _)) => {
                notes.push(
                    "search index is being refreshed by another process; searching the existing index"
                        .to_owned(),
                );
                return Ok(notes);
            }
            Err(error) => return Err(format!("cannot open search index for writing: {error}")),
        };
        for path in removed {
            writer.delete_term(Term::from_field_text(self.fields.path, path));
        }
        for file in stale {
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
                    .map_err(|error| format!("cannot index {}: {error}", file.path))?;
                continue;
            }
            for unit in units {
                let mut document = doc!(
                    fields.path => file.path.as_str(),
                    fields.mdpath => unit.mdpath,
                    fields.bytes => unit.bytes,
                    fields.context => unit.context,
                    fields.body => unit.body_text,
                    fields.raw => unit.raw,
                    fields.mtime => file.mtime,
                    fields.size => file.size,
                    fields.kind => KIND_UNIT,
                );
                if let Some(section_bytes) = unit.section_bytes {
                    document.add_u64(fields.section_bytes, section_bytes);
                }
                writer
                    .add_document(document)
                    .map_err(|error| format!("cannot index {}: {error}", file.path))?;
            }
        }
        writer
            .commit()
            .map_err(|error| format!("cannot commit search index: {error}"))?;
        writer
            .wait_merging_threads()
            .map_err(|error| format!("cannot finish search index merge: {error}"))?;
        Ok(notes)
    }

    /// Returns up to ten best-scoring units for `words` under the search
    /// root, ordered by score, path, and document path, with paths made
    /// relative to the search root.
    ///
    /// # Errors
    ///
    /// Returns a message when the index cannot be read.
    pub fn search(&self, words: &str) -> Result<Vec<Hit>, String> {
        let searcher = self.reader()?.searcher();
        let query = build_query(&self.index, &self.fields, words, &self.scope.prefix);
        let top = searcher
            .search(&*query, &TopDocs::with_limit(HIT_LIMIT).order_by_score())
            .map_err(|error| format!("cannot search: {error}"))?;
        let mut hits = Vec::with_capacity(top.len());
        for (score, address) in top {
            let document: TantivyDocument = searcher
                .doc(address)
                .map_err(|error| format!("cannot load search hit: {error}"))?;
            let text = |field| {
                document
                    .get_first(field)
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_owned()
            };
            let number = |field| document.get_first(field).and_then(|value| value.as_u64());
            let Some(mdpath) = document
                .get_first(self.fields.mdpath)
                .and_then(|value| value.as_str())
            else {
                // Unreachable: the query requires `kind:unit`, and every unit
                // carries an `mdpath`. Kept so that a malformed document is
                // dropped rather than rendered with an empty address.
                continue;
            };
            let mut path = text(self.fields.path);
            if path.starts_with(&self.scope.prefix) {
                path.drain(..self.scope.prefix.len());
            }
            hits.push(Hit {
                path,
                mdpath: mdpath.to_owned(),
                bytes: number(self.fields.bytes).unwrap_or(0),
                section_bytes: number(self.fields.section_bytes),
                score,
                raw: text(self.fields.raw),
            });
        }
        sort_hits(&mut hits);
        Ok(hits)
    }

    fn reader(&self) -> Result<IndexReader, String> {
        self.index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|error| format!("cannot open search index: {error}"))
    }

    /// The `(mtime, size)` signature of every indexed file, read from fast
    /// fields without loading stored documents.
    fn indexed_files(&self) -> Result<HashMap<String, (u64, u64)>, String> {
        let searcher = self.reader()?.searcher();
        let mut known = HashMap::new();
        for segment in searcher.segment_readers() {
            collect_segment_files(segment, &mut known)
                .map_err(|error| format!("cannot read search index: {error}"))?;
        }
        Ok(known)
    }
}

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
    let mut path = String::new();
    for doc in segment.doc_ids_alive() {
        let doc: DocId = doc;
        let Some(ord) = paths.term_ords(doc).next() else {
            continue;
        };
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
    Skipped(String),
    /// The file could not be opened or read. The cause (permissions, a
    /// device error) can go away without touching the signature, so nothing
    /// is recorded and the file is retried on the next refresh.
    Unreadable(String),
}

/// Reads and splits one walked file into units, or explains in one note why
/// it cannot be indexed.
fn load_units(repo: &Path, file: &WalkedFile) -> Load {
    let bytes = match read_capped(&repo.join(&file.path)) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            return Load::Skipped(format!(
                "skipping {}: larger than {} MiB",
                file.path,
                MAX_FILE_SIZE >> 20
            ))
        }
        Err(error) => return Load::Unreadable(format!("skipping {}: {error}", file.path)),
    };
    let Ok(source) = String::from_utf8(bytes) else {
        return Load::Skipped(format!("skipping {}: not valid UTF-8", file.path));
    };
    match index_units(&file.path, &source) {
        Ok(units) => Load::Units(units),
        Err(error) => Load::Skipped(format!("skipping {}: {error}", file.path)),
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
