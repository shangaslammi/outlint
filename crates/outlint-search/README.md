# outlint-search

Full-text search over Markdown documents by outline.

The crate splits Markdown source into addressable index units and maintains a
Tantivy index for searching those units. It is used by the opt-in `search`
functionality of the `outlint` command-line tool.
