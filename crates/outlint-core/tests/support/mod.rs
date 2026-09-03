//! Shared helpers for the integration tests.
//!
//! Each integration test file is its own crate, so this module is compiled
//! separately into every binary that declares `mod support;`. A helper used by
//! one such binary and not another would otherwise read as dead code there.

pub mod jsonpath_path;
