//! Frontmatter locator parsing and the private query wrapper.
//!
//! Reserved for the locator phase. Nothing is implemented here yet; the module
//! exists so the boundary is settled before the work starts.
//!
//! This module will own the Outlint locator grammar, the binding of a locator
//! to a frontmatter shape, singularity analysis, and lookup. It will also own
//! the private wrapper around the JSONPath engine: parsing the `fm[...]` form,
//! collapsing duplicate result nodes, converting a result location to a JSON
//! Pointer, and enforcing evaluation limits. Nothing about the underlying
//! engine is re-exported; callers see Outlint types only.
