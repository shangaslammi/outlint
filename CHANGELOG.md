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
  Windows (x64). The npm package downloads and verifies the matching binary
  during installation.
- **Suppressions.** `<!-- outlint-disable <id>,... -->` before a heading and
  `<!-- outlint-disable-file <id>,... -->` anywhere in a file.
- **Conformance corpus.** [`testdata/`](testdata/README.md), an
  implementation-independent fixture set driven in CI by the Rust runner and
  reusable by other implementations.
- **Dual licensing** under [MIT](LICENSE-MIT) or
  [Apache-2.0](LICENSE-APACHE), and a declared MSRV of Rust 1.86 tested in CI.

[Unreleased]: https://github.com/shangaslammi/outlint/commits/main/
