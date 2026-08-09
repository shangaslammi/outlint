//! Types returned when loading an Outlint schema from source text.
//!
//! Source provenance lives in this layer rather than in the semantic
//! [`Schema`]. A successfully loaded schema can therefore be compared and used
//! independently of its original formatting while diagnostics can still point
//! back to the declarations that produced it. Provenance is multi-source so a
//! load error in an external frontmatter JSON Schema can name that file rather
//! than being incorrectly anchored to its path in the primary document.

use std::{collections::BTreeMap, sync::Arc};

use crate::schema::{NonEmpty, Schema};

/// The result of parsing, validating, and normalizing a schema document.
pub type LoadSchemaResult = Result<LoadedSchema, InvalidSchema>;

/// A valid semantic schema together with its source provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedSchema {
    /// The final normalized schema.
    pub schema: Schema,
    /// The primary document and any external JSON Schema sources it loaded.
    pub sources: SchemaSources,
    /// Locations of semantic nodes retained for later diagnostics.
    pub locations: SchemaLocations,
}

/// A schema document that could not be converted into a valid [`Schema`].
///
/// No partial semantic schema is exposed on failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidSchema {
    /// The primary document and any external JSON Schema sources read before
    /// loading failed.
    pub sources: SchemaSources,
    /// One or more syntax, shape, or schema-validation errors.
    pub errors: NonEmpty<SchemaError>,
}

/// The source text and optional display name of a schema document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSource {
    /// A path, URI, or caller-provided label used when presenting diagnostics.
    pub label: Option<SourceLabel>,
    /// The complete original source text.
    pub text: Arc<str>,
}

/// All source documents participating in one schema load.
///
/// [`SourceId`] values are local to this collection. Keeping source identity
/// in the parser result, instead of in semantic schema nodes, preserves
/// position-independent equality for [`Schema`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSources {
    /// The Outlint schema document requested by the caller.
    pub primary: SourceId,
    /// Source text keyed by the ids used in locations and errors.
    pub documents: BTreeMap<SourceId, SchemaSource>,
}

/// The identity of one source within [`SchemaSources`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct SourceId(pub u32);

/// A human-readable name for a schema source.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct SourceLabel(pub String);

/// Side-car locations for nodes in a successfully built [`Schema`].
///
/// Node addresses follow the structure of the normalized schema rather than
/// using rule ids, which are optional and only locally unique.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaLocations {
    /// The range covering the complete primary Outlint schema document.
    pub document: SourceRange,
    /// Source ranges for semantic nodes needed by validation diagnostics.
    pub nodes: BTreeMap<SchemaNode, SourceRange>,
}

/// The address of a semantic schema node with retained source provenance.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SchemaNode {
    /// The optional title matcher.
    Title,
    /// The normalized frontmatter policy object.
    Frontmatter,
    /// The inline mapping or external path in the policy's `schema` field.
    FrontmatterSchemaDeclaration,
    /// The parsed JSON Schema document.
    ///
    /// For an inline schema this may share a range with its declaration. For
    /// an external schema it points into that external source, while
    /// [`Self::FrontmatterSchemaDeclaration`] remains in the primary source.
    FrontmatterSchemaDocument,
    /// A section rule at a structural path.
    Rule(RulePath),
    /// A constraint at a structural path.
    Constraint(ConstraintPath),
}

/// The structural address of a section rule.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RulePath {
    /// The scope containing the rule.
    pub scope: ScopePath,
    /// The rule's zero-based index within that scope.
    pub index: RuleIndex,
}

/// The structural address of a constraint.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConstraintPath {
    /// The scope containing the constraint.
    pub scope: ScopePath,
    /// The constraint's zero-based index within that scope.
    pub index: ConstraintIndex,
}

/// A path to a rule-owned child scope.
///
/// The empty path denotes the schema root. Each index selects a rule whose
/// child scope contains the next index.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ScopePath(pub Vec<RuleIndex>);

/// A zero-based rule index within one sibling rule list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct RuleIndex(pub usize);

/// A zero-based constraint index within one scope's constraint list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ConstraintIndex(pub usize);

/// A half-open byte range in [`SchemaSource::text`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextRange {
    pub start: ByteOffset,
    pub end: ByteOffset,
}

/// A byte offset in UTF-8 source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ByteOffset(pub usize);

/// A byte range associated with one source in [`SchemaSources`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceRange {
    pub source: SourceId,
    pub range: TextRange,
}

/// A positioned error produced while loading a schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaError {
    /// A machine-readable error category.
    pub kind: SchemaErrorKind,
    /// The primary source range associated with the error.
    pub range: SourceRange,
    /// Additional declarations or values relevant to the error.
    pub related: Vec<RelatedLocation>,
    /// A human-readable explanation.
    pub message: String,
}

/// A secondary source range attached to a schema error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelatedLocation {
    pub range: SourceRange,
    pub message: String,
}

/// Machine-readable categories for schema loading failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SchemaErrorKind {
    /// The input is not syntactically valid YAML or JSON.
    Syntax,
    /// The parsed value does not have the required schema document shape.
    InvalidDocumentShape,
    /// The declared schema version is not supported.
    UnsupportedVersion,
    /// Two rules in one sibling scope resolve to the same id.
    DuplicateId,
    /// A constraint reference does not resolve to a rule.
    UnresolvedRef,
    /// A constraint references a rule that denies matching sections.
    ForbiddenRef,
    /// A constraint contains the same resolved proposition more than once.
    DuplicateRef,
    /// A top-level rule uses the reserved `fm` identifier.
    ReservedId,
    /// A matcher cannot be normalized or compiled.
    InvalidMatcher,
    /// A repeat declaration is malformed or has inconsistent bounds.
    InvalidRepeat,
    /// References in an ordered constraint do not share a concrete scope.
    OrderedScopeMismatch,
    /// A rule declares both `required` and `repeat`, or denies a cardinality.
    ConflictingCardinality,
    /// The frontmatter policy both requires and forbids frontmatter.
    ConflictingFrontmatter,
    /// `root_level` is outside Markdown's supported header levels.
    InvalidRootLevel,
    /// A title matcher was declared while the root scope is at h1.
    InvalidTitleLevel,
    /// A frontmatter JSON Schema is malformed or uses an unsupported dialect.
    InvalidFrontmatterSchema,
}
