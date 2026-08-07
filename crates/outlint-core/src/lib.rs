//! outlint-core: schema model, markdown outline parser, validator.
//! Public API is intentionally coarse: validate(schema, doc) -> diagnostics.

pub mod diagnostics;

pub use diagnostics::Diagnostic;

/// Validate a markdown document against an outlint schema.
/// Both inputs are source text. Returns a list of diagnostics;
/// an empty list means the document conforms.
pub fn validate(_schema_yaml: &str, _markdown: &str) -> Vec<Diagnostic> {
    // Implementation follows spec/outlint-spec.md section 8.
    unimplemented!("validator not yet implemented")
}
