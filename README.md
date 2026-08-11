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

This builds the `outlint` binary from source. Pre-built binaries are not
published yet.

## Quickstart

Write a schema. The default project schema is `.outlint.yml`:

```yaml
version: 1
title: "*"                  # exactly one h1, any text
sections:                   # rules for h2 headings
  - id: overview
    match: "Overview"
    required: true
  - id: design
    match: "Design"
    required: true
    strict: true            # only the child rules below are allowed
    sections:
      - match: "Alternatives"
  - id: rollout
    match: "Rollout"
    required: false
constraints:
  - ordered: [overview, design]
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
design.md:1:1 [ordered] the `ordered` constraint is not satisfied; header_path=""; schema_node=constraint(scope=[],index=0); schema_location=".outlint.yml":17:12; involved_headers=["Design"@3:1, "Overview"@7:1]; references=[overview=>exact:"Overview", design=>exact:"Design"]
design.md:5:1 [unexpected-section] the section is not permitted in this closed scope; header_path="Design > Implementation Notes"; schema_node=rule(scope=[],index=1); schema_location=".outlint.yml":7:7
2 diagnostics in 1 file
```

Two problems: `## Overview` comes after `## Design` although the schema
orders them the other way, and `### Implementation Notes` is not one of the
children the `strict` Design scope permits. Each line carries a stable
diagnostic id (`ordered`, `unexpected-section`), the document location, and
the schema location that produced it, so both sides of a failure are
traceable.

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
version: 1

options:
  match_case: false            # matchers are case-insensitive by default
  root_level: 2                # `sections` describes h2 headings

frontmatter:
  required: true
  schema: ./frontmatter.schema.json   # delegated JSON Schema (draft 2020-12)

title: "*"                     # the h1: any text, exactly one

sections:
  - id: overview
    match: "Overview"          # exact matcher
    required: true             # 1..1
    strict: true               # unmatched children are diagnostics
    sections:
      - match: "Goals"         # auto-id: goals
        required: true
  - id: api
    match: "/API: .+/"         # regex, anchored to the whole header text
    repeat: "0..n"             # any number of these
  - id: changelog
    match: "Changelog"
    required: false            # 0..1
  - id: history
    match: "History"
    required: false
  - match: "Appendix *"        # glob
    required: false
  - match: "*"                 # first match wins, so catch-alls come last
    allow: false               # any other h2 is a violation

constraints:
  - one_of: [changelog, history]
  - requires: { if: api, then: overview }
  - ordered: [overview, api]
```

The pieces:

- **Matchers.** `match` is exact by default; `/.../` is an anchored regex
  (RE2 dialect — no backreferences or lookaround), a string containing `*`
  is a glob, and a bare `*` matches anything. Rules in a scope are tried in
  order and the first match wins.
- **Cardinality.** `required: true` means `1..1`, `required: false` means
  `0..1`, `repeat: "min..max"` sets explicit bounds with `n` for unbounded,
  and the default is `0..n`.
- **Scopes.** A rule's `sections` describes the headings one level deeper.
  `strict: true` closes a scope so unmatched children are reported;
  `allow: false` turns a match into a violation outright.
- **Constraints.** `one_of`, `any_of`, `at_most_one`, `all_or_none`,
  `requires`, `conflicts`, and `ordered` relate rules addressed by id, at
  the schema root or inside any rule's scope.
- **Frontmatter.** outlint checks presence (`required`, `allow`) and
  delegates value validation to a JSON Schema, given inline or as a path
  relative to the schema file. Note that `fm.` propositions in constraints
  are accepted by the loader but are **not evaluated yet**: a constraint
  depending on one is never satisfied.

Schema mistakes are diagnostics too, with their own stable ids
(`duplicate-id`, `unresolved-ref`, `invalid-matcher`, `invalid-repeat`,
`ordered-scope-mismatch`, …).

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
file looking for `.outlint.yml` and uses the nearest one. Discovery is per
file, so one invocation can check documents belonging to different projects.
Only the exact name `.outlint.yml` is discovered; files named
`*.outlint.yml` are explicit schemas and must be passed with `--schema`.

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
{"results":[{"diagnostics":[{"header_path":["Design"],"id":"missing-section","location":{"column":1,"line":1},"message":"matched 0 sections, but at least 1 are required","schema_location":{"column":7,"line":7,"path":".outlint.yml"},"schema_node":{"index":1,"kind":"rule","scope":[]}}],"kind":"document","path":"rollout.md","schema":".outlint.yml"}],"summary":{"diagnostics":1,"documents":1,"files":1,"schemas":0},"version":1}
```

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

The full CLI contract is [`spec/cli.md`](spec/cli.md).

## Layout

- `crates/outlint-core` — schema model, parser, validator; a pure, IO-free
  library ([README](crates/outlint-core/README.md))
- `crates/outlint-cli` — the `outlint` command-line tool
  ([README](crates/outlint-cli/README.md))
- `spec/` — the specification (normative):
  [`outlint-spec.md`](spec/outlint-spec.md), [`cli.md`](spec/cli.md)
- `testdata/` — conformance corpus shared by all implementations
  ([README](testdata/README.md))
- `npm/` — npm distribution packaging (not functional yet)

## Status and stability

outlint is at version 0.1.0. It implements Outlint Schema Specification v1
and the CLI contract in [`spec/cli.md`](spec/cli.md), and the shared
conformance corpus in `testdata/` runs in CI. One documented gap remains:
`fm.` propositions in constraints are parsed and validated but not yet
evaluated.

This is a 0.x release: expect breaking changes to the schema language, the
diagnostic set, the JSON shape, and the library API before 1.0. Where an
implementation and the specification disagree, the specification in `spec/`
is the normative reference and the implementation is the bug.

## MSRV

Rust 1.86.

The minimum supported Rust version is part of the compatibility contract:
raising it is a minor-version change (a patch release will never require a
newer toolchain), and the MSRV is tested in CI.

## Contributing

Bug reports, specification questions, and pull requests are welcome via the
issue tracker at <https://github.com/shangaslammi/outlint>. Behavior changes
should come with a `testdata/` case and a specification update; please open
an issue before large changes so the design can be settled first.

Before submitting, run:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets
cargo test --workspace
```

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
