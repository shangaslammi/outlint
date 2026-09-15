//! The filesystem shell: workspace discovery, the Markdown walk, and the
//! on-disk tantivy index with its refresh and search operations.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use ignore::WalkBuilder;
use tantivy::{
    collector::TopDocs, directory::error::LockError, doc, schema::Value, DocId, Index, IndexReader,
    ReloadPolicy, SegmentReader, TantivyDocument, TantivyError, Term,
};

use crate::{
    index::{build_query, build_schema, Fields, INDEX_FORMAT_VERSION, MTIME, PATH, SIZE},
    render::{sort_hits, Hit},
    units::index_units,
};

/// Markdown files larger than this are not indexed.
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
    /// Path relative to the workspace root, with forward slashes.
    pub path: String,
    /// Modification time in seconds since the Unix epoch (`0` when unknown).
    pub mtime: u64,
    /// File size in bytes.
    pub size: u64,
}

/// The nearest ancestor of `start` (inclusive) containing `.git`, else
/// `start` itself.
pub fn workspace_root(start: &Path) -> PathBuf {
    start
        .ancestors()
        .find(|directory| directory.join(".git").exists())
        .unwrap_or(start)
        .to_path_buf()
}

/// Lists the Markdown files under `root`, respecting ignore files and skipping
/// hidden entries, symlinks, and oversized files.
///
/// Returns the files sorted by path plus one note per entry that could not be
/// examined.
pub fn walk_markdown(root: &Path) -> (Vec<WalkedFile>, Vec<String>) {
    let mut files = Vec::new();
    let mut notes = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(true)
        .follow_links(false)
        .filter_entry(|entry| entry.file_name() != ".outlint")
        .build();
    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                notes.push(format!("cannot walk: {error}"));
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
        if !matches!(extension, Some("md" | "markdown")) {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                notes.push(format!("cannot stat {}: {error}", entry.path().display()));
                continue;
            }
        };
        if metadata.len() > MAX_FILE_SIZE {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |elapsed| elapsed.as_secs());
        files.push(WalkedFile {
            path: relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/"),
            mtime,
            size: metadata.len(),
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    (files, notes)
}

/// An opened on-disk search index for one workspace.
pub struct Store {
    root: PathBuf,
    index: Index,
    fields: Fields,
}

impl Store {
    /// Opens the index under `<root>/.outlint/search/`, creating it — and the
    /// `.outlint/.gitignore` that keeps it out of version control — when
    /// missing, and rebuilding it when its format marker or its files are
    /// unusable.
    ///
    /// # Errors
    ///
    /// Returns a message when the directory or the index cannot be created.
    pub fn open(root: &Path) -> Result<Self, String> {
        let outlint_dir = root.join(".outlint");
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
                        root: root.to_path_buf(),
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
            root: root.to_path_buf(),
            index,
            fields,
        })
    }

    /// Brings the index in line with `files`: files whose `(mtime, size)`
    /// changed or are new are re-indexed, files no longer present are removed,
    /// unchanged files are untouched.
    ///
    /// Returns notes for the caller to print: files skipped because they are
    /// not UTF-8 or do not parse, or the fact that another process holds the
    /// writer lock, in which case the index is left as it is.
    ///
    /// # Errors
    ///
    /// Returns a message when the index cannot be read or written.
    pub fn refresh(&self, files: &[WalkedFile]) -> Result<Vec<String>, String> {
        let known = self.indexed_files()?;
        let walked: HashSet<&String> = files.iter().map(|file| &file.path).collect();
        let stale: Vec<&WalkedFile> = files
            .iter()
            .filter(|file| known.get(&file.path) != Some(&(file.mtime, file.size)))
            .collect();
        let removed: Vec<&String> = known.keys().filter(|path| !walked.contains(path)).collect();
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
            let source = match fs::read(self.root.join(&file.path)) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(source) => source,
                    Err(_) => {
                        notes.push(format!("skipping {}: not valid UTF-8", file.path));
                        continue;
                    }
                },
                Err(error) => {
                    notes.push(format!("skipping {}: {error}", file.path));
                    continue;
                }
            };
            let units = match index_units(&file.path, &source) {
                Ok(units) => units,
                Err(error) => {
                    notes.push(format!("skipping {}: {error}", file.path));
                    continue;
                }
            };
            let fields = &self.fields;
            for unit in units {
                writer
                    .add_document(doc!(
                        fields.path => file.path.as_str(),
                        fields.mdpath => unit.mdpath,
                        fields.line => unit.line,
                        fields.context => unit.context,
                        fields.body => unit.body_text,
                        fields.raw => unit.raw,
                        fields.mtime => file.mtime,
                        fields.size => file.size,
                    ))
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

    /// Returns up to ten best-scoring units for `words`, ordered by score,
    /// path, and line.
    ///
    /// # Errors
    ///
    /// Returns a message when the index cannot be read.
    pub fn search(&self, words: &str) -> Result<Vec<Hit>, String> {
        let searcher = self.reader()?.searcher();
        let query = build_query(&self.index, &self.fields, words);
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
            hits.push(Hit {
                path: text(self.fields.path),
                mdpath: text(self.fields.mdpath),
                line: document
                    .get_first(self.fields.line)
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0),
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
        known.entry(path.clone()).or_insert((mtime, size));
    }
    Ok(())
}
