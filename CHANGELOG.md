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
  any rule scope. Constraint refs may address frontmatter as well as rules:
  `fm.key` is presence of a non-null value, `fm.key=value` is typed scalar
  equality under the YAML core schema, and dotted paths step through nested
  mappings.
- **Options.** `match_case`, `strip_inline_markup`, and
  `allow_skipped_levels`, normalized with defaults applied by the loader.
- **Document shape.** `title` is the rule for every `h1` and `sections`
  describes the `h2` headings. A document has at most one `h1`; if one exists
  the root scope is its `h2` children, otherwise it is the document's `h2`s. A
  surplus `h1` is `too-many-sections`. A header outside the `h1` and
  everything below it — or, with no `h1`, outside the document's `h2`s and
  everything below them — is `detached-section` at any level, reported once
  per detached subtree root, and takes part in no rule matching, no
  cardinality count, and no constraint. A document with no `h1` conforms.
- **Frontmatter.** Presence checking (`required`, `allow`) plus value
  validation delegated to a JSON Schema given inline or as a path relative to
  the schema file, including linked `$ref` resource graphs. A block that does
  not parse is reported with the YAML parser's own wording and position, and a
  byte-order mark leading the block is dropped rather than becoming part of the
  first key. Alias expansion is bounded by a multiple of the block's own size
  and collection nesting by a fixed depth limit, which the value an alias
  expands to counts against exactly as the written text does. A linked graph is
  bounded in turn by how many `$ref` and `$dynamicRef` keywords it declares in
  all, counted across its documents rather than within each, because a chain of
  references costs a stack frame per link however shallowly its documents nest.
- **YAML tag handling.** In a schema file as in a frontmatter block, a
  core-schema tag is honoured when it names its node's own kind and refused
  when it does not — `sections: !!map` over a block sequence is an error — and
  a tag outside the yaml.org namespace is refused anywhere, including on the
  document root. A schema file, like a frontmatter block, may begin with a
  byte-order mark.
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

- `npm/` distribution packaging exists but is not functional.
- No pre-built binaries are published; `cargo install outlint` builds from
  source.

[Unreleased]: https://github.com/shangaslammi/outlint/commits/main/
