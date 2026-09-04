//! Pure validation of a parsed Markdown outline against a normalized schema.
//!
//! Validation is deliberately separate from parsing and IO: callers can load
//! and parse fixture text once, then pass only values to [`validate`].

mod constraints;
mod diagnostic;
mod engine;
mod frontmatter_values;
mod prepare;
mod value_order;

#[cfg(test)]
mod tests;

pub use diagnostic::{
    Diagnostic, DiagnosticId, DiagnosticLocation, DiagnosticReference, DiagnosticTarget,
    FrontmatterBlock, FrontmatterLineRange, HeaderPath, InvolvedHeader, PrepareValidationError,
    ValidationError, ValidationOperationalError,
};

use crate::{Document, Schema};

use prepare::ValidationPlan;

/// A schema compiled once for validating any number of documents.
pub struct PreparedValidator {
    schema: Schema,
    plan: ValidationPlan,
}

impl PreparedValidator {
    /// Compiles matchers and the immutable JSON Schema resource registry.
    ///
    /// Callers should pass a [`Schema`] produced by the loader. Preparation is
    /// not a substitute for the loader's semantic checks when a schema has
    /// been assembled manually from its public fields.
    ///
    /// # Errors
    ///
    /// Returns an error if a matcher or frontmatter JSON Schema cannot be compiled.
    /// A schema returned by the loader has already passed equivalent checks,
    /// but preparation retains a defensive failure path rather than assuming
    /// every caller obtained the value from that boundary.
    pub fn new(schema: &Schema) -> Result<Self, PrepareValidationError> {
        Ok(Self {
            schema: schema.clone(),
            plan: ValidationPlan::new(schema)?,
        })
    }

    /// Validates one parsed document without recompiling schema state.
    ///
    /// Frontmatter validation is included, and the `fm[...]` propositions in
    /// constraints evaluate against the document's frontmatter (§4.6).
    ///
    /// Diagnostic order is deterministic for a given schema and document but
    /// follows the validation walk and is not a contract of this crate: a
    /// refactor may reorder it between releases. Callers that promise an
    /// output order must sort on diagnostic content, as the CLI does with a
    /// documented total key.
    ///
    /// # Errors
    ///
    /// Returns a [`ValidationOperationalError`] if validation could not run to
    /// completion, in which case the document has no verdict. Success carries
    /// the document's complete diagnostic set, so a caller can never observe a
    /// partial set that reads as a clean document (§11.5).
    ///
    /// The present engine has no failure path and always returns `Ok`. The
    /// result type is the channel through which the evaluation limits of
    /// JSONPath frontmatter propositions will surface.
    pub fn validate(
        &self,
        document: &Document,
    ) -> Result<Vec<Diagnostic>, ValidationOperationalError> {
        Ok(engine::validate_document(
            &self.schema,
            document,
            &self.plan,
        ))
    }
}

/// Prepares and validates one document.
///
/// Use [`PreparedValidator`] directly when validating multiple documents.
/// Diagnostic order is deterministic but not a contract; see
/// [`PreparedValidator::validate`].
///
/// # Errors
///
/// Returns [`ValidationError::Preparation`] if the schema cannot be compiled,
/// or [`ValidationError::Operational`] if validation could not run to
/// completion.
///
/// # Example
///
/// ```
/// use outlint_core::{load_schema, parse_markdown, validate, MarkdownOptions};
///
/// let loaded = load_schema("version: 1\ntitle: '*'\nsections: []\n")?;
/// let document = parse_markdown("# Guide\n", MarkdownOptions::default());
/// let diagnostics = validate(&loaded.schema, &document)
///     .expect("the loaded schema compiles and validation completes");
///
/// assert!(diagnostics.is_empty());
/// # Ok::<(), outlint_core::InvalidSchema>(())
/// ```
pub fn validate(schema: &Schema, document: &Document) -> Result<Vec<Diagnostic>, ValidationError> {
    let prepared = PreparedValidator::new(schema)?;
    Ok(prepared.validate(document)?)
}
