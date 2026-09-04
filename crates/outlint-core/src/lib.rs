#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

mod case_fold;
mod load_result;
mod loader;
mod locator;
mod markdown;
mod matcher;
mod regex_capture;
mod schema;
mod typed_value;
mod validator;
mod yaml;

pub use load_result::{
    ByteOffset, CapturePath, ConstraintIndex, ConstraintPath, InvalidSchema,
    JsonSchemaExternalReference, JsonSchemaResourceContents, JsonSchemaResourceInput,
    LinkedJsonSchemaInput, LoadSchemaResult, LoadedSchema, OrderEntryPath, OrderIndex,
    RelatedLocation, RuleIndex, RulePath, SchemaError, SchemaErrorKind, SchemaLocations,
    SchemaNode, SchemaSource, SchemaSources, ScopePath, SourceId, SourceLabel, SourceRange,
    TextRange,
};
pub use loader::{
    json_schema_external_references, linked_frontmatter_schema_path, load_schema,
    load_schema_with_label, load_schema_with_resources,
};
pub use markdown::{
    parse_markdown, Document, DocumentFrontmatter, FrontmatterAnchor, FrontmatterAnchors,
    FrontmatterLocation, Heading, HeadingLocation, MarkdownOptions, Section, SuppressedDiagnostic,
    Suppressions,
};
pub use schema::{
    AtLeastTwo, BoundRuleStep, CanonicalFloat, CanonicalInteger, CaptureName, Cardinality,
    Constraint, ExactText, FrontmatterCapture, FrontmatterCaptureView, FrontmatterCaptures,
    FrontmatterPolicy, FrontmatterScalar, FrontmatterSchema, GlobPattern, HeaderLevel, Matcher,
    NonEmpty, Options, OutlineProvenance, Proposition, RefAnchor, RegexPattern,
    ResolvedFrontmatterCapture, ResolvedFrontmatterQuery, ResolvedIntrinsicTextLocator,
    ResolvedOutlineLocator, ResolvedRuleCaptureLocator, ResolvedRuleLocator, RuleCapture, RuleId,
    RuleOutcome, Schema, SchemaVersion, SectionRule, UpperBound, ValueOrderDirection,
    ValueOrderEntry,
};
pub use validator::{
    validate, Diagnostic, DiagnosticId, DiagnosticLocation, DiagnosticReference, DiagnosticTarget,
    FrontmatterBlock, FrontmatterLineRange, HeaderPath, InvolvedHeader, PrepareValidationError,
    PreparedValidator, ValidationError, ValidationOperationalError,
};
