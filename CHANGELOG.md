# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Because outlint is at `0.x`, the schema language, the diagnostic set, the JSON
output shape, and the library API may all change in a minor release. See
[README.md](README.md#status-and-stability).

## [Unreleased]

### Added

- **Outlint Schema Specification v2.** Ordered scopes are now consuming rule
  phases with canonical assignment and deterministic recovery; headings that
  match a rule but cannot occupy its phase report `misplaced-section`.
  Prohibitions move to first-class `forbid_sections` guards, unmatched headings
  can be admitted with `extras: anywhere`, and `unordered: true` selects a
  declaration-first classifier where explicit `ordered` constraints remain
  available. Omitted and empty `sections` are now distinct, and `outline: []`
  is the explicit empty root grammar. Collection-shaped matchers must spell a
  cardinality (`missing-cardinality`), while rules shadowed by an unordered
  wildcard are rejected as `unreachable-rule`. Migration removes `strict`,
  accepting-rule `allow`, rule-level `ordered`, and
  `options.ordered_sections`; replaces denials with guards and open scopes with
  `extras` or a positioned wildcard; marks locally unordered scopes; and
  reviews formerly implicit exact-rule cardinalities. Overlapping v1 matchers
  may require unordered classification plus an explicit `ordered` constraint,
  while exception-before-denial has no exact translation because guards run
  first (spec §§3, 6, 8, 10).
- **Typed Values and unified locators.** Regex rules and frontmatter may now
  declare `int`, `bool`, `date`, `semver`, `dotted`, and `text` captures:
  `captures` on a regex rule binds its named groups, `frontmatter.captures`
  exports values addressed by a singular RFC 9535 JSONPath and may mark them
  `required`, and values are parsed without coercion, reporting
  `invalid-value` and `missing-value`. A rule's `order` list orders that
  rule's own repeated headings by a captured value, ascending or descending
  and optionally strict, reporting `order-violation` per offending adjacent
  pair. Constraint operands are now unified locators: relative and
  `$.`-absolute name paths, `[i]` positional narrowing, structural steps, and
  a singularity requirement on every non-terminal step. Frontmatter document
  queries are written `fm[...]` and take a complete RFC 9535 query — bare as
  a typed boolean read, `=literal` as type-preserving existential equality —
  with a portable guaranteed core of child name, index, and wildcard segments
  and non-surrogate quoted-name escapes, and a provider-dependent vendor tier
  beyond it that includes surrogate escape pairs. `fm[...]` evaluation is
  resource-bounded; exceeding the bound is an operational failure with exit
  code 2 and no partial verdict. Meanwhile, `fm.<name>` now refers only to a
  declared frontmatter capture: the former dynamic-key meaning is gone and
  legacy `fm.key=value` is invalid. CLI machine output is envelope version 4,
  whose diagnostics carry tagged `rule`,
  `frontmatter_query`, and `frontmatter_capture` references preserving the
  written locator and its typed-value metadata, plus guard schema nodes; there
  is no earlier-envelope compatibility mode, so consumers must reject envelope
  versions they do not support (spec §§2.3–2.4, 3.8, 4.4–4.6, 6, 11.3–11.4).
- **Per-document schema discovery.** Without `--schema`, discovery now checks
  each ancestor directory for `<stem>.outlint.yml` — the document's file name
  with its final extension removed, so `CHANGELOG.md` discovers
  `CHANGELOG.outlint.yml` — before the directory default `.outlint.yml`. The
  nearest directory still wins; within one directory the document-specific
  name takes precedence. Other `*.outlint.yml` files remain explicit
  `--schema` input (spec §11.2). This repository uses the mechanism on
  itself: `CHANGELOG.outlint.yml` validates this changelog in CI.

### Fixed

- **Discovery ignores non-files.** A directory (or other non-regular file)
  named `.outlint.yml` — or now `<stem>.outlint.yml` — no longer terminates
  schema discovery only to fail when read; it is skipped as if absent and
  the search continues upward (spec §11.2).

## [0.1.0] - 2026-08-30

### Added

- **Specification.** [`spec/outlint-spec.md`](spec/outlint-spec.md) defines
  Outlint Schema Specification v1: the document model, schema format, matching
  semantics, rule identifiers and reference paths, constraints, diagnostics,
  options, a normative validation algorithm, and the command-line contract.
- **Schema language.** A top-level `outline:` list of `h1` rules — carrying
  cardinality, `strict`, denial, and child `sections` like any nested rule —
  or its permanent sugar: a `title` matcher with a `sections` list for the
  common single-`h1` document, `title: null` for a document with no `h1` at
  all, and bare `sections` without `title`, which implies `title: "*"` —
  exactly one `h1` of any text. The two forms are mutually exclusive
  (schema error `conflicting-outline`, anchored at the later-declared key),
  and an empty `outline: []` is refused toward `title: null`, which says
  what it means. Exact, glob, anchored-regex (RE2 dialect),
  and `*` matchers with first-match-wins resolution; `required` /
  `repeat: "min..max"` cardinality; `strict` scopes and `allow: false`
  denials; explicit and derived rule `id`s with dotted reference paths.
- **Constraints.** `one_of`, `any_of`, `at_most_one`, `all_or_none`,
  `requires`, `conflicts`, and `ordered`, usable at the schema root or inside
  any rule scope. Constraint refs may address frontmatter as well as rules:
  `fm.key` is presence of a non-null value, `fm.key=value` is typed scalar
  equality under the YAML core schema, and dotted paths step through nested
  mappings.
- **Ordered scopes.** A scope's rule list is its document order by default:
  every header an earlier accepting rule matched must precede every header
  a later one matched, reported as `ordered` with the pair that broke.
  `options.ordered_sections` sets the default for every scope and a rule's
  `ordered: <bool>` overrides it for its child scope. An explicit `ordered`
  constraint over an already-ordered scope is refused as
  `ordered-scope-mismatch`, since it is either redundant or contradictory;
  the constraint remains for partial orders and for scopes declared
  `ordered: false`.
- **Options.** `match_case`, `strip_inline_markup`, and
  `allow_skipped_levels`, normalized with defaults applied by the loader.
- **Document shape.** The document root is a virtual level-0 header enclosing
  the whole document: `outline` rules describe its `h1` children exactly as
  nested rules describe any header's children, and every scope — the root
  included — is bound per parent header. Under the sugar, `title` is the rule
  for the document's one `h1` and `sections` describes the `h2`s beneath it —
  the document's own `h2`s under `title: null`. A document missing its `h1`
  where a title matcher is spelled or implied is `missing-title`; a surplus
  `h1` there is `too-many-sections`. A top-level header deeper than the root
  admits skips a level against the virtual root itself, reported as
  `skipped-level` once per skipping subtree root unless `allow_skipped_levels`
  admits it into the enclosing scope.
- **Frontmatter.** Presence checking (`required`, `allow`) plus value
  validation delegated to a self-contained inline JSON Schema or a linked JSON
  Schema whose path is relative to the Outlint schema file, including linked
  `$ref` resource graphs. Inline references must be fragment-only. A block that
  does not parse is reported with the YAML parser's own wording and position,
  and a byte-order mark leading the block is dropped rather than becoming part
  of the first key. Alias expansion is bounded by a multiple of the block's own size
  and collection nesting by a fixed depth limit, which the value an alias
  expands to counts against exactly as the written text does. A linked graph is
  bounded in turn by how many `$ref` and `$dynamicRef` members it declares in
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
  (invocation or operational failure). Human output is reader-oriented, with
  labeled source and schema locations and expected-versus-observed ordering
  evidence, and deliberately has no stable textual grammar; the versioned JSON
  object is the interface for scripts and integrations.
- **Binary distribution.** GitHub Releases provide pre-built binaries for
  macOS (x64 and arm64), Linux glibc (x64 and arm64), Linux musl (x64), and
  Windows (x64), plus shell and PowerShell installers. The `@outlint/cli` npm
  package has no install-time lifecycle script: its first invocation
  downloads, verifies, and caches the matching GitHub Release binary. The core
  library and CLI are also published to crates.io.
- **Suppressions.** `<!-- outlint-disable <id>,... -->` before a heading and
  `<!-- outlint-disable-file <id>,... -->` anywhere in a file.
- **Conformance corpus.** [`testdata/`](testdata/README.md), an
  implementation-independent fixture set driven in CI by the Rust runner and
  reusable by other implementations.
- **Dual licensing** under [MIT](LICENSE-MIT) or
  [Apache-2.0](LICENSE-APACHE), and a declared MSRV of Rust 1.86 tested in CI.

[Unreleased]: https://github.com/shangaslammi/outlint/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/shangaslammi/outlint/releases/tag/v0.1.0
