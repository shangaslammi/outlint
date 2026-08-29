# AGENTS.md

## Project

Outlint validates the header structure (outline) of Markdown documents
against a declarative schema. Rust workspace at 0.1.0, unreleased; expect
breaking changes before 1.0.

    crates/outlint-core   schema model, schema loader, validator (library)
    crates/outlint-cli    the `outlint` binary
    spec/outlint-spec.md  the specification (normative)
    testdata/             conformance corpus, shared by all implementations
    npm/outlint/          npm distribution packaging

## The spec is normative

`spec/outlint-spec.md` defines the language and the observable behavior:
document model (§1), schema format (§2), matching semantics (§3), rule
identifiers and reference paths (§4), constraints (§5), diagnostics (§6),
options and defaults (§7), a normative validation algorithm (§8), and the
command-line contract (§11). §9 (complete example) and §10 (authoring
guidance) are non-normative.

Implement to the spec, not to intuition. Before writing validation,
matching, or diagnostic code, read the relevant section.

If the spec is wrong, ambiguous, or silent on something the task needs,
change the spec in the same commit and say so. Do not resolve a spec gap
silently in code — a divergence between the two is a defect regardless of
which side is "better".

Diagnostic ids and schema-error ids are a public contract (§6). Do not
invent, rename, or repurpose one without a spec change.

## Current state

Feature-complete against the spec for a first release. The
pipeline in `outlint-core`: `load_schema` / `load_schema_with_resources`
(`loader.rs`) turn schema text into a normalized `Schema` or an
`InvalidSchema` carrying positioned `SchemaError`s; `parse_markdown`
(`markdown.rs`) turns document text into a `Document` (section tree,
frontmatter, suppressions); `validate` / `PreparedValidator`
(`validator.rs`) map a `Schema` plus a `Document` to `Vec<Diagnostic>`.
`matcher.rs` and `case_fold.rs` are private helpers.

All YAML — schema files and frontmatter alike — goes through one
saphyr-parser event reader. Its input limits (nesting depth, node budget,
alias-expansion bound) live in `markdown.rs` and the loader shares them.

Frontmatter is implemented: the delimited block parses into
`DocumentFrontmatter` with per-key anchors, and a schema's `frontmatter`
policy (optional/required/forbidden) may attach an inline self-contained JSON
Schema or one linked from a file — enforced via the `jsonschema` crate, with
`json_pointer` and line ranges on the resulting diagnostics. `fm.`
propositions in constraints are evaluated by the validator's
`frontmatter_satisfied` against the parsed frontmatter mapping: presence
of a non-null value for `fm.key`, typed scalar equality for `fm.key=value`
(both sides resolve through the loader's `parse_frontmatter_scalar`, so
the YAML core schema types agree), honouring `options.match_case` for
string comparison.

The CLI has two subcommands: `outlint check <FILE>...` (nearest
`.outlint.yml` discovered per file unless `--schema` is given; `-` reads
stdin and requires `--schema`) and `outlint schema check <SCHEMA>...`.
Both take `--format human|json` and `--color auto|always|never`. Exit
codes: 0 clean, 1 diagnostics, 2 usage or operational error. The CLI's
JSON output is the full diagnostic shape; the conformance corpus's
`expected.json` records only portable `{id, target}` entries. The two
diverge deliberately — do not "align" them.

MSRV is declared: `rust-version = "1.86"` in `[workspace.package]`, with
a pinned CI job running `cargo check --workspace --all-targets` on it.

Test surface: unit tests sit next to the code in `loader.rs`,
`markdown.rs`, and `validator.rs`, including property tests over header
parsing and YAML/matcher normalization; `crates/outlint-core/tests/`
holds the public-API check and committed schema-range baselines
(`schema_ranges/`); `crates/outlint-cli/tests/` holds end-to-end CLI
tests (`cli.rs`) and the conformance runner (`conformance.rs`), which
shells out to the built binary with `--format json` and compares
order-insensitively against each `testdata/*/expected.json`.

Absent by design, do not add speculatively:

- Error/reporting frameworks (`thiserror`, `anyhow`, `miette`). Errors are
  plain data structs carrying ranges; the CLI formats them.
- Async, threading, and parallel file checking.
- Config files, caching, incremental checking, LSP. The CLI's
  nearest-schema discovery is the only lookup that exists.

`lib.rs` re-exports its modules with globs. Acceptable pre-1.0; revisit
before stabilizing, and keep the modules themselves private.

## Pure core, thin IO shell

Write logic as referentially transparent functions: value in, value out,
same answer every time, no observable effect beyond the return value. Push
IO, the clock, the environment, the filesystem, process exit, and anything
else non-deterministic to the outermost edge, where it does nothing but
fetch inputs, call a pure function, and act on what comes back.

`loader.rs` is the pattern to copy. `load_schema` and
`load_schema_with_resources` are total and pure: they consume schema text
plus any already-loaded JSON Schema resources. The filesystem side is
`crates/outlint-cli/src/schema_loading.rs`, which walks the `$ref` graph
of a linked frontmatter schema and preloads its resources before calling
core — real, load-bearing IO code, and it stays in the CLI, never in
core. New subsystems follow the same split:

- Markdown scanning takes source text, not a path.
- Validation takes a `Schema` and a parsed document, not a directory to
  walk. It returns diagnostics; it does not print or exit.
- The CLI is the shell: it reads argv, reads files, calls core, formats
  output, and picks the exit code. Business logic does not live there.

Consequences to hold to:

- No `fs`, `io`, `std::env`, `println!`/`eprintln!`, `SystemTime`, or
  randomness in a function that computes a result. If a core function needs
  the outside world, it takes the data as a parameter instead.
- Do not hide effects behind a trait or callback so that a "pure" function
  can perform IO through it. Injecting an effect is not removing it —
  restructure so the effect happens before or after the computation.
- Prefer returning a description of what should happen (diagnostics, a
  planned edit, a report) over performing it. The shell interprets it.
- No global or thread-local mutable state, no lazily initialized caches
  observable in a return value. Interior mutability that is invisible to
  callers is fine only when it cannot change a result.
- Internal mutation is fine. A function that builds a `Vec` in a loop is
  still referentially transparent; `Loader` accumulating errors in `&mut
  self` behind a pure entry point is the intended shape. This rule is about
  observable effects at a function's boundary, not about avoiding `mut`.

The reason is testability and the conformance corpus: `testdata/` fixtures
can only be driven by functions that map input text to output data. A rule
that can only be exercised by writing files and reading stdout is a defect
in the design, not a fact about the problem.

## Rust conventions in force

These are established by `schema.rs` and `load_result.rs`. Match them.

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

Current in core: `saphyr-parser` (the YAML parser — pure Rust, pinned
exactly because every `0.0.z` release is a breaking change under Cargo's
semver rules), `serde`/`serde_json`, `jsonschema`, `regex`,
`pulldown-cmark`, `casefold`, `num-bigint`, `unicode-normalization`.
The CLI adds only `serde_json`. YAML is read directly from
`saphyr-parser` events; if the loader needs a different YAML crate,
raise it rather than working around it.

Anything else, including a CLI argument parser, needs a concrete
justification — argv handling is hand-rolled today and that is fine at
this size.

No Cargo features exist. Do not add one to make a dependency optional
unless a user has asked to build without it.

## Tests

Behavior changes require tests. The suites listed under "Current state"
exist; extend them rather than inventing a parallel harness.

- Conformance behavior belongs in `testdata/` fixtures, driven by
  `crates/outlint-cli/tests/conformance.rs` — that corpus is meant to be
  reusable by non-Rust implementations, so it must not depend on
  Rust-side details.
- Loader and matcher invariants belong in unit tests next to the code.
- Do not weaken or delete a test to make it pass. If a fixture's
  `expected.json` is genuinely wrong, fix it against the spec and say why.

Matching and Markdown scanning parse untrusted input; the property tests
in `loader.rs` and `markdown.rs` cover them — extend those when touching
either parser.

## Before completion

Run:

    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

CI (`.github/workflows/ci.yml`) runs every check above with `--locked`:
the tests on stable across Linux, macOS, and Windows, fmt and clippy on
Linux, the doc build with `RUSTDOCFLAGS="-D warnings"`, and the full test
suite on the 1.86 MSRV. It additionally runs `cargo deny check` against
the committed `deny.toml` (advisories, licenses, bans, sources) — run it
locally only when touching dependencies. If a check here starts catching
things CI misses, add it to CI.

`--all-features` is omitted deliberately — no crate defines features.

If a change touches release packaging, keep `npm/outlint/package.json`
`version` in sync with `[workspace.package] version` in `Cargo.toml`.

Report which checks were actually run.

## Review

Correctness against the spec first, then API design, then maintainability.
Cite the spec section a behavior claim rests on.

Reject IO, environment access, or printing that has leaked into a
computation. Ask where the pure function is and whether a fixture could
call it directly; if it could not, the split is in the wrong place.

Challenge unnecessary public items, dependencies, allocations, clones, and
any `Arc`/`Mutex`/channel — with one standing exception: `Arc<str>` in
`SchemaSource::text` is intentional, so diagnostics can hold source text
cheaply.

Do not challenge the type-safety machinery in `schema.rs` as speculative
generality; it is the crate's design. Do challenge new abstractions that
do not prevent an invalid state or serve a caller that exists.

Passing tests are necessary, not sufficient — check the fixture corpus
actually covers the new behavior.
