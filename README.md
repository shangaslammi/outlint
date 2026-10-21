# outlint

Outlint is a linter for the *structure* of Markdown. You describe the outline
a kind of document must have — which sections exist, in what order, how often,
what their headings look like, what the frontmatter must contain — in a small
YAML schema, and `outlint check` reports every place a document departs from
it. It does not check prose or formatting, and it never rewrites a file.

It is for anyone who keeps Markdown that has to follow a template: ADRs,
RFCs, runbooks, postmortems, changelogs, docs-as-code sites, agent
instruction files, and Markdown produced by LLMs.

Outlint ships as a Rust CLI, a Rust library crate (`outlint-core`), and an
npm package. It is pre-1.0; see [Status](#status-and-stability). Optional,
experimental extensions build on the same document model; see
[Extensions](#extensions-experimental).

## A first look

A schema for architecture decision records, saved as `.outlint.yml` next to
the records:

```yaml
version: 1
title: "*"
sections:
  - match: "Context and Problem Statement"
  - match: "Decision Drivers"
    required: false
    content:
      - block: list
        list_kind: bullet
  - match: "Considered Options"
    content:
      - block: list
        list_kind: bullet
  - match: "Decision Outcome"
    sections:
      - match: "Consequences"
        required: false
        content:
          - block: list
            list_kind: bullet
            items:
              - match: '/(Good|Neutral|Bad), because .+/'
                repeat: "1..n"
```

Every ADR has one title and the four `##` sections in this order (Decision
Drivers optional), bullet lists where the template calls for them, and every
consequence states whether it is good, neutral, or bad. A record that skips
`Considered Options` and has one free-form consequence,
`0007-use-postgres.md`:

```markdown
# Use PostgreSQL for the order service

## Context and Problem Statement

The order service needs a transactional store with mature tooling.

## Decision Outcome

Chosen option: PostgreSQL, because it is the only option that meets all
drivers.

### Consequences

- Good, because the team already operates PostgreSQL.
- We need to provision a new cluster.
```

```sh
outlint check 0007-use-postgres.md
```

```text
0007-use-postgres.md:1:1 [missing-section] matched 0 sections, but at least 1 are required
  expected: "Considered Options"
  rule: .outlint.yml:10:5

0007-use-postgres.md:15:1 [unexpected-item] direct item 1 matches no item rule
  list within: "Use PostgreSQL for the order service > Decision Outcome > Consequences"
  list: 0
  item: 1
  content rule: .outlint.yml:19:13

2 diagnostics in 1 file
```

The command exits `1`. Each finding has a stable id, the document location,
and the schema line that produced it. A clean document prints nothing and
exits `0`. The human layout above is for reading and may change between
releases; `--format json` is the stable interface for tools (see
[CI and automation](#ci-and-automation)).

## Use cases

**Keep templated documents consistent in CI.** ADRs, RFCs, runbooks,
incident postmortems, PR and issue write-ups, and changelogs all have a house
structure that is normally enforced by review comments. Commit the structure
as a schema, run `outlint check` in CI, and the review can be about content.
This repository does it to itself: [`CHANGELOG.outlint.yml`](CHANGELOG.outlint.yml)
validates [`CHANGELOG.md`](CHANGELOG.md), including that release headings
carry a semver version and a date and run newest-first. Swap two releases
and the first finding is:

```text
CHANGELOG.md:11:1 [order-violation] the values of capture `version` are out of their declared strictly descending order: `1.1.0` must be greater than `1.2.0`
  section: "Changelog > [1.2.0] - 2026-10-01"
  order entry: 0
  out-of-order pair:
    1. CHANGELOG.md:5:1 "Changelog > [1.1.0] - 2026-09-07"
    2. CHANGELOG.md:11:1 "Changelog > [1.2.0] - 2026-10-01"
  declared: CHANGELOG.outlint.yml:51:9
```

**Validate Markdown that an LLM or agent produced.** When a model is asked
for a document in a fixed shape — a design note, a report, a decision record
— the schema is the contract and `outlint check --format json` is the test.
Diagnostics carry a stable `id`, a `target` naming the heading or block
concerned, and a `message`, so a pipeline can reject the output or feed the
findings back to the model for another pass instead of eyeballing it.

**Guard agent instruction files and knowledge bases.** `AGENTS.md`,
`CLAUDE.md`, skill files, and team knowledge bases are read by machines as
much as by people, and a missing or renamed section silently changes what an
agent does. A schema makes the expected sections, their order, and the
required list items explicit, and the check fails before the file is merged.

**Enforce docs-as-code templates and frontmatter.** Static-site content and
API reference pages usually combine required frontmatter keys with a fixed
page layout. Outlint validates the frontmatter against a JSON Schema and can
tie its values to the outline: a page whose `status` is `deprecated` must
carry a `## Deprecated` section, a `draft` page must not have a roadmap.

## Installation

Outlint comes in two editions. Both install one executable named `outlint`.

| Edition | Contents | Release archive |
|---|---|---|
| Check-only | the validator: `check`, `schema check` | `outlint-<target>.tar.xz` (Windows: `.zip`) |
| Full | the validator plus all [extensions](#extensions-experimental) | `outlint-full-<target>.tar.xz` (Windows: `.zip`) |

**Cargo** (builds from source; Rust 1.88 or newer):

```sh
cargo install outlint --locked                 # check-only
cargo install outlint --locked --all-features  # full: all extensions
```

Each extension is a Cargo feature of the `outlint` crate; `--all-features`
enables every extension. `--locked` uses the release's tested dependency
versions.

**npm** (check-only edition):

```sh
npm install --global @outlint/cli
```

The package has no install-time script. The first `outlint` command
downloads the matching binary from the same-version GitHub Release, verifies
its SHA-256 sidecar, and caches it, so the first run needs access to GitHub
Releases.

**Pre-built binaries.** Each [GitHub Release](https://github.com/shangaslammi/outlint/releases)
ships archives with checksums for macOS (arm64, x64), Linux glibc (arm64,
x64), Linux musl (x64), and Windows (x64), plus installer scripts:
`outlint-installer.sh` / `outlint-installer.ps1` for check-only and
`outlint-full-installer.sh` / `outlint-full-installer.ps1` for full. The
release page has the copy-paste one-liners. Installing one edition over the
other in the same directory replaces it.

## Quickstart

1. Write a schema and save it as `.outlint.yml` in the directory that holds
   the documents (or above it). The [first look](#a-first-look) above is a
   complete one.
2. Check it on its own:

   ```sh
   outlint schema check .outlint.yml
   ```

   Schema mistakes are diagnostics too. A glob, regex, or `*` matcher with no
   cardinality, for example:

   ```text
   api.outlint.yml:3:12 [missing-cardinality] regex, glob, and wildcard rules must declare `required` or `repeat`

   1 diagnostic in 1 file
   ```

3. Check documents:

   ```sh
   outlint check docs/adr/*.md
   ```

   Without `--schema`, each file finds its own schema by walking up from its
   directory, looking first for `<stem>.outlint.yml` (so `CHANGELOG.md` picks
   up `CHANGELOG.outlint.yml`) and then for `.outlint.yml`. Files in one
   invocation may use different schemas. `--schema <FILE>` (`-s`) uses one
   schema for every input and disables discovery.

Outlint takes files, not directories; expand with your shell or `find`:

```sh
find docs -name '*.md' -print0 | xargs -0 outlint check
```

`-` reads standard input and requires `--schema`:

```sh
git show HEAD:CHANGELOG.md | outlint check - --schema CHANGELOG.outlint.yml
```

## The schema language

This is a tour of the pieces you will use most. The normative definition,
with every rule and diagnostic, is [`spec/outlint-spec.md`](spec/outlint-spec.md).

```yaml
version: 1

options:
  match_case: false            # default: matchers ignore case
  strip_inline_markup: true    # default: match `Foo bar`, not `**Foo** [bar](x)`

frontmatter:
  required: true
  schema: ./frontmatter.schema.json   # JSON Schema (draft 2020-12): a path or an inline mapping
  captures:                            # typed values outlint itself reads
    released: { path: "$.release.date", type: date }

title: "*"                     # exactly one h1, any text

sections:                      # the h2s, in this order; the list is exhaustive
  - id: overview
    match: "Overview"          # exact text; exactly one by default
    sections:
      - match: "Goals"         # default id: goals
      - match: "Non-goals"
        required: false        # 0..1
  - id: api
    match: "/API: .+/"         # anchored regex (RE2 dialect)
    repeat: "0..n"             # pattern matchers must spell a cardinality
    extras: anywhere           # unlisted h3s under an API section are admitted
    sections:
      - match: "Errors"
        required: false
  - id: changelog
    match: "Changelog"
    required: false
    sections:
      - id: release
        match: '/\[(?<version>\d+\.\d+\.\d+)\] - (?<date>\d{4}-\d{2}-\d{2})/'
        repeat: "1..n"
        captures: { version: semver, date: date }
        order:
          - { by: version, dir: desc, strict: true }
  - match: "Appendix *"        # glob
    repeat: "0..n"
  - id: deprecated
    match: "Deprecated"
    required: false

forbid_sections:
  - match: "TODO"              # rejected anywhere among the h2s

constraints:
  - requires: { if: api, then: overview.goals }
  - conflicts: { if: deprecated, then_not: api }
  - requires: { if: "fm[$.status]=deprecated", then: deprecated }
  - requires: { if: fm.released, then: changelog }
```

**Matchers.** `match` is exact text by default. `/.../` is a regex anchored
to the whole heading (RE2 dialect: no backreferences or lookaround), a
string containing `*` is a glob, and a bare `*` matches any heading. Matching
is case-insensitive and ignores inline markup unless `options` say otherwise.

**Cardinality.** An exact matcher is required exactly once unless you say
otherwise: `required: false` makes it `0..1`, and `repeat: "min..max"` sets
explicit bounds with `n` for unbounded. Glob, regex, and `*` rules must spell
`required` or `repeat`, so a collection never gets a silent default.

**Scopes and order.** A rule's `sections` describes the headings one level
deeper, and each parent heading binds its own scope: two `## API: …` sections
each get their own optional `### Errors`. A declared list is exhaustive and
ordered by default: unlisted headings are `unexpected-section`, and a listed
heading in the wrong place is `misplaced-section`. Use `extras: anywhere` to
admit unlisted headings without validating them, `forbid_sections` to reject
specific headings outright, and `unordered: true` on a scope whose sections
may come in any order:

```yaml
version: 1
title: "*"
unordered: true                # any order
extras: anywhere               # other h2s are fine
sections:
  - match: "Overview"
  - match: "Installation"
  - match: "Usage"
```

**Preamble content and list items.** `content` is an ordered grammar over
the blocks that sit directly under a heading, before its first subheading:
`block: p`, `block: list` (optionally `list_kind: bullet` or `ordered`),
`block: any`, or a `one_of` of those. A `block: list` rule may add `items`, a
grammar over the list's direct items matched by their text, as the ADR
schema above does for consequences. Omitting `content` or `items` leaves that
level unvalidated; an empty list demands emptiness.

**Typed values and order.** A regex rule can declare its named groups as
typed `captures` (`int`, `bool`, `date`, `semver`, `dotted`, `text`), and
`frontmatter.captures` exports typed values from frontmatter by JSONPath.
Values are parsed, never coerced: an unquoted `version: 1.2` is a YAML float,
not a semver. A rule's `order` sorts its own repeated matches by a capture,
`asc` or `desc`, with `strict: true` also forbidding duplicates; each
violating adjacent pair is one `order-violation`.

**Frontmatter.** `frontmatter.required` and `allow` set presence policy;
`schema` delegates the shape of the mapping to a JSON Schema, inline or in a
file next to the outlint schema (local `$ref` graphs are followed, never the
network). Schema violations are reported as `frontmatter-schema` with the
JSON Pointer of the offending value:

```text
retired.md:3:1 [frontmatter-schema] "retired" is not one of "draft", "published" or "deprecated"
  frontmatter: lines 1-4
  value: "/status"
  schema: .outlint.yml:5:5

1 diagnostic in 1 file
```

**Constraints.** `one_of`, `any_of`, `at_most_one`, `all_or_none`,
`requires`, `conflicts`, and `ordered` relate sections to each other and to
frontmatter, at the root or inside any rule. Operands are locators: a rule
id (`deprecated`), a dotted path (`overview.goals`, or `$.overview.goals` from
the root), `fm[<JSONPath>]` as a boolean read of the frontmatter or
`fm[<JSONPath>]=literal` as an equality test, and `fm.<name>` for a declared
frontmatter capture. Locators are checked when the schema loads, so a typo
is an `unresolved-ref` error rather than a silently false test:

```text
legacy-api.md:1:1 [requires] a required consequence is missing
  references:
    - fm[$.status]=deprecated
    - deprecated (exact "Deprecated")
  constraint: .outlint.yml:20:5

1 diagnostic in 1 file
```

**Several h1s.** `title:` + `sections:` is sugar for a document with one
h1. A multi-part document uses the general form, `outline:`, a list of the
same rule objects describing the h1s themselves; every part binds its own
obligations:

```yaml
version: 1
outline:                       # several h1s: rules for the h1s themselves
  - match: "Introduction"
  - match: "Part *"
    repeat: "1..n"
    sections:
      - match: "Overview"      # required in every part separately
      - match: "*"
        repeat: "0..n"
```

`title: null` declares a document with no h1 at all.

**Suppressions.** `<!-- outlint-disable <id>,... -->` on the line before a
heading, block, or list item suppresses those diagnostics for that node;
`<!-- outlint-disable-file <id>,... -->` suppresses them for the whole file.
Missing-section findings have no node to attach to and need the file-wide
form. Schema errors are never suppressible.

For the full inventory of diagnostic ids, the validation algorithm, worked
examples, and authoring advice, see the specification's
[§6](spec/outlint-spec.md#6-diagnostics),
[§9](spec/outlint-spec.md#9-complete-examples), and
[§10](spec/outlint-spec.md#10-examples-and-authoring-guidance-non-normative).

## Extensions (experimental)

The validator is the product: `check` and `schema check`, the schema
language, and the diagnostics are what the specification defines and what
the stability promises below cover. Extensions are optional commands layered
on the validator's document model. They are experimental, outside those
promises, shipped only in the full edition and behind Cargo features of the
`outlint` crate, and the set is expected to grow. Two exist today.

Both are aimed at people and AI agents working in large Markdown corpora:
find the one block that matters, then fetch exactly that section by address
instead of reading whole files. An agent can locate a paragraph in a 164 kB
specification and read 249 bytes. Both share `--format human|json|compact`;
the `OUTLINT_FORMAT` environment variable sets the default, so an agent
harness can export `OUTLINT_FORMAT=compact` once.

### search

`outlint search <WORD>...` indexes the Markdown files under the current
directory (or `--root <DIR>`) and prints the blocks that contain every word,
best match first. Each hit is a file path, a document path addressing the
block, and its size, followed by a snippet:

```sh
outlint search exit status operational error
```

```text
docs/outlint-spec.md $.11-command-line-interface.11-5-exit-status/p[1] 249B (section 1.5kB)
  The command uses three exit statuses: | Code | Meaning | | ---: | --- | | `0` | Every checked document and schema is valid. | | `1` | Validation completed and…

docs/outlint-spec.md $.11-command-line-interface.11-5-exit-status/p[2] 511B (section 1.5kB)
  …makes the invocation exit with status 2.

docs/outlint-spec.md $.11-command-line-interface.11-2-document-checking-and-schema-selection/p[2] 886B (section 2.4kB)
  …operational error, exit status 2) or skipped behind an invalid schema result. An absent result is never a pass.

```

Text is matched after English stemming, a block must contain all the words,
and a heading chain that also contains them ranks the block higher. The
`--limit <N>` option prints up to 1000 hits (default 10); when more match, a
note says so. With no hits the command exits `1` and reports how many blocks
contain each word, so the query can be loosened.

The index lives in `.outlint/search/` at the root of the enclosing Git
repository (or in the search root when there is none), is created with its
own `.gitignore`, and is refreshed on every search. The walk honours ignore
files and skips hidden entries and symlinks.

The compact format is one tab-separated line per hit; JSON is a single
object with a `version`, the `query`, the `total`, and the `hits`:

```sh
outlint search --limit 2 --format compact schema discovery
```

```text
docs/outlint-spec.md	$.11-command-line-interface.11-2-document-checking-and-schema-selection/p[0]	1.2kB/2.4kB	`outlint check` validates each named Markdown document. With `--schema <SCHEMA>`, that schema is used for every document and automatic discovery is disabled.…
docs/outlint-spec.md	$.11-command-line-interface.11-2-document-checking-and-schema-selection/p[2]	886B/2.4kB	…discovery makes multiple documents depend on the same invalid schema path, its errors are reported once, at the position of the first dependent input. Other…
```

### read

`outlint read <FILE> [MDPATH]` prints the node of a Markdown file that a
document path addresses — the whole file (`$`, the default), a section, one
paragraph or list, or one list item. `--tree` lists the structure under a
node instead, with the size of every part, which is the cheapest way to
decide what is worth reading:

```sh
outlint read --tree --depth 1 docs/outlint-spec.md
```

```text
$                                                   164.4kB  Outlint Schema — Specification
$.1-document-model                                  14.2kB  1. Document model
$.2-schema-format                                   36.3kB  2. Schema format
$.3-matching-semantics                              20.0kB  3. Matching semantics
$.4-names-and-locators                              20.4kB  4. Names and locators
$.5-constraints                                     5.7kB  5. Constraints
$.6-diagnostics                                     23.4kB  6. Diagnostics
$.7-options                                         554B  7. Options
$.8-validation-algorithm-normative-reference        11.1kB  8. Validation algorithm (normative reference)
$.9-complete-examples                               4.5kB  9. Complete examples
$.10-examples-and-authoring-guidance-non-normative  9.2kB  10. Examples and authoring guidance (non-normative)
$.11-command-line-interface                         18.1kB  11. Command-line interface
```

A document path starts at `$` and descends through heading slugs (`.`), or
anywhere below with `..`; a trailing `/p[i]`, `/list[i]`, or
`/list[i]/item[j]` selects a block or item, zero-based. A sole h1 is the
document itself, so its `##` sections are `$.something`. The leading `$` may
be left off on the command line. `--tree --blocks` also lists blocks and list
items, and `--depth N` limits how many levels of subsections are included.
Reading a node prints its Markdown source as written:

```sh
outlint read adr/0008-use-postgres.md .decision-outcome.consequences
```

```text
### Consequences

- Good, because the team already operates PostgreSQL.
- Bad, because we need to provision a new cluster.
```

Search hit paths are exactly what `read` takes, so the two compose: search
for a phrase, then read the one block or section it landed in. In JSON the
content comes back as a string alongside its address and size:

```sh
outlint read --format json adr/0008-use-postgres.md .considered-options
```

```json
{"version":1,"file":"adr/0008-use-postgres.md","mdpath":"$.considered-options","bytes":47,"content":"## Considered Options\n\n- PostgreSQL\n- DynamoDB\n"}
```

A path that does not resolve exits `1` and lists the sections that are
available, so the next attempt can be exact. Neither extension is part of
the schema specification; their output envelopes are version `1` and may
change before 1.0.

## CI and automation

**Exit codes** for `check` and `schema check`:

| Code | Meaning |
| ---: | --- |
| `0` | All checked documents and schemas are valid |
| `1` | Validation completed and at least one diagnostic was emitted |
| `2` | A usage or operational failure prevented normal validation |

Status 2 takes precedence when both happen. A document that could not be
checked has no result at all, never an empty one that reads as a pass.

**JSON output.** `--format json` writes one object for the whole invocation
to stdout, never colorized, with a `version` (currently `2`), a result for
each checked document or schema, and a `summary`. Each diagnostic has a stable `id`, a `location`, a
`message`, the `schema_location` and `schema_node` that produced it, and a
`target` that says what it is about, tagged by `kind` (`header`,
`missing_header`, `document`, `frontmatter`, `block`, `missing_block`,
`item`, `missing_item`):

```sh
outlint check 0007-use-postgres.md --format json | jq '.results[0].diagnostics[0]'
```

```json
{
  "id": "missing-section",
  "location": {
    "column": 1,
    "line": 1
  },
  "message": "matched 0 sections, but at least 1 are required",
  "schema_location": {
    "column": 5,
    "line": 10,
    "path": ".outlint.yml"
  },
  "schema_node": {
    "index": 2,
    "kind": "rule",
    "scope": []
  },
  "target": {
    "kind": "missing_header",
    "matcher": "Considered Options",
    "parent": []
  }
}
```

Diagnostics within a result are in a fixed total order, independent of how
validation discovered them. Before 1.0 the shape may change under the same
`version` number, so pin the outlint release you parse. The full shape is in
the specification's [§11.3](spec/outlint-spec.md#113-formats-streams-and-json-data).

**GitHub Actions.** The npm package needs no Rust toolchain (Node 18 or
newer) and downloads the check-only binary on first use. Pin the version so
every run uses the same release:

```yaml
name: docs
on: [push, pull_request]
jobs:
  outlint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - run: npx --yes @outlint/cli@0.1.0 schema check .outlint.yml
      - run: find docs -name '*.md' -print0 | xargs -0 npx --yes @outlint/cli@0.1.0 check
```

`xargs` reports any failing `outlint check` as exit status 123, so the step
fails on diagnostics and on errors alike; the log shows which.

With a Rust toolchain on the runner, `cargo install outlint --locked` (or the
release installer script from the release page) works the same way and the
full edition is `cargo install outlint --locked --all-features`.

**Editors and agents.** There is no editor plugin or language server. The
integration surface is the CLI: exit codes and `--format json`. An agent
harness that can run shell commands can use it as is; the
[extensions](#extensions-experimental) add agent-oriented navigation on the
same footing.

## Library

[`outlint-core`](crates/outlint-core/README.md) is the pure, IO-free engine
the CLI is built on: it turns schema text and Markdown text into normalized
values and diagnostics and never touches the filesystem, the network, or the
terminal. Load a schema once, compile it into a `PreparedValidator`, and
validate any number of parsed documents; the crate README has a complete
example and explains how to supply linked JSON Schema resources from your own
IO layer. [`outlint-search`](crates/outlint-search) is the indexing library
behind the experimental `search` extension.

## Repository layout

- `crates/outlint-core` — schema model, Markdown outline parser, validator,
  document paths ([README](crates/outlint-core/README.md))
- `crates/outlint-cli` — the `outlint` command-line tool
  ([README](crates/outlint-cli/README.md))
- `crates/outlint-search` — full-text indexing backing the `search`
  extension
- `spec/` — the normative [Outlint specification](spec/outlint-spec.md)
- `testdata/` — the conformance corpus, reusable by other implementations
  ([README](testdata/README.md))
- `npm/` — the `@outlint/cli` package

## Status and stability

Outlint is at version 0.1.x. The validator is the stable focus of the
project, but this is a 0.x release: expect breaking changes to the schema
language, the diagnostic set, the JSON shape, and the library API before
1.0. Schema files keep `version: 1` and the JSON envelope keeps `version: 2`
across those changes; the changelog records them and the release version is
what to pin.

Where the implementation and the specification disagree, the specification
in `spec/` is the normative reference and the implementation is the bug. The
conformance corpus in `testdata/` runs in CI.

[Extensions](#extensions-experimental) are experimental and sit outside
these promises: an extension, its options, its output envelope, and the
document path syntax it uses may change or be removed independently of the
validator.

**MSRV:** Rust 1.88. Raising it is a minor-version change; a patch release
never requires a newer toolchain, and the MSRV is tested in CI.

Outlint is developed with AI coding agents under human design,
specification, and review: the maintainer owns the design and the
specification and reviews every change, and commits an agent co-authored say
so in a `Co-Authored-By` trailer. Every change is held to the same bar
regardless of who wrote it. Contributors may use the same tools under the
same conditions; see [CONTRIBUTING.md](CONTRIBUTING.md#ai-assisted-contributions).

## Contributing

Bug reports, specification questions, and pull requests are welcome at
<https://github.com/shangaslammi/outlint>. Please open an issue before a
large change. A change to specified behavior updates the specification and
adds a `testdata/` fixture in the same pull request; a fix that brings the
code in line with the existing specification needs the fixture and a
citation, not a specification edit. [CONTRIBUTING.md](CONTRIBUTING.md) has
the verification commands, the design conventions, and the two rules that
are easy to miss.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in this work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
