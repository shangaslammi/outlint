//! The §4.6 `fm[...]` wrapper and the §2.3 absolute singular capture path.
//!
//! Two things live here, and keeping them apart is the point of the module.
//!
//! A **full query** is whatever RFC 9535 admits. §4.6 delegates: "the full RFC
//! 9535 grammar remains admitted. A query using any other RFC construct MUST
//! NOT be rejected merely for falling outside the guaranteed core; it is
//! submitted in full to the implementation's JSONPath provider." Admission is
//! therefore *delegation*, never classification: nothing here recognizes the
//! guaranteed core, because a recognizer used as an admission gate would
//! reject exactly the queries the spec promises to accept.
//!
//! A **capture path** is narrower on purpose. §2.3 requires "an absolute,
//! `$`-rooted RFC 9535 **singular query**: its segments are name or index
//! segments only, as defined by RFC 9535 §2.3.5.1", and makes anything else
//! `invalid-capture`. The provider validates the query but exposes no parsed
//! form, so singularity is decided by an independent recognizer here. That
//! recognizer is not a core classifier and is not used to admit full queries;
//! it exists because the capture binding site has a stricter grammar and
//! because a capture needs its components decoded, not just accepted.
//!
//! Nothing in either direction stores a provider AST. A query's semantic
//! identity is its exact source — §5.4 decides `duplicate-ref` on whether
//! "their query source is identical" — so the source is what is retained, and
//! a prepared form is compiled from it again when a document is at hand.

use std::fmt;

use serde_json::Value;
use serde_json_path::JsonPath;

use super::path::LocatedNodeSet;
use super::syntax::{LocatorParseError, LocatorParseErrorKind, LocatorSource};

/// The source of one complete RFC 9535 query, validated and retained.
///
/// Equality and hashing are over the exact source, which is what §5.4 asks
/// for: "`fm[...]` propositions duplicate when their query source is
/// identical [...] Syntactically different JSONPath queries are not treated as
/// duplicates merely because they may select the same nodes." So `$.a` and
/// `$['a']` are two queries here, deliberately.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub(crate) struct FullQuerySource(Box<str>);

impl FullQuerySource {
    /// Validates `source` as one complete query and keeps its spelling.
    ///
    /// The provider's parsed form is dropped on purpose. Holding it would put
    /// a provider type inside a semantic value, tie that value's lifetime and
    /// equality to the provider, and make the retained source decorative
    /// rather than authoritative.
    pub(crate) fn parse(source: &str) -> Result<Self, QueryParseError> {
        JsonPath::parse(source).map_err(QueryParseError::from_provider)?;
        Ok(Self(source.into()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Compiles the retained source for evaluation.
    ///
    /// This re-parses, which is the cost of not storing a provider AST. It is
    /// paid once per prepared validator rather than once per document, and it
    /// cannot fail for a source that [`parse`](Self::parse) accepted — the
    /// error is returned rather than asserted away because the library "must
    /// not panic on malformed input" and a provider that disagreed with itself
    /// is a bug to report, not to crash on.
    pub(crate) fn prepare(&self) -> Result<PreparedQuery, QueryParseError> {
        JsonPath::parse(&self.0)
            .map(PreparedQuery)
            .map_err(QueryParseError::from_provider)
    }
}

impl fmt::Display for FullQuerySource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A query compiled and ready to evaluate.
///
/// The provider type is private to this wrapper and appears in no signature
/// this module offers, so nothing outside can reach `NormalizedPath`,
/// `to_json_pointer`, or any other provider rendering.
#[derive(Debug)]
pub(crate) struct PreparedQuery(JsonPath);

impl PreparedQuery {
    /// Evaluates the query against one frontmatter view.
    ///
    /// §4.6: "Implementations MUST evaluate the complete result and MUST NOT
    /// silently truncate it." The provider's located result is taken whole and
    /// converted whole; there is no limit, no `take`, and no early exit, so a
    /// caller that only needs to know whether *some* node is `true` still
    /// evaluates every node — which is what makes §4.6's `invalid-value`
    /// suppression reachable rather than short-circuited away.
    pub(crate) fn evaluate<'a>(&self, document: &'a Value) -> LocatedNodeSet<'a> {
        LocatedNodeSet::from_provider(&self.0.query_located(document))
    }
}

/// An Outlint-owned copy of a provider parse failure.
///
/// `serde_json_path::ParseError` is never exposed: it would put a provider
/// type in an Outlint error's public shape and tie the message format to a
/// version bump. Only the message and the position survive the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueryParseError {
    message: Box<str>,
    position: usize,
}

impl QueryParseError {
    fn from_provider(error: serde_json_path::ParseError) -> Self {
        Self {
            message: error.message().into(),
            position: error.position(),
        }
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    /// The offset into the *query* at which the provider stopped.
    pub(crate) fn position(&self) -> usize {
        self.position
    }
}

impl fmt::Display for QueryParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at position {}", self.message, self.position)
    }
}

/// `fm[query]` or `fm[query]=literal`.
///
/// The equality remainder is kept raw. §4.6 says "the literal is the remainder
/// of the locator and is resolved as one YAML 1.2 core-schema scalar", and
/// that resolution belongs to the lane that owns typed values; resolving it
/// here would decide `fm[$.x]=null`, `match_case`, and scalar typing in a
/// module that knows nothing about any of them.
///
/// `None` and `Some("")` are different locators and stay different: the first
/// is a bare boolean read, the second is an equality proposition against the
/// empty scalar. Collapsing them would silently turn one §4.6 form into the
/// other.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct FrontmatterQueryLocator {
    source: LocatorSource,
    query: FullQuerySource,
    equality: Option<Box<str>>,
}

impl FrontmatterQueryLocator {
    pub(crate) fn source(&self) -> &LocatorSource {
        &self.source
    }

    pub(crate) fn query(&self) -> &FullQuerySource {
        &self.query
    }

    /// The raw equality remainder, or `None` for a bare boolean read.
    pub(crate) fn equality(&self) -> Option<&str> {
        self.equality.as_deref()
    }
}

/// The literal `fm[` that opens the wrapper.
const WRAPPER_OPEN: &str = "fm[";

/// Parses `fm[query]` or `fm[query]=literal`.
///
/// §4.6: "the wrapper ends after parsing that complete query, not at the first
/// `]` or `=` occurring inside it; only an `=` following the wrapper
/// introduces Outlint equality." So the close is found by scanning, not by
/// searching: the scan tracks JSONPath bracket nesting and both quote forms,
/// honouring backslash escapes inside a string, and only a `]` at wrapper
/// depth and outside a string closes the wrapper. `$['a]b']` and
/// `$[?@['k'] == 'v]=']` both survive that; neither survives a `split_once`.
///
/// The caller has already established that `source` begins with `fm[`.
pub(crate) fn parse_frontmatter_query(
    source: &str,
) -> Result<FrontmatterQueryLocator, LocatorParseError> {
    let open = WRAPPER_OPEN.len();
    let close = find_wrapper_close(source, open).ok_or_else(|| {
        LocatorParseError::new(LocatorParseErrorKind::UnterminatedQueryWrapper, open)
    })?;

    let query_source = &source[open..close];
    if query_source.is_empty() {
        return Err(LocatorParseError::new(
            LocatorParseErrorKind::EmptyQuery,
            open,
        ));
    }
    let query = FullQuerySource::parse(query_source).map_err(|error| {
        LocatorParseError::detailed(
            LocatorParseErrorKind::InvalidQuery,
            open + error.position(),
            error.message(),
        )
    })?;

    let after = &source[close + ']'.len_utf8()..];
    let equality = match after.strip_prefix('=') {
        // §4.6: the literal is "the remainder of the locator", so every
        // trailing character belongs to it, `=` and `]` included.
        Some(literal) => Some(Box::from(literal)),
        None if after.is_empty() => None,
        None => {
            return Err(LocatorParseError::new(
                LocatorParseErrorKind::TrailingTextAfterQuery,
                close + ']'.len_utf8(),
            ))
        }
    };

    Ok(FrontmatterQueryLocator {
        source: LocatorSource::new(source),
        query,
        equality,
    })
}

/// Finds the byte offset of the `]` that closes the wrapper opened at `open`.
fn find_wrapper_close(source: &str, open: usize) -> Option<usize> {
    /// What the scan is in the middle of.
    enum State {
        /// Outside any string literal.
        Bare,
        /// Inside a string opened by this quote character.
        Quoted(char),
        /// Inside a string, immediately after a reverse solidus.
        Escaped(char),
    }

    let mut state = State::Bare;
    // Nesting of `[` *inside* the query. The wrapper's own bracket is not
    // counted, so a `]` at zero depth is the one that closes it.
    let mut depth = 0_usize;

    for (offset, character) in source[open..].char_indices() {
        state = match (state, character) {
            (State::Quoted(quote), '\\') => State::Escaped(quote),
            (State::Escaped(quote), _) => State::Quoted(quote),
            (State::Quoted(quote), character) if character == quote => State::Bare,
            (State::Quoted(quote), _) => State::Quoted(quote),
            (State::Bare, quote @ ('\'' | '"')) => State::Quoted(quote),
            (State::Bare, '[') => {
                depth += 1;
                State::Bare
            }
            (State::Bare, ']') => {
                let Some(remaining) = depth.checked_sub(1) else {
                    return Some(open + offset);
                };
                depth = remaining;
                State::Bare
            }
            (state, _) => state,
        };
    }
    None
}

// ---------------------------------------------------------------------------
// Absolute singular capture paths
// ---------------------------------------------------------------------------

/// One decoded component of a singular capture path.
///
/// An index is signed and an ordinary locator position is not, which is why
/// these are different types rather than one shared "index". §2.3 keeps
/// negative indices ("a path is declarative even when it uses a negative
/// index"), while §4.4's `[i]` matches `0|[1-9][0-9]*`. The bounds differ too:
/// RFC 9535's I-JSON range caps this one, and §4.4 gives the other "no upper
/// bound".
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum SingularComponent {
    /// A child name, with every RFC escape already decoded.
    Name(Box<str>),
    /// A child index, possibly negative, within the I-JSON exact range.
    Index(i64),
}

/// An absolute `$`-rooted RFC 9535 singular query, as §2.3 requires of
/// `frontmatter.captures.<name>.path`.
///
/// Both the exact source and the decoded components are kept. The source is
/// what a diagnostic quotes back; the components are what a lookup walks, and
/// decoding them once here means no later code has to understand RFC string
/// escapes to find out which member a path names.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct AbsoluteSingularPath {
    source: Box<str>,
    components: Vec<SingularComponent>,
}

impl AbsoluteSingularPath {
    /// Parses one absolute singular query.
    ///
    /// Validity is the provider's answer and singularity is this module's.
    /// Delegating first means a path is never called "not singular" when it is
    /// really just malformed, and it keeps the recognizer from having to be a
    /// second, competing JSONPath parser.
    pub(crate) fn parse(source: &str) -> Result<Self, SingularPathError> {
        // §2.3: "A relative, `@`-rooted query is `invalid-capture` because
        // this binding site supplies no current node." Checked first so that
        // spelling gets its own answer instead of a generic parse failure.
        if !source.starts_with('$') {
            return Err(SingularPathError::new(
                SingularPathErrorKind::NotAbsolute,
                0,
            ));
        }
        JsonPath::parse(source).map_err(|error| {
            SingularPathError::detailed(
                SingularPathErrorKind::InvalidQuery,
                error.position(),
                error.message(),
            )
        })?;

        Ok(Self {
            source: source.into(),
            components: decode_singular_segments(source)?,
        })
    }

    pub(crate) fn source(&self) -> &str {
        &self.source
    }

    /// Awaiting frontmatter capture evaluation, which walks these to find
    /// the node a declaration's path names.
    #[allow(dead_code)]
    pub(crate) fn components(&self) -> &[SingularComponent] {
        &self.components
    }
}

/// Why a capture path was refused, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SingularPathError {
    kind: SingularPathErrorKind,
    offset: usize,
    detail: Option<Box<str>>,
}

impl SingularPathError {
    fn new(kind: SingularPathErrorKind, offset: usize) -> Self {
        Self {
            kind,
            offset,
            detail: None,
        }
    }

    fn detailed(kind: SingularPathErrorKind, offset: usize, detail: &str) -> Self {
        Self {
            kind,
            offset,
            detail: Some(detail.into()),
        }
    }

    // The capture loader reports a refused path through `Display`, which
    // already carries all three of these. They are kept for a caller that
    // wants to branch on the fault or highlight it inside the path.
    #[allow(dead_code)]
    pub(crate) fn kind(&self) -> SingularPathErrorKind {
        self.kind
    }

    #[allow(dead_code)]
    pub(crate) fn offset(&self) -> usize {
        self.offset
    }

    /// The provider's own message, when the failure came from the provider.
    #[allow(dead_code)]
    pub(crate) fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

impl fmt::Display for SingularPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at offset {}", self.kind, self.offset)?;
        if let Some(detail) = &self.detail {
            write!(formatter, ": {detail}")?;
        }
        Ok(())
    }
}

/// The faults a capture path can have.
///
/// Validity is settled by the provider before this recognizer runs, so in
/// practice a malformed path reports [`InvalidQuery`](Self::InvalidQuery) and
/// the recognizer's own well-formedness answers —
/// [`UnterminatedName`](Self::UnterminatedName),
/// [`InvalidEscape`](Self::InvalidEscape),
/// [`InvalidIndex`](Self::InvalidIndex) — are reached only if the two ever
/// disagree. They are kept because the recognizer must be correct on its own
/// terms: it decodes names the provider never hands back, and a decoder that
/// leaned on someone else's validation would be one provider bump away from
/// producing a name no document contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SingularPathErrorKind {
    /// The path is not `$`-rooted.
    NotAbsolute,
    /// The provider rejected the query outright.
    InvalidQuery,
    /// A construct outside RFC 9535 §2.3.5.1's name and index segments.
    NotSingular,
    /// A string literal with no closing quote.
    UnterminatedName,
    /// An escape sequence RFC 9535 does not define.
    InvalidEscape,
    /// An index outside `int`, or outside the I-JSON exact range.
    InvalidIndex,
}

impl fmt::Display for SingularPathErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            SingularPathErrorKind::NotAbsolute => "a capture path must be `$`-rooted",
            SingularPathErrorKind::InvalidQuery => "not a valid JSONPath query",
            SingularPathErrorKind::NotSingular => {
                "a capture path takes only name and index segments"
            }
            SingularPathErrorKind::UnterminatedName => "unterminated quoted name",
            SingularPathErrorKind::InvalidEscape => "invalid escape sequence",
            SingularPathErrorKind::InvalidIndex => "invalid index",
        };
        formatter.write_str(message)
    }
}

/// The I-JSON exact range §4.6 requires of an index selector: "−9,007,199,254,
/// 740,991 through 9,007,199,254,740,991 inclusive".
const I_JSON_MAX: i64 = 9_007_199_254_740_991;

/// Recognizes and decodes RFC 9535 §2.3.5.1 singular-query segments.
///
/// The grammar admitted is exactly
///
/// ```text
/// abs-singular-query      = root-identifier singular-query-segments
/// singular-query-segments = *(S (name-segment / index-segment))
/// name-segment            = "[" name-selector "]" / "." member-name-shorthand
/// index-segment           = "[" int "]"
/// ```
///
/// which is narrower than a bracketed selection: no whitespace inside the
/// brackets, one selector per segment, and no wildcard, union, slice,
/// descendant, filter, or function anywhere. Each of those is refused by
/// falling off this grammar rather than by being listed, so a construct
/// nobody thought of is refused too.
///
/// Reachable on its own, not only through [`AbsoluteSingularPath::parse`],
/// because the provider gate in front of it is in one respect *narrower* than
/// RFC 9535: `serde_json_path = 0.7.2` refuses a surrogate-pair escape such as
/// `$['\ud83d\ude00']`, which the RFC's `hexchar` admits. This function
/// implements the RFC rule, and testing it directly is what keeps that correct
/// while the gate in front of it is not. See the provider-boundary tests,
/// which pin the limitation so a provider bump surfaces it.
pub(crate) fn decode_singular_segments(
    source: &str,
) -> Result<Vec<SingularComponent>, SingularPathError> {
    let characters: Vec<(usize, char)> = source.char_indices().collect();
    let mut cursor = Cursor::new(&characters, source.len());
    if !cursor.eat('$') {
        return Err(SingularPathError::new(
            SingularPathErrorKind::NotAbsolute,
            0,
        ));
    }

    let mut components = Vec::new();
    loop {
        cursor.skip_whitespace();
        match cursor.peek() {
            None => return Ok(components),
            Some('.') => {
                cursor.advance();
                components.push(SingularComponent::Name(cursor.take_shorthand_name()?));
            }
            Some('[') => {
                cursor.advance();
                let component = match cursor.peek() {
                    Some(quote @ ('\'' | '"')) => {
                        cursor.advance();
                        SingularComponent::Name(cursor.take_quoted_name(quote)?)
                    }
                    // Only a string literal or an `int` opens a singular
                    // segment. A `*`, a `?`, or a `:` is a wildcard, filter,
                    // or slice — plural constructs, not malformed indices.
                    Some('-' | '0'..='9') => SingularComponent::Index(cursor.take_index()?),
                    _ => {
                        return Err(SingularPathError::new(
                            SingularPathErrorKind::NotSingular,
                            cursor.offset(),
                        ))
                    }
                };
                if !cursor.eat(']') {
                    return Err(SingularPathError::new(
                        SingularPathErrorKind::NotSingular,
                        cursor.offset(),
                    ));
                }
                components.push(component);
            }
            Some(_) => {
                return Err(SingularPathError::new(
                    SingularPathErrorKind::NotSingular,
                    cursor.offset(),
                ))
            }
        }
    }
}

/// A cursor over a capture path's characters, carrying byte offsets.
struct Cursor<'a> {
    characters: &'a [(usize, char)],
    end: usize,
    index: usize,
}

impl<'a> Cursor<'a> {
    fn new(characters: &'a [(usize, char)], end: usize) -> Self {
        Self {
            characters,
            end,
            index: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.characters
            .get(self.index)
            .map(|(_, character)| *character)
    }

    /// The byte offset of the current character, or of the end of the path.
    fn offset(&self) -> usize {
        self.characters
            .get(self.index)
            .map_or(self.end, |(offset, _)| *offset)
    }

    fn advance(&mut self) {
        self.index += 1;
    }

    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Skips RFC 9535's `S`, which separates segments but never appears
    /// inside a singular query's brackets.
    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.advance();
        }
    }

    /// Takes a `member-name-shorthand` after a `.`.
    ///
    /// Rejecting `.` and `*` here is what refuses a descendant segment and a
    /// wildcard: neither is a `name-first` character.
    fn take_shorthand_name(&mut self) -> Result<Box<str>, SingularPathError> {
        let start = self.offset();
        let mut name = String::new();
        match self.peek() {
            Some(character) if is_name_first(character) => {
                name.push(character);
                self.advance();
            }
            _ => {
                return Err(SingularPathError::new(
                    SingularPathErrorKind::NotSingular,
                    start,
                ))
            }
        }
        while let Some(character) = self.peek() {
            if !is_name_char(character) {
                break;
            }
            name.push(character);
            self.advance();
        }
        Ok(name.into())
    }

    /// Takes a `name-selector` string literal, decoding every escape.
    ///
    /// The opening quote has already been consumed. Decoding here rather than
    /// keeping the literal means the component holds the member name the
    /// document would have to contain, so a lookup is a map lookup and never
    /// a second round of unescaping.
    fn take_quoted_name(&mut self, quote: char) -> Result<Box<str>, SingularPathError> {
        let mut name = String::new();
        loop {
            let offset = self.offset();
            let Some(character) = self.peek() else {
                return Err(SingularPathError::new(
                    SingularPathErrorKind::UnterminatedName,
                    offset,
                ));
            };
            self.advance();
            match character {
                character if character == quote => return Ok(name.into()),
                '\\' => name.push(self.take_escape(quote, offset)?),
                // `unescaped` excludes the C0 controls; a literal one must be
                // written as an escape.
                control if (control as u32) < 0x20 => {
                    return Err(SingularPathError::new(
                        SingularPathErrorKind::InvalidEscape,
                        offset,
                    ))
                }
                other => name.push(other),
            }
        }
    }

    /// Decodes one escape sequence, the reverse solidus already consumed.
    ///
    /// `escapable` does not include either quote character: a string may
    /// escape only the quote that delimits it, so `\'` is valid in a
    /// single-quoted name and invalid in a double-quoted one.
    fn take_escape(&mut self, quote: char, start: usize) -> Result<char, SingularPathError> {
        let invalid = || SingularPathError::new(SingularPathErrorKind::InvalidEscape, start);
        let character = self.peek().ok_or_else(invalid)?;
        self.advance();
        match character {
            'b' => Ok('\u{0008}'),
            'f' => Ok('\u{000C}'),
            'n' => Ok('\n'),
            'r' => Ok('\r'),
            't' => Ok('\t'),
            '/' => Ok('/'),
            '\\' => Ok('\\'),
            character if character == quote => Ok(quote),
            'u' => self.take_hex_escape(start),
            _ => Err(invalid()),
        }
    }

    /// Decodes `\uXXXX`, joining a surrogate pair into one scalar value.
    ///
    /// RFC 9535's `hexchar` admits a high surrogate only when a `\u` low
    /// surrogate follows, and admits a lone low surrogate never. Rust's `char`
    /// cannot hold a surrogate at all, so this is the only place a name's
    /// astral characters can be reconstructed.
    fn take_hex_escape(&mut self, start: usize) -> Result<char, SingularPathError> {
        let invalid = || SingularPathError::new(SingularPathErrorKind::InvalidEscape, start);
        let first = self.take_four_hex_digits().ok_or_else(invalid)?;
        match first {
            0xD800..=0xDBFF => {
                if !(self.eat('\\') && self.eat('u')) {
                    return Err(invalid());
                }
                let second = self.take_four_hex_digits().ok_or_else(invalid)?;
                if !(0xDC00..=0xDFFF).contains(&second) {
                    return Err(invalid());
                }
                let combined = 0x1_0000 + ((first - 0xD800) << 10) + (second - 0xDC00);
                char::from_u32(combined).ok_or_else(invalid)
            }
            0xDC00..=0xDFFF => Err(invalid()),
            scalar => char::from_u32(scalar).ok_or_else(invalid),
        }
    }

    fn take_four_hex_digits(&mut self) -> Option<u32> {
        let mut value = 0_u32;
        for _ in 0..4 {
            let digit = self.peek()?.to_digit(16)?;
            self.advance();
            value = value * 16 + digit;
        }
        Some(value)
    }

    /// Takes an `int` index selector and applies the I-JSON bound.
    ///
    /// `int = "0" / (["-"] DIGIT1 *DIGIT)`, so `-0` and a leading zero are
    /// both refused — which is also what keeps `[01]` and `[0:2]` out.
    fn take_index(&mut self) -> Result<i64, SingularPathError> {
        let start = self.offset();
        let invalid = || SingularPathError::new(SingularPathErrorKind::InvalidIndex, start);

        let negative = self.eat('-');
        let mut digits = String::new();
        while let Some(character) = self.peek() {
            if !character.is_ascii_digit() {
                break;
            }
            digits.push(character);
            self.advance();
        }
        match digits.as_bytes() {
            [b'0'] if !negative => {}
            [b'1'..=b'9', ..] => {}
            _ => return Err(invalid()),
        }

        let magnitude: i64 = digits.parse().map_err(|_| invalid())?;
        let value = if negative { -magnitude } else { magnitude };
        if !(-I_JSON_MAX..=I_JSON_MAX).contains(&value) {
            return Err(invalid());
        }
        Ok(value)
    }
}

/// RFC 9535 `name-first = ALPHA / "_" / %x80-D7FF / %xE000-10FFFF`.
fn is_name_first(character: char) -> bool {
    character.is_ascii_alphabetic() || character == '_' || !character.is_ascii()
}

/// RFC 9535 `name-char = name-first / DIGIT`.
fn is_name_char(character: char) -> bool {
    is_name_first(character) || character.is_ascii_digit()
}
