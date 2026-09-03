//! Pure Markdown outline parsing.
//!
//! CommonMark block recognition is delegated to `pulldown-cmark`; this keeps
//! fenced-code and Setext-heading behavior aligned with the Markdown model
//! while this module owns Outlint's section tree and suppression metadata.

mod body;
mod frontmatter;
mod lines;
mod model;

#[cfg(test)]
mod tests;

pub use model::{
    Document, DocumentFrontmatter, FrontmatterAnchor, FrontmatterAnchors, FrontmatterLocation,
    Heading, HeadingLocation, MarkdownOptions, Section, SuppressedDiagnostic, Suppressions,
};

use body::ParsedBody;
use lines::{mask_source_range, normalize_bare_cr, LineIndex};

/// Parses source text into Outlint's positioned Markdown section model.
///
/// The function is total and performs no IO. Malformed or incomplete Markdown
/// is interpreted according to CommonMark recovery rules.
///
/// # Example
///
/// ```
/// use outlint_core::{parse_markdown, HeaderLevel, MarkdownOptions};
///
/// let document = parse_markdown(
///     "# Guide\n\n## Install\n",
///     MarkdownOptions::default(),
/// );
///
/// assert_eq!(document.sections[0].heading.level, HeaderLevel::H1);
/// assert_eq!(document.sections[0].children[0].heading.text, "Install");
/// ```
pub fn parse_markdown(source: &str, options: MarkdownOptions) -> Document {
    let line_index = LineIndex::new(source);
    let (frontmatter, frontmatter_range) = frontmatter::parse(source, &line_index);
    // Both transformations preserve byte length. pulldown-cmark ranges into
    // `parser_source` can therefore safely address the original `source`.
    let masked_source = frontmatter_range.map(|range| mask_source_range(source, range));
    let parser_source = normalize_bare_cr(masked_source.as_deref().unwrap_or(source));
    let ParsedBody {
        sections,
        file_suppressions,
    } = body::parse(source, &parser_source, options, &line_index);

    Document {
        frontmatter,
        sections,
        file_suppressions,
    }
}
