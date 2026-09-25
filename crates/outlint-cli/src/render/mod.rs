//! Output rendering: dispatch between the human and JSON formats.

mod human;
mod json;
#[cfg(feature = "read")]
pub(crate) mod read;
#[cfg(feature = "search")]
pub(crate) mod search;

use crate::{args::OutputFormat, diagnostics::ValidationResult};

/// Formats a byte count consistently across search and read presentations.
#[cfg(any(feature = "search", feature = "read"))]
pub(crate) fn format_bytes(bytes: u64) -> String {
    let tenths = |unit: u64| bytes.saturating_add(unit / 20) / (unit / 10);
    if bytes < 1_000 {
        format!("{bytes}B")
    } else if bytes < 1_000_000 {
        let tenths = tenths(1_000);
        format!("{}.{}kB", tenths / 10, tenths % 10)
    } else {
        let tenths = tenths(1_000_000);
        format!("{}.{}MB", tenths / 10, tenths % 10)
    }
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
    format: OutputFormat,
    use_color: bool,
) -> String {
    match format {
        OutputFormat::Human => human::render_human(results, use_color),
        OutputFormat::Json => json::render_json(results),
        OutputFormat::Compact => String::new(),
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
