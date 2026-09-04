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

// Nothing here is reachable from `lib.rs` yet: the loader and the validator
// are wired to this module in a later lane, and until then every item in it
// is dead in a non-test build. The allowance is stated once, for this private
// module, rather than sprinkled over individual items — and never for the
// crate, whose other modules must keep failing on genuinely dead code. It
// comes out when the wiring lands.
#![allow(dead_code)]

// No facade re-export lives here yet. One would have to be `pub(crate)` and,
// with nothing outside this module importing from it, would need a second
// suppression on top of the one above. The lane that wires this module up
// picks the surface it actually needs and re-exports exactly that.
mod jsonpath;
mod path;
mod syntax;

#[cfg(test)]
mod tests;
