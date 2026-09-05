//! Shared fixtures for the CLI surface regression tests.
//!
//! **That suite mixes contract tests and presentation regression tests.** The
//! command grammar, discovery, stdin, streams, JSON data model and ordering,
//! exit status, no-mutation rule, and offline behavior are normative in
//! `spec/outlint-spec.md` §11. Human output deliberately has no stable grammar:
//! exact wording, punctuation, layout, grouping, and ordering remain
//! implementation details.
//!
//! Two consequences worth knowing before you change anything in the sibling
//! test files:
//!
//! - A failure there is always a **regression signal**, but is evidence of
//!   a specification violation only when the asserted behavior is stated in the
//!   specification. Check §11 before treating a presentation assertion as a
//!   portable requirement.
//! - Conversely, do not treat a passing test there as proof that behavior is
//!   specified. Much of what those tests assert is incidental: exact wording of
//!   messages, punctuation and escaping choices, and help-text layout.
//!
//! Ordering in particular: the conformance corpus (`testdata/`) deliberately does
//! **not** constrain validator order — its format is shared by independent
//! implementations. Section 11.4 specifies a total order for the reference
//! CLI's JSON diagnostics only. Human ordering is presentation.
//!
//! Presentation assertions are regression sentinels for unintended drift,
//! not promises to consumers. A deliberate human-format redesign may update
//! them without changing the specification; JSON contract assertions may not.

// Every integration-test binary compiles this module in full, so items only
// some of them use are not dead code.
#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::Value;

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

pub(crate) const VALID_SCHEMA: &str =
    "version: 1\ntitle: null\nsections:\n  - match: Required\n    required: true\n";

pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new(name: &str) -> Self {
        for _ in 0..100 {
            let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "outlint-cli-{name}-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("temporary test directory should be creatable: {error}"),
            }
        }
        panic!("could not allocate a unique temporary test directory")
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn write(&self, relative: &str, contents: impl AsRef<[u8]>) {
        let path = self.0.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("fixture parent should be creatable");
        }
        fs::write(path, contents).expect("fixture should be writable");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn run(directory: &TempDir, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_outlint"))
        .args(arguments)
        .current_dir(directory.path())
        .output()
        .expect("outlint should run")
}

pub(crate) fn stdout(output: &Output) -> &str {
    std::str::from_utf8(&output.stdout).expect("stdout should be UTF-8")
}

pub(crate) fn stderr(output: &Output) -> &str {
    std::str::from_utf8(&output.stderr).expect("stderr should be UTF-8")
}

pub(crate) fn json_output(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("stdout should be one JSON document")
}
