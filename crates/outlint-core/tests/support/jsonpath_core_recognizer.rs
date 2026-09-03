//! An independent recognizer for the §4.6 guaranteed JSONPath core.
//!
//! This is test infrastructure. It exists so that the official compliance
//! suite can be partitioned into cases Outlint guarantees and cases it does
//! not, *without* asking the provider which is which. It therefore works only
//! on raw query text: it never imports `serde_json_path`, never inspects a
//! provider AST, never consults a CTS case name or tag, and never classifies
//! by whether evaluation happened to succeed.
//!
//! It recognizes exactly the §4.6 grammar:
//!
//! ```text
//! core-query    = "$" *(S core-segment)
//! core-segment  = "." (member-name-shorthand / "*")
//!               / "[" S core-selector S "]"
//! core-selector = name-selector / index-selector / "*"
//! ```
//!
//! `S`, `member-name-shorthand`, `name-selector`, and `index-selector` keep
//! their RFC 9535 definitions, and §4.6 additionally bounds a core index
//! selector to the I-JSON exact range.
//!
//! This classifier must never become a gate on user input. §4.6 is explicit
//! that a query outside the core "MUST NOT be rejected merely for falling
//! outside the guaranteed core"; non-core queries go to the provider in full.

/// The largest magnitude a core index selector may spell (§4.6).
const I_JSON_MAX_DIGITS: &str = "9007199254740991";

/// Why a query is not in the guaranteed core.
pub type Reason = String;

/// The result of classifying one query's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classification {
    /// Inside the §4.6 guaranteed core.
    Core,
    /// Outside it, with the first reason found. Vendor tier, not invalid.
    NonCore(Reason),
}

impl Classification {
    pub fn is_core(&self) -> bool {
        matches!(self, Self::Core)
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Core => None,
            Self::NonCore(reason) => Some(reason),
        }
    }
}

/// Classifies raw JSONPath query text against the §4.6 core grammar.
pub fn classify(query: &str) -> Classification {
    let chars: Vec<char> = query.chars().collect();
    match Scanner::new(&chars).query() {
        Ok(()) => Classification::Core,
        Err(reason) => Classification::NonCore(reason),
    }
}

struct Scanner<'a> {
    chars: &'a [char],
    at: usize,
}

impl<'a> Scanner<'a> {
    fn new(chars: &'a [char]) -> Self {
        Self { chars, at: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let current = self.peek();
        if current.is_some() {
            self.at += 1;
        }
        current
    }

    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    /// RFC 9535 `S = *B`, `B = %x20 / %x09 / %x0A / %x0D`. Returns how many
    /// characters were consumed, so a caller can reject trailing whitespace.
    fn whitespace(&mut self) -> usize {
        let start = self.at;
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.at += 1;
        }
        self.at - start
    }

    /// `core-query = "$" *(S core-segment)`
    fn query(&mut self) -> Result<(), Reason> {
        if !self.eat('$') {
            return Err("a query must start with the root identifier `$`".to_owned());
        }

        loop {
            let consumed = self.whitespace();
            let Some(next) = self.peek() else {
                // `S` is admitted only *before* a segment, so whitespace that
                // ends the query is not part of the grammar.
                if consumed > 0 {
                    return Err("trailing whitespace after a completed query".to_owned());
                }
                return Ok(());
            };
            match next {
                '.' => self.dot_segment()?,
                '[' => self.bracket_segment()?,
                other => {
                    return Err(format!(
                        "expected a child segment, found `{other}` at character {}",
                        self.at
                    ))
                }
            }
        }
    }

    /// `"." (member-name-shorthand / "*")`
    fn dot_segment(&mut self) -> Result<(), Reason> {
        self.bump();
        match self.peek() {
            Some('.') => Err("descendant segment `..` is vendor tier".to_owned()),
            Some('*') => {
                self.bump();
                Ok(())
            }
            Some(_) => self.member_name_shorthand(),
            None => Err("a dot segment needs a name or `*`".to_owned()),
        }
    }

    /// `member-name-shorthand = name-first *name-char`
    fn member_name_shorthand(&mut self) -> Result<(), Reason> {
        let Some(first) = self.peek() else {
            return Err("a dot segment needs a name".to_owned());
        };
        if !is_name_first(first) {
            return Err(format!("`{first}` cannot start a shorthand member name"));
        }
        self.bump();
        while let Some(next) = self.peek() {
            if is_name_char(next) {
                self.bump();
            } else {
                break;
            }
        }
        Ok(())
    }

    /// `"[" S core-selector S "]"` with exactly one selector.
    fn bracket_segment(&mut self) -> Result<(), Reason> {
        self.bump();
        self.whitespace();

        match self.peek() {
            Some('*') => {
                self.bump();
            }
            Some(quote @ ('\'' | '"')) => self.string_literal(quote)?,
            Some('-') | Some('0'..='9') => self.index_selector()?,
            Some(other) => {
                return Err(format!(
                    "`{other}` is not a core selector; slices, filters, and \
                     descendants are vendor tier"
                ))
            }
            None => return Err("unterminated bracket segment".to_owned()),
        }

        self.whitespace();
        match self.peek() {
            Some(']') => {
                self.bump();
                Ok(())
            }
            Some(',') => Err("a union of selectors is vendor tier".to_owned()),
            Some(':') => Err("a slice selector is vendor tier".to_owned()),
            Some(other) => Err(format!(
                "expected `]` after one core selector, found `{other}`"
            )),
            None => Err("unterminated bracket segment".to_owned()),
        }
    }

    /// `name-selector = string-literal`, RFC 9535 §2.3.1.1.
    fn string_literal(&mut self, quote: char) -> Result<(), Reason> {
        self.bump();
        loop {
            let Some(next) = self.bump() else {
                return Err("unterminated quoted name".to_owned());
            };
            if next == quote {
                return Ok(());
            }
            if next == '\\' {
                self.escape(quote)?;
                continue;
            }
            // `unescaped`, plus the opposite quote character, which each
            // string form admits literally.
            let opposite = if quote == '\'' { '"' } else { '\'' };
            if next == opposite || is_unescaped(next) {
                continue;
            }
            return Err(format!(
                "U+{:04X} must be escaped inside a quoted name",
                next as u32
            ));
        }
    }

    /// `ESC %x27 / ESC %x22 / ESC escapable`
    fn escape(&mut self, quote: char) -> Result<(), Reason> {
        let Some(next) = self.bump() else {
            return Err("a quoted name ends with a dangling escape".to_owned());
        };
        // Only the string's own delimiter is escapable as a quote: `\'` is
        // valid inside single quotes and `\"` inside double quotes.
        if next == quote {
            return Ok(());
        }
        match next {
            'b' | 'f' | 'n' | 'r' | 't' | '/' | '\\' => Ok(()),
            'u' => self.hexchar(),
            other => Err(format!("`\\{other}` is not a legal escape")),
        }
    }

    /// `hexchar`, including the surrogate-pair rule.
    fn hexchar(&mut self) -> Result<(), Reason> {
        let first = self.four_hex_digits()?;
        if (0xD800..=0xDBFF).contains(&first) {
            // A high surrogate must be completed by `\uDC00`-`\uDFFF`.
            if !self.eat('\\') || !self.eat('u') {
                return Err("a high surrogate must be followed by a low surrogate".to_owned());
            }
            let second = self.four_hex_digits()?;
            if !(0xDC00..=0xDFFF).contains(&second) {
                return Err(format!(
                    "U+{second:04X} is not a low surrogate; a surrogate pair is required"
                ));
            }
            return Ok(());
        }
        if (0xDC00..=0xDFFF).contains(&first) {
            return Err(format!("U+{first:04X} is an unpaired low surrogate"));
        }
        Ok(())
    }

    fn four_hex_digits(&mut self) -> Result<u32, Reason> {
        let mut value = 0u32;
        for _ in 0..4 {
            let Some(digit) = self.bump() else {
                return Err("a `\\u` escape needs four hexadecimal digits".to_owned());
            };
            let Some(nibble) = digit.to_digit(16) else {
                return Err(format!("`{digit}` is not a hexadecimal digit"));
            };
            value = value * 16 + nibble;
        }
        Ok(value)
    }

    /// `index-selector = int`, bounded to the I-JSON exact range by §4.6.
    ///
    /// The magnitude check is by digit count and then lexicographic order, so
    /// a spelling of any length costs only its own length: no value-sized
    /// integer is ever allocated.
    fn index_selector(&mut self) -> Result<(), Reason> {
        let negative = self.eat('-');
        let start = self.at;

        let Some(first) = self.peek() else {
            return Err("an index selector needs at least one digit".to_owned());
        };

        if first == '0' {
            self.bump();
            if negative {
                return Err("`-0` is not a legal index spelling".to_owned());
            }
            if matches!(self.peek(), Some('0'..='9')) {
                return Err("an index may not have a leading zero".to_owned());
            }
            return Ok(());
        }

        if !first.is_ascii_digit() {
            return Err(format!("`{first}` is not a digit"));
        }
        while matches!(self.peek(), Some('0'..='9')) {
            self.bump();
        }

        let digits: String = self.chars[start..self.at].iter().collect();
        if digits.len() > I_JSON_MAX_DIGITS.len()
            || (digits.len() == I_JSON_MAX_DIGITS.len() && digits.as_str() > I_JSON_MAX_DIGITS)
        {
            return Err(format!(
                "index magnitude {digits} is outside the I-JSON exact range"
            ));
        }
        Ok(())
    }
}

/// `name-first = ALPHA / "_" / %x80-D7FF / %xE000-10FFFF`
///
/// Rust `char` cannot hold a surrogate, so the excluded range needs no test.
fn is_name_first(character: char) -> bool {
    character.is_ascii_alphabetic() || character == '_' || (character as u32) >= 0x80
}

/// `name-char = name-first / DIGIT`
fn is_name_char(character: char) -> bool {
    is_name_first(character) || character.is_ascii_digit()
}

/// `unescaped = %x20-21 / %x23-26 / %x28-5B / %x5D-D7FF / %xE000-10FFFF`
///
/// That is: not a C0 control, and not `"`, `'`, or `\`.
fn is_unescaped(character: char) -> bool {
    let code = character as u32;
    if code < 0x20 {
        return false;
    }
    !matches!(character, '"' | '\'' | '\\')
}
