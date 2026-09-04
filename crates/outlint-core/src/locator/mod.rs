//! Locator syntax and the private JSONPath wrapper.
//!
//! This module owns the Outlint locator grammar of §4.4 and, once the query
//! forms land, the private wrapper around the JSONPath engine. Nothing about
//! that engine is re-exported: callers see Outlint types only, and no
//! provider type appears in any signature this module offers.
//!
//! What lives here is *lexical*. A locator's names are admitted, not
//! resolved: §4.4's binding-time principle puts rule ids, capture names, and
//! structural kinds at schema load, which happens after parsing and with a
//! schema in hand. Binding, singularity analysis, cardinality suppression,
//! proposition truth, and diagnostics all live outside this module.
//!
//! [`tests`] opens with an executable statement of what the pinned JSONPath
//! provider does and does not do. Every design decision below rests on those
//! facts — that a quoted `]` does not close a bracket, that duplicate located
//! paths arrive un-collapsed, that a location offers name and index
//! components and not a trustworthy spelling — so they are pinned there
//! rather than rediscovered from a failure later.

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
mod syntax;

#[cfg(test)]
mod tests;
