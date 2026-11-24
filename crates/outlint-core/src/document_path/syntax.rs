//! Document path parsing and rendering.

use std::fmt;

use crate::slug::HeadingSlug;
use crate::{BlockKind, ByteOffset};

use super::model::{
    CanonicalDocumentPath, DocumentPath, DocumentPathSyntaxError, DocumentPathTerminal, SectionStep,
};

impl DocumentPath {
    /// Parses the textual spelling of a path (see the grammar on [`DocumentPath`]).
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentPathSyntaxError`] positioned at the first byte that
    /// cannot continue a valid path.
    pub fn parse(source: &str) -> Result<Self, DocumentPathSyntaxError> {
        Parser::new(source).path()
    }
}

impl fmt::Display for CanonicalDocumentPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        DocumentPath::from(self).fmt(formatter)
    }
}

impl std::str::FromStr for DocumentPath {
    type Err = DocumentPathSyntaxError;

    fn from_str(source: &str) -> Result<Self, Self::Err> {
        Self::parse(source)
    }
}

impl fmt::Display for DocumentPath {
    /// Writes the canonical spelling: every terminal index explicit, no other
    /// whitespace or decoration. A descendant step is written back as `..`,
    /// so a parsed path round-trips; only paths from [`document_paths`](super::document_paths) are
    /// canonical spellings of a node.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("$")?;
        for step in &self.sections {
            let (separator, slug, index) = match step {
                SectionStep::Named { slug, index } => (".", Some(slug), *index),
                SectionStep::Position(index) => (".", None, Some(*index)),
                SectionStep::Descendant { slug, index } => ("..", Some(slug), *index),
            };
            formatter.write_str(separator)?;
            if let Some(slug) = slug {
                write!(formatter, "{slug}")?;
            }
            if let Some(index) = index {
                write!(formatter, "[{index}]")?;
            }
        }
        match self.terminal {
            None => {}
            Some(DocumentPathTerminal::DirectBlock { kind, index }) => {
                write!(formatter, "/{}[{index}]", block_kind_keyword(kind))?;
            }
            Some(DocumentPathTerminal::ListItem {
                list_index,
                item_index,
            }) => {
                write!(formatter, "/list[{list_index}]/item[{item_index}]")?;
            }
        }
        Ok(())
    }
}

fn block_kind_keyword(kind: BlockKind) -> &'static str {
    match kind {
        BlockKind::Paragraph => "p",
        BlockKind::List => "list",
        BlockKind::Quote => "quote",
        BlockKind::Code => "code",
        BlockKind::Html => "html",
        BlockKind::Break => "break",
    }
}

fn block_kind_from_keyword(keyword: &str) -> Option<BlockKind> {
    match keyword {
        "p" => Some(BlockKind::Paragraph),
        "list" => Some(BlockKind::List),
        "quote" => Some(BlockKind::Quote),
        "code" => Some(BlockKind::Code),
        "html" => Some(BlockKind::Html),
        "break" => Some(BlockKind::Break),
        _ => None,
    }
}

/// A cursor over the bytes of a path spelling.
///
/// Every token of the grammar is ASCII, so byte-wise scanning is exact and a
/// stopping position is always a character boundary; error offsets are byte
/// offsets into the original text.
struct Parser<'a> {
    source: &'a str,
    position: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            position: 0,
        }
    }

    fn path(mut self) -> Result<DocumentPath, DocumentPathSyntaxError> {
        if !self.eat(b'$') {
            return Err(self.error("a document path starts with `$`"));
        }
        let mut sections = Vec::new();
        while self.eat(b'.') {
            sections.push(self.section_step()?);
        }
        let terminal = self.eat(b'/').then(|| self.terminal()).transpose()?;
        match self.peek_char() {
            None => Ok(DocumentPath { sections, terminal }),
            Some(character) if terminal.is_none() => Err(self.error(format!(
                "unexpected {character:?}; expected `.`, `/`, or the end of the path"
            ))),
            Some(character) => Err(self.error(format!(
                "unexpected {character:?}; expected the end of the path"
            ))),
        }
    }

    /// Parses the step after a consumed `.`: a second `.` makes it a
    /// descendant step, which has no positional form.
    fn section_step(&mut self) -> Result<SectionStep, DocumentPathSyntaxError> {
        if self.eat(b'.') {
            let slug = self.slug(
                "expected a heading slug (`[a-z0-9]+(-[a-z0-9]+)*`) after `..`; \
                 a descendant step has no `[index]` position form",
            )?;
            let index = self.optional_index()?;
            return Ok(SectionStep::Descendant { slug, index });
        }
        if self.eat(b'[') {
            return self.index().map(SectionStep::Position);
        }
        let slug = self
            .slug("expected a heading slug (`[a-z0-9]+(-[a-z0-9]+)*`) or a `[index]` position")?;
        let index = self.optional_index()?;
        Ok(SectionStep::Named { slug, index })
    }

    fn optional_index(&mut self) -> Result<Option<usize>, DocumentPathSyntaxError> {
        if self.eat(b'[') {
            self.index().map(Some)
        } else {
            Ok(None)
        }
    }

    /// Parses a slug one run at a time so that the error offset points at the
    /// byte that broke the form: a `-` without a following letter or digit
    /// fails after the `-`, not at the start of the segment. `missing` is the
    /// message when no slug starts here.
    fn slug(&mut self, missing: &str) -> Result<HeadingSlug, DocumentPathSyntaxError> {
        let start = self.position;
        if self.slug_run().is_empty() {
            return Err(self.error(missing));
        }
        while self.eat(b'-') {
            if self.slug_run().is_empty() {
                return Err(self.error("expected a letter or digit after `-` in a heading slug"));
            }
        }
        // Each run is non-empty and the runs are joined by single hyphens,
        // which is exactly the slug form.
        let text = self.source.get(start..self.position).unwrap_or_default();
        Ok(HeadingSlug(text.to_owned()))
    }

    /// Consumes one `[a-z0-9]*` run.
    fn slug_run(&mut self) -> &'a str {
        self.take_while(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    }

    fn terminal(&mut self) -> Result<DocumentPathTerminal, DocumentPathSyntaxError> {
        let start = self.position;
        let keyword = self.take_while(|byte| byte.is_ascii_lowercase());
        let Some(kind) = block_kind_from_keyword(keyword) else {
            let message = if keyword == "item" {
                "`item` must directly follow a `list` step"
            } else {
                "expected a direct block kind: `p`, `list`, `code`, `quote`, `html`, or `break`"
            };
            return Err(self.error_at(start, message));
        };
        let index = if self.eat(b'[') { self.index()? } else { 0 };
        if kind != BlockKind::List || !self.eat(b'/') {
            return Ok(DocumentPathTerminal::DirectBlock { kind, index });
        }

        let item_start = self.position;
        let keyword = self.take_while(|byte| byte.is_ascii_lowercase());
        if keyword != "item" {
            return Err(self.error_at(item_start, "expected `item` directly after a `list` step"));
        }
        let item_index = if self.eat(b'[') { self.index()? } else { 0 };
        Ok(DocumentPathTerminal::ListItem {
            list_index: index,
            item_index,
        })
    }

    /// Parses the digits and closing bracket of an index whose `[` has been
    /// consumed.
    fn index(&mut self) -> Result<usize, DocumentPathSyntaxError> {
        let start = self.position;
        let digits = self.take_while(|byte| byte.is_ascii_digit());
        if digits.is_empty() {
            return Err(self.error_at(start, "expected a zero-based index"));
        }
        if digits.len() > 1 && digits.starts_with('0') {
            return Err(self.error_at(start, "an index has no leading zero"));
        }
        let index = digits
            .parse()
            .map_err(|_| self.error_at(start, "index is too large"))?;
        if !self.eat(b']') {
            return Err(self.error("expected `]` after the index"));
        }
        Ok(index)
    }

    fn eat(&mut self, expected: u8) -> bool {
        if self.source.as_bytes().get(self.position) == Some(&expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn take_while(&mut self, accept: impl Fn(u8) -> bool) -> &'a str {
        let start = self.position;
        while self
            .source
            .as_bytes()
            .get(self.position)
            .is_some_and(|byte| accept(*byte))
        {
            self.position += 1;
        }
        self.source.get(start..self.position).unwrap_or_default()
    }

    fn peek_char(&self) -> Option<char> {
        self.source.get(self.position..)?.chars().next()
    }

    fn error(&self, message: impl Into<String>) -> DocumentPathSyntaxError {
        self.error_at(self.position, message)
    }

    fn error_at(&self, offset: usize, message: impl Into<String>) -> DocumentPathSyntaxError {
        DocumentPathSyntaxError {
            offset: ByteOffset(offset),
            message: message.into(),
        }
    }
}
