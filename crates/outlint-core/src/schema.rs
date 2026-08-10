//! The normalized, type-safe representation of an Outlint schema.
//!
//! These types intentionally do not mirror the YAML document one-for-one.
//! Surface syntax such as `required`, dotted references, slash-delimited
//! regular expressions, `fm.` propositions, and `"n"` repeat bounds is
//! expected to be normalized by the schema loader before constructing this
//! model.

use std::collections::BTreeMap;

use serde_json::Value as JsonValue;

/// A parsed Outlint schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schema {
    /// The schema language version used by this document.
    pub version: SchemaVersion,
    /// Matcher for the single header immediately above the root scope.
    pub title: Option<Matcher>,
    /// Document parsing and matching options, with defaults already applied.
    pub options: Options,
    /// The normalized frontmatter presence and value-validation policy.
    pub frontmatter: FrontmatterPolicy,
    /// Rules for headers in the root scope, in first-match order.
    pub sections: Vec<SectionRule>,
    /// Presence and ordering constraints attached to the root scope.
    pub constraints: Vec<Constraint>,
}

/// The document's normalized frontmatter policy.
///
/// This representation makes the invalid `required: true, allow: false`
/// combination unrepresentable while retaining a schema declared alongside
/// `allow: false`; if forbidden frontmatter is nevertheless present, the
/// validation algorithm still evaluates that schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontmatterPolicy {
    /// Frontmatter may be absent; validate it against `schema` when present.
    Optional { schema: Option<FrontmatterSchema> },
    /// Frontmatter must be present and is validated against `schema`.
    Required { schema: Option<FrontmatterSchema> },
    /// Frontmatter must not be present; validate it against `schema` if it is
    /// nevertheless present.
    Forbidden { schema: Option<FrontmatterSchema> },
}

/// An opaque, normalized JSON Schema resource graph.
///
/// Construction is restricted to the loader, which checks the dialect,
/// meta-schema, and every reference before this value enters a [`Schema`].
/// Resource identifiers are logical URIs rather than filesystem locations, so
/// moving an otherwise identical schema does not change semantic equality.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontmatterSchema {
    pub(crate) root_uri: String,
    pub(crate) root: JsonValue,
    pub(crate) resources: BTreeMap<String, JsonValue>,
}

/// A supported version of the Outlint schema language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SchemaVersion {
    V1,
}

/// Options controlling Markdown parsing and matcher behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Whether all matcher forms compare text case-sensitively.
    pub match_case: bool,
    /// Whether inline Markdown is reduced to its text before matching.
    pub strip_inline_markup: bool,
    /// Whether a header may be more than one level below its parent.
    pub allow_skipped_levels: bool,
    /// The header level at which the schema's root section rules apply.
    pub root_level: HeaderLevel,
}

/// A Markdown ATX header level.
///
/// Using an enum keeps values outside Markdown's `h1` through `h6` range out
/// of an already-parsed schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum HeaderLevel {
    H1 = 1,
    H2 = 2,
    H3 = 3,
    H4 = 4,
    H5 = 5,
    H6 = 6,
}

impl HeaderLevel {
    /// Returns the next shallower Markdown heading level, if one exists.
    pub const fn predecessor(self) -> Option<Self> {
        match self {
            Self::H1 => None,
            Self::H2 => Some(Self::H1),
            Self::H3 => Some(Self::H2),
            Self::H4 => Some(Self::H3),
            Self::H5 => Some(Self::H4),
            Self::H6 => Some(Self::H5),
        }
    }
}

impl TryFrom<u8> for HeaderLevel {
    type Error = ();

    /// Converts only Markdown's representable h1 through h6 levels.
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::H1),
            2 => Ok(Self::H2),
            3 => Ok(Self::H3),
            4 => Ok(Self::H4),
            5 => Ok(Self::H5),
            6 => Ok(Self::H6),
            _ => Err(()),
        }
    }
}

/// A rule for headers within one scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionRule {
    /// The explicit or generated identifier used by constraints.
    ///
    /// Non-exact matchers without an explicit identifier remain `None`.
    pub id: Option<RuleId>,
    /// The header matcher for this rule.
    pub matcher: Matcher,
    /// Whether matching headers are accepted and, if so, their cardinality.
    pub outcome: RuleOutcome,
    /// Whether headers unmatched by a child rule are rejected.
    pub strict: bool,
    /// Rules for direct child headers, in first-match order.
    pub sections: Vec<SectionRule>,
    /// Presence and ordering constraints attached to the child scope.
    pub constraints: Vec<Constraint>,
}

/// The result of matching a header against a section rule.
///
/// A denied rule has no cardinality, making the invalid combination of
/// `allow: false` and `required`/`repeat` unrepresentable here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleOutcome {
    Allow(Cardinality),
    Deny,
}

/// The permitted number of sibling headers matched by one rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cardinality {
    pub min: u32,
    pub max: UpperBound,
}

/// The inclusive upper bound of a rule's cardinality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpperBound {
    Bounded(u32),
    Unbounded,
}

/// A normalized header matcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Matcher {
    /// Literal header text equality.
    Exact(ExactText),
    /// A pattern in which `*` matches any substring.
    Glob(GlobPattern),
    /// A full-string regular expression, without the delimiting slashes.
    Regex(RegexPattern),
    /// Any header text; the normalized form of `match: "*"`.
    Any,
}

/// Literal text used by an exact matcher.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct ExactText(pub String);

/// The body of a glob matcher.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct GlobPattern(pub String);

/// The body of a regular-expression matcher, without `/` delimiters.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct RegexPattern(pub String);

/// A validated rule identifier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct RuleId(pub String);

/// A cross-section presence or ordering constraint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Constraint {
    /// Exactly one proposition must be satisfied.
    OneOf(AtLeastTwo<Proposition>),
    /// At least one proposition must be satisfied.
    AnyOf(AtLeastTwo<Proposition>),
    /// Zero or one proposition may be satisfied.
    AtMostOne(AtLeastTwo<Proposition>),
    /// Either all propositions or none of them must be satisfied.
    AllOrNone(AtLeastTwo<Proposition>),
    /// If `condition` is satisfied, every `consequence` must be satisfied.
    Requires {
        condition: Proposition,
        consequences: NonEmpty<Proposition>,
    },
    /// If `condition` is satisfied, every `exclusion` must be unsatisfied.
    Conflicts {
        condition: Proposition,
        exclusions: NonEmpty<Proposition>,
    },
    /// Every occurrence of each satisfied ref must precede every occurrence of
    /// the next satisfied ref (`last(A) < first(B)`).
    ///
    /// Frontmatter propositions are excluded because they have no document
    /// position among headers.
    Ordered(AtLeastTwo<RuleRef>),
}

/// A proposition accepted by presence constraints.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Proposition {
    /// Presence of a concrete rule path in the section tree.
    Rule(RuleRef),
    /// Presence or typed equality of a value in document frontmatter.
    Frontmatter(FrontmatterRef),
}

/// A normalized `fm.` frontmatter proposition.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FrontmatterRef {
    /// One or more mapping keys below the frontmatter root.
    pub path: NonEmpty<FrontmatterKey>,
    /// A typed scalar for the equality form, or `None` for presence alone.
    pub equals: Option<FrontmatterScalar>,
}

/// A frontmatter mapping key addressable by the `fm.` syntax.
///
/// The loader ensures this is non-empty and contains neither `.` nor `=`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct FrontmatterKey(pub String);

/// A scalar resolved according to the YAML 1.2 core schema.
///
/// Integer and float values retain arbitrary precision as canonical strings.
/// Their distinct variants preserve the spec's typed equality (`1` is not
/// equal to `1.0`) without forcing a numeric precision limit on schema input.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FrontmatterScalar {
    Null,
    Boolean(bool),
    Integer(CanonicalInteger),
    Float(CanonicalFloat),
    String(String),
}

/// The canonical, arbitrary-precision value of a YAML integer scalar.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct CanonicalInteger(pub String);

/// The canonical, arbitrary-precision value of a YAML float scalar.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct CanonicalFloat(pub String);

/// A normalized reference to a rule path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuleRef {
    /// The scope from which the first path segment is resolved.
    pub anchor: RefAnchor,
    /// One or more rule identifiers forming the path to the target.
    pub path: NonEmpty<RuleId>,
}

/// The starting scope for resolving a rule reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefAnchor {
    /// Resolve from the direct-child scope where the constraint is attached.
    CurrentScope,
    /// Resolve from the schema's root scope; the normalized form of a leading
    /// `$.` in source. `$` alone is not a valid reference.
    SchemaRoot,
}

/// A collection statically guaranteed to contain at least one item.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NonEmpty<T> {
    pub first: T,
    pub rest: Vec<T>,
}

impl<T> NonEmpty<T> {
    /// Iterates every item in collection order.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        std::iter::once(&self.first).chain(&self.rest)
    }
}

/// A collection statically guaranteed to contain at least two items.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AtLeastTwo<T> {
    pub first: T,
    pub second: T,
    pub rest: Vec<T>,
}

impl<T> AtLeastTwo<T> {
    /// Iterates every item in collection order.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        std::iter::once(&self.first)
            .chain(std::iter::once(&self.second))
            .chain(&self.rest)
    }
}
