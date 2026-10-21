//! Shared heading slug normalization for rule ids and document paths.

use std::fmt;

use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

/// A heading slug in the form `[a-z0-9]+(-[a-z0-9]+)*`.
///
/// Construction is restricted to [`heading_slug`], which derives a slug from
/// heading text, and to [`HeadingSlug::parse`], which accepts only text that
/// already has the slug form. A value therefore always spells a valid named
/// section segment.
///
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct HeadingSlug(pub(crate) String);

impl HeadingSlug {
    /// Accepts `text` as a slug when it already has the slug form.
    ///
    /// Unlike [`heading_slug`], this performs no normalization: `Rollback
    /// Plan` is rejected rather than turned into `rollback-plan`.
    pub fn parse(text: &str) -> Option<Self> {
        let mut previous_hyphen = true;
        for character in text.chars() {
            match character {
                'a'..='z' | '0'..='9' => previous_hyphen = false,
                '-' if !previous_hyphen => previous_hyphen = true,
                _ => return None,
            }
        }
        (!previous_hyphen).then(|| Self(text.to_owned()))
    }

    /// Returns the slug text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the slug, yielding its text without copying it.
    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for HeadingSlug {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Derives the slug that names a heading in a document path.
///
/// The text is NFKD-decomposed and lowercased; ASCII letters and digits are
/// kept, combining marks are discarded so that `Mälardalen` becomes
/// `malardalen` rather than `m-lardalen`, and every other run of characters
/// becomes a single `-`. Leading and trailing separators are dropped. Returns
/// `None` when nothing remains, in which case the heading can only be
/// addressed positionally.
///
/// The same algorithm generates the default rule id of a schema rule with an
/// exact matcher, so a heading and the rule spelling its exact text share one
/// name.
pub fn heading_slug(text: &str) -> Option<HeadingSlug> {
    let mut result = String::new();
    let mut separator_pending = false;
    for character in text.nfkd().flat_map(char::to_lowercase) {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            if separator_pending && !result.is_empty() {
                result.push('-');
            }
            result.push(character);
            separator_pending = false;
        } else if is_combining_mark(character) {
            // NFKD splits letters such as `ä` into an ASCII base followed by
            // a combining mark. The mark modifies that base; it is not a word
            // boundary and therefore must not introduce a slug separator.
        } else {
            separator_pending = true;
        }
    }
    (!result.is_empty()).then_some(HeadingSlug(result))
}
