//! Types returned when loading an Outlint schema from source text.
//!
//! Source provenance lives in this layer rather than in the semantic
//! [`Schema`]. A successfully loaded schema can therefore be compared and used
//! independently of its original formatting while diagnostics can still point
//! back to the declarations that produced it. Provenance is multi-source so a
//! load error in an external frontmatter JSON Schema can name that file rather
//! than being incorrectly anchored to its path in the primary document.

use std::{collections::BTreeMap, error::Error, fmt, sync::Arc};

use crate::schema::{CaptureName, NonEmpty, Schema};

/// The result of parsing, validating, and normalizing a schema document.
pub type LoadSchemaResult = Result<LoadedSchema, InvalidSchema>;

/// One attempted JSON Schema resource supplied to the pure schema loader.
///
/// The outer I/O shell assigns a stable logical `uri` for reference
/// resolution. The display `label` and contents remain provenance only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonSchemaResourceInput {
    /// Logical absolute URI used by JSON Schema reference resolution.
    pub uri: String,
    /// Human-readable filesystem path or caller label.
    pub label: Option<SourceLabel>,
    /// Either the complete UTF-8 document or the shell's read failure.
    pub contents: JsonSchemaResourceContents,
}

/// Contents of one attempted linked JSON Schema resource read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonSchemaResourceContents {
    /// Complete UTF-8 JSON document.
    Loaded(Arc<str>),
    /// Exact filesystem or UTF-8 error produced by the I/O shell.
    ReadFailure(String),
}

/// Complete immutable input graph for one linked frontmatter JSON Schema.
///
/// Every attempted local resource reachable from `root_uri` must be present,
/// including failed reads. Missing, unreadable, or remote resources are
/// reported as `invalid-frontmatter-schema` rather than being retrieved by
/// core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedJsonSchemaInput {
    /// URI of the resource named by `frontmatter.schema`.
    pub root_uri: String,
    /// Root and transitive resource documents, keyed by each entry's `uri`.
    pub resources: Vec<JsonSchemaResourceInput>,
}

/// One external JSON Schema document reference resolved for both I/O and validation.
///
/// Keeping the two identities in one value prevents a shell from accidentally
/// using a `$id`-rebased URI as the location of a file to preload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonSchemaExternalReference {
    /// Target resolved from the resource's lexical file URI without applying `$id`.
    pub physical_uri: String,
    /// Target resolved according to JSON Schema base-URI and `$id` semantics.
    pub logical_uri: String,
}

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
    /// The primary document and any external JSON Schema source attempts made
    /// before loading failed.
    pub sources: SchemaSources,
    /// One or more syntax, shape, or schema-validation errors.
    pub errors: NonEmpty<SchemaError>,
}

impl fmt::Display for InvalidSchema {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let additional = self.errors.rest.len();
        write!(formatter, "{}", self.errors.first)?;
        if additional > 0 {
            write!(formatter, " (and {additional} more schema error")?;
            if additional != 1 {
                formatter.write_str("s")?;
            }
            formatter.write_str(")")?;
        }
        Ok(())
    }
}

impl Error for InvalidSchema {}

/// The available source text and optional display name of a schema document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSource {
    /// A path, URI, or caller-provided label used when presenting diagnostics.
    pub label: Option<SourceLabel>,
    /// The complete original text, or empty when reading this source failed.
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
    /// The policy's `schema` value in the primary Outlint schema source.
    ///
    /// This is either the linked schema path or the complete inline mapping.
    FrontmatterSchemaDeclaration,
    /// The parsed JSON Schema root document.
    ///
    /// For a linked schema this points into the external source while
    /// [`Self::FrontmatterSchemaDeclaration`] remains in the primary source.
    /// For an inline schema both nodes cover its mapping in the primary source.
    FrontmatterSchemaDocument,
    /// A section rule at a structural path.
    Rule(RulePath),
    /// One capture declared by a section rule (§2.1).
    Capture(CapturePath),
    /// One capture declared by `frontmatter.captures` (§2.3).
    ///
    /// Frontmatter captures occupy their own named scope rooted at `fm`
    /// (§4.3), so a name alone addresses one; there are no owning rule
    /// coordinates to carry.
    FrontmatterCapture(CaptureName),
    /// One entry of a section rule's `order` list (§2.1, §3.8).
    OrderEntry(OrderEntryPath),
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

/// The structural address of one rule capture declaration.
///
/// A capture is addressed by its owning rule's coordinates plus its name,
/// because a rule's `captures` mapping has no semantic order for an index to
/// name (§2.1).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapturePath {
    /// The rule that declares the capture.
    pub rule: RulePath,
    /// The declared capture name.
    pub name: CaptureName,
}

/// The structural address of one `order` entry.
///
/// Unlike a capture, an order entry is addressed positionally: `order` is a
/// list whose entry order is semantic (§3.8).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OrderEntryPath {
    /// The rule that declares the `order` list.
    pub rule: RulePath,
    /// The entry's zero-based index within that list.
    pub order_index: OrderIndex,
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
/// Each index selects a rule whose child scope contains the next index. For an
/// `outline:` schema, the empty path denotes [`Schema::outline`]. For a
/// `title:` + `sections:` sugar schema, it denotes the synthesized title
/// rule's child scope — the source's top-level `sections:` list. This preserves
/// the public addressing of the source form after normalization.
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

/// A zero-based entry index within one rule's `order` list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct OrderIndex(pub usize);

/// A half-open byte range in [`SchemaSource::text`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextRange {
    /// Inclusive byte offset at which the range begins.
    pub start: ByteOffset,
    /// Exclusive byte offset at which the range ends.
    pub end: ByteOffset,
}

/// A byte offset in UTF-8 source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ByteOffset(pub usize);

/// A byte range associated with one source in [`SchemaSources`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceRange {
    /// Document containing the range.
    pub source: SourceId,
    /// Half-open byte range within that document's source text.
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

impl fmt::Display for SchemaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.kind, self.message)
    }
}

impl Error for SchemaError {}

/// A secondary source range attached to a schema error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelatedLocation {
    /// Secondary source range relevant to the error.
    pub range: SourceRange,
    /// Explanation of how the secondary location relates to the error.
    pub message: String,
}

/// Machine-readable categories for schema loading failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
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
    /// A capture declaration is malformed, repeated, unsupported, or declared
    /// where §2.1 and §2.3 do not permit one.
    InvalidCapture,
    /// An `order` list, or one of its entries, is malformed (§2.1).
    InvalidOrder,
    /// An ordered constraint uses a non-positional frontmatter proposition,
    /// mixes scopes, descends through a repeatable ancestor, or targets a
    /// scope already ordered by its rule list.
    OrderedScopeMismatch,
    /// A rule declares both `required` and `repeat`, or denies a cardinality.
    ConflictingCardinality,
    /// The frontmatter policy both requires and forbids frontmatter.
    ConflictingFrontmatter,
    /// A schema declares `outline` together with `title` or `sections`.
    ConflictingOutline,
    /// A frontmatter JSON Schema is malformed or uses an unsupported dialect.
    InvalidFrontmatterSchema,
}

impl SchemaErrorKind {
    /// Returns the stable diagnostic spelling defined by specification §6.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Syntax => "syntax",
            Self::InvalidDocumentShape => "invalid-document-shape",
            Self::UnsupportedVersion => "unsupported-version",
            Self::DuplicateId => "duplicate-id",
            Self::UnresolvedRef => "unresolved-ref",
            Self::ForbiddenRef => "forbidden-ref",
            Self::DuplicateRef => "duplicate-ref",
            Self::ReservedId => "reserved-id",
            Self::InvalidMatcher => "invalid-matcher",
            Self::InvalidRepeat => "invalid-repeat",
            Self::InvalidCapture => "invalid-capture",
            Self::InvalidOrder => "invalid-order",
            Self::OrderedScopeMismatch => "ordered-scope-mismatch",
            Self::ConflictingCardinality => "conflicting-cardinality",
            Self::ConflictingFrontmatter => "conflicting-frontmatter",
            Self::ConflictingOutline => "conflicting-outline",
            Self::InvalidFrontmatterSchema => "invalid-frontmatter-schema",
        }
    }
}

impl fmt::Display for SchemaErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::SchemaErrorKind;

    #[test]
    fn schema_error_ids_use_the_public_spellings() {
        let expected = [
            (SchemaErrorKind::Syntax, "syntax"),
            (
                SchemaErrorKind::InvalidDocumentShape,
                "invalid-document-shape",
            ),
            (SchemaErrorKind::UnsupportedVersion, "unsupported-version"),
            (SchemaErrorKind::DuplicateId, "duplicate-id"),
            (SchemaErrorKind::UnresolvedRef, "unresolved-ref"),
            (SchemaErrorKind::ForbiddenRef, "forbidden-ref"),
            (SchemaErrorKind::DuplicateRef, "duplicate-ref"),
            (SchemaErrorKind::ReservedId, "reserved-id"),
            (SchemaErrorKind::InvalidMatcher, "invalid-matcher"),
            (SchemaErrorKind::InvalidRepeat, "invalid-repeat"),
            (SchemaErrorKind::InvalidCapture, "invalid-capture"),
            (SchemaErrorKind::InvalidOrder, "invalid-order"),
            (
                SchemaErrorKind::OrderedScopeMismatch,
                "ordered-scope-mismatch",
            ),
            (
                SchemaErrorKind::ConflictingCardinality,
                "conflicting-cardinality",
            ),
            (
                SchemaErrorKind::ConflictingFrontmatter,
                "conflicting-frontmatter",
            ),
            (SchemaErrorKind::ConflictingOutline, "conflicting-outline"),
            (
                SchemaErrorKind::InvalidFrontmatterSchema,
                "invalid-frontmatter-schema",
            ),
        ];
        for (kind, spelling) in expected {
            assert_eq!(kind.as_str(), spelling);
        }
    }
}
