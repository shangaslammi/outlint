# outlint

A linter for the *structure* of Markdown documents. Instead of checking prose
or formatting, outlint validates a document's heading outline — hierarchy,
which sections must exist, how often they may repeat, what order they come
in, which sections may not co-occur — plus its YAML frontmatter, against a
declarative YAML schema.

## Why

Some Markdown is not free-form. Runbooks, ADRs, RFCs, incident postmortems,
and per-endpoint API docs are usually expected to follow a house structure:
every ADR has `## Context`, `## Decision`, and `## Consequences`, in that
order; every runbook has a `## Rollback` section; a page marked
`status: deprecated` in its frontmatter must carry a `## Deprecated` notice.

That structure is normally enforced by review comments, which is slow and
inconsistent. outlint makes it a schema you commit to the repository and a
command you run in CI. It never rewrites your files — it only reports.

## Install

```sh
cargo install outlint
```

This builds the `outlint` binary from source. To install the matching pre-built
binary through npm instead:

```sh
npm install --global @outlint/cli
```

The npm package has no install-time lifecycle script. On its first run it
downloads the matching binary from the same-version GitHub Release, verifies
the cargo-dist SHA-256 sidecar, and keeps the binary in the user cache for
later runs. The first run therefore requires access to GitHub Releases.

Each release also ships pre-built binaries and installer scripts on its
[GitHub Release](https://github.com/shangaslammi/outlint/releases) — see the
release page for the shell and PowerShell one-liners and per-platform
archives with checksums.

## Quickstart

Write a schema. The default project schema is `.outlint.yml`:

```yaml
version: 2
title: "*"                  # exactly one h1, any text
sections:                   # rules for h2 headings, in document order
  - id: overview
    match: "Overview"
    required: true
  - id: design
    match: "Design"
    required: true
    sections:               # declared child scopes are exhaustive
      - match: "Alternatives"
  - id: rollout
    match: "Rollout"
    required: false
```

Point it at a document. `design.md`:

```markdown
# Widget Redesign

## Design

### Implementation Notes

## Overview
```

```sh
outlint check design.md
```

```text
design.md:1:1 [missing-section] matched 0 sections, but at least 1 are required
  expected: "Design"
  rule: .outlint.yml:7:5

design.md:3:1 [misplaced-section] the section matches a rule but cannot occupy its ordered phase
  section: "Widget Redesign > Design"
  schema: .outlint.yml:2:8

2 diagnostics in 1 file
```

This is an illustration of the current human presentation, not a parseable
output grammar. Its wording and layout may change between releases; use
`--format json` for scripts and integrations.

`## Overview` comes after `## Design` although the schema lists them the
other way round. Ordered recovery leaves `Design` misplaced, so its required
rule is also missing; child grammar never changes that assignment. Each
finding carries a stable diagnostic id (`misplaced-section`,
`missing-section`), the document location, and
the schema location that produced it, so both sides of a failure are
traceable regardless of presentation.

Fix the document — `good.md`:

```markdown
# Widget Redesign

## Overview

## Design

### Alternatives

## Rollout
```

```sh
outlint check good.md
```

A clean run prints nothing and exits `0`.

## Schema sketch

This is a taste of the schema language, not a manual —
[`spec/outlint-spec.md`](spec/outlint-spec.md) is the normative definition.

```yaml
version: 2

options:
  match_case: false            # matchers are case-insensitive by default
  strip_inline_markup: true    # match on `Foo bar`, not `**Foo** [bar](x)`

frontmatter:
  required: true
  schema: ./frontmatter.schema.json   # delegated JSON Schema (draft 2020-12)
  captures:                           # typed values outlint itself consumes
    released: { path: "$.release.date", type: date, required: true }
    draft: { type: bool }

title: "*"                     # the h1: any text, exactly one

sections:
  - id: overview
    match: "Overview"          # exact matcher
    required: true             # 1..1
    sections:                  # declared scopes reject unmatched children
      - match: "Goals"         # default id: goals
        required: true
  - id: api
    match: "/API: .+/"         # regex, anchored to the whole header text
    repeat: "0..n"             # any number of these
  - id: changelog
    match: "Changelog"
    required: false            # 0..1
    sections:
      - id: release
        match: '/\[(?<version>\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)\] - (?<date>\d{4}-\d{2}-\d{2})/'
        repeat: "1..n"
        captures: { version: semver, date: date }   # typed named groups
        order:                                      # newest release first
          - { by: version, dir: desc, strict: true }
          - { by: date, dir: desc }
  - id: history
    match: "History"
    required: false
  - match: "Deprecated"        # default id: deprecated
    required: false
  - match: "Appendix *"        # glob
    required: false
forbid_sections:
  - match: "*"                 # guards reject before accepting assignment

constraints:
  - one_of: [changelog, history]
  - requires: { if: api, then: overview }
  - requires: { if: "fm[$.status]=deprecated", then: deprecated }
  - requires: { if: fm.released, then: changelog }
  - conflicts: { if: fm.draft, then_not: deprecated }
```

The pieces:

- **Matchers.** `match` is exact by default; `/.../` is an anchored regex
  (RE2 dialect — no backreferences or lookaround), a string containing `*`
  is a glob, and a bare `*` matches anything. Ordered scopes consume rule
  phases; `unordered: true` scopes classify with the first matching rule.
- **Cardinality.** `required: true` means `1..1`, `required: false` means
  `0..1`, `repeat: "min..max"` sets explicit bounds with `n` for unbounded,
  exact rules default to `1..1`; collection matchers require an explicit
  cardinality.
- **Scopes.** A rule's `sections` describes the headings one level deeper.
  Every declared list is exhaustive; `extras: anywhere` filters unmatched
  headings, while `forbid_sections` guards reject before assignment.
- **Order.** Rules bind in document order by default: the list above says
  Overview comes before the API sections, which come before Changelog. Set
  `unordered: true` on a scope where sections may come in any order. That
  governs assignment across rules; ordering the repeated matches of a rule is
  rule's own `order` list, below.
- **Typed values.** A regex rule can declare its named groups as typed
  captures (`captures: { version: semver, date: date }`), and
  `frontmatter.captures` exports typed values from the frontmatter mapping,
  each with a `type`, an optional `$`-rooted singular JSONPath `path`
  defaulting to the capture name, and an optional `required`. The type set
  is closed: `int`, `bool`, `date`, `semver`, `dotted`, and `text`. Values
  are parsed, never coerced — unquoted `version: 1.2` is a YAML float and so
  is not a `semver` — and a value that fails its type's spelling, YAML kind,
  calendar, or numeric bound is `invalid-value`. A frontmatter capture
  declared `required: true` whose value is absent is `missing-value` —
  except when the block itself is absent or does not parse, where captures
  are not evaluated at all and the block-level diagnostic is the whole
  report.
- **Value order.** A rule's `order` list orders that rule's own repeated
  matches by one of its captures: `by` names the capture, `dir` is `asc` or
  `desc`, and `strict: true` demands uniqueness as well. Entries are
  independent single-key orders, not a multi-key sort, and each parent
  section binds its own sequence, so occurrences under different parents are
  never compared. Every violating adjacent pair is one `order-violation`. If
  any selected value in a sequence is invalid, that entry reports no
  ordering for that scope, because skipping the bad value would invent an
  adjacency between its neighbours.
- **Constraints.** `one_of`, `any_of`, `at_most_one`, `all_or_none`,
  `requires`, `conflicts`, and `ordered` relate *locators*, at the schema
  root or inside any rule's scope; `ordered` spells a partial order inside an
  unordered scope. A locator is a name path:
  `deployment.rollback-plan` reads relative to the scope the constraint is
  attached to, `$.overview.goals` from the outermost scope, and `[i]`
  narrows a step to its i-th match (`$.release[0]`). Name steps are joined
  with `.` and structural steps with `/` — `/text`, a heading's own text, is
  the one such step this version allocates — and a name step may never
  follow a structural one. Every non-terminal step must be singular, by
  declared cardinality or by `[i]`; only the last step may stay plural. A
  locator that ends in a rule id is a proposition, satisfied when it matches
  at least one header; an outline locator ending in a captured or intrinsic
  value is a value, not automatically a proposition, and is rejected where a
  proposition is required.
- **Frontmatter.** outlint checks presence (`required`, `allow`) and
  delegates structural validation to either a self-contained inline JSON
  Schema or a linked JSON Schema whose path is relative to the Outlint schema
  file. Inline schemas may use fragment-only references; linked schemas may
  use local multi-file reference graphs. `frontmatter.captures` is the
  complementary mechanism: it exports typed singular values rather than
  validating shape. Constraints address frontmatter two ways. `fm[...]`
  wraps one complete RFC 9535 JSONPath query over the frontmatter mapping —
  bare, it is a typed boolean read, satisfied only when some result node is
  the boolean `true`; `false`, null, and an empty result leave it
  unsatisfied, and any non-boolean, non-null result node is `invalid-value`
  and suppresses the entire constraint containing it — a `true` alongside it
  does not rescue the read. `fm[$.status]=deprecated` is instead
  existential, type-preserving equality over the non-null result nodes.
  Results are always evaluated in full, and duplicate result nodes are
  collapsed by identity. Child name, index, and wildcard segments are the
  portable *guaranteed core*, along with the escapes inside a quoted name
  whose code unit is not a surrogate — a surrogate escape pair is
  vendor-tier, so write a non-BMP character literally for portability.
  Slices, descendant segments, filters, multiple selectors in one segment,
  and functions are likewise admitted but *vendor-tier*, so their behavior
  depends on the implementation's JSONPath provider. `fm.<name>`, by
  contrast, names a capture declared under `frontmatter.captures` and is
  checked when the schema loads, so a typo is `unresolved-ref` rather than a
  quietly false test. It is not a dynamic YAML-key lookup: the former
  `fm.key=value` spelling is invalid, and a document key is queried as
  `fm[$.key]=value`.

A document with several `h1` parts drops the `title:` sugar for the general
form: `outline:` is a list of the same rule objects, one level up,
describing the `h1`s themselves. Every scope binds per parent, so each part
carries its own obligations:

```yaml
version: 2
outline:
  - match: "Part *"
    repeat: "1..n"
    sections:
      - match: "Overview"      # required in every part separately
        required: true
```

`title: <matcher>` + `sections:` is exactly `outline:` with one required
rule; bare `sections:` implies `title: "*"`; and `title: null` declares a
document with no `h1` at all.

Schema mistakes are diagnostics too, with their own stable ids
(`duplicate-id`, `unresolved-ref`, `invalid-matcher`, `invalid-repeat`,
`invalid-capture`, `invalid-order`, `ordered-scope-mismatch`, …).

## CLI

```text
outlint check <FILE>...          Validate Markdown documents
outlint schema check <SCHEMA>... Validate Outlint schema files
outlint --help | --version
```

Both validation commands accept:

```text
      --format human|json     Select output format (default: human)
      --color auto|always|never
                              Control human-output color (default: auto)
  -h, --help                  Show help
```

`check` additionally accepts `-s, --schema <SCHEMA>` to use one schema for
every input. A bare `--` ends option parsing.

**Schema discovery.** Without `--schema`, outlint walks up from each input
file and uses the nearest schema, checking each directory first for
`<stem>.outlint.yml` — the file's name with its extension removed, so
`CHANGELOG.md` discovers `CHANGELOG.outlint.yml` — and then for the
directory default `.outlint.yml`. Discovery is per file, so one invocation
can check documents belonging to different projects. Other schema filenames
are explicit schemas and must be passed with `--schema`. (This repository
uses the mechanism on itself: `CHANGELOG.outlint.yml` at the root validates
`CHANGELOG.md` in CI.)

**stdin.** A file argument of `-` reads standard input. Because stdin has no
filesystem location, discovery is impossible and `--schema` is required:

```sh
git show HEAD:README.md | outlint check - --schema .outlint.yml
```

**Paths only.** v1 takes individual files, not directories. Use your shell
or `find` to expand:

```sh
find docs -name '*.md' -print0 | xargs -0 outlint check
```

**JSON output.** `--format json` emits one versioned object for the whole
invocation on stdout, never colorized. For a `rollout.md` that has
`# Widget Redesign`, `## Overview`, and `## Rollout` but no `## Design`:

```sh
outlint check rollout.md --format json
```

```json
{"results":[{"diagnostics":[{"id":"missing-section","location":{"column":1,"line":1},"message":"matched 0 sections, but at least 1 are required","schema_location":{"column":5,"line":7,"path":".outlint.yml"},"schema_node":{"index":1,"kind":"rule","scope":[]},"target":{"kind":"missing_header","matcher":"Design","parent":[]}}],"kind":"document","path":"rollout.md","schema":".outlint.yml"}],"summary":{"diagnostics":1,"documents":1,"files":1,"schemas":0},"version":4}
```

The envelope is exactly version `4`. There is no compatibility mode: a
consumer must read `version` and reject anything it
does not understand rather than assume the older reference shape.

Every diagnostic carries a `target` saying what it is about, tagged by
`kind`: `header` (a header the document has, as a `path` array),
`missing_header` (a section the schema requires, as the `parent` path it
belongs under plus the schema's `matcher` label), `document`, or
`frontmatter` (with the block's `line_range` and, when a JSON Schema or a
typed value rejected one entry, its `pointer`). The kinds are distinct
because their text has different provenance — a `missing_header` matcher is
schema text that may appear nowhere in the document.

Where a diagnostic names the schema locators it evaluated, it carries
`references`, a tagged union of three `kind`s, each keeping the locator's
original spelling in `locator`:

- `rule` — the resolved outline locator: its `anchor` (`current_scope` or
  `schema_root`), the `path` of declared names, an optional `positions`
  array aligned with `path` holding each step's `[i]` or null, and the
  target rule's `matcher`. Positions are JSON integers of arbitrary size;
  consumers must not assume they fit in 64 bits.
- `frontmatter_query` — the `query` inside `fm[...]` without the wrapper,
  plus `equals` (as `type` and `value`) when the locator spelled an equality
  literal.
- `frontmatter_capture` — the capture's `name` and its declared `type`.

Diagnostics from Typed Values are located in the schema like any other:
`schema_node` gains the `capture`, `frontmatter_capture`, and `order_entry`
kinds alongside the existing ones such as `rule` and `constraint`, so an
`invalid-value` or `order-violation` points at the declaration that produced
it. Each structured member appears only where the corresponding semantic
data exists and is omitted otherwise.

**JSON diagnostic order.** Within each JSON result, diagnostics have a fixed
total order: source line, then byte column, then diagnostic id, then
`schema_location` as `(path, line, column)` with absent first, then
`target` — by kind in the order above, then by its members — then
`message`, with the remaining rendered fields breaking any residual tie so
no two distinct diagnostics ever compare equal. The order is a pure
function of the reported diagnostics, never of the order validation
discovered them in. Human output may group or order findings for readability
and is not a stable machine interface.

**Schemas alone.** `outlint schema check .outlint.yml` runs every
schema-load-time check without needing a document — useful in CI when the
schema itself changes.

**Suppressions.** `<!-- outlint-disable <id>,... -->` on the line before a
heading suppresses those diagnostics for that heading;
`<!-- outlint-disable-file <id>,... -->` suppresses them for the whole file.
Schema errors are load-time failures and are never suppressible.

**Exit codes.**

| Code | Meaning |
| ---: | --- |
| `0` | All checked documents and schemas are valid |
| `1` | Validation completed and at least one diagnostic was emitted |
| `2` | An invocation or operational failure prevented normal validation |

```sh
outlint check README.md
case $? in
  0) echo "valid" ;;
  1) echo "outlint violations" ;;
  2) echo "outlint could not complete" ;;
esac
```

The normative CLI behavior is specified in
[`spec/outlint-spec.md` §11](spec/outlint-spec.md#11-command-line-interface).
The [CLI crate README](crates/outlint-cli/README.md) is the user guide,
including the complete `--help` surface.

## Layout

- `crates/outlint-core` — schema model, parser, validator; a pure, IO-free
  library ([README](crates/outlint-core/README.md))
- `crates/outlint-cli` — the `outlint` command-line tool
  ([README](crates/outlint-cli/README.md))
- `spec/` — the normative
  [Outlint specification](spec/outlint-spec.md)
- `testdata/` — conformance corpus shared by all implementations
  ([README](testdata/README.md))
- `npm/` — npm distribution packaging

## Status and stability

outlint is at version 0.1.0. It implements Outlint Schema Specification v1,
including its command-line contract in §11, and the shared conformance corpus
in `testdata/` runs in CI.

This is a 0.x release: expect breaking changes to the schema language, the
diagnostic set, the JSON shape, and the library API before 1.0. Where an
implementation and the specification disagree, the specification in `spec/`
is the normative reference and the implementation is the bug.

## Development process

outlint is developed with AI coding agents under human design, specification,
and review. The maintainer owns the design and the specification, reviews
every change, and is accountable for the result; agents implement, test, and
draft documentation against that specification, and commits they co-authored
say so in a `Co-Authored-By` trailer. Every change is held to the same bar
regardless of who wrote it: the specification is normative, the conformance
corpus in `testdata/` runs in CI, and a diagnostic that disagrees with the
specification is a bug whoever authored it. Contributors may use the same
tools under the same conditions — see
[CONTRIBUTING.md](CONTRIBUTING.md#ai-assisted-contributions).

## MSRV

Rust 1.86.

The minimum supported Rust version is part of the compatibility contract:
raising it is a minor-version change (a patch release will never require a
newer toolchain), and the MSRV is tested in CI.

## Contributing

Bug reports, specification questions, and pull requests are welcome via the
issue tracker at <https://github.com/shangaslammi/outlint>. Changes to specified
behavior should update the specification and add a `testdata/` case. A bug fix
that brings code into line with the existing specification needs the fixture
and a citation to that specification text, not a redundant specification edit.
Please open an issue before large changes so the design can be settled first.

Before submitting, run:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

CI runs the dependency-resolving Cargo commands with `--locked`, runs tests on
stable Linux, macOS, and Windows, and runs the full test suite on Rust 1.86.
The local commands above omit `--locked` so an intentional lockfile change is
reported normally; `cargo fmt` does not resolve dependencies.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in this work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
