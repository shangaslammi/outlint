# Contributing to outlint

Thanks for your interest. outlint is a small, single-maintainer project, so
the fastest path to a merged change is a short issue first — especially for
anything that alters observable behavior.

- Bugs and feature requests: <https://github.com/shangaslammi/outlint/issues>
- Security issues: **do not** open an issue; see [SECURITY.md](SECURITY.md).
- Conduct: [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## Two rules that are easy to miss

outlint has two conventions that a new contributor will almost certainly get
wrong on the first attempt. Read these before writing code.

### 1. The specification is normative

[`spec/outlint-spec.md`](spec/outlint-spec.md) defines the schema language,
observable validation behavior, and command-line contract. The implementation
is not the reference — where the two disagree,
**the specification is right and the code is the bug**.

Consequences:

- Read the relevant specification section before writing matching, validation,
  frontmatter, or diagnostic code. Implement to the specification, not to
  intuition.
- If the specification is wrong, ambiguous, or silent on something your change
  needs, **change the specification in the same pull request** and say so in
  the description. Do not resolve a specification gap silently in code.
- Diagnostic ids and schema-error ids (specification §6) are a public
  contract. Do not invent, rename, or repurpose one without a specification
  change. The same applies to the JSON output shape and exit codes
  (specification §11).

A pure refactor, a docs fix, or a bug fix that makes the code match what the
specification already says needs no specification change.

### 2. The conformance corpus is implementation-independent

[`testdata/`](testdata/README.md) is a shared corpus intended to be reusable
by non-Rust implementations of the specification. Its contract is documented
in [`testdata/README.md`](testdata/README.md) and it is binding:

- Each child directory is one fixture: a `schema.outlint.yml`, one or more
  `*.md` documents, an `expected.json` mapping **every and only** the Markdown
  files in that directory to their expected diagnostic multiset, and
  optionally JSON Schema resources for `frontmatter.schema`.
- Each expected diagnostic is `{ "id": ..., "target": ... }` — the public
  diagnostic id and the tagged target shape specified in §6.1.
- Diagnostic order is deliberately ignored; multiplicity is significant.
  The reference CLI specifies a deterministic order only for its versioned
  JSON interface (§11.4). Human presentation is not a parseable or stable
  contract.
- Fixtures must not depend on Rust-specific APIs, source locations, or a flat
  resource layout. The Rust runner (`crates/outlint-cli/tests/conformance.rs`)
  discovers directories automatically; adding a fixture requires no code
  change.

New user-visible behavior, and every bug fix that changes a diagnostic, gets a
fixture here. Loader and matcher invariants that are not user-visible belong in
unit tests next to the code instead.

Do not weaken or delete a test to make it pass. If an `expected.json` is
genuinely wrong, fix it against the specification and say why in the pull
request.

## Getting set up

MSRV is **Rust 1.86**. That is a compatibility contract: raising it is a
minor-version change, a patch release will never require a newer toolchain,
and it is checked by a dedicated CI job. Use stable for development and check
against 1.86 if you touch anything that might depend on a newer feature.

```sh
git clone https://github.com/shangaslammi/outlint
cd outlint
cargo build --workspace
cargo run -p outlint -- check README.md --schema path/to/schema.outlint.yml
```

The workspace has two crates:

- `crates/outlint-core` — schema model, loader, Markdown outline parser, and
  validator. A pure library: no filesystem, no environment, no printing.
- `crates/outlint-cli` — the `outlint` binary. The IO shell: reads argv, reads
  files, calls core, formats output, picks the exit code.

## Verification

Run all four before opening a pull request. CI runs all four: tests on stable
across Linux, macOS, and Windows, formatting, Clippy, and rustdoc on Linux. It
also runs the full test suite on Rust 1.86 and `cargo deny check`. Commands
that resolve the Cargo dependency graph use the committed lockfile in CI; the
local commands below omit `--locked` so Cargo can report an intentionally
changed lockfile normally.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

`--all-features` is omitted deliberately: no crate defines any Cargo features,
and adding one to make a dependency optional needs a concrete user request.

Say which checks you actually ran in the pull request description.

## Design conventions

These are established by the existing code. Match them.

**Pure core, thin IO shell.** Write logic as referentially transparent
functions: value in, value out, no observable effect beyond the return value.
Push the filesystem, the clock, the environment, process exit, and anything
else non-deterministic to the outermost edge, which does nothing but fetch
inputs, call a pure function, and act on the result.

- No `fs`, `io`, `std::env`, `println!`/`eprintln!`, `SystemTime`, or
  randomness in a function that computes a result. Take the data as a
  parameter instead.
- Do not hide an effect behind a trait or callback so a "pure" function can
  perform IO through it. Injecting an effect is not removing it.
- Prefer returning a description of what should happen (diagnostics, a report)
  over performing it. The shell interprets it.
- No global or thread-local mutable state and no caches observable in a return
  value. Internal mutation is fine — building a `Vec` in a loop, or
  accumulating errors in `&mut self` behind a pure entry point, is the
  intended shape. The rule is about observable effects at a function's
  boundary, not about avoiding `mut`.

The reason is the conformance corpus: fixtures can only drive functions that
map input text to output data. A rule that can only be exercised by writing
files and reading stdout is a design defect, not a fact about the problem.

**Make invalid states unrepresentable.** This is the crate's central design
idea, not over-engineering. `HeaderLevel` keeps levels outside h1–h6 out of a
parsed schema; accepting `SectionRule`s and matcher-only `SectionGuard`s are
distinct, so a prohibition cannot combine with cardinality; `NonEmpty<T>`
makes empty operand lists unrepresentable. Prefer extending this style over
re-validating the same invariant at every use site.

**Wrap primitives in newtypes** when confusion is possible (`RuleId`,
`ExactText`, `GlobPattern`, `RegexPattern`, `ByteOffset`, `RuleIndex`), with
`#[repr(transparent)]` for single-field wrappers.

**Normalize at the boundary.** Surface syntax — `required`, dotted refs,
slash-delimited regexes, `"n"` repeat forms, defaulted options — is resolved
by the loader. `Schema` holds only normalized values with defaults applied.
Do not let surface forms leak inward.

**Keep provenance out of the semantic model.** Source ranges live in
`SchemaLocations`, addressed by structural `SchemaNode` paths, so two schemas
differing only in formatting compare equal. Do not add spans to `Schema`
types.

**Never panic on malformed input.** Schema loading returns `InvalidSchema`
with one or more positioned `SchemaError`s and never a partial `Schema`;
validation returns diagnostics. No `unwrap`/`expect`/indexing on anything
derived from user input, and no `unsafe`. Collect errors rather than stopping
at the first — the error and diagnostic models are plural by design.

**Document every public item.** Explain the invariant or the reason for a
design choice; do not restate the signature.

## Dependencies

New dependencies need a concrete justification in the pull request. The CLI
argument parser is hand-rolled on purpose and that is fine at this size.
YAML is read directly from `saphyr-parser` events — a pure-Rust parser,
pinned exactly because every `0.0.z` release is a breaking change under
Cargo's semver rules; if you need different YAML behavior, raise it in
an issue rather than working around it.

Not planned, and please do not add speculatively: async or threading, an
error-reporting framework (`thiserror`, `anyhow`, `miette` — errors are plain
data structs carrying ranges and the CLI formats them), validation-result or
incremental caches, incremental checking, or an LSP. This does not prohibit
the documented npm launcher from caching the released native binary it
acquires before validation begins.

## AI-assisted contributions

You may use AI coding tools to prepare a contribution; the maintainer does
(see the [README](README.md#development-process)). The conditions are the
ones that apply to every change, stated explicitly:

- You have read and understood everything you submit and can explain it in
  review. You are the author of record and accountable for the change; a
  tool is not.
- Say in the pull request description which tools were used and for what.
  Commits an agent co-authored carry a `Co-Authored-By` trailer.
- The specification, the conformance corpus, and the verification commands
  apply unchanged. Generated prose is not a substitute for a fixture or a
  specification citation.
- Do not submit unreviewed tool output. Bulk-generated pull requests, and
  bug reports that do not reproduce with a concrete command, will be closed.

## Pull requests

- One logical change per pull request. Conventional-commit subjects
  (`feat:`, `fix:`, `docs:`, `test:`, `chore:`) are used in this repository.
- Fill in [the checklist](.github/PULL_REQUEST_TEMPLATE.md).
- Add an entry to the `## [Unreleased]` section of
  [CHANGELOG.md](CHANGELOG.md) for anything user-visible.
- If a change touches release packaging, keep `npm/outlint/package.json`
  `version` in sync with `[workspace.package] version` in `Cargo.toml`.

Review priority is correctness against the specification first, then API
design, then maintainability. Cite the specification section a behavior claim
rests on. Passing tests are necessary, not sufficient — the corpus has to
actually cover the new behavior.

## License

By contributing you agree that your contributions are dual licensed under
[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at the user's option,
matching the license of the project:

> Unless you explicitly state otherwise, any contribution intentionally
> submitted for inclusion in this work by you, as defined in the Apache-2.0
> license, shall be dual licensed as above, without any additional terms or
> conditions.
