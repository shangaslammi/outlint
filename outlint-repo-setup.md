# Outlint Repository Setup Instructions

Instructions for setting up the `outlint` repository from scratch. Follow the
steps IN ORDER. Do not skip steps. Do not improvise or add extra files, tools,
or structure beyond what is written here. Each step ends with a VERIFY command;
run it and confirm the expected result before moving to the next step. If a
VERIFY fails, stop and fix that step before continuing.

Prerequisites (check first):

```bash
rustc --version    # must print a version, 1.75 or newer
cargo --version    # must print a version
git --version      # must print a version
node --version     # must print v20 or newer (only needed for step 8)
```

If any command fails, install the missing tool before starting. Do not
continue without them.

---

## Step 1 — Create the repository root

```bash
mkdir outlint
cd outlint
git init -b main
```

All later commands are run from this `outlint/` directory unless a step says
otherwise.

VERIFY:

```bash
git status
```

Expected: output says "On branch main" and "No commits yet".

---

## Step 2 — Create the directory structure

Create exactly these directories and no others:

```bash
mkdir -p crates/outlint-core/src
mkdir -p crates/outlint-cli/src
mkdir -p npm/outlint
mkdir -p spec
mkdir -p testdata
mkdir -p docs
mkdir -p .github/workflows
```

VERIFY:

```bash
find . -type d -not -path './.git*' | sort
```

Expected output (exactly these lines):

```
.
./.github
./.github/workflows
./crates
./crates/outlint-cli
./crates/outlint-cli/src
./crates/outlint-core
./crates/outlint-core/src
./docs
./npm
./npm/outlint
./spec
./testdata
```

---

## Step 3 — Create the workspace Cargo.toml

Create the file `Cargo.toml` in the repository root with EXACTLY this content:

```toml
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"
repository = "https://github.com/OWNER/outlint"
```

Replace `OWNER` with the actual GitHub user or organization name. Make no
other changes.

VERIFY:

```bash
grep -c 'members = \["crates/\*"\]' Cargo.toml
```

Expected: `1`

---

## Step 4 — Create the core crate

Create `crates/outlint-core/Cargo.toml` with EXACTLY this content:

```toml
[package]
name = "outlint-core"
description = "Markdown outline schema validator (core library)"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "0.9"
```

Create `crates/outlint-core/src/lib.rs` with EXACTLY this content:

```rust
//! outlint-core: schema model, markdown outline parser, validator.
//! Public API is intentionally coarse: validate(schema, doc) -> diagnostics.

pub mod diagnostics;

pub use diagnostics::Diagnostic;

/// Validate a markdown document against an outlint schema.
/// Both inputs are source text. Returns a list of diagnostics;
/// an empty list means the document conforms.
pub fn validate(_schema_yaml: &str, _markdown: &str) -> Vec<Diagnostic> {
    // Implementation follows spec/outlint-spec.md section 8.
    unimplemented!("validator not yet implemented")
}
```

Create `crates/outlint-core/src/diagnostics.rs` with EXACTLY this content:

```rust
use serde::Serialize;

/// One validation finding. Serialized form is a stable public interface;
/// see spec/outlint-spec.md section 6.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    /// Stable diagnostic id, e.g. "missing-section", "one_of".
    pub id: String,
    /// Header path in the document, e.g. "Overview > Goals".
    pub path: String,
    /// 1-based source line of the relevant header, if known.
    pub line: Option<u32>,
    /// Human-readable message.
    pub message: String,
}
```

VERIFY:

```bash
cargo check -p outlint-core
```

Expected: finishes with no errors. Warnings about unused code are acceptable.

---

## Step 5 — Create the CLI crate

Create `crates/outlint-cli/Cargo.toml` with EXACTLY this content:

```toml
[package]
name = "outlint"
description = "Lint the header structure of Markdown documents against a schema"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true

[[bin]]
name = "outlint"
path = "src/main.rs"

[dependencies]
outlint-core = { path = "../outlint-core", version = "0.1.0" }
serde_json = "1"
```

Note: the package is named `outlint` (not `outlint-cli`) on purpose, so that
`cargo install outlint` installs the CLI. The directory name stays
`outlint-cli`. Do not "fix" this mismatch.

Create `crates/outlint-cli/src/main.rs` with EXACTLY this content:

```rust
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check") => run_check(&args[1..]),
        Some("--version") | Some("-V") => {
            println!("outlint {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: outlint check <files...> [--schema <file>] [--format json]");
            ExitCode::from(2)
        }
    }
}

fn run_check(_args: &[String]) -> ExitCode {
    // Wire to outlint_core::validate. Exit 0 = clean, 1 = violations, 2 = usage/error.
    eprintln!("not yet implemented");
    ExitCode::from(2)
}
```

VERIFY:

```bash
cargo run -p outlint -- --version
```

Expected: prints `outlint 0.1.0`.

---

## Step 6 — Add the spec and a first conformance test case

If a file named `outlint-spec.md` was provided to you, copy it to
`spec/outlint-spec.md`. If it was not provided, create `spec/outlint-spec.md`
containing only the line `# Outlint Schema — Specification v1 (placeholder)`
and continue.

Create `testdata/basic-required/schema.outlint.yml` with EXACTLY:

```yaml
version: 1
title: "*"
sections:
  - match: "Overview"
    required: true
```

Create `testdata/basic-required/pass.md` with EXACTLY:

```markdown
# Some Title

## Overview

Text.
```

Create `testdata/basic-required/fail.md` with EXACTLY:

```markdown
# Some Title

## Other Section

Text.
```

Create `testdata/basic-required/expected.json` with EXACTLY:

```json
{
  "pass.md": [],
  "fail.md": [
    { "id": "missing-section", "path": "Overview" }
  ]
}
```

VERIFY:

```bash
ls testdata/basic-required/
```

Expected: `expected.json  fail.md  pass.md  schema.outlint.yml`

---

## Step 7 — Add .gitignore, LICENSE, README

Create `.gitignore` in the repository root with EXACTLY:

```
/target
node_modules/
*.log
```

Create `LICENSE` in the repository root containing the standard MIT license
text, with the current year and the owner's name on the copyright line.

Create `README.md` in the repository root with EXACTLY:

```markdown
# outlint

Lint the header structure (outline) of Markdown documents against a
declarative schema.

Status: pre-alpha. The normative specification lives in
[spec/outlint-spec.md](spec/outlint-spec.md).

## Usage

    outlint check README.md --schema .outlint.yml

## Layout

- `crates/outlint-core` — schema model, parser, validator (library)
- `crates/outlint-cli` — the `outlint` command-line tool
- `spec/` — the specification (normative)
- `testdata/` — conformance corpus shared by all implementations
- `npm/` — npm distribution packaging
```

VERIFY:

```bash
ls README.md LICENSE .gitignore
```

Expected: all three filenames print without error.

---

## Step 8 — Create the npm launcher package (packaging only, no logic)

Create `npm/outlint/package.json` with EXACTLY this content, replacing OWNER:

```json
{
  "name": "outlint",
  "version": "0.1.0",
  "description": "Lint the header structure of Markdown documents against a schema",
  "license": "MIT",
  "repository": "github:OWNER/outlint",
  "bin": { "outlint": "bin.js" },
  "files": ["bin.js"]
}
```

Create `npm/outlint/bin.js` with EXACTLY:

```js
#!/usr/bin/env node
// Placeholder launcher. The release pipeline replaces this with a loader
// that resolves the platform-specific binary via optionalDependencies.
console.error("outlint: binary distribution not wired up yet");
process.exit(2);
```

Do NOT run `npm install` and do NOT run `npm publish`. This step only creates
files.

VERIFY:

```bash
node npm/outlint/bin.js; echo "exit=$?"
```

Expected: prints the placeholder message and `exit=2`.

---

## Step 9 — CI workflow

Create `.github/workflows/ci.yml` with EXACTLY:

```yaml
name: CI
on:
  push:
    branches: [main]
  pull_request:

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo fmt --check
      - run: cargo clippy --workspace -- -D warnings
      - run: cargo test --workspace
```

Do NOT create a release workflow yet. Release/publishing (npm OIDC trusted
publishing, crates.io auth action) is configured later by a human, because it
requires web-UI steps and a first manual publish.

VERIFY:

```bash
cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo test --workspace
```

Expected: all three commands succeed. If `cargo fmt --check` fails, run
`cargo fmt` once and re-verify.

---

## Step 10 — First commit

```bash
git add -A
git status
```

Check `git status` output: every file created in steps 3–9 must be staged,
and nothing else. `target/` must NOT appear (it is gitignored).

```bash
git commit -m "Bootstrap outlint workspace: core + cli crates, spec, testdata, npm packaging, CI"
```

VERIFY:

```bash
git log --oneline
git ls-files | wc -l
```

Expected: exactly one commit; file count is 16 (17 if a real spec file was
copied in step 6 — the count is the same either way since the placeholder
also counts; expect 16).

---

## Final state checklist

Confirm every line:

- [ ] `cargo run -p outlint -- --version` prints `outlint 0.1.0`
- [ ] `cargo test --workspace` passes
- [ ] `find . -type d` matches the step-2 listing (plus `target/`, which is untracked)
- [ ] `git log` shows exactly one commit on `main`
- [ ] No files exist outside the structure defined above

## Explicitly out of scope — do not attempt

- Implementing the validator (separate task with its own instructions)
- Publishing to npm or crates.io
- Creating the GitHub repository or pushing (`git remote`, `git push`)
- Adding dependencies beyond those listed
- Creating `outlint-wasm`, LSP, or platform npm packages
- DNS, domains, or release workflows
