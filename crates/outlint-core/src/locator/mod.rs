//! Locator syntax and the private JSONPath wrapper.
//!
//! Three files, one boundary:
//!
//! - [`syntax`] reads the §4.4 locator grammar into types that cannot spell an
//!   invalid locator.
//! - [`jsonpath`] owns the §4.6 `fm[...]` wrapper, the §2.3 singular capture
//!   path, and every call into the JSONPath provider.
//! - [`path`] owns result paths and the two renderings §4.6 and §6.1 require.
//!
//! **The provider does not leak.** No `serde_json_path` type appears in any
//! signature offered outside this module: a query's identity is its own
//! source, a result path is Outlint's own components, and a parse failure is
//! an Outlint error carrying a copied message. Swapping the provider is a
//! change to these three files.
//!
//! **Nothing here binds.** A locator's names are admitted, not resolved:
//! §4.4's binding-time principle puts rule ids, capture names, and structural
//! kinds at schema load, which happens after parsing and with a schema in
//! hand. Singularity analysis, cardinality suppression, proposition truth,
//! YAML scalar resolution, and diagnostics all live outside this module.
//!
//! **Rendering is Outlint's.** §4.6: "a JSONPath provider's rendered path is
//! not authoritative." Every normalized path and every §6.1 pointer is built
//! from owned components, never from provider display text.
//!
//! [`tests`] opens with an executable statement of what the pinned provider
//! does and does not do. Every design decision above rests on those facts —
//! that a quoted `]` does not close a bracket, that duplicate located paths
//! arrive un-collapsed, that a location offers name and index components and
//! not a trustworthy spelling — so they are pinned there rather than
//! rediscovered from a failure later. Where the provider is narrower than
//! RFC 9535, the gap is pinned too, not papered over.

// There is deliberately no module-wide dead-code allowance: one would silence
// a genuinely dead kernel path as readily as a deliberate one, and this module
// is where an unused kernel path is most likely to go unnoticed. Items the
// production build does not reach are `#[cfg(test)]` instead, which says the
// same thing without hiding anything else.

mod jsonpath;
mod path;
mod syntax;

// The facade: exactly the kernel items the rest of the crate names, and
// nothing more. Everything reachable through it is `pub(crate)`, so no
// provider type escapes this module by being re-exported here, and adding a
// name is a deliberate act rather than a consequence of opening the module
// wholesale.
pub(crate) use self::jsonpath::{AbsoluteSingularPath, FrontmatterQueryLocator};
// `PreparedQuery` is what a validation plan stores per distinct §4.6 query,
// so that a query compiles once per schema rather than once per proposition
// per document; `SingularComponent` is what frontmatter capture evaluation
// walks, and re-exporting it is what keeps that walk from reparsing a §2.3
// path's RFC escapes for itself.
pub(crate) use self::jsonpath::{PreparedQuery, QueryLimitExceeded, SingularComponent};
pub(crate) use self::syntax::{
    parse_locator, FrontmatterCaptureLocator, LocatorAnchor, LocatorPosition, LocatorSource,
    ParsedLocator, UnboundOutlineLocator,
};

#[cfg(test)]
mod tests;
