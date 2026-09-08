//! Physical line indexing and the byte-offset-preserving source rewrites.
//!
//! Every helper here keeps byte offsets addressable in the original source,
//! which is the invariant the Markdown scan and the frontmatter anchors both
//! rest on.

use super::error::MarkdownParseError as ParseError;
use std::borrow::Cow;

use crate::{ByteOffset, TextRange};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineRange {
    start: usize,
    end: usize,
    terminator_end: usize,
}

fn line_ranges(source: &str) -> Vec<LineRange> {
    let bytes = source.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        let terminator_end = match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => index + 2,
            b'\r' | b'\n' => index + 1,
            _ => {
                index += 1;
                continue;
            }
        };
        lines.push(LineRange {
            start,
            end: index,
            terminator_end,
        });
        start = terminator_end;
        index = terminator_end;
    }
    lines.push(LineRange {
        start,
        end: source.len(),
        terminator_end: source.len(),
    });
    lines
}

pub(super) struct LineIndex {
    lines: Vec<LineRange>,
}

impl LineIndex {
    pub(super) fn new(source: &str) -> Self {
        Self {
            lines: line_ranges(source),
        }
    }

    pub(super) fn line_number(&self, offset: usize) -> usize {
        self.lines.partition_point(|line| line.start <= offset)
    }

    pub(super) fn line_start(&self, line: usize) -> Result<usize, ParseError> {
        line.checked_sub(1)
            .and_then(|index| self.lines.get(index).map(|line| line.start))
            .ok_or(ParseError::RANGE)
    }

    pub(super) fn line_end(&self, line: usize) -> Result<usize, ParseError> {
        line.checked_sub(1)
            .and_then(|index| self.lines.get(index).map(|line| line.end))
            .ok_or(ParseError::RANGE)
    }

    pub(super) fn line_terminator_end(&self, line: usize) -> Result<usize, ParseError> {
        line.checked_sub(1)
            .and_then(|index| self.lines.get(index).map(|line| line.terminator_end))
            .ok_or(ParseError::RANGE)
    }

    pub(super) fn line_text<'a>(&self, source: &'a str, line: usize) -> Option<&'a str> {
        let start = self.line_start(line).ok()?;
        let end = line
            .checked_sub(1)
            .and_then(|index| self.lines.get(index).map(|line| line.end))?;
        source.get(start..end)
    }

    pub(super) fn line_count(&self) -> usize {
        self.lines.len()
    }
}

/// Validates bounds, ordering, and both UTF-8 boundaries before using parser offsets.
pub(super) fn source_range(
    source: &str,
    range: std::ops::Range<usize>,
) -> Result<std::ops::Range<usize>, ParseError> {
    source.get(range.clone()).ok_or(ParseError::RANGE)?;
    Ok(range)
}

pub(super) fn text_range(start: usize, end: usize) -> TextRange {
    TextRange {
        start: ByteOffset(start),
        end: ByteOffset(end),
    }
}

pub(super) fn byte_column(line_start: usize, offset: usize) -> Result<u64, ParseError> {
    offset
        .checked_sub(line_start)
        .and_then(|column| column.checked_add(1))
        .and_then(|column| u64::try_from(column).ok())
        .ok_or(ParseError::RANGE)
}

pub(super) fn without_trailing_blank_lines(
    source: &str,
    range: std::ops::Range<usize>,
    lines: &LineIndex,
) -> Result<std::ops::Range<usize>, ParseError> {
    let safe = source_range(source, range)?;
    if safe.is_empty() {
        return Ok(safe);
    }

    let mut end = safe.end;
    loop {
        let probe = end.checked_sub(1).unwrap_or(safe.start).max(safe.start);
        let line = lines.line_number(probe);
        let text = lines.line_text(source, line).ok_or(ParseError::RANGE)?;
        if !text.bytes().all(|byte| matches!(byte, b' ' | b'\t')) {
            break;
        }
        let line_start = lines.line_start(line)?;
        if line_start < safe.start {
            break;
        }
        end = line_start;
        if end == safe.start {
            break;
        }
    }
    Ok(safe.start..end)
}

pub(super) fn physical_lines(source: &str) -> Result<Vec<&str>, ParseError> {
    line_ranges(source)
        .into_iter()
        .filter(|line| line.start < source.len())
        .map(|line| source.get(line.start..line.end).ok_or(ParseError::RANGE))
        .collect()
}

pub(super) fn mask_source_range(
    source: &str,
    range: std::ops::Range<usize>,
) -> Result<String, ParseError> {
    let range = source_range(source, range)?;
    let bytes = source
        .bytes()
        .enumerate()
        .map(|(index, byte)| {
            if range.contains(&index) && !matches!(byte, b'\r' | b'\n') {
                b' '
            } else {
                byte
            }
        })
        .collect();
    String::from_utf8(bytes).map_err(|_| ParseError::RANGE)
}

pub(super) fn normalize_bare_cr(source: &str) -> Cow<'_, str> {
    let has_bare_cr =
        source.as_bytes().iter().enumerate().any(|(index, byte)| {
            *byte == b'\r' && source.as_bytes().get(index + 1) != Some(&b'\n')
        });
    if !has_bare_cr {
        return Cow::Borrowed(source);
    }

    Cow::Owned(
        source
            .char_indices()
            .map(|(index, character)| {
                if character == '\r' && source.as_bytes().get(index + 1) != Some(&b'\n') {
                    '\n'
                } else {
                    character
                }
            })
            .collect(),
    )
}

#[cfg(test)]
mod failure_tests {
    use super::*;

    #[test]
    fn source_helpers_reject_invalid_slices_and_line_coordinates() {
        let source = "é\r\n界\r";
        let lines = LineIndex::new(source);
        for range in [
            1..2,
            0..1,
            5..7,
            0..usize::MAX,
            std::ops::Range { start: 4, end: 2 },
        ] {
            assert!(source_range(source, range.clone()).is_err());
            assert!(mask_source_range(source, range.clone()).is_err());
            assert!(without_trailing_blank_lines(source, range, &lines).is_err());
        }
        for line in [0, 4, usize::MAX] {
            assert!(lines.line_start(line).is_err());
            assert!(lines.line_end(line).is_err());
            assert!(lines.line_terminator_end(line).is_err());
        }
        assert!(byte_column(2, 1).is_err());
        assert!(byte_column(0, usize::MAX).is_err());
        assert_eq!(
            physical_lines(source).expect("valid UTF-8 lines"),
            ["é", "界"]
        );
        assert_eq!(
            mask_source_range(source, 0..2).expect("whole codepoint"),
            "  \r\n界\r"
        );
    }
}
