//! Typed frontmatter values: parsing, normalization, equality, and ordering.
//!
//! Reserved for the typed-value phase. Nothing is implemented here yet; the
//! module exists so the boundary is settled before the work starts and so
//! typed-value logic does not accrete inside the loader or the validator.
//!
//! This module will own the resolution of a tagged scalar to its kind, the
//! canonical form each kind normalizes to, and the equality and ordering
//! relations built on those canonical forms. It will not own locator syntax
//! or regex analysis, which have modules of their own.
