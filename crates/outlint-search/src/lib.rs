//! Full-text search over Markdown documents by outline.
//!
//! A prototype: every Markdown file under a search root is split into *index
//! units* — one per section heading, one per visible preamble block, and one
//! for the document root when it has frontmatter or a preamble — and each
//! unit is stored as one tantivy document addressed by its
//! [`outlint_core::DocumentPath`]. Searching returns the best-scoring units
//! with their source slices.
//!
//! The split follows the workspace convention: [`index_units`] and
//! [`render_hits`] are pure functions over text; [`Store`] and the walk are
//! the filesystem shell around them.

#![warn(missing_docs)]

mod index;
mod render;
mod store;
mod units;

pub use render::{render_hits, Hit};
pub use store::{walk_markdown, Scope, Store, WalkedFile};
pub use units::{index_units, IndexUnit};
