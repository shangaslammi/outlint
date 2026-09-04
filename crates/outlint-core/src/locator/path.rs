//! Outlint-owned result paths, and the two renderings §4.6 and §6.1 need.
//!
//! §4.6 makes this Outlint's job, not the provider's: "Outlint owns path
//! rendering at this boundary: whenever it renders a normalized path or
//! derives a §6.1 `pointer`, it MUST construct the representation from the
//! node's path components according to RFC 9535 §2.7, including correct
//! escaping of quotes, backslashes, and C0 controls. A JSONPath provider's
//! rendered path is not authoritative."
//!
//! That is not a precaution. The pinned provider's `NormalizedPath: Display`
//! interpolates each name raw, applying none of the §2.7 escaping, and its
//! `PathElement: Display` emits one reverse solidus where the RFC requires
//! two; both produce spellings that do not round-trip. Its *components* are
//! sound, which is why a path is copied out of them the moment it crosses this
//! boundary and every spelling is built here.
//!
//! Owning the components also settles lifetimes. A provider path borrows the
//! document it was found in; an Outlint path does not, so a diagnostic can
//! outlive the query that produced it without holding the document open.

use serde_json::Value;
use serde_json_path::{LocatedNodeList, NormalizedPath, PathElement};

/// One component of a result path.
///
/// An index is already resolved: RFC 9535 normalizes a negative index to its
/// non-negative position before a path element carries it, so nothing
/// downstream has to know an index was ever written `-1`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum PathComponent {
    /// An object member name, as it appears in the document.
    Name(Box<str>),
    /// A zero-based array index.
    Index(usize),
}

/// A result node's path, owned by Outlint.
///
/// Equality is over the components, which is what makes it a node's identity:
/// §4.6 requires that "duplicate references to the same result node are
/// collapsed", and two references to one node agree on every component
/// whatever selectors reached them.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct OutlintNormalizedPath(Vec<PathComponent>);

impl OutlintNormalizedPath {
    /// Copies a provider location into owned components.
    ///
    /// The conversion goes through `PathElement`, never through the
    /// provider's `Display` or `to_json_pointer`: those are the renderings
    /// §4.6 declines to treat as authoritative.
    pub(crate) fn from_provider(path: &NormalizedPath<'_>) -> Self {
        Self(
            path.iter()
                .map(|element| match element {
                    PathElement::Name(name) => PathComponent::Name((*name).into()),
                    PathElement::Index(index) => PathComponent::Index(*index),
                })
                .collect(),
        )
    }

    pub(crate) fn components(&self) -> &[PathComponent] {
        &self.0
    }

    /// Renders an RFC 9535 §2.7 normalized path.
    ///
    /// The result is always a valid JSONPath query selecting exactly the node
    /// it names, which is the property the round-trip tests check.
    pub(crate) fn render_normalized(&self) -> String {
        let mut out = String::from("$");
        for component in &self.0 {
            match component {
                PathComponent::Index(index) => {
                    out.push('[');
                    out.push_str(&index.to_string());
                    out.push(']');
                }
                PathComponent::Name(name) => {
                    out.push_str("['");
                    push_normalized_name(&mut out, name);
                    out.push_str("']");
                }
            }
        }
        out
    }

    /// Renders an RFC 6901 JSON Pointer, for the §6.1 `pointer` field.
    ///
    /// The root is the empty pointer; every other path is a sequence of
    /// `/`-prefixed reference tokens.
    pub(crate) fn render_pointer(&self) -> String {
        let mut out = String::new();
        for component in &self.0 {
            out.push('/');
            match component {
                PathComponent::Index(index) => out.push_str(&index.to_string()),
                PathComponent::Name(name) => push_pointer_token(&mut out, name),
            }
        }
        out
    }
}

/// Applies §2.7's `normal-single-quoted` escaping to one member name.
///
/// The five controls with short escapes take them; apostrophe and reverse
/// solidus are backslash-escaped; every other C0 control takes the four-digit
/// lowercase `\u00xx` form. Everything else is literal — including a double
/// quote, which needs no escape inside a single-quoted name, and every
/// non-ASCII character, which §2.7 leaves as itself.
fn push_normalized_name(out: &mut String, name: &str) {
    for character in name.chars() {
        match character {
            '\u{0008}' => out.push_str("\\b"),
            '\u{0009}' => out.push_str("\\t"),
            '\u{000A}' => out.push_str("\\n"),
            '\u{000C}' => out.push_str("\\f"),
            '\u{000D}' => out.push_str("\\r"),
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => out.push(other),
        }
    }
}

/// Escapes one RFC 6901 reference token.
///
/// Only `~` and `/` are escaped, as `~0` and `~1`. Quotes, backslashes, C0
/// controls, and non-ASCII characters are literal token characters; escaping
/// them for transport is the job of whatever serializes the pointer into JSON.
fn push_pointer_token(out: &mut String, name: &str) {
    for character in name.chars() {
        match character {
            '~' => out.push_str("~0"),
            '/' => out.push_str("~1"),
            other => out.push(other),
        }
    }
}

/// The complete result of one query, as §4.6's node set.
///
/// Three properties are deliberate, and each is a spec sentence:
///
/// - **Complete.** "Implementations MUST evaluate the complete result and MUST
///   NOT silently truncate it." The provider's result is materialized in full;
///   there is no limit, no `take`, and no early exit.
/// - **Deduplicated.** "Duplicate references to the same result node are
///   collapsed." Identity is the component path — never the value, since two
///   distinct nodes may hold equal JSON.
/// - **Unordered.** "The resulting node set's order is not observable." No
///   caller may depend on the sequence this iterates in; it is an artifact of
///   how duplicates are removed and may change. That is also why there is no
///   `first`, `last`, or `get` here: an ordered accessor on an unordered set
///   would read as a semantic choice, and §4.6 grants none.
#[derive(Debug)]
pub(crate) struct LocatedNodeSet<'a> {
    nodes: Vec<(OutlintNormalizedPath, &'a Value)>,
}

impl<'a> LocatedNodeSet<'a> {
    /// Copies a complete provider result into owned paths, collapsing
    /// duplicate references to one node.
    ///
    /// Deduplication sorts by path and drops adjacent equals. Sorting is
    /// sound precisely because the order is unobservable, and it keeps the
    /// identity index from having to hold a second copy of every path.
    pub(crate) fn from_provider(located: &LocatedNodeList<'a>) -> Self {
        let mut nodes: Vec<(OutlintNormalizedPath, &'a Value)> = located
            .iter()
            .map(|node| {
                (
                    OutlintNormalizedPath::from_provider(node.location()),
                    node.node(),
                )
            })
            .collect();
        nodes.sort_by(|left, right| left.0.cmp(&right.0));
        nodes.dedup_by(|left, right| left.0 == right.0);
        Self { nodes }
    }

    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Iterates every node as a `(path, value)` pair, in unspecified order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&OutlintNormalizedPath, &'a Value)> + '_ {
        self.nodes.iter().map(|(path, value)| (path, *value))
    }
}

impl PartialEq for LocatedNodeSet<'_> {
    /// Set equality: two node sets are equal when they hold the same nodes.
    ///
    /// Both sides are already sorted by path, so this compares them directly
    /// without reintroducing an order dependence.
    fn eq(&self, other: &Self) -> bool {
        self.nodes == other.nodes
    }
}

impl Eq for LocatedNodeSet<'_> {}
