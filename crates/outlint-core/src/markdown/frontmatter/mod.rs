//! YAML frontmatter recognition, extraction, and document anchoring.
//!
//! The delimited block is cut out here, handed to the exact reader in
//! [`yaml`], and the body-relative positions that reader records are lifted
//! back into document coordinates.

use std::collections::BTreeMap;

use saphyr_parser::Span;

pub(super) mod yaml;

use super::error::MarkdownParseError as ParseError;
use super::lines::{text_range, LineIndex};
use super::model::{
    DocumentFrontmatter, FrontmatterAnchor, FrontmatterAnchors, FrontmatterLocation,
};

/// Reads the frontmatter block a document opens with, if it opens with one.
///
/// The returned range is the block's extent, which the caller masks out of the
/// source before the Markdown scan reads it.
pub(super) fn parse(
    source: &str,
    lines: &LineIndex,
) -> Result<(DocumentFrontmatter, Option<std::ops::Range<usize>>), ParseError> {
    if lines.line_text(source, 1).ok_or(ParseError::RANGE)? != "---" {
        return Ok((DocumentFrontmatter::Absent, None));
    }
    let mut closing_line = None;
    for line in 2..=lines.line_count() {
        if lines.line_text(source, line).ok_or(ParseError::RANGE)? == "---" {
            closing_line = Some(line);
            break;
        }
    }
    let Some(closing_line) = closing_line else {
        let location = FrontmatterLocation {
            range: text_range(0, source.len()),
            start_line: 1,
            end_line: lines.line_count() as u64,
        };
        return Ok((
            DocumentFrontmatter::Invalid {
                location,
                message: "frontmatter opening delimiter has no closing `---` line".into(),
            },
            Some(0..source.len()),
        ));
    };
    let body_start = lines.line_start(2)?;
    let body_end = lines.line_start(closing_line)?;
    let block_end = lines.line_terminator_end(closing_line)?;
    let range = 0..block_end;
    let location = FrontmatterLocation {
        range: text_range(range.start, range.end),
        start_line: 1,
        end_line: closing_line as u64,
    };
    let body = source.get(body_start..body_end).ok_or(ParseError::RANGE)?;
    // A byte-order mark heading the block is removed once, here, where the body
    // is cut out and before the reader below is handed it. YAML gives one no
    // meaning at the head of a stream, but the parser does not drop it either,
    // so it arrives as the first character of the first key and leaves a
    // document whose `version` entry is invisibly named something else while
    // §1.6's mapping keys are the text their author wrote. Exactly one is
    // removed, so a second stays part of the key and remains as visible as any
    // other stray character, and every reported position counts it back in.
    let (body, mark) = match body.strip_prefix('\u{feff}') {
        Some(body) => (body, 1),
        None => (body, 0),
    };
    let frontmatter = match yaml::exact_frontmatter_mapping(body, mark) {
        Ok((value, positions)) => DocumentFrontmatter::Mapping {
            value,
            location,
            anchors: document_frontmatter_anchors(source, lines, &location, positions, mark)?,
        },
        Err(message) => DocumentFrontmatter::Invalid { location, message },
    };
    Ok((frontmatter, Some(range)))
}

/// Entry positions as the parser records them: one-based body lines and
/// zero-based character columns. Conversion checks arithmetic at the document boundary.
/// Duplicate mapping keys are rejected upstream, so no pointer occurs twice.
pub(in crate::markdown) type BodyAnchors = Vec<(String, BodyPosition)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::markdown) struct BodyPosition {
    line: usize,
    column: usize,
}

/// Reads a span's start into a body position.
///
/// `saphyr-parser` counts columns from zero — its scanner opens every stream
/// at column 0. Keep that coordinate until the document-anchor conversion
/// can check the addition to a one-based column. The marker counts characters,
/// not bytes; [`LineCursor`] converts it against the document's own line.
fn body_position(span: &Span) -> BodyPosition {
    BodyPosition {
        line: span.start.line(),
        column: span.start.col(),
    }
}

/// Lifts body-relative parser positions into document coordinates.
///
/// The body handed to the parser starts on the document's second line, so a
/// document line is the body line plus one. Body columns count characters
/// while [`DiagnosticLocation`](crate::DiagnosticLocation) counts bytes, so the
/// column is re-measured against the document line itself. That re-measurement
/// doubles as a consistency check: a position that does not fall inside the
/// block, or names a column the line does not have, fails parsing rather than
/// exposing partial anchor information.
///
/// Re-measuring each entry from the start of its line would be quadratic in a
/// block that puts many entries on one line, which a flow sequence does. The
/// positions are therefore ordered and converted by one left-to-right walk per
/// line. The conversion already emits them in document order, so the sort is
/// only a guard against depending on that.
///
/// `mark` is how many characters the block's removed byte-order mark took from
/// the head of the body, which is the only text the parser was not shown.
/// Positions on the body's first line are counted back over it, so an entry is
/// reported where the document actually spells it rather than one character
/// earlier.
fn document_frontmatter_anchors(
    source: &str,
    lines: &LineIndex,
    location: &FrontmatterLocation,
    mut positions: BodyAnchors,
    mark: usize,
) -> Result<FrontmatterAnchors, ParseError> {
    positions.sort_unstable_by_key(|(_, position)| (position.line, position.column));
    let mut anchors = BTreeMap::new();
    let mut cursor = LineCursor::default();
    for (pointer, position) in positions {
        let line = position.line.checked_add(1).ok_or(ParseError::RANGE)?;
        // Entries lie strictly between the opening and closing delimiters.
        if line < 2 || line as u64 >= location.end_line {
            return Err(ParseError::RANGE);
        }
        if cursor.line != line {
            let text = lines.line_text(source, line).ok_or(ParseError::RANGE)?;
            cursor = LineCursor::new(line, text);
        }
        let shift = if position.line == 1 { mark } else { 0 };
        let character_column = position
            .column
            .checked_add(1)
            .and_then(|column| column.checked_add(shift))
            .ok_or(ParseError::RANGE)?;
        let column = cursor
            .byte_column(character_column)
            .ok_or(ParseError::RANGE)?;
        if anchors
            .insert(
                pointer,
                FrontmatterAnchor {
                    line: line as u64,
                    column,
                },
            )
            .is_some()
        {
            return Err(ParseError::RANGE);
        }
    }
    Ok(FrontmatterAnchors(anchors))
}

/// A left-to-right walk of one line that converts one-based character columns
/// into one-based byte columns, keeping what it has already measured.
///
/// Columns must be requested in non-decreasing order; the walk never rewinds
/// and reports a column it has passed as unavailable.
#[derive(Default)]
pub(in crate::markdown) struct LineCursor<'a> {
    /// The document line being walked, or 0 before any line is.
    line: usize,
    /// The line's text from [`Self::column`] onward.
    rest: &'a str,
    /// One-based character column reached so far.
    column: usize,
    /// Byte offset of that column within the line.
    byte: usize,
}

impl<'a> LineCursor<'a> {
    pub(in crate::markdown) fn new(line: usize, text: &'a str) -> Self {
        Self {
            line,
            rest: text,
            column: 1,
            byte: 0,
        }
    }

    pub(in crate::markdown) fn byte_column(&mut self, character_column: usize) -> Option<u64> {
        if character_column < self.column {
            return None;
        }
        while self.column < character_column {
            let character = self.rest.chars().next()?;
            self.rest = self.rest.get(character.len_utf8()..)?;
            self.byte += character.len_utf8();
            self.column += 1;
        }
        Some(self.byte as u64 + 1)
    }
}

#[cfg(test)]
mod failure_tests {
    use super::*;

    #[test]
    fn impossible_yaml_positions_do_not_produce_partial_anchors() {
        let source = "---\né: 1\n---\n";
        let lines = LineIndex::new(source);
        let location = FrontmatterLocation {
            range: text_range(0, source.len()),
            start_line: 1,
            end_line: 3,
        };
        for position in [
            BodyPosition {
                line: usize::MAX,
                column: 1,
            },
            BodyPosition { line: 2, column: 1 },
            BodyPosition { line: 0, column: 0 },
            BodyPosition {
                line: 1,
                column: 30,
            },
        ] {
            let positions = vec![
                ("/valid".into(), BodyPosition { line: 1, column: 0 }),
                ("/invalid".into(), position),
            ];
            assert!(document_frontmatter_anchors(source, &lines, &location, positions, 0).is_err());
        }
        assert!(document_frontmatter_anchors(
            source,
            &lines,
            &location,
            vec![
                ("/duplicate".into(), BodyPosition { line: 1, column: 0 }),
                ("/duplicate".into(), BodyPosition { line: 1, column: 0 }),
            ],
            0
        )
        .is_err());
        assert!(document_frontmatter_anchors(
            source,
            &lines,
            &location,
            vec![("/overflow".into(), BodyPosition { line: 1, column: 1 })],
            usize::MAX
        )
        .is_err());
    }
}
