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
    /// Wraps a normalized value. Private, and reachable only from a parser,
    /// which is what makes [`TypedValue`] opaque.
    fn from_normalized(value: NormalizedValue) -> TypedValue {
        TypedValue { value }
    }

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
///
/// The fields are written only by [`DateValue::new`], which admits nothing
/// the calendar does not, so no invalid date reaches normalized storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DateValue {
    year: u16,
    month: u8,
    day: u8,
}

impl DateValue {
    /// Builds a date, or `None` if the fields name no day in the proleptic
    /// Gregorian calendar. Years `0000` through `9999` are in range; `0000`
    /// is astronomical year numbering, and it is a leap year.
    fn new(year: u16, month: u8, day: u8) -> Option<DateValue> {
        if year > 9999 || day < 1 || day > days_in_month(year, month)? {
            return None;
        }
        Some(DateValue { year, month, day })
    }
}

/// The length of a month, or `None` if `month` is not one of the twelve.
fn days_in_month(year: u16, month: u8) -> Option<u8> {
    let length = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => return None,
    };
    Some(length)
}

/// The proleptic-Gregorian leap rule, applied to every year in range rather
/// than only to years the Gregorian calendar was historically in force for.
fn is_leap_year(year: u16) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// A SemVer version admitted by §2.4, which is to say one whose build
/// metadata is empty.
///
/// The field is written only by [`SemverValue::new`], so build metadata can
/// never reach a normalized value and therefore can never influence equality
/// or ordering.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SemverValue {
    version: semver::Version,
}

impl SemverValue {
    /// Admits a version, or `None` if it carries build metadata.
    fn new(version: semver::Version) -> Option<SemverValue> {
        if !version.build.is_empty() {
            return None;
        }
        Some(SemverValue { version })
    }

    /// The admitted version, whose build metadata is empty.
    fn version(&self) -> &semver::Version {
        &self.version
    }
}

/// A non-empty `dotted` component sequence.
///
/// The field is written only by [`DottedValue::new`], which rejects the
/// empty sequence, so a `dotted` value always has a first component to
/// compare.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DottedValue {
    components: Vec<u32>,
}

impl DottedValue {
    /// Builds a component sequence, or `None` if it is empty.
    fn new(components: Vec<u32>) -> Option<DottedValue> {
        if components.is_empty() {
            return None;
        }
        Some(DottedValue { components })
    }

    /// The components, outermost first.
    fn components(&self) -> &[u32] {
        &self.components
    }
}

// ---------------------------------------------------------------------------
// Lexical parsers
//
// Each parser is total over `&str`: every string either yields a normalized
// value or a structured failure, and none of them can panic. They are shared
// by both source paths, because §2.4 gives a type one grammar however the
// string was reached.
// ---------------------------------------------------------------------------

/// Parses `-?[0-9]+` into a signed 64-bit integer.
///
/// ASCII digits only. A `+` sign, surrounding whitespace, digit separators,
/// and non-ASCII digits are all lexical failures rather than tolerated
/// spellings. Leading zeros are allowed and carry no meaning, so `-01` is
/// `-1` and `-0` is `0`.
fn parse_int(source: &str) -> Result<i64, ParseFailure> {
    let digits = match source.strip_prefix('-') {
        Some(rest) => rest,
        None => source,
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ParseFailure::Lexical);
    }
    // The grammar held, so the only remaining way to fail is the bound. That
    // separation is the reason the shape is checked here rather than left to
    // `FromStr`, whose single error would conflate the two.
    match source.parse::<i64>() {
        Ok(value) => Ok(value),
        Err(_) => Err(ParseFailure::BoundOverflow {
            component: BoundComponent::Int,
        }),
    }
}

/// Parses the header spelling of a boolean, which is exactly `true` or
/// `false`.
///
/// Lowercase only: a frontmatter `bool` reaches the kernel already resolved
/// by YAML, so the wider YAML spellings never pass through here.
fn parse_bool(source: &str) -> Result<bool, ParseFailure> {
    match source {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(ParseFailure::Lexical),
    }
}

/// Parses `YYYY-MM-DD` into a calendar date.
///
/// The shape is fixed: ten ASCII characters, dashes at positions 4 and 7,
/// and decimal digits elsewhere. A well-shaped string that names no day —
/// `2023-02-29`, `2024-13-01`, `2024-04-31` — fails the calendar rather than
/// the grammar, which lets the reporting phase say which one it was.
fn parse_date(source: &str) -> Result<DateValue, ParseFailure> {
    let bytes: [u8; 10] = match source.as_bytes().try_into() {
        Ok(bytes) => bytes,
        Err(_) => return Err(ParseFailure::Lexical),
    };
    let [y3, y2, y1, y0, first_dash, m1, m0, second_dash, d1, d0] = bytes;
    if first_dash != b'-' || second_dash != b'-' {
        return Err(ParseFailure::Lexical);
    }
    let digits = [y3, y2, y1, y0, m1, m0, d1, d0];
    if !digits.iter().all(|byte| byte.is_ascii_digit()) {
        return Err(ParseFailure::Lexical);
    }

    // Four digits never exceed `u16` and two never exceed `u8`, so these are
    // exact conversions rather than bound checks.
    let year = u16::from(decimal_digit(y3)) * 1000
        + u16::from(decimal_digit(y2)) * 100
        + u16::from(decimal_digit(y1)) * 10
        + u16::from(decimal_digit(y0));
    let month = decimal_digit(m1) * 10 + decimal_digit(m0);
    let day = decimal_digit(d1) * 10 + decimal_digit(d0);

    match DateValue::new(year, month, day) {
        Some(date) => Ok(date),
        None => Err(ParseFailure::InvalidDate),
    }
}

/// The value of one ASCII decimal digit; any other byte contributes zero.
///
/// Callers check `is_ascii_digit` first, so the fallback is unreachable in
/// practice and exists only to keep this total.
fn decimal_digit(byte: u8) -> u8 {
    if byte.is_ascii_digit() {
        byte - b'0'
    } else {
        0
    }
}

/// Parses `[0-9]+(?:\.[0-9]+)*` into a non-empty component sequence.
///
/// Every component is a non-empty run of ASCII digits, so an empty input, a
/// leading dot, a doubled dot, and a trailing dot are all lexical failures.
/// Leading zeros within a component are allowed and carry no meaning, which
/// is what makes `1.02.0` the same value as `1.2.0`.
fn parse_dotted(source: &str) -> Result<DottedValue, ParseFailure> {
    let mut components = Vec::new();
    for (index, component) in source.split('.').enumerate() {
        if component.is_empty() || !component.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(ParseFailure::Lexical);
        }
        match component.parse::<u32>() {
            Ok(value) => components.push(value),
            Err(_) => {
                return Err(ParseFailure::BoundOverflow {
                    component: BoundComponent::DottedComponent { index },
                })
            }
        }
    }
    match DottedValue::new(components) {
        Some(dotted) => Ok(dotted),
        // `split` yields at least one item, and an empty one already failed
        // above, so the sequence is non-empty by the time it gets here.
        None => Err(ParseFailure::Lexical),
    }
}

/// Preserves the source string exactly.
///
/// No trimming, no case folding, no Unicode normalization, and no Markdown
/// processing: `text` equality and order compare the code points that were
/// supplied, so anything done here would be a silent change of value.
fn parse_text(source: &str) -> String {
    source.to_owned()
}

/// Parses a SemVer 2.0.0 version and rejects build metadata.
///
/// Grammar and precedence belong to the `semver` crate: a handwritten SemVer
/// parser would be a second, quietly divergent reading of a specification
/// this crate already implements. Two things it does not decide are decided
/// here.
///
/// The first is Outlint's own bound. §2.4 bounds every numeric identifier —
/// major, minor, patch, and numeric pre-release identifiers — to unsigned 64
/// bits, and the crate stores the pre-release as text, so an oversized
/// numeric pre-release identifier would otherwise pass unnoticed. The scan
/// runs only where the identifier is already shaped like a number, leaving
/// every other malformation to the crate.
///
/// The second is build metadata. §2.4 rejects a `+` suffix and requires the
/// diagnostic to name it, so an otherwise valid version carrying valid build
/// metadata gets its own failure with the suffix attached, `+` included.
fn parse_semver(source: &str) -> Result<SemverValue, ParseFailure> {
    // `+` appears in SemVer only as the build separator, so the first one
    // ends the part the bound applies to.
    let (numeric_part, build) = match source.split_once('+') {
        Some((head, tail)) => (head, Some(tail)),
        None => (source, None),
    };

    if let Some(failure) = semver_bound_failure(numeric_part) {
        return Err(failure);
    }

    let version = match semver::Version::parse(source) {
        Ok(version) => version,
        Err(_) => return Err(ParseFailure::Lexical),
    };

    match SemverValue::new(version) {
        Some(admitted) => Ok(admitted),
        None => Err(ParseFailure::BuildMetadata {
            suffix: match build {
                Some(build) => format!("+{build}"),
                // Unreachable: a version cannot carry build metadata that
                // the source did not spell after a `+`.
                None => "+".to_owned(),
            },
        }),
    }
}

/// Reports the first numeric identifier of `numeric_part` that exceeds
/// unsigned 64 bits, or `None` if the bound holds or the shape is one the
/// `semver` crate should judge instead.
///
/// `numeric_part` is the source with any build metadata already removed.
fn semver_bound_failure(numeric_part: &str) -> Option<ParseFailure> {
    // The version core is digits and dots, so the first `-` is the
    // pre-release separator.
    let (core, prerelease) = match numeric_part.split_once('-') {
        Some((core, prerelease)) => (core, Some(prerelease)),
        None => (numeric_part, None),
    };

    let mut fields = core.split('.');
    for component in [
        BoundComponent::SemverMajor,
        BoundComponent::SemverMinor,
        BoundComponent::SemverPatch,
    ] {
        let field = fields.next()?;
        if !is_ascii_decimal(field) {
            // Not a number at all; the crate will say so.
            return None;
        }
        if field.parse::<u64>().is_err() {
            return Some(ParseFailure::BoundOverflow { component });
        }
    }
    if fields.next().is_some() {
        // More than three core fields is a grammar failure, not a bound one.
        return None;
    }

    for (index, identifier) in prerelease?.split('.').enumerate() {
        // An alphanumeric identifier carries no bound; only a numeric one
        // does, and it is compared as a number by SemVer precedence.
        if is_ascii_decimal(identifier) && identifier.parse::<u64>().is_err() {
            return Some(ParseFailure::BoundOverflow {
                component: BoundComponent::SemverPrerelease { index },
            });
        }
    }
    None
}

/// Whether `text` is a non-empty run of ASCII decimal digits.
fn is_ascii_decimal(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

// ---------------------------------------------------------------------------
// Source entry points
// ---------------------------------------------------------------------------

/// Parses a rule capture's header substring.
///
/// The source is the case-preserving substring the named group selected from
/// the §1.3 matcher input, after the configured inline-markup handling and
/// before any case folding used to decide the match. The type's lexical
/// grammar applies to it exactly as written: a header `bool` is `true` or
/// `false` and nothing else, because no YAML resolver stands between the
/// document and this call.
fn parse_header(value_type: ValueType, source: &str) -> Result<TypedValue, ParseFailure> {
    parse_lexical(value_type, source)
}

/// Parses a frontmatter capture's selected node.
///
/// The YAML kind is checked strictly first, and only against the single kind
/// §2.4 gives the type: nothing is coerced, so an unquoted `version: 1.2` is
/// a YAML float and fails an `int` and a `semver` alike. `to_string()` is
/// never called on a nonmatching value, because rendering one kind as
/// another is the coercion the specification forbids.
///
/// Past the kind check the two sources agree. A YAML boolean arrives already
/// resolved, so the document spelling `True` succeeds here while the header
/// spelling `True` does not. A YAML integer is read from its exact
/// arbitrary-precision spelling rather than through `as_i64`, which would
/// erase the difference between a value past the bound and one that was
/// never an integer.
///
/// A `value` whose shape disagrees with `yaml_kind` is a producer bug; it is
/// reported as a kind failure against the shape actually supplied, and never
/// panics.
fn parse_frontmatter(
    value_type: ValueType,
    supplied: FrontmatterValue<'_>,
) -> Result<TypedValue, ParseFailure> {
    let expected = value_type.frontmatter_kind();
    if supplied.yaml_kind() != expected {
        return Err(ParseFailure::KindMismatch {
            expected,
            actual: supplied.yaml_kind(),
        });
    }

    match (expected, supplied.value()) {
        // `as_str` is the arbitrary-precision spelling, which this crate
        // enables so that §1.6's exact mathematical value survives.
        (ResolvedYamlKind::Integer, Value::Number(number)) => Ok(TypedValue::from_normalized(
            NormalizedValue::Int(parse_int(number.as_str())?),
        )),
        (ResolvedYamlKind::Boolean, Value::Bool(resolved)) => Ok(TypedValue::from_normalized(
            NormalizedValue::Bool(*resolved),
        )),
        (ResolvedYamlKind::String, Value::String(resolved)) => parse_lexical(value_type, resolved),
        (_, value) => Err(ParseFailure::KindMismatch {
            expected,
            actual: json_shape_kind(value),
        }),
    }
}

/// Applies the type's lexical grammar to a source string.
///
/// §2.4 gives a type one grammar however its string was reached, so both
/// entry points share this; what differs between them is admission, not
/// parsing.
fn parse_lexical(value_type: ValueType, source: &str) -> Result<TypedValue, ParseFailure> {
    let value = match value_type {
        ValueType::Int => NormalizedValue::Int(parse_int(source)?),
        ValueType::Bool => NormalizedValue::Bool(parse_bool(source)?),
        ValueType::Date => NormalizedValue::Date(parse_date(source)?),
        ValueType::Semver => NormalizedValue::Semver(parse_semver(source)?),
        ValueType::Dotted => NormalizedValue::Dotted(parse_dotted(source)?),
        ValueType::Text => NormalizedValue::Text(parse_text(source)),
    };
    Ok(TypedValue::from_normalized(value))
}

/// The kind a JSON value's own shape implies.
///
/// Used only to report a `value`/`yaml_kind` pair that disagrees, which is a
/// producer bug. A JSON number cannot say which YAML scalar produced it —
/// that is the whole reason the kind travels separately — so a number is
/// reported as an integer here; the accurate kind was the one the producer
/// failed to supply.
fn json_shape_kind(value: &Value) -> ResolvedYamlKind {
    match value {
        Value::Null => ResolvedYamlKind::Null,
        Value::Bool(_) => ResolvedYamlKind::Boolean,
        Value::Number(_) => ResolvedYamlKind::Integer,
        Value::String(_) => ResolvedYamlKind::String,
        Value::Array(_) => ResolvedYamlKind::Sequence,
        Value::Object(_) => ResolvedYamlKind::Mapping,
    }
}
