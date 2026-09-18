//! The tantivy schema and query construction shared by the store.

use std::ops::Bound;

use tantivy::{
    query::{BooleanQuery, BoostQuery, Occur, Query, QueryParser, RangeQuery, TermQuery},
    schema::{
        Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, FAST, STORED, STRING,
    },
    Index, Term,
};

/// Bumped whenever the schema or the unit derivation changes incompatibly;
/// the store rebuilds an index recorded under another version.
pub(crate) const INDEX_FORMAT_VERSION: u32 = 6;

/// Field names, so the schema and the fast-field readers agree.
pub(crate) const PATH: &str = "path";
pub(crate) const MDPATH: &str = "mdpath";
pub(crate) const BYTES: &str = "bytes";
pub(crate) const SECTION_BYTES: &str = "section_bytes";
pub(crate) const CONTEXT: &str = "context";
pub(crate) const BODY: &str = "body";
pub(crate) const SNIPPET: &str = "snippet";
pub(crate) const MTIME: &str = "mtime";
pub(crate) const SIZE: &str = "size";
pub(crate) const KIND: &str = "kind";

/// `kind` value of a document that holds one index unit and can be a hit.
pub(crate) const KIND_UNIT: &str = "unit";
/// `kind` value of a tombstone: a document carrying only a file's change
/// signature, written for a walked file that yields no units so the file is
/// not re-read on every refresh. Every query excludes tombstones.
pub(crate) const KIND_TOMBSTONE: &str = "tombstone";

/// Handles of every schema field.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fields {
    pub(crate) path: Field,
    pub(crate) mdpath: Field,
    pub(crate) bytes: Field,
    pub(crate) section_bytes: Field,
    pub(crate) context: Field,
    pub(crate) body: Field,
    pub(crate) snippet: Field,
    pub(crate) mtime: Field,
    pub(crate) size: Field,
    pub(crate) kind: Field,
}

impl Fields {
    /// Looks every field up in an opened index's schema.
    pub(crate) fn of(schema: &Schema) -> Result<Self, String> {
        let field = |name: &str| {
            schema
                .get_field(name)
                .map_err(|error| format!("search index lacks field {name}: {error}"))
        };
        Ok(Self {
            path: field(PATH)?,
            mdpath: field(MDPATH)?,
            bytes: field(BYTES)?,
            section_bytes: field(SECTION_BYTES)?,
            context: field(CONTEXT)?,
            body: field(BODY)?,
            snippet: field(SNIPPET)?,
            mtime: field(MTIME)?,
            size: field(SIZE)?,
            kind: field(KIND)?,
        })
    }
}

/// Builds the schema of a fresh index.
///
/// `path` is a fast field so a refresh can enumerate indexed files without
/// loading stored documents; `mtime` and `size` are fast fields for the same
/// reason. `context`, `body`, and `snippet` are stemmed English text; the
/// first two are never stored, while `snippet` is stored as well, since a hit
/// is excerpted from it, and indexed only so the snippet generator can
/// tokenize it the way the query terms were — it takes no part in scoring.
/// `bytes` and `section_bytes` are stored and never indexed, and
/// `section_bytes` is absent on section and root units. `kind` is indexed
/// and never stored: it is [`KIND_UNIT`] or [`KIND_TOMBSTONE`], and every
/// query requires the former so a tombstone can never take a hit's place.
pub(crate) fn build_schema() -> (Schema, Fields) {
    let stemmed = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer("en_stem")
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );
    let mut builder = Schema::builder();
    let fields = Fields {
        path: builder.add_text_field(PATH, STRING | STORED | FAST),
        mdpath: builder.add_text_field(MDPATH, STRING | STORED),
        bytes: builder.add_u64_field(BYTES, STORED),
        section_bytes: builder.add_u64_field(SECTION_BYTES, STORED),
        context: builder.add_text_field(CONTEXT, stemmed.clone()),
        body: builder.add_text_field(BODY, stemmed.clone()),
        snippet: builder.add_text_field(SNIPPET, stemmed.set_stored()),
        mtime: builder.add_u64_field(MTIME, FAST),
        size: builder.add_u64_field(SIZE, FAST),
        kind: builder.add_text_field(KIND, STRING),
    };
    (builder.build(), fields)
}

/// Score multiplier for a heading chain that contains every query word.
const CONTEXT_BOOST: f32 = 2.0;

/// Builds the query for `words` within `prefix`: a hit must be a unit (never
/// a tombstone) and must match every word on its own `body` text, and a
/// `context` (heading chain) that also contains every word only raises the
/// score — it can never satisfy the query by itself. A non-empty `prefix` (a
/// repository-relative directory ending in `/`) additionally restricts hits
/// to paths under it.
/// Both parses are lenient: unparseable fragments are dropped rather than
/// reported. When no body term survives (`*`, say), the context clause is
/// omitted, but the unit and prefix restrictions still apply, so tombstones
/// are excluded in the query itself rather than after the top hits are
/// taken, where they could crowd real hits out of the limit.
pub(crate) fn build_query(
    index: &Index,
    fields: &Fields,
    words: &str,
    prefix: &str,
) -> Box<dyn Query> {
    let parse = |field: Field| {
        let mut parser = QueryParser::for_index(index, vec![field]);
        parser.set_conjunction_by_default();
        let (query, _errors) = parser.parse_query_lenient(words);
        query
    };
    let body = parse(fields.body);
    let mut has_terms = false;
    body.query_terms(&mut |_, _| has_terms = true);
    let is_unit = TermQuery::new(
        Term::from_field_text(fields.kind, KIND_UNIT),
        IndexRecordOption::Basic,
    );
    let mut clauses: Vec<(Occur, Box<dyn Query>)> =
        vec![(Occur::Must, body), (Occur::Must, Box::new(is_unit))];
    if has_terms {
        let context = BoostQuery::new(parse(fields.context), CONTEXT_BOOST);
        clauses.push((Occur::Should, Box::new(context)));
    }
    if let Some(under_prefix) = path_prefix_query(fields.path, prefix) {
        clauses.push((Occur::Must, Box::new(under_prefix)));
    }
    Box::new(BooleanQuery::new(clauses))
}

/// Builds the query whose terms choose what a hit's snippet centres on:
/// `words` parsed leniently against `snippet` alone, any word sufficing. It
/// is never run against the index — [`build_query`] decides the hits — so
/// disjunction is right: a fragment holding any query word beats the
/// leading fragment.
pub(crate) fn snippet_query(index: &Index, fields: &Fields, words: &str) -> Box<dyn Query> {
    let parser = QueryParser::for_index(index, vec![fields.snippet]);
    let (query, _errors) = parser.parse_query_lenient(words);
    query
}

/// Matches exactly the paths that start with `prefix`, or nothing to filter
/// when `prefix` is empty. Paths are raw terms ordered bytewise, and `prefix`
/// ends in `/`, so they are the terms from `prefix` inclusive up to the same
/// string with its final `/` bumped to the next character `0`, exclusive —
/// `docs/` therefore covers `docs/a.md` but not `docs-old/x.md` or `docs0`.
fn path_prefix_query(path: Field, prefix: &str) -> Option<RangeQuery> {
    let stem = prefix.strip_suffix('/')?;
    Some(RangeQuery::new(
        Bound::Included(Term::from_field_text(path, prefix)),
        Bound::Excluded(Term::from_field_text(path, &format!("{stem}0"))),
    ))
}

#[cfg(test)]
mod tests {
    use tantivy::{collector::TopDocs, doc, schema::Value, TantivyDocument};

    use super::*;

    #[test]
    fn body_must_match_and_context_only_reranks() {
        let (schema, fields) = build_schema();
        let index = Index::create_in_ram(schema);
        let mut writer = index.writer(15_000_000).expect("writer");
        for (path, context, body) in [
            (
                "body-only",
                "Overview / Setup",
                "Rate limiting caps requests per client.",
            ),
            (
                "context-only",
                "Overview / Rate limiting",
                "Tokens refill once per second.",
            ),
            (
                "both",
                "Overview / Rate limiting",
                "Rate limiting caps requests per client.",
            ),
        ] {
            writer
                .add_document(doc!(
                    fields.path => path,
                    fields.context => context,
                    fields.body => body,
                    fields.kind => KIND_UNIT,
                ))
                .expect("add");
        }
        writer.commit().expect("commit");

        let searcher = index.reader().expect("reader").searcher();
        let query = build_query(&index, &fields, "rate limiting", "");
        let top = searcher
            .search(&*query, &TopDocs::with_limit(10).order_by_score())
            .expect("search");
        let paths: Vec<String> = top
            .iter()
            .map(|(_, address)| {
                let document: TantivyDocument = searcher.doc(*address).expect("doc");
                document
                    .get_first(fields.path)
                    .and_then(|value| value.as_str())
                    .expect("path")
                    .to_owned()
            })
            .collect();
        assert_eq!(paths, ["both", "body-only"]);
        assert!(top[0].0 > top[1].0, "context match must raise the score");
    }

    #[test]
    fn snippet_query_terms_are_on_the_snippet_field_only() {
        let (schema, fields) = build_schema();
        let index = Index::create_in_ram(schema);
        let query = snippet_query(&index, &fields, "rate limiting");
        let mut terms = Vec::new();
        query.query_terms(&mut |term, _| {
            assert_eq!(term.field(), fields.snippet);
            terms.push(term.value().as_str().unwrap_or("").to_owned());
        });
        terms.sort();
        assert_eq!(terms, ["limit", "rate"]);
        // The search query never touches the snippet field.
        let query = build_query(&index, &fields, "rate limiting", "");
        query.query_terms(&mut |term, _| assert_ne!(term.field(), fields.snippet));
    }

    #[test]
    fn prefix_restricts_hits_to_paths_under_the_directory() {
        let (schema, fields) = build_schema();
        let index = Index::create_in_ram(schema);
        let mut writer = index.writer(15_000_000).expect("writer");
        for path in [
            "docs/a.md",
            "docs/sub/c.md",
            "docs-old/x.md",
            "docs0",
            "other/b.md",
        ] {
            writer
                .add_document(doc!(
                    fields.path => path,
                    fields.context => "Intro",
                    fields.body => "Kumquat cultivation.",
                    fields.kind => KIND_UNIT,
                ))
                .expect("add");
        }
        // A tombstone under the prefix: a signature with no text and no
        // `mdpath`, which must never surface, not even for a term-less query.
        writer
            .add_document(doc!(
                fields.path => "docs/empty.md",
                fields.mtime => 1_u64,
                fields.size => 0_u64,
                fields.kind => KIND_TOMBSTONE,
            ))
            .expect("add tombstone");
        writer.commit().expect("commit");

        let searcher = index.reader().expect("reader").searcher();
        let paths_for = |words: &str, prefix: &str| {
            let query = build_query(&index, &fields, words, prefix);
            let top = searcher
                .search(&*query, &TopDocs::with_limit(10).order_by_score())
                .expect("search");
            let mut paths: Vec<String> = top
                .iter()
                .map(|(_, address)| {
                    let document: TantivyDocument = searcher.doc(*address).expect("doc");
                    document
                        .get_first(fields.path)
                        .and_then(|value| value.as_str())
                        .expect("path")
                        .to_owned()
                })
                .collect();
            paths.sort();
            paths
        };
        assert_eq!(
            paths_for("kumquat", "docs/"),
            ["docs/a.md", "docs/sub/c.md"]
        );
        assert_eq!(paths_for("kumquat", "docs/sub/"), ["docs/sub/c.md"]);
        assert_eq!(paths_for("kumquat", "").len(), 5);
        // A term-less query still stays under the prefix and never returns
        // the tombstone.
        assert_eq!(paths_for("*", "docs/"), ["docs/a.md", "docs/sub/c.md"]);
        assert_eq!(paths_for("*", "").len(), 5);
        assert!(paths_for("*", "")
            .iter()
            .all(|path| path != "docs/empty.md"));
    }

    #[test]
    fn tombstones_never_crowd_out_hits() {
        let (schema, fields) = build_schema();
        let index = Index::create_in_ram(schema);
        let mut writer = index.writer(15_000_000).expect("writer");
        // More tombstones than the hit limit, all sorting before the one
        // real unit, so an after-the-fact filter would leave no hits.
        for number in 0..10 {
            writer
                .add_document(doc!(
                    fields.path => format!("{number:02}.md"),
                    fields.mtime => 1_u64,
                    fields.size => 0_u64,
                    fields.kind => KIND_TOMBSTONE,
                ))
                .expect("add tombstone");
        }
        writer
            .add_document(doc!(
                fields.path => "z.md",
                fields.mdpath => "/",
                fields.context => "Intro",
                fields.body => "Kumquat cultivation.",
                fields.kind => KIND_UNIT,
            ))
            .expect("add");
        writer.commit().expect("commit");

        let searcher = index.reader().expect("reader").searcher();
        let query = build_query(&index, &fields, "*", "");
        let top = searcher
            .search(&*query, &TopDocs::with_limit(10).order_by_score())
            .expect("search");
        let paths: Vec<String> = top
            .iter()
            .map(|(_, address)| {
                let document: TantivyDocument = searcher.doc(*address).expect("doc");
                document
                    .get_first(fields.path)
                    .and_then(|value| value.as_str())
                    .expect("path")
                    .to_owned()
            })
            .collect();
        assert_eq!(paths, ["z.md"]);
    }
}
