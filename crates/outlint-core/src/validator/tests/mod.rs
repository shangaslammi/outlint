//! Shared helpers for the validator's unit tests.

mod captures;
mod constraints;
mod dependency_suppression;
mod engine;
mod frontmatter;
mod frontmatter_values;
mod ordering;
mod prepare;
mod value_order;

use crate::validator::{validate, Diagnostic, DiagnosticId, DiagnosticTarget};
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

/// Every diagnostic one schema produces for one document, under the default
/// Markdown options.
fn diagnostics(schema: &str, markdown: &str) -> Vec<Diagnostic> {
    diagnostics_with(schema, markdown, MarkdownOptions::default())
}

/// The same, with the Markdown options spelled out.
///
/// §1.3 gates the matcher input on `strip_inline_markup`, and §2.4 takes a
/// rule capture's source string from that input, so a capture test has to be
/// able to say which input it means.
fn diagnostics_with(schema: &str, markdown: &str, options: MarkdownOptions) -> Vec<Diagnostic> {
    let loaded = load_schema(schema).expect("test schema is valid");
    let document = parse_markdown(markdown, options);
    validate(&loaded.schema, &document).expect("schema prepares")
}
