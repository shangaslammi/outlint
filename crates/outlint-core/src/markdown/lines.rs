//! Physical line indexing and the byte-offset-preserving source rewrites.
//!
//! Every helper here keeps byte offsets addressable in the original source,
//! which is the invariant the Markdown scan and the frontmatter anchors both
//! rest on.

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

    pub(super) fn line_start(&self, line: usize) -> usize {
        line.checked_sub(1)
            .and_then(|index| self.lines.get(index).map(|line| line.start))
            .unwrap_or_default()
    }

    pub(super) fn line_end(&self, line: usize, source_len: usize) -> usize {
        line.checked_sub(1)
            .and_then(|index| self.lines.get(index).map(|line| line.end))
            .unwrap_or(source_len)
    }

    pub(super) fn line_terminator_end(&self, line: usize, source_len: usize) -> usize {
        line.checked_sub(1)
            .and_then(|index| self.lines.get(index).map(|line| line.terminator_end))
            .unwrap_or(source_len)
    }

    pub(super) fn line_text<'a>(&self, source: &'a str, line: usize) -> Option<&'a str> {
        let start = self.line_start(line);
        let end = line
            .checked_sub(1)
            .and_then(|index| self.lines.get(index).map(|line| line.end))?;
        source.get(start..end)
    }

    pub(super) fn line_count(&self) -> usize {
        self.lines.len()
    }
}

pub(super) fn clamp_range(
    range: std::ops::Range<usize>,
    source_len: usize,
) -> std::ops::Range<usize> {
    range.start.min(source_len)..range.end.min(source_len).max(range.start.min(source_len))
}

pub(super) fn text_range(start: usize, end: usize) -> TextRange {
    TextRange {
        start: ByteOffset(start),
        end: ByteOffset(end),
    }
}

pub(super) fn byte_column(line_start: usize, offset: usize) -> u64 {
    (offset - line_start + 1) as u64
}

pub(super) fn physical_lines(source: &str) -> Vec<&str> {
    line_ranges(source)
        .into_iter()
        .filter(|line| line.start < source.len())
        .filter_map(|line| source.get(line.start..line.end))
        .collect()
}

pub(super) fn mask_source_range(source: &str, range: std::ops::Range<usize>) -> String {
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
    match String::from_utf8(bytes) {
        Ok(masked) => masked,
        // Replacing bytes with ASCII cannot invalidate the original UTF-8,
        // but retain total behavior if this invariant is ever changed.
        Err(_) => source.to_owned(),
    }
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
