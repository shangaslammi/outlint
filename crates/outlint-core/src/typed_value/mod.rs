//! Typed capture values: parsing, normalization, equality, and ordering.
//!
//! This module owns the closed set of capture types of specification §2.4,
//! the normalized form each type parses to, and the equality and ordering
//! relations built on those normalized forms. It does not own locator syntax,
//! regex analysis, JSONPath selection, or diagnostic wording, each of which
//! has a home of its own.
//!
//! Two sources feed the kernel and they keep distinct admission rules:
//!
//! - A rule capture supplies a header substring, parsed by the type's lexical
//!   grammar exactly as written ([`parse_header`]).
//! - A frontmatter capture supplies a selected node of the §1.6 YAML-to-JSON
//!   view together with the YAML kind that node resolved to, and the kind is
//!   checked strictly before any parse ([`parse_frontmatter`]).
//!
//! Both sources normalize to the same [`TypedValue`], so one comparison
//! relation serves every consumer.

// The kernel is deliberately unwired: the loader, validator, and diagnostic
// layers begin consuming it in a later phase. Until then every item here is
// reachable only from this module's tests, so the crate build sees the whole
// module as dead. Remove this attribute when the spine starts calling in.
#![allow(dead_code)]

#[cfg(test)]
mod tests;

use serde_json::Value;

/// The closed set of capture types (§2.4).
///
/// No type outside this set exists, and none is added without a
/// specification change: an unknown type name is a schema error at load
/// time, not a kernel concern.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ValueType {
    /// `-?[0-9]+`, bounded to signed 64 bits.
    Int,
    /// Exactly `true` or `false` in a header; a YAML boolean in frontmatter.
    Bool,
    /// `YYYY-MM-DD` denoting a proleptic-Gregorian calendar date.
    Date,
    /// SemVer 2.0.0 without build metadata.
    Semver,
    /// `[0-9]+(?:\.[0-9]+)*`, each component bounded to unsigned 32 bits.
    Dotted,
    /// Any string, preserved verbatim.
    Text,
}

impl ValueType {
    /// Every type in the closed set, in specification-table order.
    ///
    /// Exhaustiveness of this constant is enforced by the round-trip test:
    /// a new variant that is not listed here fails it.
    pub(crate) const ALL: [ValueType; 6] = [
        ValueType::Int,
        ValueType::Bool,
        ValueType::Date,
        ValueType::Semver,
        ValueType::Dotted,
        ValueType::Text,
    ];

    /// Resolves a schema-spelled type name, or `None` if the name is not one
    /// of the six. Matching is exact: the names are lowercase-only.
    pub(crate) fn from_name(name: &str) -> Option<ValueType> {
        match name {
            "int" => Some(ValueType::Int),
            "bool" => Some(ValueType::Bool),
            "date" => Some(ValueType::Date),
            "semver" => Some(ValueType::Semver),
            "dotted" => Some(ValueType::Dotted),
            "text" => Some(ValueType::Text),
            _ => None,
        }
    }

    /// The schema spelling of this type.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ValueType::Int => "int",
            ValueType::Bool => "bool",
            ValueType::Date => "date",
            ValueType::Semver => "semver",
            ValueType::Dotted => "dotted",
            ValueType::Text => "text",
        }
    }

    /// The single YAML kind a frontmatter capture of this type accepts.
    ///
    /// §2.4 admits no coercion: `int` accepts only a YAML integer, `bool`
    /// only a YAML boolean, and every other type only a YAML string. An
    /// unquoted `version: 1.2` is a YAML float and therefore not a `semver`.
    pub(crate) fn frontmatter_kind(self) -> ResolvedYamlKind {
        match self {
            ValueType::Int => ResolvedYamlKind::Integer,
            ValueType::Bool => ResolvedYamlKind::Boolean,
            ValueType::Date | ValueType::Semver | ValueType::Dotted | ValueType::Text => {
                ResolvedYamlKind::String
            }
        }
    }
}

/// The kind a frontmatter node resolved to in YAML, carried alongside the
/// JSON view because the JSON view cannot express it.
///
/// `Integer` and `Float` both appear as `serde_json::Number`; only this kind
/// separates them, which is exactly what the strict `int` check needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ResolvedYamlKind {
    /// A YAML null, including the empty scalar.
    Null,
    /// A YAML boolean.
    Boolean,
    /// A YAML integer scalar.
    Integer,
    /// A YAML finite-decimal scalar.
    Float,
    /// A YAML string scalar.
    String,
    /// A YAML sequence.
    Sequence,
    /// A YAML mapping.
    Mapping,
}

/// A frontmatter node handed to the kernel: the selected JSON value plus the
/// YAML kind it resolved to.
///
/// The contract the producing phase must honour:
///
/// - `value` is the node a §2.3 singular query selected in the exact §1.6
///   YAML-to-JSON view. It is borrowed, never cloned: the kernel reads it and
///   keeps nothing.
/// - `yaml_kind` is resolved from the YAML node itself, after alias and tag
///   handling, and is supplied separately because the JSON view loses it.
/// - YAML integer and finite-decimal scalars both arrive as JSON numbers, so
///   a caller must never infer the YAML kind from `serde_json::Number`.
/// - A scalar carrying an unrecognized tag has the kind its text resolves to
///   under the YAML 1.2 core schema (§2.3), so `!custom 42` is `Integer`.
/// - A string arrives as its resolved YAML string verbatim; Markdown inline
///   processing never applies to frontmatter.
/// - Exact integers are read through the arbitrary-precision number
///   spelling, so a value beyond machine bounds is reported as a bound
///   failure rather than being confused with a lexical one.
///
/// Supplying a `value` whose shape disagrees with `yaml_kind` is a producer
/// bug; the kernel reports it as a kind failure and never panics.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FrontmatterValue<'a> {
    value: &'a Value,
    yaml_kind: ResolvedYamlKind,
}

impl<'a> FrontmatterValue<'a> {
    /// Pairs a borrowed selected node with the YAML kind it resolved to.
    pub(crate) fn new(value: &'a Value, yaml_kind: ResolvedYamlKind) -> Self {
        FrontmatterValue { value, yaml_kind }
    }

    /// The borrowed selected node.
    pub(crate) fn value(&self) -> &'a Value {
        self.value
    }

    /// The separately supplied YAML kind.
    pub(crate) fn yaml_kind(&self) -> ResolvedYamlKind {
        self.yaml_kind
    }
}

/// Why a source failed to become a [`TypedValue`].
///
/// These are facts, not messages. Diagnostic wording is decided by the
/// reporting phase, which has the schema, the document, and the location
/// this module deliberately does not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ParseFailure {
    /// A frontmatter node resolved to a YAML kind the type does not accept.
    KindMismatch {
        /// The one kind this type accepts.
        expected: ResolvedYamlKind,
        /// The kind the node actually resolved to.
        actual: ResolvedYamlKind,
    },
    /// The source does not satisfy the type's lexical grammar.
    Lexical,
    /// The source is well formed but a numeric component exceeds its bound.
    BoundOverflow {
        /// Which bounded position overflowed.
        component: BoundComponent,
    },
    /// The source is a well-formed `YYYY-MM-DD` that names no calendar day.
    InvalidDate,
    /// The source is a valid SemVer 2.0.0 version carrying build metadata,
    /// which §2.4 rejects. The suffix retains its leading `+`, so
    /// `1.0.0+build.7` reports `+build.7`.
    BuildMetadata {
        /// The rejected suffix, leading `+` included.
        suffix: String,
    },
}

/// The bounded numeric position that overflowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoundComponent {
    /// The whole `int` value, bounded to signed 64 bits.
    Int,
    /// The SemVer major identifier, bounded to unsigned 64 bits.
    SemverMajor,
    /// The SemVer minor identifier, bounded to unsigned 64 bits.
    SemverMinor,
    /// The SemVer patch identifier, bounded to unsigned 64 bits.
    SemverPatch,
    /// A numeric SemVer pre-release identifier, bounded to unsigned 64 bits.
    SemverPrerelease {
        /// Zero-based position of the identifier within the pre-release.
        index: usize,
    },
    /// A `dotted` component, bounded to unsigned 32 bits.
    DottedComponent {
        /// Zero-based position of the component within the sequence.
        index: usize,
    },
}

/// A parsed capture value in its normalized form.
///
/// Opaque by construction: the normalized representation is private, so the
/// only values that exist are ones a parser admitted. There is no way to
/// build an invalid date, a build-bearing SemVer, or an empty `dotted`
/// sequence from outside this module.
#[derive(Clone, Debug)]
pub(crate) struct TypedValue {
    value: NormalizedValue,
}

impl TypedValue {
    /// The type this value was parsed as.
    pub(crate) fn value_type(&self) -> ValueType {
        match self.value {
            NormalizedValue::Int(_) => ValueType::Int,
            NormalizedValue::Bool(_) => ValueType::Bool,
            NormalizedValue::Date(_) => ValueType::Date,
            NormalizedValue::Semver(_) => ValueType::Semver,
            NormalizedValue::Dotted(_) => ValueType::Dotted,
            NormalizedValue::Text(_) => ValueType::Text,
        }
    }
}

/// The normalized representation behind a [`TypedValue`].
#[derive(Clone, Debug)]
enum NormalizedValue {
    /// A mathematical integer within signed 64 bits; `-0` normalizes to `0`.
    Int(i64),
    /// A boolean, ordered `false < true`.
    Bool(bool),
    /// A calendar day that exists.
    Date(DateValue),
    /// A SemVer version whose build metadata is always empty.
    Semver(SemverValue),
    /// A non-empty sequence of unsigned 32-bit components.
    Dotted(DottedValue),
    /// A string preserved exactly as it was supplied.
    Text(String),
}

/// A valid proleptic-Gregorian calendar date, stored as numeric fields so
/// chronological order is structural rather than lexical.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DateValue {
    year: u16,
    month: u8,
    day: u8,
}

/// A SemVer version admitted by §2.4, which is to say one whose build
/// metadata is empty.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SemverValue {
    version: semver::Version,
}

/// A non-empty `dotted` component sequence.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DottedValue {
    components: Vec<u32>,
}
