//! The §4.4 locator grammar, parsed without a schema.
//!
//! A locator denotes a node list: a relative or `$.`-anchored name path,
//! optionally continued by structural steps, with a zero-based `[i]` allowed
//! after any step. This module decides only what a locator *says*. It never
//! consults a schema, so it cannot and does not decide whether a terminal name
//! is a rule id or a capture, whether a structural kind is allocated, or
//! whether an intermediate step is singular. Those are binding questions, and
//! they belong to the lane that owns binding.
//!
//! The types below are shaped so the lexical invariants of §4.4 cannot be
//! violated by construction rather than re-checked at each use:
//!
//! - Name steps and structural steps live in separate fields, so "a locator
//!   may move from names to structure but MUST NOT use a name step after a
//!   structural step" is a fact about the type, not a rule someone must
//!   remember.
//! - The terminal `/text` intrinsic is its own optional field rather than a
//!   structural kind, so nothing can follow it.
//! - The name path is [`NonEmpty`], so "`$` alone is not accepted" needs no
//!   emptiness check downstream.
//! - A position is a [`BigUint`], because §4.4 gives `i` "no upper bound".

use std::fmt;

use num_bigint::BigUint;

use crate::schema::NonEmpty;

/// A locator exactly as it was supplied.
///
/// §5.4 decides `duplicate-ref` for `fm[...]` propositions on "their query
/// source", and diagnostics quote locators back to their author, so the
/// original spelling is retained byte-for-byte and is never reconstructed
/// from the parsed steps.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub(crate) struct LocatorSource(Box<str>);

impl LocatorSource {
    pub(crate) fn new(source: &str) -> Self {
        Self(source.into())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LocatorSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A zero-based positional subscript `[i]`.
///
/// §4.4: `i` matches `0|[1-9][0-9]*` and "denotes a mathematical non-negative
/// integer with no upper bound. An index beyond the end of a concrete node
/// list selects nothing and produces the empty list; its magnitude is never an
/// error." The same paragraph forbids work proportional to the value:
/// "Implementations MUST NOT allocate memory or perform work proportional to
/// an index's numeric value; processing an index may be proportional only to
/// the length of its spelling."
///
/// A [`BigUint`] is what makes both halves true at once. It admits a
/// ten-thousand-digit index without saturating it to something smaller — which
/// would silently change *which* node is selected — while costing memory
/// linear in the spelling. Selection is [`select`](Self::select), whose one
/// checked conversion is the only place the value meets a machine integer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub(crate) struct LocatorPosition(BigUint);

impl LocatorPosition {
    /// Parses the digits between the brackets, or `None` if they are not
    /// `0|[1-9][0-9]*`.
    ///
    /// The grammar is checked before the conversion, not by it: `BigUint`
    /// would happily accept `007`, and a locator that spells one index two
    /// ways is two locators for §5.4's duplicate check.
    fn parse(digits: &str) -> Option<Self> {
        let mut bytes = digits.bytes();
        match bytes.next()? {
            b'0' if digits.len() == 1 => {}
            b'1'..=b'9' => {}
            _ => return None,
        }
        if !bytes.all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        BigUint::parse_bytes(digits.as_bytes(), 10).map(Self)
    }

    /// Selects the addressed element of `nodes`, or `None` if there is none.
    ///
    /// This is the whole of §4.4's out-of-range rule and the whole of its
    /// complexity bound. An index too large for `usize` cannot address any
    /// element of any slice that fits in memory, so the failed conversion is
    /// the empty list — reached in constant time, with no loop, no counter, no
    /// allocation, and no comparison against the collection's length beyond
    /// the one `get` already performs.
    pub(crate) fn select<'a, T>(&self, nodes: &'a [T]) -> Option<&'a T> {
        let index = usize::try_from(&self.0).ok()?;
        nodes.get(index)
    }

    /// The index as a mathematical integer.
    pub(crate) fn value(&self) -> &BigUint {
        &self.0
    }
}

impl fmt::Display for LocatorPosition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Where a locator's first name step resolves.
///
/// §4.5: a bare relative name "starts in the named scope to which the
/// constraint is attached", while "a leading `$.` starts at the outermost
/// named scope".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum LocatorAnchor {
    /// A bare relative name path.
    CurrentScope,
    /// A `$.`-anchored name path.
    SchemaRoot,
}

/// A name step's spelling, admitted but not yet bound.
///
/// A name step may name a rule or a capture, and §4.4 is explicit that which
/// one it is depends on the scope it lands in. Parsing therefore admits the
/// union of the two grammars — the §4.1 slug `[a-z0-9]+(-[a-z0-9]+)*` and the
/// §2.2 capture name `[a-z][a-z0-9_]*` — and leaves the choice between them to
/// binding. Admitting the union rather than something looser matters: a
/// spelling outside both grammars can name nothing in any scope, and §4.4
/// makes invalid locator syntax `invalid-document-shape` rather than an
/// unresolved reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub(crate) struct StepName(Box<str>);

impl StepName {
    fn parse(text: &str) -> Option<Self> {
        (is_slug(text) || is_capture_name(text)).then(|| Self(text.into()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// A structural kind step's spelling, admitted but not yet allocated.
///
/// §4.4 allocates exactly one structural member in this version — `/text`,
/// which has its own type — and says of the rest that "other structural kinds
/// and intrinsic members, including `/label`, remain unallocated until the
/// document features that own them are specified". Nothing may therefore be
/// resolved here; the token is retained so a later feature can allocate it.
///
/// Admission reuses the §4.1 slug grammar. The spec states no character
/// grammar for a kind spelling, and slug is the only identifier grammar it
/// defines, so reusing it keeps the admitted set inside what the spec already
/// writes rather than inventing a wider one. Every kind the spec names
/// (`text`, `label`) is a slug.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub(crate) struct StructuralKind(Box<str>);

impl StructuralKind {
    fn parse(text: &str) -> Option<Self> {
        is_slug(text).then(|| Self(text.into()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// A declared frontmatter capture's name, under the §2.2 grammar.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub(crate) struct CaptureName(Box<str>);

impl CaptureName {
    fn parse(text: &str) -> Option<Self> {
        is_capture_name(text).then(|| Self(text.into()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// One `.`-separated name step with its optional subscript.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct NameStep {
    name: StepName,
    position: Option<LocatorPosition>,
}

impl NameStep {
    pub(crate) fn name(&self) -> &StepName {
        &self.name
    }

    pub(crate) fn position(&self) -> Option<&LocatorPosition> {
        self.position.as_ref()
    }
}

/// One `/`-separated structural kind step with its optional subscript.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct StructuralStep {
    kind: StructuralKind,
    position: Option<LocatorPosition>,
}

impl StructuralStep {
    pub(crate) fn kind(&self) -> &StructuralKind {
        &self.kind
    }

    pub(crate) fn position(&self) -> Option<&LocatorPosition> {
        self.position.as_ref()
    }
}

/// The terminal `/text` intrinsic: a heading's case-preserving §1.3 text.
///
/// It carries only a subscript because §4.4 makes it "a terminal intrinsic
/// value". Having no field able to hold a following step is what enforces
/// that: the parser rejects a continuation, and no later code can build one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct IntrinsicTextStep {
    position: Option<LocatorPosition>,
}

impl IntrinsicTextStep {
    pub(crate) fn position(&self) -> Option<&LocatorPosition> {
        self.position.as_ref()
    }
}

/// An outline locator whose names have not been resolved against a schema.
///
/// "Unbound" is the whole point. §4.4's binding-time principle puts rule ids,
/// capture names, and structural kinds at schema load, and parsing runs
/// before that, so this type deliberately cannot say what any of its steps
/// denote. What it does say is exactly the lexical shape §4.4 fixes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct UnboundOutlineLocator {
    source: LocatorSource,
    anchor: LocatorAnchor,
    name_steps: NonEmpty<NameStep>,
    structural_steps: Vec<StructuralStep>,
    text: Option<IntrinsicTextStep>,
}

impl UnboundOutlineLocator {
    pub(crate) fn source(&self) -> &LocatorSource {
        &self.source
    }

    pub(crate) fn anchor(&self) -> LocatorAnchor {
        self.anchor
    }

    pub(crate) fn name_steps(&self) -> &NonEmpty<NameStep> {
        &self.name_steps
    }

    pub(crate) fn structural_steps(&self) -> &[StructuralStep] {
        &self.structural_steps
    }

    pub(crate) fn intrinsic_text(&self) -> Option<&IntrinsicTextStep> {
        self.text.as_ref()
    }
}

/// `fm.<name>`: a reference to a declared frontmatter capture.
///
/// §4.6 keeps this apart from `fm[...]` on purpose — "`fm[$.x]` performs a
/// document-time query, while `fm.x` is the typo-safe reference to a
/// declaration" — so the two are separate types here and neither can be built
/// from the other's source.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct FrontmatterCaptureLocator {
    source: LocatorSource,
    name: CaptureName,
}

impl FrontmatterCaptureLocator {
    pub(crate) fn source(&self) -> &LocatorSource {
        &self.source
    }

    pub(crate) fn name(&self) -> &CaptureName {
        &self.name
    }
}

/// A locator, classified by the form it was written in.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ParsedLocator {
    /// An outline name path, possibly continued structurally.
    Outline(UnboundOutlineLocator),
    /// `fm.<name>`.
    FrontmatterCapture(FrontmatterCaptureLocator),
}

impl ParsedLocator {
    pub(crate) fn source(&self) -> &LocatorSource {
        match self {
            ParsedLocator::Outline(locator) => locator.source(),
            ParsedLocator::FrontmatterCapture(locator) => locator.source(),
        }
    }
}

/// Why a locator was not admitted, and where.
///
/// §4.4 gives every one of these the same diagnostic id — "invalid locator
/// syntax is `invalid-document-shape`" — so the kind exists for the message
/// an author reads, not for a branch a caller takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocatorParseError {
    kind: LocatorParseErrorKind,
    offset: usize,
}

impl LocatorParseError {
    fn new(kind: LocatorParseErrorKind, offset: usize) -> Self {
        Self { kind, offset }
    }

    pub(crate) fn kind(&self) -> LocatorParseErrorKind {
        self.kind
    }

    /// The byte offset into the original locator where the problem starts.
    pub(crate) fn offset(&self) -> usize {
        self.offset
    }
}

impl fmt::Display for LocatorParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at offset {}", self.kind, self.offset)
    }
}

/// The lexical faults a locator can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum LocatorParseErrorKind {
    /// The locator is empty.
    Empty,
    /// `$` alone, which §4.4 accepts from no constraint in this version.
    BareSchemaRoot,
    /// A leading `@`, or a `$` not followed by `.`.
    MalformedAnchor,
    /// A separator with no step on one side of it.
    EmptyStep,
    /// A name step outside both the slug and capture-name grammars.
    InvalidName,
    /// A structural kind step outside the slug grammar.
    InvalidStructuralKind,
    /// A `.` name step after a `/` structural step.
    NameAfterStructure,
    /// Any step after the terminal `/text` intrinsic.
    StepAfterIntrinsicText,
    /// A second `[i]` on one step.
    RepeatedPosition,
    /// A `[` with no closing `]`.
    UnterminatedPosition,
    /// Subscript digits outside `0|[1-9][0-9]*`.
    InvalidPosition,
    /// A character where a separator or the end of the locator was required.
    UnexpectedCharacter,
    /// Bare `fm`, which names neither a capture nor a query.
    BareFrontmatterRoot,
    /// `fm.` followed by something that is not one capture name.
    MalformedFrontmatterCapture,
    /// `fm[`, whose query form is not parsed yet.
    // Removed in the commit that adds the `fm[...]` wrapper.
    FrontmatterQueryUnsupported,
}

impl fmt::Display for LocatorParseErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            LocatorParseErrorKind::Empty => "empty locator",
            LocatorParseErrorKind::BareSchemaRoot => "`$` alone is not a locator",
            LocatorParseErrorKind::MalformedAnchor => "expected a name or `$.`",
            LocatorParseErrorKind::EmptyStep => "empty step",
            LocatorParseErrorKind::InvalidName => "invalid name step",
            LocatorParseErrorKind::InvalidStructuralKind => "invalid structural kind",
            LocatorParseErrorKind::NameAfterStructure => {
                "a name step cannot follow a structural step"
            }
            LocatorParseErrorKind::StepAfterIntrinsicText => "`/text` is terminal",
            LocatorParseErrorKind::RepeatedPosition => "a step takes at most one `[i]`",
            LocatorParseErrorKind::UnterminatedPosition => "unterminated `[i]`",
            LocatorParseErrorKind::InvalidPosition => "an index must match `0|[1-9][0-9]*`",
            LocatorParseErrorKind::UnexpectedCharacter => "expected `.`, `/`, or the end",
            LocatorParseErrorKind::BareFrontmatterRoot => "`fm` alone is not a locator",
            LocatorParseErrorKind::MalformedFrontmatterCapture => {
                "`fm.` takes exactly one capture name"
            }
            LocatorParseErrorKind::FrontmatterQueryUnsupported => {
                "the `fm[...]` query form is not supported yet"
            }
        };
        formatter.write_str(message)
    }
}

/// Parses one locator.
///
/// Total over arbitrary UTF-8: every input either yields a [`ParsedLocator`]
/// or a positioned [`LocatorParseError`], and none panics. That matters
/// because locators arrive from schema files, which are untrusted input.
pub(crate) fn parse_locator(source: &str) -> Result<ParsedLocator, LocatorParseError> {
    if let Some(parsed) = parse_frontmatter_form(source)? {
        return Ok(parsed);
    }
    parse_outline(source).map(ParsedLocator::Outline)
}

/// Recognizes the §4.6 frontmatter forms, or `None` for an outline locator.
///
/// §4.1 reserves the leading name `fm`, and §4.6 gives it exactly three
/// spellings: `fm[query]`, `fm[query]=literal`, and `fm.<name>`. Anything else
/// beginning with those two letters is therefore either an ordinary name that
/// merely starts with them — `fm-plan`, `fmt` — or invalid.
///
/// The reservation is on the *leading* name only, so `$.fm.x` and
/// `deployment.fm` are ordinary outline locators here. `fm` cannot be a rule
/// id (§4.1 makes one `reserved-id`), so the first of those fails at binding
/// rather than at parsing; that is a schema-resolution answer, not a lexical
/// one, and it is not this module's to give.
fn parse_frontmatter_form(source: &str) -> Result<Option<ParsedLocator>, LocatorParseError> {
    let Some(rest) = source.strip_prefix("fm") else {
        return Ok(None);
    };
    match rest.chars().next() {
        None => Err(LocatorParseError::new(
            LocatorParseErrorKind::BareFrontmatterRoot,
            0,
        )),
        Some('.') => {
            let name = CaptureName::parse(&rest['.'.len_utf8()..]).ok_or_else(|| {
                LocatorParseError::new(
                    LocatorParseErrorKind::MalformedFrontmatterCapture,
                    "fm.".len(),
                )
            })?;
            Ok(Some(ParsedLocator::FrontmatterCapture(
                FrontmatterCaptureLocator {
                    source: LocatorSource::new(source),
                    name,
                },
            )))
        }
        Some('[') => Err(LocatorParseError::new(
            LocatorParseErrorKind::FrontmatterQueryUnsupported,
            "fm".len(),
        )),
        Some(_) => Ok(None),
    }
}

/// Parses the outline form: an anchor, a name path, then structure.
fn parse_outline(source: &str) -> Result<UnboundOutlineLocator, LocatorParseError> {
    let mut scanner = Scanner::new(source);
    let anchor = parse_anchor(&mut scanner)?;

    let first = parse_name_step(&mut scanner)?;
    let mut rest = Vec::new();
    while scanner.eat('.') {
        rest.push(parse_name_step(&mut scanner)?);
    }
    let name_steps = NonEmpty { first, rest };

    let mut structural_steps = Vec::new();
    let mut text = None;
    while scanner.eat('/') {
        let start = scanner.offset();
        let token = scanner.take_token();
        if token == INTRINSIC_TEXT {
            text = Some(IntrinsicTextStep {
                position: scanner.take_position()?,
            });
            break;
        }
        let kind = StructuralKind::parse(token).ok_or_else(|| {
            let kind = if token.is_empty() {
                LocatorParseErrorKind::EmptyStep
            } else {
                LocatorParseErrorKind::InvalidStructuralKind
            };
            LocatorParseError::new(kind, start)
        })?;
        structural_steps.push(StructuralStep {
            kind,
            position: scanner.take_position()?,
        });
    }

    if let Some(character) = scanner.peek() {
        let kind = match character {
            '.' | '/' if text.is_some() => LocatorParseErrorKind::StepAfterIntrinsicText,
            '.' if !structural_steps.is_empty() => LocatorParseErrorKind::NameAfterStructure,
            _ => LocatorParseErrorKind::UnexpectedCharacter,
        };
        return Err(LocatorParseError::new(kind, scanner.offset()));
    }

    Ok(UnboundOutlineLocator {
        source: LocatorSource::new(source),
        anchor,
        name_steps,
        structural_steps,
        text,
    })
}

/// The one structural member §4.4 allocates in this version.
const INTRINSIC_TEXT: &str = "text";

fn parse_anchor(scanner: &mut Scanner<'_>) -> Result<LocatorAnchor, LocatorParseError> {
    match scanner.peek() {
        None => Err(LocatorParseError::new(LocatorParseErrorKind::Empty, 0)),
        // §4.4: "The former `@` prefix is not part of the locator language."
        Some('@') => Err(LocatorParseError::new(
            LocatorParseErrorKind::MalformedAnchor,
            0,
        )),
        Some('$') => {
            scanner.advance('$');
            if scanner.eat('.') {
                Ok(LocatorAnchor::SchemaRoot)
            } else if scanner.peek().is_none() {
                Err(LocatorParseError::new(
                    LocatorParseErrorKind::BareSchemaRoot,
                    0,
                ))
            } else {
                Err(LocatorParseError::new(
                    LocatorParseErrorKind::MalformedAnchor,
                    scanner.offset(),
                ))
            }
        }
        Some(_) => Ok(LocatorAnchor::CurrentScope),
    }
}

fn parse_name_step(scanner: &mut Scanner<'_>) -> Result<NameStep, LocatorParseError> {
    let start = scanner.offset();
    let token = scanner.take_token();
    let name = StepName::parse(token).ok_or_else(|| {
        let kind = if token.is_empty() {
            LocatorParseErrorKind::EmptyStep
        } else {
            LocatorParseErrorKind::InvalidName
        };
        LocatorParseError::new(kind, start)
    })?;
    Ok(NameStep {
        name,
        position: scanner.take_position()?,
    })
}

/// §4.1's slug grammar, `[a-z0-9]+(-[a-z0-9]+)*`.
fn is_slug(text: &str) -> bool {
    !text.is_empty()
        && text.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|character| matches!(character, 'a'..='z' | '0'..='9'))
        })
}

/// §2.2's capture-name grammar, `[a-z][a-z0-9_]*`.
fn is_capture_name(text: &str) -> bool {
    let mut characters = text.chars();
    matches!(characters.next(), Some('a'..='z'))
        && characters.all(|character| matches!(character, 'a'..='z' | '0'..='9' | '_'))
}

/// A cursor over one locator's bytes.
///
/// Every offset it reports is a byte offset into the whole locator, so a parse
/// error points at the original spelling and not at some suffix of it.
struct Scanner<'a> {
    source: &'a str,
    offset: usize,
}

impl<'a> Scanner<'a> {
    fn new(source: &'a str) -> Self {
        Self { source, offset: 0 }
    }

    fn offset(&self) -> usize {
        self.offset
    }

    fn rest(&self) -> &'a str {
        &self.source[self.offset..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    /// Consumes `expected`, which the caller has already peeked.
    fn advance(&mut self, expected: char) {
        self.offset += expected.len_utf8();
    }

    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.advance(expected);
            true
        } else {
            false
        }
    }

    /// Takes everything up to the next separator or subscript.
    ///
    /// Taking the whole run rather than only the characters the grammar admits
    /// is what lets `Föö` fail as an invalid *name* instead of as a stray
    /// character after a zero-length one. The three delimiters are ASCII, so
    /// the split is always on a character boundary.
    fn take_token(&mut self) -> &'a str {
        let rest = self.rest();
        let end = rest.find(['.', '/', '[']).unwrap_or(rest.len());
        self.offset += end;
        &rest[..end]
    }

    /// Takes one optional `[i]`.
    fn take_position(&mut self) -> Result<Option<LocatorPosition>, LocatorParseError> {
        if self.peek() != Some('[') {
            return Ok(None);
        }
        let open = self.offset;
        self.advance('[');
        let rest = self.rest();
        let Some(end) = rest.find(']') else {
            return Err(LocatorParseError::new(
                LocatorParseErrorKind::UnterminatedPosition,
                open,
            ));
        };
        let digits = &rest[..end];
        self.offset += end + ']'.len_utf8();
        let position = LocatorPosition::parse(digits).ok_or_else(|| {
            LocatorParseError::new(
                LocatorParseErrorKind::InvalidPosition,
                open + '['.len_utf8(),
            )
        })?;
        if self.peek() == Some('[') {
            return Err(LocatorParseError::new(
                LocatorParseErrorKind::RepeatedPosition,
                self.offset,
            ));
        }
        Ok(Some(position))
    }
}
