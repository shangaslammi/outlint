//! Human-readable output: headlines, details, evidence, and escaping.

use crate::diagnostics::{
    RenderedDiagnostic, RenderedMatcher, RenderedReference, RenderedSchemaNode, RenderedTarget,
    ValidationResult,
};

pub(super) fn render_human(results: &[ValidationResult], use_color: bool) -> String {
    let diagnostic_count = results
        .iter()
        .map(|result| result.diagnostics.len())
        .sum::<usize>();
    if diagnostic_count == 0 {
        return String::new();
    }
    let mut output = String::new();
    for (index, diagnostic) in results
        .iter()
        .flat_map(|result| &result.diagnostics)
        .enumerate()
    {
        if index != 0 {
            output.push('\n');
        }
        let id = if use_color {
            format!("\u{1b}[31m[{}]\u{1b}[0m", escape_human(&diagnostic.id))
        } else {
            format!("[{}]", escape_human(&diagnostic.id))
        };
        output.push_str(&format!(
            "{}:{}:{} {} {}",
            escape_human(&diagnostic.source_path),
            diagnostic.line,
            diagnostic.column,
            id,
            human_headline(diagnostic)
        ));
        output.push('\n');
        append_human_details(&mut output, diagnostic);
    }
    let files = results
        .iter()
        .filter(|result| !result.diagnostics.is_empty())
        .count();
    output.push('\n');
    output.push_str(&format!(
        "{} {} in {} {}\n",
        diagnostic_count,
        plural(diagnostic_count, "diagnostic", "diagnostics"),
        files,
        plural(files, "file", "files")
    ));
    output
}

fn human_headline(diagnostic: &RenderedDiagnostic) -> String {
    let headline = match diagnostic.id.as_str() {
        // The constraint form lists its expected order below; the implicit
        // form (§3.7) carries no references, and its message already names
        // the pair that broke.
        "ordered" if !diagnostic.references.is_empty() => "sections are not in the required order",
        "one_of" => "exactly one referenced condition must be satisfied",
        "any_of" => "at least one referenced condition must be satisfied",
        "at_most_one" => "at most one referenced condition may be satisfied",
        "all_or_none" => "all referenced conditions or none of them must be satisfied",
        "requires" => "a required consequence is missing",
        "conflicts" => "conflicting conditions are satisfied",
        _ => diagnostic.message.as_str(),
    };
    escape_human(headline)
}

fn append_human_details(output: &mut String, diagnostic: &RenderedDiagnostic) {
    if let Some(target) = &diagnostic.target {
        match target {
            RenderedTarget::Header { path } if diagnostic.id == "ordered" => {
                append_human_header_detail(output, "within", path);
            }
            RenderedTarget::Header { path } => {
                append_human_header_detail(output, "section", path);
            }
            RenderedTarget::MissingHeader { parent, matcher } => {
                append_human_quoted_detail(output, "expected", matcher);
                if !parent.is_empty() {
                    append_human_header_detail(output, "within", parent);
                }
            }
            RenderedTarget::Document => {}
            RenderedTarget::Frontmatter {
                line_range,
                pointer,
            } => {
                if let Some(range) = line_range {
                    output.push_str(&format!(
                        "  frontmatter: lines {}-{}\n",
                        range.start_line, range.end_line
                    ));
                }
                if let Some(pointer) = pointer {
                    if pointer.is_empty() {
                        output.push_str("  value: <frontmatter root>\n");
                    } else {
                        append_human_quoted_detail(output, "value", pointer);
                    }
                }
            }
        }
    }

    append_human_declaration_detail(output, diagnostic);

    if diagnostic.id == "ordered" {
        append_human_ordering_evidence(output, diagnostic);
    } else {
        append_human_constraint_evidence(output, diagnostic);
    }

    if let Some(location) = &diagnostic.schema_location {
        let duplicates_primary = location.path == diagnostic.source_path
            && location.line == diagnostic.line
            && location.column == diagnostic.column;
        if !duplicates_primary {
            let label = match diagnostic.schema_node.as_ref() {
                Some(RenderedSchemaNode::Constraint { .. }) => "constraint",
                Some(RenderedSchemaNode::Rule { .. }) => "rule",
                Some(RenderedSchemaNode::Capture { .. })
                | Some(RenderedSchemaNode::FrontmatterCapture { .. }) => "capture",
                Some(RenderedSchemaNode::OrderEntry { .. }) => "order entry",
                _ => "schema",
            };
            output.push_str(&format!(
                "  {label}: {}:{}:{}\n",
                escape_human(&location.path),
                location.line,
                location.column
            ));
        }
    }
}

/// Names the declaration a typed-value diagnostic is about.
///
/// `invalid-value`, `missing-value`, and `order-violation` all anchor on a
/// declaration rather than on a section, and their messages read badly
/// without one: which of a rule's captures failed, or which `order` entry was
/// violated, is not recoverable from a line and column. Nothing emits those
/// ids yet, so this branch is inert today; it exists so the format is settled
/// before the lane that emits them arrives, rather than being invented under
/// the pressure of making a diagnostic legible.
///
/// The capture name is schema-controlled text and goes through the same
/// quoting and control-character escaping as every other untrusted value.
fn append_human_declaration_detail(output: &mut String, diagnostic: &RenderedDiagnostic) {
    match diagnostic.schema_node.as_ref() {
        Some(RenderedSchemaNode::Capture { name, .. }) => {
            append_human_quoted_detail(output, "capture", name);
        }
        Some(RenderedSchemaNode::FrontmatterCapture { name }) => {
            append_human_quoted_detail(output, "capture", &format!("fm.{name}"));
        }
        Some(RenderedSchemaNode::OrderEntry { order_index, .. }) => {
            output.push_str(&format!("  order entry: {order_index}\n"));
        }
        _ => {}
    }
}

fn append_human_quoted_detail(output: &mut String, label: &str, value: &str) {
    output.push_str(&format!("  {label}: \"{}\"\n", escape_human_quoted(value)));
}

fn append_human_header_detail(output: &mut String, label: &str, path: &[String]) {
    output.push_str(&format!("  {label}: \"{}\"\n", human_header_path(path)));
}

fn append_human_ordering_evidence(output: &mut String, diagnostic: &RenderedDiagnostic) {
    if !diagnostic.references.is_empty() {
        output.push_str("  expected order (among sections that are present):\n");
        for (index, reference) in diagnostic.references.iter().enumerate() {
            output.push_str(&format!(
                "    {}. {}\n",
                index + 1,
                human_reference(reference)
            ));
        }
    }
    if !diagnostic.involved_headers.is_empty() {
        output.push_str("  observed order:\n");
        for header in &diagnostic.involved_headers {
            output.push_str(&format!(
                "    {}:{}:{} \"{}\"\n",
                escape_human(&diagnostic.source_path),
                header.line,
                header.column,
                human_header_path(&header.header_path)
            ));
        }
    }
}

fn append_human_constraint_evidence(output: &mut String, diagnostic: &RenderedDiagnostic) {
    if !diagnostic.references.is_empty() {
        output.push_str("  references:\n");
        for reference in &diagnostic.references {
            output.push_str("    - ");
            output.push_str(&human_reference(reference));
            output.push('\n');
        }
    }
    if !diagnostic.involved_headers.is_empty() {
        output.push_str("  involved sections:\n");
        for header in &diagnostic.involved_headers {
            output.push_str(&format!(
                "    {}:{}:{} \"{}\"\n",
                escape_human(&diagnostic.source_path),
                header.line,
                header.column,
                human_header_path(&header.header_path)
            ));
        }
    }
}

fn human_matcher(matcher: &RenderedMatcher) -> String {
    match matcher {
        RenderedMatcher::Exact(value) => format!("exact \"{}\"", escape_human_quoted(value)),
        RenderedMatcher::Glob(value) => format!("glob \"{}\"", escape_human_quoted(value)),
        RenderedMatcher::Regex(value) => format!("regex \"{}\"", escape_human_quoted(value)),
        RenderedMatcher::Any => "any heading".to_owned(),
        RenderedMatcher::Unknown => "unknown matcher".to_owned(),
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
/// are escaped so document- or schema-controlled text cannot drive or spoof
/// the terminal. Printable quotes and backslashes remain verbatim here; text
/// inside formatter-owned quotes goes through [`escape_human_quoted`].
fn escape_human(value: &str) -> String {
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

/// Joins a header's already-distinct path segments for human presentation.
fn human_header_path(path: &[String]) -> String {
    path.iter()
        .map(|part| escape_human_quoted(part))
        .collect::<Vec<_>>()
        .join(" > ")
}

fn human_reference(reference: &RenderedReference) -> String {
    match reference {
        // Every form quotes the author's own locator rather than rebuilding
        // one from bound steps: it is the text they would edit, and it is
        // retained precisely so it need not be reconstructed. It is
        // schema-controlled, so it is escaped like every other untrusted
        // value here.
        RenderedReference::Rule {
            locator, matcher, ..
        } => {
            format!("{} ({})", escape_human(locator), human_matcher(matcher))
        }
        // §4.6 makes the equality literal "the remainder of the locator", so
        // the retained spelling already carries it and nothing is appended.
        RenderedReference::FrontmatterQuery { locator, .. } => escape_human(locator),
        RenderedReference::FrontmatterCapture {
            locator,
            value_type,
            ..
        } => format!("{} ({})", escape_human(locator), escape_human(value_type)),
    }
}

fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

#[cfg(test)]
mod tests {
    use super::{escape_human, escape_human_quoted};

    #[test]
    fn human_escaping_neutralizes_terminal_controls_and_bidi_formatting() {
        // Free text keeps quotes and backslashes verbatim; terminal controls,
        // Unicode line separators, and bidi formatting characters are rewritten.
        assert_eq!(
            escape_human("\"title\" C:\\ \u{1b}\n\u{85}\u{2028}\u{202e}\u{2066}"),
            "\"title\" C:\\ \\x1b\\n\\u{85}\\u{2028}\\u{202e}\\u{2066}"
        );
        // Inside a quote-delimited field the delimiter characters are escaped
        // on top of the same control-character policy.
        assert_eq!(
            escape_human_quoted("say \"hi\" \\ now\t"),
            "say \\\"hi\\\" \\\\ now\\t"
        );
    }
}
