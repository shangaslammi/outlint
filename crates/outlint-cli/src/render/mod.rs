//! Output rendering: dispatch between the human and JSON formats.

mod human;
mod json;
#[cfg(feature = "read")]
pub(crate) mod read;
#[cfg(feature = "search")]
pub(crate) mod search;

use crate::{args::ValidationFormat, diagnostics::ValidationResult};

#[cfg(any(feature = "search", feature = "read"))]
const KILOBYTE: u64 = 1_000;
#[cfg(any(feature = "search", feature = "read"))]
const MEGABYTE: u64 = 1_000_000;

/// Formats a byte count consistently across search and read presentations.
#[cfg(any(feature = "search", feature = "read"))]
pub(crate) fn format_bytes(bytes: u64) -> String {
    let tenths = |unit: u64| bytes.saturating_add(unit / 20) / (unit / 10);
    if bytes < KILOBYTE {
        format!("{bytes}B")
    } else if bytes < MEGABYTE {
        let tenths = tenths(KILOBYTE);
        format!("{}.{}kB", tenths / 10, tenths % 10)
    } else {
        let tenths = tenths(MEGABYTE);
        format!("{}.{}MB", tenths / 10, tenths % 10)
    }
}

/// Whether a character can alter terminal layout or the visual ordering of
/// trusted formatter text.
fn escape_human_character(character: char, escaped: &mut String) -> bool {
    match character {
        '\n' => escaped.push_str("\\n"),
        '\r' => escaped.push_str("\\r"),
        '\t' => escaped.push_str("\\t"),
        '\u{1b}' => escaped.push_str("\\x1b"),
        character
            if character.is_control()
                || matches!(
                    character,
                    '\u{061c}'
                        | '\u{200e}'
                        | '\u{200f}'
                        | '\u{2028}'..='\u{202e}'
                        | '\u{2066}'..='\u{206f}'
                ) =>
        {
            escaped.push_str(&format!("\\u{{{:x}}}", u32::from(character)));
        }
        _ => return false,
    }
    true
}

/// Escapes untrusted text for a free-text position in human output.
///
/// Control characters, Unicode line separators, and bidi formatting controls
/// are escaped so document-, path-, query-, or error-controlled text cannot
/// drive or spoof the terminal. Printable quotes and backslashes remain
/// verbatim here; text inside formatter-owned quotes goes through
/// [`escape_human_quoted`].
pub(crate) fn escape_human(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        if !escape_human_character(character, &mut escaped) {
            escaped.push(character);
        }
    }
    escaped
}

/// Escapes untrusted text inside a formatter-owned `"..."` field.
fn escape_human_quoted(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        if escape_human_character(character, &mut escaped) {
            continue;
        }
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            character => escaped.push(character),
        }
    }
    escaped
}

/// Escapes a compact-format field without introducing tabs, line endings, or
/// invisible control characters. Backslashes are doubled so escapes remain
/// reversible.
#[cfg(any(feature = "search", feature = "read"))]
pub(crate) fn escape_compact(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{{{:x}}}", u32::from(character)));
            }
            character => escaped.push(character),
        }
    }
    escaped
}

/// Renders one invocation's results in the requested format. `--color auto`
/// is resolved by the caller; `use_color` is the caller's decision.
pub(crate) fn render(
    results: &[ValidationResult],
    format: ValidationFormat,
    use_color: bool,
) -> String {
    match format {
        ValidationFormat::Human => human::render_human(results, use_color),
        ValidationFormat::Json => json::render_json(results),
    }
}

#[cfg(all(test, any(feature = "search", feature = "read")))]
mod tests {
    use super::{escape_compact, format_bytes};

    #[test]
    fn byte_sizes_use_human_units() {
        assert_eq!(format_bytes(412), "412B");
        assert_eq!(format_bytes(3_140), "3.1kB");
        assert_eq!(format_bytes(2_450_000), "2.5MB");
    }

    #[test]
    fn compact_fields_escape_delimiters_controls_and_backslashes() {
        assert_eq!(
            escape_compact("a\tb\nc\rd\\e\u{7}"),
            "a\\tb\\nc\\u{d}d\\\\e\\u{7}"
        );
    }
}
