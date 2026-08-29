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
///
/// Obtain this value from [`load_schema`](crate::load_schema) or
/// [`load_schema_with_resources`](crate::load_schema_with_resources). Although
/// its normalized fields are public for inspection, constructing them directly
/// can bypass loader-established invariants such as valid references and
/// compiled matchers.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Schema {
    /// The schema language version used by this document.
    pub version: SchemaVersion,
    /// Document parsing and matching options, with defaults already applied.
    pub options: Options,
    /// The normalized frontmatter presence and value-validation policy.
    pub frontmatter: FrontmatterPolicy,
    /// Rules for the document's `h1` headers, in first-match order.
    ///
    /// This is the canonical form of the schema's top level. The
    /// `title:` + `sections:` sugar desugars into a single synthesized rule
    /// here — its matcher is the title matcher (or any-text when no title is
    /// declared), its cardinality is exactly one, and its child rules are the
    /// top-level `sections` list. [`Schema::outline_provenance`] records which
    /// spelling produced the list; public [`ScopePath`]s keep addressing what
    /// the source spelled, so for sugar schemas the empty scope names the
    /// synthesized rule's child scope rather than this list.
    ///
    /// [`ScopePath`]: crate::ScopePath
    pub outline: Vec<SectionRule>,
    /// Presence and ordering constraints attached to the outline (`h1`) scope.
    ///
    /// Only the general `outline:` form can declare these. A sugar schema's
    /// top-level constraints attach to the synthesized rule's child scope
    /// (its `constraints` field) — the scope the `sections` list describes —
    /// so this list is empty for every sugar schema.
    pub constraints: Vec<Constraint>,
    /// How the source document declared its `h1` level.
    pub outline_provenance: OutlineProvenance,
}

impl Schema {
    /// Whether the h1 level was declared through sugar rather than `outline:`.
    ///
    /// Sugar schemas keep their pre-`outline` public addressing: the empty
    /// [`ScopePath`] and the `$.` reference anchor both name the synthesized
    /// rule's child scope (the `sections` list), and the synthesized `h1` rule
    /// itself is addressed as [`SchemaNode::Title`] rather than as a rule.
    ///
    /// [`ScopePath`]: crate::ScopePath
    /// [`SchemaNode::Title`]: crate::SchemaNode::Title
    pub(crate) fn is_sugar(&self) -> bool {
        !matches!(self.outline_provenance, OutlineProvenance::Outline)
    }

    /// The rules the empty public [`ScopePath`] (and the `$.` anchor) names.
    ///
    /// For the general form this is [`Schema::outline`]; for sugar it is the
    /// synthesized rule's child scope, which is what the source's `sections`
    /// list spelled. Rule references and scope walks resolve from here so
    /// that a sugar schema's references keep meaning what they always meant.
    ///
    /// [`ScopePath`]: crate::ScopePath
    pub(crate) fn addressed_root_rules(&self) -> &[SectionRule] {
        if self.is_sugar() {
            self.outline
                .first()
                .map_or(&[], |rule| rule.sections.as_slice())
        } else {
            &self.outline
        }
    }
}

/// The surface form a schema used to declare its `h1` level.
///
/// The loader normalizes every form into [`Schema::outline`], the canonical
/// `h1`-rule list. The provenance records which spelling produced it: the
/// validator keeps `missing-title` and the wrong-title diagnostics anchored at
/// [`SchemaNode::Title`] for the sugar forms, preserves their lax handling of
/// documents without an `h1`, and gives `outline:` and `title: null` their own
/// semantics.
///
/// [`SchemaNode::Title`]: crate::SchemaNode::Title
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutlineProvenance {
    /// `title: <matcher>` with `sections:` — sugar for a single required
    /// `h1` rule whose child rules are the top-level `sections` list.
    Title,
    /// `sections:` without `title:` — desugars like [`Self::Title`] with an
    /// any-text matcher: `title: "*"` implied, so the document must have
    /// exactly one `h1`. A document with none writes `title: null` into its
    /// schema instead. With no `title:` key to blame, title diagnostics
    /// anchor on the `sections` key — the spelling that implied the rule.
    BareSections,
    /// `title: null` — the document is declared to have no `h1`. Desugars to
    /// a denied any-text `h1` rule: a present `h1` is `not-allowed`, and the
    /// `sections` list describes the document's top-level `h2` headers.
    NoTitle,
    /// The general `outline:` form: [`Schema::outline`] is exactly what the
    /// source spelled.
    Outline,
}

/// The document's normalized frontmatter policy.
///
/// This representation makes the invalid `required: true, allow: false`
/// combination unrepresentable while retaining a schema declared alongside
/// `allow: false`; if forbidden frontmatter is nevertheless present, the
/// validation algorithm still evaluates that schema.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrontmatterPolicy {
    /// Frontmatter may be absent; validate it against `schema` when present.
    Optional {
        /// JSON Schema to apply when frontmatter is present.
        schema: Option<FrontmatterSchema>,
    },
    /// Frontmatter must be present and is validated against `schema`.
    Required {
        /// JSON Schema to apply to the required frontmatter mapping.
        schema: Option<FrontmatterSchema>,
    },
    /// Frontmatter must not be present; validate it against `schema` if it is
    /// nevertheless present.
    Forbidden {
        /// JSON Schema to apply if forbidden frontmatter is present.
        schema: Option<FrontmatterSchema>,
    },
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
    /// Version 1 of the schema language.
    V1,
}

/// Options controlling Markdown parsing and matcher behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Options {
    /// Whether all matcher forms compare text case-sensitively.
    pub match_case: bool,
    /// Whether inline Markdown is reduced to its text before matching.
    pub strip_inline_markup: bool,
    /// Whether a header may be more than one level below its parent.
    pub allow_skipped_levels: bool,
    /// Whether a scope's rules bind in document order unless a rule's own
    /// `ordered` says otherwise (specification §3.7).
    pub ordered_sections: bool,
}

impl Options {
    /// Sets case sensitivity for every matcher form.
    pub const fn with_match_case(mut self, match_case: bool) -> Self {
        self.match_case = match_case;
        self
    }

    /// Sets whether inline Markdown is reduced to visible text for matching.
    pub const fn with_strip_inline_markup(mut self, strip_inline_markup: bool) -> Self {
        self.strip_inline_markup = strip_inline_markup;
        self
    }

    /// Sets whether headings may skip a level in the document tree.
    pub const fn with_allow_skipped_levels(mut self, allow_skipped_levels: bool) -> Self {
        self.allow_skipped_levels = allow_skipped_levels;
        self
    }

    /// Sets the default for whether each scope's rules bind in document order.
    pub const fn with_ordered_sections(mut self, ordered_sections: bool) -> Self {
        self.ordered_sections = ordered_sections;
        self
    }
}

impl Default for Options {
    /// Uses the defaults defined by specification §7.
    fn default() -> Self {
        Self {
            match_case: false,
            strip_inline_markup: true,
            allow_skipped_levels: false,
            ordered_sections: true,
        }
    }
}

/// A Markdown ATX header level.
///
/// Using an enum keeps values outside Markdown's `h1` through `h6` range out
/// of an already-parsed schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum HeaderLevel {
    /// A level-one heading (`#`).
    H1 = 1,
    /// A level-two heading (`##`).
    H2 = 2,
    /// A level-three heading (`###`).
    H3 = 3,
    /// A level-four heading (`####`).
    H4 = 4,
    /// A level-five heading (`#####`).
    H5 = 5,
    /// A level-six heading (`######`).
    H6 = 6,
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
#[non_exhaustive]
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
    /// Whether the child rules bind in document order: every header matched
    /// by an earlier accepting rule must precede every header matched by a
    /// later one (specification §3.7). Resolved from the rule's own `ordered`
    /// key or, absent that, [`Options::ordered_sections`].
    pub ordered: bool,
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
    /// Matching headings are accepted subject to the carried cardinality.
    Allow(Cardinality),
    /// Matching headings are rejected, so no cardinality applies.
    Deny,
}

/// The permitted number of sibling headers matched by one rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cardinality {
    /// Inclusive minimum number of matching sibling headings.
    pub min: u32,
    /// Inclusive maximum number of matching sibling headings.
    pub max: UpperBound,
}

/// The inclusive upper bound of a rule's cardinality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpperBound {
    /// At most the carried number of headings may match.
    Bounded(u32),
    /// Any number of headings may match.
    Unbounded,
}

/// A normalized header matcher.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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

/// The validated body of a glob matcher.
///
/// Construction is restricted to the schema loader so callers cannot bypass
/// normalization and validation. Use [`Self::as_str`] to inspect the value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct GlobPattern(pub(crate) String);

impl GlobPattern {
    /// Returns the normalized pattern body.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The validated body of a regular-expression matcher, without `/` delimiters.
///
/// Construction is restricted to the schema loader so callers cannot create a
/// semantic schema containing an invalid regular expression.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct RegexPattern(pub(crate) String);

impl RegexPattern {
    /// Returns the normalized pattern body.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A validated rule identifier.
///
/// Construction is restricted to the schema loader, which enforces the
/// identifier grammar and reserved-name rules.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct RuleId(pub(crate) String);

impl RuleId {
    /// Returns the normalized identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A cross-section presence or ordering constraint.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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
        /// Proposition that activates the requirement.
        condition: Proposition,
        /// Propositions required whenever the condition is satisfied.
        consequences: NonEmpty<Proposition>,
    },
    /// If `condition` is satisfied, every `exclusion` must be unsatisfied.
    Conflicts {
        /// Proposition that activates the conflict.
        condition: Proposition,
        /// Propositions forbidden whenever the condition is satisfied.
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
pub struct FrontmatterKey(pub(crate) String);

impl FrontmatterKey {
    /// Returns the mapping key as it appeared in the normalized reference.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A scalar resolved according to the YAML 1.2 core schema.
///
/// Integer and float values retain arbitrary precision as canonical strings.
/// Their distinct variants preserve the spec's typed equality (`1` is not
/// equal to `1.0`) without forcing a numeric precision limit on schema input.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FrontmatterScalar {
    /// YAML's null value.
    Null,
    /// A YAML boolean.
    Boolean(bool),
    /// A YAML integer in canonical arbitrary-precision form.
    Integer(CanonicalInteger),
    /// A YAML floating-point value in canonical arbitrary-precision form.
    Float(CanonicalFloat),
    /// A YAML string.
    String(String),
}

/// The canonical, arbitrary-precision value of a YAML integer scalar.
///
/// Construction is restricted to the schema loader, which validates and
/// canonicalizes the source spelling.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct CanonicalInteger(pub(crate) String);

impl CanonicalInteger {
    /// Returns the canonical decimal spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The canonical, arbitrary-precision value of a YAML float scalar.
///
/// Construction is restricted to the schema loader, which validates and
/// canonicalizes the source spelling.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct CanonicalFloat(pub(crate) String);

impl CanonicalFloat {
    /// Returns the canonical decimal spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
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
    /// Resolve from the schema's root scope; the normalized form of a leading
    /// `$.` in source. `$` alone is not a valid reference.
    SchemaRoot,
}

/// A collection statically guaranteed to contain at least one item.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NonEmpty<T> {
    /// The item whose presence establishes the non-empty invariant.
    pub first: T,
    /// Remaining items in collection order.
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
    /// First item in collection order.
    pub first: T,
    /// Second item, whose presence establishes the at-least-two invariant.
    pub second: T,
    /// Remaining items in collection order.
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
