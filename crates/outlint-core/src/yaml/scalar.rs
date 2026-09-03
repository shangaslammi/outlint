//! What a YAML scalar means, and the canonical spellings behind that answer.

use num_bigint::{BigInt, BigUint};
use saphyr_parser::{ScalarStyle, Tag as YamlTag};

use crate::{CanonicalFloat, CanonicalInteger, FrontmatterScalar};

/// A scalar's source text beside the two things that decide what it means.
///
/// The parser hands the text out as a `Cow` borrowed from the block, and this
/// takes ownership of it: the tree outlives the parser that produced it, and
/// alias expansion clones nodes anyway. What the borrow would have saved is
/// smaller than what threading its lifetime through the tree would cost.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ExactYamlScalar {
    pub(crate) value: String,
    pub(crate) style: ScalarStyle,
    pub(crate) tag: Option<YamlTag>,
}

/// Why a YAML scalar or tag resolves to no JSON value.
///
/// The variants carry facts rather than sentences because two document paths
/// share the conversion and neither's vocabulary suits the other: the
/// frontmatter reader speaks of "frontmatter" and the schema loader of
/// "invalid YAML". Each wording lives beside the path that reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum YamlValueError {
    /// An explicitly `!!null`-tagged scalar not spelled like a null.
    TaggedNull,
    /// An explicitly `!!bool`-tagged scalar not spelled like a boolean.
    TaggedBool,
    /// An explicitly `!!int`-tagged scalar not spelled like an integer.
    TaggedInt,
    /// An explicitly `!!float`-tagged scalar not spelled like a float.
    TaggedFloat,
    /// A collection tag — `!!seq` or `!!map` — on a scalar.
    ScalarTag,
    /// The wrong core-schema tag on a collection; carries the expected suffix.
    ContainerTag(&'static str),
    /// An infinity or NaN, which JSON has no value for.
    NonFinite,
    /// A number the JSON value domain refused; carries the spelling and why.
    Unrepresentable { lexeme: String, error: String },
}

fn json_number(source: &str) -> Result<serde_json::Value, YamlValueError> {
    serde_json::from_str(source).map_err(|error| YamlValueError::Unrepresentable {
        lexeme: source.to_owned(),
        error: error.to_string(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JsonNumberKind {
    Integer,
    Float,
}

fn json_number_preserving_lexeme(
    source: &str,
    canonical: &str,
    expected_kind: JsonNumberKind,
) -> Result<serde_json::Value, YamlValueError> {
    // JSON's decimal point/exponent markers distinguish floats from integers.
    // Preserve a valid spelling only when it cannot erase that YAML identity.
    let source_kind = if source
        .bytes()
        .any(|byte| matches!(byte, b'.' | b'e' | b'E'))
    {
        JsonNumberKind::Float
    } else {
        JsonNumberKind::Integer
    };
    if source_kind == expected_kind && serde_json::from_str::<serde_json::Number>(source).is_ok() {
        // `from_string_unchecked` is available through our direct
        // `arbitrary_precision` feature. Its input must be one valid JSON
        // number; the parse immediately above establishes that invariant.
        return Ok(serde_json::Value::Number(
            serde_json::Number::from_string_unchecked(source.to_owned()),
        ));
    }
    json_number(canonical)
}

/// Requires a collection's core-schema tag, if any, to name its own kind.
pub(crate) fn validate_yaml_container_tag(
    tag: Option<&YamlTag>,
    expected: &'static str,
) -> Result<(), YamlValueError> {
    if standard_yaml_tag(tag).is_none_or(|tag| tag == expected) {
        Ok(())
    } else {
        Err(YamlValueError::ContainerTag(expected))
    }
}

/// Resolves one scalar to the JSON value its text, style and tag spell.
///
/// Both document paths — frontmatter and the schema loader — convert through
/// this one function, so a scalar means the same thing wherever it is read.
pub(crate) fn exact_yaml_scalar_to_json(
    scalar: ExactYamlScalar,
) -> Result<serde_json::Value, YamlValueError> {
    let standard_tag = standard_yaml_tag(scalar.tag.as_ref());
    match standard_tag {
        Some("str") => Ok(serde_json::Value::String(scalar.value)),
        Some("null") => match scalar.value.as_str() {
            "null" | "Null" | "NULL" | "~" => Ok(serde_json::Value::Null),
            _ => Err(YamlValueError::TaggedNull),
        },
        Some("bool") => match scalar.value.as_str() {
            "true" | "True" | "TRUE" => Ok(serde_json::Value::Bool(true)),
            "false" | "False" | "FALSE" => Ok(serde_json::Value::Bool(false)),
            _ => Err(YamlValueError::TaggedBool),
        },
        Some("int") => exact_yaml_integer(&scalar.value),
        Some("float") => exact_yaml_float(&scalar.value),
        Some("seq" | "map") => Err(YamlValueError::ScalarTag),
        Some(_) => Ok(serde_json::Value::String(scalar.value)),
        None if scalar.style != ScalarStyle::Plain => Ok(serde_json::Value::String(scalar.value)),
        None => plain_scalar_to_json(&scalar.value),
    }
}

fn standard_yaml_tag(tag: Option<&YamlTag>) -> Option<&str> {
    tag.and_then(|tag| tag.is_yaml_core_schema().then_some(tag.suffix.as_str()))
}

fn exact_yaml_integer(source: &str) -> Result<serde_json::Value, YamlValueError> {
    let canonical = canonical_tagged_yaml_integer(source).ok_or(YamlValueError::TaggedInt)?;
    json_number_preserving_lexeme(source, &canonical, JsonNumberKind::Integer)
}

fn canonical_tagged_yaml_integer(source: &str) -> Option<String> {
    let (negative, unsigned) = if let Some(unsigned) = source.strip_prefix('-') {
        (true, unsigned)
    } else {
        (false, source.strip_prefix('+').unwrap_or(source))
    };
    if unsigned.starts_with(['+', '-']) {
        return None;
    }
    let (base, digits) = if let Some(digits) = unsigned.strip_prefix("0x") {
        (16, digits)
    } else if let Some(digits) = unsigned.strip_prefix("0o") {
        (8, digits)
    } else if let Some(digits) = unsigned.strip_prefix("0b") {
        (2, digits)
    } else {
        if unsigned.len() > 1 && unsigned.starts_with('0') {
            return None;
        }
        (10, unsigned)
    };
    if digits.is_empty() {
        return None;
    }
    let value = BigUint::parse_bytes(digits.as_bytes(), base)?;
    if value == BigUint::from(0_u8) {
        Some("0".into())
    } else {
        Some(format!("{}{value}", if negative { "-" } else { "" }))
    }
}

fn exact_yaml_float(source: &str) -> Result<serde_json::Value, YamlValueError> {
    if let Some(canonical) = canonical_float(source) {
        if matches!(canonical.as_str(), "inf" | "-inf" | "nan") {
            return Err(YamlValueError::NonFinite);
        }
        return json_number_preserving_lexeme(source, &canonical, JsonNumberKind::Float);
    }
    let unsigned = source.strip_prefix(['-', '+']).unwrap_or(source);
    let crate::FrontmatterScalar::Integer(value) = parse_frontmatter_scalar(source) else {
        return Err(YamlValueError::TaggedFloat);
    };
    if unsigned.is_empty() || !unsigned.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(YamlValueError::TaggedFloat);
    }
    json_number(&format!("{}e0", value.0))
}

/// Resolves an untagged plain scalar by the YAML core schema, §1.6-exactly.
fn plain_scalar_to_json(source: &str) -> Result<serde_json::Value, YamlValueError> {
    match parse_frontmatter_scalar(source) {
        crate::FrontmatterScalar::Null => Ok(serde_json::Value::Null),
        crate::FrontmatterScalar::Boolean(value) => Ok(serde_json::Value::Bool(value)),
        crate::FrontmatterScalar::Integer(value) => {
            json_number_preserving_lexeme(source, &value.0, JsonNumberKind::Integer)
        }
        crate::FrontmatterScalar::Float(value) => {
            if matches!(value.0.as_str(), "inf" | "-inf" | "nan") {
                Err(YamlValueError::NonFinite)
            } else {
                json_number_preserving_lexeme(source, &value.0, JsonNumberKind::Float)
            }
        }
        crate::FrontmatterScalar::String(value) => Ok(serde_json::Value::String(value)),
    }
}

pub(crate) fn parse_frontmatter_scalar(source: &str) -> FrontmatterScalar {
    match source {
        "" | "~" | "null" | "Null" | "NULL" => FrontmatterScalar::Null,
        "true" | "True" | "TRUE" => FrontmatterScalar::Boolean(true),
        "false" | "False" | "FALSE" => FrontmatterScalar::Boolean(false),
        _ => {
            if let Some(integer) = canonical_integer(source) {
                FrontmatterScalar::Integer(CanonicalInteger(integer))
            } else if let Some(float) = canonical_float(source) {
                FrontmatterScalar::Float(CanonicalFloat(float))
            } else {
                FrontmatterScalar::String(source.to_owned())
            }
        }
    }
}

pub(super) fn canonical_integer(source: &str) -> Option<String> {
    let (negative, unsigned) = strip_sign(source);
    let (base, digits) = if let Some(digits) = unsigned.strip_prefix("0o") {
        (8_u8, digits)
    } else if let Some(digits) = unsigned.strip_prefix("0x") {
        (16, digits)
    } else {
        (10, unsigned)
    };
    if digits.is_empty() {
        return None;
    }
    let value = BigUint::parse_bytes(digits.as_bytes(), u32::from(base))?;
    if value == BigUint::from(0_u8) {
        return Some("0".into());
    }
    Some(format!("{}{value}", if negative { "-" } else { "" }))
}

pub(super) fn canonical_float(source: &str) -> Option<String> {
    let (negative, unsigned) = strip_sign(source);
    if matches!(unsigned, ".inf" | ".Inf" | ".INF") {
        return Some(if negative { "-inf" } else { "inf" }.into());
    }
    if matches!(unsigned, ".nan" | ".NaN" | ".NAN") {
        return (source == unsigned).then(|| "nan".into());
    }
    let (mantissa, exponent) = unsigned.split_once(['e', 'E']).unwrap_or((unsigned, "0"));
    let has_float_marker = mantissa.contains('.') || unsigned.contains(['e', 'E']);
    if !has_float_marker {
        return None;
    }
    let exponent = exponent.parse::<BigInt>().ok()?;
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    if !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let digits = format!("{whole}{fraction}");
    let trimmed_leading = digits.trim_start_matches('0');
    if trimmed_leading.is_empty() {
        return Some("0e0".into());
    }
    let trailing = trimmed_leading.len() - trimmed_leading.trim_end_matches('0').len();
    let coefficient = trimmed_leading.trim_end_matches('0');
    let adjusted = exponent - BigInt::from(fraction.len()) + BigInt::from(trailing);
    Some(format!(
        "{}{coefficient}e{adjusted}",
        if negative { "-" } else { "" }
    ))
}

fn strip_sign(source: &str) -> (bool, &str) {
    if let Some(unsigned) = source.strip_prefix('-') {
        (true, unsigned)
    } else if let Some(unsigned) = source.strip_prefix('+') {
        (false, unsigned)
    } else {
        (false, source)
    }
}
