//! The tantivy schema and query construction shared by the store.

use std::ops::Bound;

use tantivy::{
    query::{BooleanQuery, BoostQuery, Occur, Query, QueryParser, RangeQuery},
    schema::{
        Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, FAST, STORED, STRING,
    },
    Index, Term,
};

/// Bumped whenever the schema or the unit derivation changes incompatibly;
/// the store rebuilds an index recorded under another version.
pub(crate) const INDEX_FORMAT_VERSION: u32 = 2;

/// Field names, so the schema and the fast-field readers agree.
pub(crate) const PATH: &str = "path";
pub(crate) const MDPATH: &str = "mdpath";
pub(crate) const BYTES: &str = "bytes";
pub(crate) const SECTION_BYTES: &str = "section_bytes";
pub(crate) const CONTEXT: &str = "context";
pub(crate) const BODY: &str = "body";
pub(crate) const RAW: &str = "raw";
pub(crate) const MTIME: &str = "mtime";
pub(crate) const SIZE: &str = "size";

/// Handles of every schema field.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fields {
    pub(crate) path: Field,
    pub(crate) mdpath: Field,
    pub(crate) bytes: Field,
    pub(crate) section_bytes: Field,
    pub(crate) context: Field,
    pub(crate) body: Field,
    pub(crate) raw: Field,
    pub(crate) mtime: Field,
    pub(crate) size: Field,
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
            raw: field(RAW)?,
            mtime: field(MTIME)?,
            size: field(SIZE)?,
        })
    }
}

/// Builds the schema of a fresh index.
///
/// `path` is a fast field so a refresh can enumerate indexed files without
/// loading stored documents; `mtime` and `size` are fast fields for the same
/// reason. `context` and `body` are stemmed English text and never stored;
/// `raw`, `bytes`, and `section_bytes` are stored and never indexed, and
/// `section_bytes` is absent on section and root units.
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
        body: builder.add_text_field(BODY, stemmed),
        raw: builder.add_text_field(RAW, STORED),
        mtime: builder.add_u64_field(MTIME, FAST),
        size: builder.add_u64_field(SIZE, FAST),
    };
    (builder.build(), fields)
}

/// Score multiplier for a heading chain that contains every query word.
const CONTEXT_BOOST: f32 = 2.0;

/// Builds the query for `words` within `prefix`: a hit must match every word
/// on its own `body` text, and a `context` (heading chain) that also contains
/// every word only raises the score — it can never satisfy the query by
/// itself. A non-empty `prefix` (a repository-relative directory ending in
/// `/`) additionally restricts hits to paths under it.
/// Both parses are lenient: unparseable fragments are dropped rather than
/// reported, and when no body term survives, the body parse is returned
/// unchanged.
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
    if !has_terms {
        return body;
    }
    let context = BoostQuery::new(parse(fields.context), CONTEXT_BOOST);
    let mut clauses: Vec<(Occur, Box<dyn Query>)> =
        vec![(Occur::Must, body), (Occur::Should, Box::new(context))];
    if let Some(under_prefix) = path_prefix_query(fields.path, prefix) {
        clauses.push((Occur::Must, Box::new(under_prefix)));
    }
    Box::new(BooleanQuery::new(clauses))
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
                ))
                .expect("add");
        }
        writer.commit().expect("commit");

        let searcher = index.reader().expect("reader").searcher();
        let paths_for = |prefix: &str| {
            let query = build_query(&index, &fields, "kumquat", prefix);
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
        assert_eq!(paths_for("docs/"), ["docs/a.md", "docs/sub/c.md"]);
        assert_eq!(paths_for("docs/sub/"), ["docs/sub/c.md"]);
        assert_eq!(paths_for("").len(), 5);
    }
}
