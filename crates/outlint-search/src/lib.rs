//! Full-text search over Markdown documents by outline.
//!
//! A prototype: every Markdown file under a search root is split into *index
//! units* — one per section heading, one per visible preamble block, one per
//! non-empty direct list item, and one for the document root when it has a
//! frontmatter mapping or a title (a sole H1, which document paths merge into
//! the root) — and each unit is stored as one tantivy document addressed by its
//! [`outlint_core::DocumentPath`]. Searching returns the best-scoring units,
//! each with a snippet of its text chosen around the query words; reading
//! the full content is `outlint read`'s job.
//!
//! The split follows the workspace convention: [`index_units`] and
//! [`render_hits`] are pure functions over text; [`Store`] and the walk are
//! the filesystem shell around them.

#![warn(missing_docs)]

mod index;
mod render;
mod store;
mod units;

pub use render::{render_hits, render_no_hits, Hit, TermCount};
pub use store::{walk_markdown, Scope, Store, Walk, WalkedFile};
pub use units::{index_units, IndexUnit};
