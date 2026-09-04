//! Shared helpers for the integration tests and the manifest generator.
//!
//! Each integration test file is its own crate, and the generator example is a
//! third, so this module is compiled separately into every binary that pulls it
//! in. Each of those uses a different subset of the helpers, which is why the
//! module as a whole opts out of the dead-code lint: a helper that is unused in
//! one binary is load-bearing in another.
#![allow(dead_code)]

pub mod jsonpath_core_manifest;
pub mod jsonpath_core_recognizer;
pub mod jsonpath_path;
