//! Regenerates `tests/fixtures/jsonpath/core-manifest.json` on stdout.
//!
//! The manifest records which official compliance cases fall inside the §4.6
//! guaranteed core, so that the secondary gate evaluates those and only those.
//! Membership is decided by the same recognizer the test uses, included here by
//! path so the generator and the checker can never drift apart.
//!
//! This example only prints. It never writes into the repository:
//!
//! ```sh
//! cargo run -q -p outlint-core \
//!   --example generate_jsonpath_core_manifest --locked \
//!   > crates/outlint-core/tests/fixtures/jsonpath/core-manifest.json
//! ```
//!
//! See `tests/fixtures/jsonpath/UPDATING.md` for when regenerating is correct
//! and what must be reviewed afterwards.

#[path = "../tests/support/mod.rs"]
mod support;

use support::jsonpath_core_manifest::{build_manifest, to_canonical_json};

const CTS: &str = include_str!("../tests/fixtures/jsonpath/cts.json");

fn main() {
    let manifest = build_manifest(CTS);
    print!("{}", to_canonical_json(&manifest));
}
