//! The tantivy schema and query construction shared by the store.

use tantivy::{
    query::{Query, QueryParser},
    schema::{
        Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, FAST, STORED, STRING,
    },
    Index,
};

/// Bumped whenever the schema or the unit derivation changes incompatibly;
/// the store rebuilds an index recorded under another version.
pub(crate) const INDEX_FORMAT_VERSION: u32 = 1;

/// Field names, so the schema and the fast-field readers agree.
pub(crate) const PATH: &str = "path";
pub(crate) const MDPATH: &str = "mdpath";
pub(crate) const LINE: &str = "line";
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
    pub(crate) line: Field,
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
            line: field(LINE)?,
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
/// `raw` is stored and never indexed.
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
        line: builder.add_u64_field(LINE, STORED),
        context: builder.add_text_field(CONTEXT, stemmed.clone()),
        body: builder.add_text_field(BODY, stemmed),
        raw: builder.add_text_field(RAW, STORED),
        mtime: builder.add_u64_field(MTIME, FAST),
        size: builder.add_u64_field(SIZE, FAST),
    };
    (builder.build(), fields)
}

/// Parses `words` leniently as a conjunction over `context` and `body`, with
/// context matches weighted twice. Unparseable fragments are dropped rather
/// than reported: a search either finds something or it does not.
pub(crate) fn build_query(index: &Index, fields: &Fields, words: &str) -> Box<dyn Query> {
    let mut parser = QueryParser::for_index(index, vec![fields.context, fields.body]);
    parser.set_conjunction_by_default();
    parser.set_field_boost(fields.context, 2.0);
    let (query, _errors) = parser.parse_query_lenient(words);
    query
}
