# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Because outlint is at `0.x`, the schema language, the diagnostic set, the JSON
output shape, and the library API may all change in a minor release. See
[README.md](README.md#status-and-stability).

## [Unreleased]

Nothing has been released yet. Everything below is the initial, unreleased
feature set of the `0.1.0` workspace.

### Added

- **Specification.** [`spec/outlint-spec.md`](spec/outlint-spec.md) defines
  Outlint Schema Specification v1: the document model, schema format, matching
  semantics, rule identifiers and reference paths, constraints, diagnostics,
  options, and a normative validation algorithm.
  [`spec/cli.md`](spec/cli.md) defines the command-line contract.
- **Schema language.** `title` and nested `sections` rules; exact, glob,
  anchored-regex (RE2 dialect), and `*` matchers with first-match-wins
  resolution; `required` / `repeat: "min..max"` cardinality; `strict` scopes
  and `allow: false` denials; explicit and derived rule `id`s with dotted
  reference paths.
- **Constraints.** `one_of`, `any_of`, `at_most_one`, `all_or_none`,
  `requires`, `conflicts`, and `ordered`, usable at the schema root or inside
  any rule scope.
- **Options.** `match_case`, `strip_inline_markup`, `allow_skipped_levels`,
  and `root_level`, normalized with defaults applied by the loader.
- **Frontmatter.** Presence checking (`required`, `allow`) plus value
  validation delegated to a JSON Schema given inline or as a path relative to
  the schema file, including linked `$ref` resource graphs.
- **Core library** (`outlint-core`): a pure, IO-free schema loader, Markdown
  outline parser, and validator. Schema loading collects every error rather
  than stopping at the first, and never returns a partial schema.
- **CLI** (`outlint`): `outlint check` and `outlint schema check`, with
  `--schema`, `--format human|json`, `--color auto|always|never`, `--help`,
  and `--version`; `.outlint.yml` schema discovery walking up from each input;
  stdin input via `-`; and exit codes `0` (clean), `1` (diagnostics), `2`
  (invocation or operational failure).
- **Suppressions.** `<!-- outlint-disable <id>,... -->` before a heading and
  `<!-- outlint-disable-file <id>,... -->` anywhere in a file.
- **Conformance corpus.** [`testdata/`](testdata/README.md), an
  implementation-independent fixture set driven in CI by the Rust runner and
  reusable by other implementations.
- **Dual licensing** under [MIT](LICENSE-MIT) or
  [Apache-2.0](LICENSE-APACHE), and a declared MSRV of Rust 1.86 tested in CI.

### Known gaps

- `fm.` propositions in constraints are accepted and validated by the loader
  but are **not evaluated**: a constraint depending on one is never satisfied.
- `npm/` distribution packaging exists but is not functional.
- No pre-built binaries are published; `cargo install outlint` builds from
  source.

[Unreleased]: https://github.com/shangaslammi/outlint/commits/main/
