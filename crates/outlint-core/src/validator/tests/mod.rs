//! Shared helpers for the validator's unit tests.

mod constraints;
mod engine;
mod frontmatter;
mod ordering;
mod prepare;

use crate::validator::{validate, DiagnosticId, DiagnosticTarget};
use crate::{load_schema, parse_markdown, MarkdownOptions};

fn ids_and_targets(schema: &str, markdown: &str) -> Vec<(DiagnosticId, DiagnosticTarget)> {
    let loaded = load_schema(schema).expect("test schema is valid");
    let document = parse_markdown(markdown, MarkdownOptions::default());
    validate(&loaded.schema, &document)
        .expect("schema prepares")
        .into_iter()
        .map(|diagnostic| (diagnostic.id, diagnostic.target))
        .collect()
}
