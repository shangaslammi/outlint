//! Public diagnostic vocabulary produced by validation.

use crate::{
    Matcher, ResolvedFrontmatterCapture, ResolvedFrontmatterQuery, ResolvedRuleLocator, SchemaNode,
    TextRange,
};
use std::{error::Error, fmt};

/// A stable identifier from the diagnostic vocabulary in specification §6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DiagnosticId {
    /// A heading is more than one level below its nearest parent.
    SkippedLevel,
    /// A present heading is denied by its first matching rule or title matcher.
    NotAllowed,
    /// A heading has no matching rule in a strict scope.
    UnexpectedSection,
    /// No heading matched a rule whose minimum is nonzero.
    MissingSection,
    /// Some headings matched a rule, but fewer than its minimum.
    TooFewSections,
    /// More headings matched a rule than its finite maximum, or the document
    /// holds more than one `h1` under a sugar schema.
    TooManySections,
    /// The schema declares a title but the document has none.
    MissingTitle,
    /// A required frontmatter block is absent.
    MissingFrontmatter,
    /// A present frontmatter block is forbidden by the schema.
    ForbiddenFrontmatter,
    /// A frontmatter block is not a valid YAML mapping.
    InvalidFrontmatter,
    /// A frontmatter value fails its JSON Schema.
    FrontmatterSchema,
    /// A capture's source fails its type's lexical, kind, calendar, SemVer,
    /// or bound requirement (§2.4).
    InvalidValue,
    /// A capture declared `required: true` selected no usable value (§2.3).
    MissingValue,
    /// A rule's matches are not in the value order its `order` declares
    /// (§3.8).
    OrderViolation,
    /// An `one_of` constraint does not have exactly one satisfied ref.
    OneOf,
    /// An `any_of` constraint has no satisfied ref.
    AnyOf,
    /// An `at_most_one` constraint has more than one satisfied ref.
    AtMostOne,
    /// An `all_or_none` constraint has some but not all refs satisfied.
    AllOrNone,
    /// A `requires` condition is satisfied without every consequence.
    Requires,
    /// A `conflicts` condition and at least one exclusion are both satisfied.
    Conflicts,
    /// Concrete occurrences violate an explicit constraint or a scope's rule order.
    Ordered,
}

impl fmt::Display for DiagnosticId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl DiagnosticId {
    /// Returns the public, suppression-compatible spelling of this id.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SkippedLevel => "skipped-level",
            Self::NotAllowed => "not-allowed",
            Self::UnexpectedSection => "unexpected-section",
            Self::MissingSection => "missing-section",
            Self::TooFewSections => "too-few-sections",
            Self::TooManySections => "too-many-sections",
            Self::MissingTitle => "missing-title",
            Self::MissingFrontmatter => "missing-frontmatter",
            Self::ForbiddenFrontmatter => "forbidden-frontmatter",
            Self::InvalidFrontmatter => "invalid-frontmatter",
            Self::FrontmatterSchema => "frontmatter-schema",
            Self::InvalidValue => "invalid-value",
            Self::MissingValue => "missing-value",
            Self::OrderViolation => "order-violation",
            Self::OneOf => "one_of",
            Self::AnyOf => "any_of",
            Self::AtMostOne => "at_most_one",
            Self::AllOrNone => "all_or_none",
            Self::Requires => "requires",
            Self::Conflicts => "conflicts",
            Self::Ordered => "ordered",
        }
    }
}

/// A path of case-preserving visible heading texts.
///
/// A header path is always the complete document-tree ancestor chain, from the
/// document's topmost enclosing heading down to the header itself. It does not
/// begin at the root scope: an enclosing `h1`, which is the title when the
/// document has one, is part of the path. Two same-named sections under
/// different ancestors therefore have different paths.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct HeaderPath(pub Vec<String>);

impl HeaderPath {
    /// Returns the heading texts in ancestor-to-descendant order.
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }
}

impl fmt::Display for HeaderPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, heading) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str(" > ")?;
            }
            formatter.write_str(heading)?;
        }
        Ok(())
    }
}

/// A source anchor in the Markdown document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiagnosticLocation {
    /// The source line to highlight.
    pub range: TextRange,
    /// One-based line number.
    pub line: u64,
    /// One-based byte column.
    pub column: u64,
}

/// A concrete header relevant to a constraint violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvolvedHeader {
    /// The concrete header's document path.
    pub path: HeaderPath,
    /// The concrete header's source anchor.
    pub location: DiagnosticLocation,
}

/// A normalized constraint reference retained for diagnostic presentation.
///
/// One variant per §11.3 reference kind, listed in the order §11.3 states —
/// which is also the order §11.4's total key compares them in.
///
/// Each carries its locator source as a required part of the resolved
/// locator, never as an `Option`: a reference §11.3 must quote a spelling for
/// cannot exist without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticReference {
    /// A bound outline locator terminating at a rule, paired with that rule's
    /// matcher. §11.3 renders this as reference kind `rule`.
    Rule {
        /// The bound locator, retaining its exact schema spelling.
        locator: ResolvedRuleLocator,
        /// Matcher of the rule the locator's terminal step named.
        matcher: Matcher,
    },
    /// An `fm[...]` proposition. §11.3 renders this as reference kind
    /// `frontmatter_query`.
    FrontmatterQuery(ResolvedFrontmatterQuery),
    /// An `fm.<name>` reference to a declared frontmatter capture. §11.3
    /// renders this as reference kind `frontmatter_capture`.
    FrontmatterCapture(ResolvedFrontmatterCapture),
}

/// What a diagnostic is about.
///
/// The four cases carry text of different provenance, and conflating them in
/// one [`HeaderPath`] silently mixes document text with schema text. Only
/// [`Self::Header`] names text that occurs in the document; the matcher label
/// in [`Self::MissingHeader`] comes from the schema and may occur nowhere in
/// the document; [`Self::Document`] and [`Self::Frontmatter`] name no header
/// at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticTarget {
    /// A header that exists in the document, named by its document path.
    Header(HeaderPath),
    /// A section the schema requires but the document does not contain.
    MissingHeader {
        /// Document path of the header whose scope should have contained it.
        ///
        /// Empty when no header encloses the missing section: it belongs to the
        /// document root's scope (including a missing `h1` title), or the
        /// sugar's single-`h1` voice reports its `sections` scope as the
        /// document's.
        parent: HeaderPath,
        /// Label of the unsatisfied schema matcher: exact text, a glob, a
        /// slash-delimited regex, or `*`. This is schema text, not a heading.
        matcher: String,
    },
    /// The document as a whole, when no single header can name the violation.
    ///
    /// Used for the document root's scope, which has no parent header, and for
    /// the sugar's single-`h1` document voice described by specification §6.2.
    Document,
    /// A frontmatter block, or a value inside one. Has no header path.
    Frontmatter {
        /// The offending block, absent only when the document has none.
        block: Option<FrontmatterBlock>,
    },
}

/// The frontmatter block a diagnostic is about, and the value within it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontmatterBlock {
    /// One-based inclusive line range of the complete frontmatter block.
    pub line_range: FrontmatterLineRange,
    /// JSON Pointer of a value rejected by JSON Schema, when applicable.
    pub json_pointer: Option<String>,
}

/// One validation violation, with both document and schema-side anchors.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Diagnostic {
    /// Stable diagnostic category.
    pub id: DiagnosticId,
    /// What the diagnostic is about: a header, a missing one, the document, or
    /// frontmatter.
    pub target: DiagnosticTarget,
    /// Primary Markdown source anchor.
    pub location: DiagnosticLocation,
    /// Structural schema node responsible for the diagnostic, when one exists.
    pub schema_node: Option<SchemaNode>,
    /// Concrete headers participating in a constraint violation.
    pub involved_headers: Vec<InvolvedHeader>,
    /// Normalized references participating in a constraint violation.
    pub references: Vec<DiagnosticReference>,
    /// Human-readable context; callers should key behavior on [`Self::id`].
    pub message: String,
}

/// One-based inclusive line range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrontmatterLineRange {
    /// First line covered by the range.
    pub start_line: u64,
    /// Last line covered by the range.
    pub end_line: u64,
}

/// Failure to prepare a reusable validator from a semantic schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareValidationError {
    /// Human-readable compilation failure.
    pub message: String,
}

impl fmt::Display for PrepareValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for PrepareValidationError {}

/// Failure to complete validation of one document.
///
/// This is the runtime half of the validation error channel. It reports that
/// validation did not finish, so no verdict exists for the document. It is
/// never used to report a rule violation: those are [`Diagnostic`]s, and a
/// document that validates successfully returns its complete diagnostic set
/// however large that set is.
///
/// Returning this instead of a diagnostic list makes "partial diagnostics plus
/// failure" unrepresentable. A document yields either every diagnostic it has
/// or an operational failure, never a truncated list that reads as a clean
/// document (specification §11.5).
///
/// One thing reaches it today: §4.6's resource limit on evaluating an
/// `fm[...]` query. That section provides for it — "if an
/// implementation-specific resource limit prevents completion, validation has
/// not produced a document verdict and the CLI MUST surface an operational
/// error (§11.5), not a partial diagnostic set" — and the limit cannot be
/// reached by a guaranteed-core query at any document size.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ValidationOperationalError {
    /// Human-readable description of why validation could not complete.
    pub message: String,
}

impl ValidationOperationalError {
    /// Builds an operational failure from a human-readable description.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ValidationOperationalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ValidationOperationalError {}

/// Failure of the one-shot [`validate`] entry point.
///
/// [`validate`] both prepares a validator and runs it, so it can fail in either
/// of two unrelated ways. Callers that prepare once and validate repeatedly use
/// [`PrepareValidationError`] and [`ValidationOperationalError`] directly and
/// never see this type.
///
/// [`validate`]: crate::validate
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValidationError {
    /// The schema could not be compiled into a reusable validator.
    Preparation(PrepareValidationError),
    /// The schema compiled, but validating the document did not complete.
    Operational(ValidationOperationalError),
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Preparation(error) => error.fmt(formatter),
            Self::Operational(error) => error.fmt(formatter),
        }
    }
}

impl Error for ValidationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Preparation(error) => Some(error),
            Self::Operational(error) => Some(error),
        }
    }
}

impl From<PrepareValidationError> for ValidationError {
    fn from(error: PrepareValidationError) -> Self {
        Self::Preparation(error)
    }
}

impl From<ValidationOperationalError> for ValidationError {
    fn from(error: ValidationOperationalError) -> Self {
        Self::Operational(error)
    }
}

#[cfg(test)]
mod tests {
    use super::DiagnosticId;

    /// §6.3 fixes these spellings, and `outlint-disable` matches on them as
    /// text, so a renamed variant that forgot its spelling would silently
    /// stop being suppressible.
    #[test]
    fn diagnostic_ids_use_the_public_spellings() {
        let expected = [
            (DiagnosticId::SkippedLevel, "skipped-level"),
            (DiagnosticId::NotAllowed, "not-allowed"),
            (DiagnosticId::UnexpectedSection, "unexpected-section"),
            (DiagnosticId::MissingSection, "missing-section"),
            (DiagnosticId::TooFewSections, "too-few-sections"),
            (DiagnosticId::TooManySections, "too-many-sections"),
            (DiagnosticId::MissingTitle, "missing-title"),
            (DiagnosticId::MissingFrontmatter, "missing-frontmatter"),
            (DiagnosticId::ForbiddenFrontmatter, "forbidden-frontmatter"),
            (DiagnosticId::InvalidFrontmatter, "invalid-frontmatter"),
            (DiagnosticId::FrontmatterSchema, "frontmatter-schema"),
            (DiagnosticId::InvalidValue, "invalid-value"),
            (DiagnosticId::MissingValue, "missing-value"),
            (DiagnosticId::OrderViolation, "order-violation"),
            (DiagnosticId::OneOf, "one_of"),
            (DiagnosticId::AnyOf, "any_of"),
            (DiagnosticId::AtMostOne, "at_most_one"),
            (DiagnosticId::AllOrNone, "all_or_none"),
            (DiagnosticId::Requires, "requires"),
            (DiagnosticId::Conflicts, "conflicts"),
            (DiagnosticId::Ordered, "ordered"),
        ];
        for (id, spelling) in expected {
            assert_eq!(id.as_str(), spelling);
            assert_eq!(id.to_string(), spelling);
        }
    }
}
