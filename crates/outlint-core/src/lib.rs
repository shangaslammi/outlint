//! Pure schema loading and Markdown outline modeling for Outlint.
//!
//! Filesystem access, diagnostic formatting, and process behavior belong to
//! the CLI crate; this crate converts source text into normalized values.

mod case_fold;
mod load_result;
mod loader;
mod markdown;
mod schema;
mod validator;

pub use load_result::*;
pub use loader::*;
pub use markdown::*;
pub use schema::*;
pub use validator::*;
