//! Outlint-owned rendering of a query result's path.
//!
//! Specification §4.6 makes this Outlint's responsibility: "Outlint owns path
//! rendering at this boundary: whenever it renders a normalized path or derives
//! a §6.1 `pointer`, it MUST construct the representation from the node's path
//! components according to RFC 9535 §2.7 [...] A JSONPath provider's rendered
//! path is not authoritative."
//!
//! That is not a theoretical concern. The pinned provider's own
//! `NormalizedPath: Display` interpolates each name segment raw, applying none
//! of the §2.7 escaping, and its `PathElement: Display` emits one reverse
//! solidus where the RFC requires two. Both produce spellings that do not
//! round-trip. Only the *rendering* is affected: the provider's values and its
//! structural path components are sound, which is why building the spelling
//! here from `PathElement::as_name` and `as_index` is both correct and
//! sufficient.
//!
//! Phase 1B promotes this same component-based logic into the private
//! production locator wrapper. It lives in test support until then, with the
//! regression tests in `tests/jsonpath_core.rs` pinning every escaping rule so
//! the move carries its coverage with it.

use serde_json_path::{NormalizedPath, PathElement};

/// Renders a path as an RFC 9535 §2.7 normalized path.
///
/// The result is always a valid JSONPath query selecting exactly the node it
/// names.
pub fn render_normalized_path(path: &NormalizedPath<'_>) -> String {
    let mut out = String::from("$");
    for element in path.iter() {
        match element {
            PathElement::Index(index) => {
                out.push('[');
                out.push_str(&index.to_string());
                out.push(']');
            }
            PathElement::Name(name) => {
                out.push_str("['");
                push_normalized_name(&mut out, name);
                out.push_str("']");
            }
        }
    }
    out
}

/// Applies the §2.7 `normal-single-quoted` escaping to one member name.
///
/// The five control characters with short escapes take them; apostrophe and
/// reverse solidus are backslash-escaped; every other C0 control takes the
/// four-digit lowercase `\u00xx` form. Everything else, including a double
/// quote and any non-ASCII character, is literal: a normalized path is
/// single-quoted, so a double quote needs no escape.
fn push_normalized_name(out: &mut String, name: &str) {
    for character in name.chars() {
        match character {
            '\u{0008}' => out.push_str("\\b"),
            '\u{0009}' => out.push_str("\\t"),
            '\u{000A}' => out.push_str("\\n"),
            '\u{000C}' => out.push_str("\\f"),
            '\u{000D}' => out.push_str("\\r"),
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => out.push(other),
        }
    }
}

/// Renders a path as an RFC 6901 JSON Pointer, for the §6.1 `pointer` field.
///
/// The root is the empty pointer. Every other path is a sequence of `/`-
/// prefixed reference tokens. Only `~` and `/` are escaped, as `~0` and `~1`;
/// quotes, backslashes, C0 controls, and non-ASCII characters are literal
/// token characters, and escaping them for transport is the job of whatever
/// serializes the pointer into JSON.
pub fn render_json_pointer(path: &NormalizedPath<'_>) -> String {
    let mut out = String::new();
    for element in path.iter() {
        out.push('/');
        match element {
            PathElement::Index(index) => out.push_str(&index.to_string()),
            PathElement::Name(name) => push_pointer_token(&mut out, name),
        }
    }
    out
}

fn push_pointer_token(out: &mut String, name: &str) {
    for character in name.chars() {
        match character {
            '~' => out.push_str("~0"),
            '/' => out.push_str("~1"),
            other => out.push(other),
        }
    }
}
