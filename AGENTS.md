# AGENTS.md

## Project

Outlint validates the header structure (outline) of Markdown documents
against a declarative schema. Rust workspace, pre-alpha.

    crates/outlint-core   schema model, schema loader, validator (library)
    crates/outlint-cli    the `outlint` binary
    spec/outlint-spec.md  the specification (normative)
    testdata/             conformance corpus, shared by all implementations
    npm/outlint/          npm distribution packaging

## The spec is normative

`spec/outlint-spec.md` defines the language and the observable behavior:
document model (§1), schema format (§2), matching semantics (§3), rule
identifiers and reference paths (§4), constraints (§5), diagnostics (§6),
options and defaults (§7), and a normative validation algorithm (§8).

Implement to the spec, not to intuition. Before writing validation,
matching, or diagnostic code, read the relevant section.

If the spec is wrong, ambiguous, or silent on something the task needs,
change the spec in the same commit and say so. Do not resolve a spec gap
silently in code — a divergence between the two is a defect regardless of
which side is "better".

Diagnostic ids and schema-error ids are a public contract (§6). Do not
invent, rename, or repurpose one without a spec change.

## Current state

Early. `outlint-core` currently contains only the type model:
`schema.rs` (normalized semantic schema) and `parser.rs` (loader result
types and provenance). There is no loader implementation, no Markdown
parsing, no validator, and no test suite. `crates/outlint-cli/src/main.rs`
parses argv and `run_check` is a stub.

Absent by design, do not add speculatively:

- Frontmatter support (spec §2.3, §4.6) — model it when it is implemented.
- Error/reporting frameworks (`thiserror`, `anyhow`, `miette`). Errors are
  plain data structs carrying ranges; the CLI formats them.
- Async, threading, and parallel file checking.
- Config discovery, caching, incremental checking, LSP.

Missing and to be specified before the code needing it is written:

- **CLI contract.** Exit codes and output shapes are user-visible but only
  live in a comment in `main.rs`: 0 = clean, 1 = violations, 2 = usage or
  load error. `--schema <file>` and `--format json` are accepted per the
  README. The human and JSON diagnostic formats are unspecified. Specify
  the JSON shape in `spec/` before implementing `--format json`; make it
  match `testdata/*/expected.json`.
- **Test harness.** `testdata/basic-required/` establishes the fixture
  layout: `schema.outlint.yml` plus `pass.md` / `fail.md`, with
  `expected.json` mapping each Markdown file to its expected violations
  (`{ "id", "path" }`). Nothing reads it yet. The first validator work
  must land the runner that walks `testdata/*/` and asserts against
  `expected.json`, and new behavior adds fixture directories there.
- **MSRV.** No `rust-version` is declared and CI builds on `stable`. If
  you need a recently stabilized feature, either avoid it or set
  `rust-version` in `[workspace.package]` and pin a CI job to it.
- **Public API surface.** `lib.rs` re-exports both modules with globs.
  That is acceptable while the crate is pre-alpha and the model is the
  only content; revisit before publishing, and keep the modules private.

## Rust conventions in force

These are established by `schema.rs` and `parser.rs`. Match them.

Make invalid states unrepresentable. This is the crate's main design idea
and it is deliberate, not over-engineering: `HeaderLevel` as an enum keeps
levels outside h1–h6 out of a parsed schema; `RuleOutcome::Deny` carries no
`Cardinality` so `allow: false` cannot combine with `required`;
`NonEmpty<T>` makes empty constraint operand lists unrepresentable. Prefer
extending this style over validating the same invariant at every use site.

Wrap primitive values in newtypes when confusion is possible —
`RuleId`, `ExactText`, `GlobPattern`, `RegexPattern`, `ByteOffset`,
`RuleIndex`. Use `#[repr(transparent)]` for single-field wrappers.

Normalize at the boundary. Surface syntax (`required`, dotted refs,
slash-delimited regexes, `"n"` repeat forms, `$` anchors, defaulted
options) is resolved by the loader; `Schema` holds only normalized values
with defaults already applied. Do not let surface forms leak inward.

Keep provenance out of the semantic model. Source ranges live in
`SchemaLocations`, addressed by structural `SchemaNode` paths, so two
schemas that differ only in formatting compare equal. Do not add spans to
`Schema` types.

Document every public item. Explain the invariant or the reason for a
design choice; do not restate the signature.

The library must not panic on malformed input. Schema loading returns
`InvalidSchema` with one or more positioned `SchemaError`s and never a
partial `Schema`. Validation returns diagnostics. No `unwrap`/`expect`/
indexing on anything derived from user input; no `unsafe`.

Collect errors rather than stopping at the first — `InvalidSchema` and the
diagnostics model are both plural by design.

## Dependencies

Current: `serde`, `serde_json`, `serde_yaml` in core; `serde_json` in the
CLI. Note that `serde_yaml` is unmaintained upstream; if the loader needs
a different YAML crate, raise it rather than working around it.

Two additions are expected and need no debate when the work reaches them:
a regex engine for `Matcher::Regex`, and a Markdown parser (or a
purpose-built ATX header scanner) for the document model in spec §1.
Anything else, including a CLI argument parser, needs a concrete
justification — argv handling is hand-rolled today and that is fine at
this size.

No Cargo features exist. Do not add one to make a dependency optional
unless a user has asked to build without it.

## Tests

Behavior changes require tests. The crate has none yet, so the first
behavioral commit establishes them rather than deferring.

- Conformance behavior belongs in `testdata/` fixtures, driven by the
  shared runner — that corpus is meant to be reusable by non-Rust
  implementations, so it must not depend on Rust-side details.
- Loader and matcher invariants belong in unit tests next to the code.
- Do not weaken or delete a test to make it pass. If a fixture's
  `expected.json` is genuinely wrong, fix it against the spec and say why.

Matching and Markdown scanning parse untrusted input; property tests over
matcher normalization and header parsing are worth their cost there.

## Before completion

Run:

    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

CI (`.github/workflows/ci.yml`) currently runs a subset: fmt, clippy
without `--all-targets`, and tests. The list above is the standard; if a
check here starts catching things CI misses, add it to CI.

`--all-features` is omitted deliberately — no crate defines features.

If a change touches release packaging, keep `npm/outlint/package.json`
`version` in sync with `[workspace.package] version` in `Cargo.toml`.

Report which checks were actually run.

## Review

Correctness against the spec first, then API design, then maintainability.
Cite the spec section a behavior claim rests on.

Challenge unnecessary public items, dependencies, allocations, clones, and
any `Arc`/`Mutex`/channel — with one standing exception: `Arc<str>` in
`SchemaSource::text` is intentional, so diagnostics can hold source text
cheaply.

Do not challenge the type-safety machinery in `schema.rs` as speculative
generality; it is the crate's design. Do challenge new abstractions that
do not prevent an invalid state or serve a caller that exists.

Passing tests are necessary, not sufficient — check the fixture corpus
actually covers the new behavior.
