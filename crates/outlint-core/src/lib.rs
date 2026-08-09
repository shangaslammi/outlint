//! Pure schema loading and Markdown outline modeling for Outlint.
//!
//! Filesystem access, diagnostic formatting, and process behavior belong to
//! the CLI crate; this crate converts source text into normalized values.

mod loader;
mod markdown;
mod parser;
mod schema;

pub use loader::*;
pub use markdown::*;
pub use parser::*;
pub use schema::*;
