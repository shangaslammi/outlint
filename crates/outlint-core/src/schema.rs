//! The normalized, type-safe representation of an Outlint schema.
//!
//! These types intentionally do not mirror the YAML document one-for-one.
//! Surface syntax such as `required`, dotted references, slash-delimited
//! regular expressions, and `"n"` repeat bounds is expected to be normalized
//! by the schema loader before constructing this model.

/// A parsed Outlint schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schema {
    /// The schema language version used by this document.
    pub version: SchemaVersion,
    /// Matcher for the single header immediately above the root scope.
    pub title: Option<Matcher>,
    /// Document parsing and matching options, with defaults already applied.
    pub options: Options,
    /// Rules for headers in the root scope, in first-match order.
    pub sections: Vec<SectionRule>,
    /// Presence and ordering constraints attached to the root scope.
    pub constraints: Vec<Constraint>,
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
    /// Exactly one referenced rule path must be satisfied.
    OneOf(NonEmpty<RuleRef>),
    /// At least one referenced rule path must be satisfied.
    AnyOf(NonEmpty<RuleRef>),
    /// Zero or one referenced rule path may be satisfied.
    AtMostOne(NonEmpty<RuleRef>),
    /// Either all referenced rule paths or none of them must be satisfied.
    AllOrNone(NonEmpty<RuleRef>),
    /// If `condition` is satisfied, every `consequence` must be satisfied.
    Requires {
        condition: RuleRef,
        consequences: NonEmpty<RuleRef>,
    },
    /// If `condition` is satisfied, every `exclusion` must be unsatisfied.
    Conflicts {
        condition: RuleRef,
        exclusions: NonEmpty<RuleRef>,
    },
    /// First occurrences must follow the order of these references.
    Ordered(NonEmpty<RuleRef>),
}

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
    /// Resolve from the schema's root scope; the normalized form of `$`.
    SchemaRoot,
}

/// A collection statically guaranteed to contain at least one item.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NonEmpty<T> {
    pub first: T,
    pub rest: Vec<T>,
}
