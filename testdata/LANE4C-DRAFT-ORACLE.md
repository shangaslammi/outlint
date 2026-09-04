# Lane 4C draft oracle (staging only)

This file is **staging scaffolding**, not part of the corpus contract. It is
written from `spec/outlint-spec.md` alone, before any fixture is executed, so
that the `expected.json` files generated later have something independent to
be reviewed against. It is deleted in the activation commit; the durable parts
of its guidance move into `testdata/README.md`.

The draft documents and schemas it describes are staged as flat
`lane4c-draft--<group>--<file>` files directly under `testdata/`. The
conformance runner enumerates only immediate **child directories** of
`testdata/`, so nothing here is discovered or executed while it sits at the
root. That is deliberate: the documents are preserved and reviewable, but no
conformance claim is made before the Typed Values validator and the version 3
CLI envelope exist.

## Target abbreviations

Used **only** in this file. Every committed `expected.json` spells targets out
in full and carries exactly `id` and `target`.

```text
D              {"kind":"document"}
H("A","B")     {"kind":"header","path":["A","B"]}
F(8,"/count")  {"kind":"frontmatter",
                 "line_range":{"start_line":1,"end_line":8},
                 "pointer":"/count"}
F(6,no-pointer) frontmatter target with line_range 1..6 and no `pointer` member
F(absent)      {"kind":"frontmatter"} — no line_range, no pointer
```

Anchors are recorded as `line:column`, one-based line and one-based **byte**
column (spec 6.2). Anchors are not part of the portable expectation; they are
recorded so the raw version 3 records can be hand-reviewed at activation.

## Conventions shared by every group

- `title: null`, so the document has no h1 and the root stands at level 1
  (1.5): top-level `##` headers are the children of the `sections` scope and
  `###` headers below them skip no level.
- `options.ordered_sections: false`, so no scope is ordered (3.7) and no
  `ordered` diagnostic can arise. What each group tests — capture parsing,
  `order`-entry value ordering, locators, frontmatter propositions — is then
  isolated from across-rule list order.
- `options.match_case` keeps its `false` default throughout.
- Under the sugar's single-h1 document voice (6.2), a constraint declared at
  the schema root targets `document` and anchors at line 1, because no header
  owns the root scope.

---

## Group `typed-rule-captures`

Schema: four regex rules with one declared capture each — `release`/`version`
`semver`, `count`/`value` `int`, `flag`/`value` `bool`, `label`/`value` `text`.
All four take the `0..n` cardinality default, so no cardinality diagnostic can
arise in either document. Each capture name lives in the scope its own rule
opens (4.3), so the three separate `value` captures do not collide.

### `pass.md` — expected `[]`

| Header | Rule | Captured source | Spec verdict |
|---|---|---|---|
| `Release 1.0.0-rc.1` | `release` | `1.0.0-rc.1` | valid SemVer 2.0.0, pre-release, no `+` build metadata (2.4) |
| `Count -01` | `count` | `-01` | `int` header form `-?[0-9]+`, leading zeros allowed; equal to `-1` (2.4 table) |
| `Flag true` | `flag` | `true` | `bool` header form is exactly `true` or `false` |
| `Label Café` | `label` | `Café` | `text` accepts any string and preserves the source unchanged |

No rule is `allow: false`, no scope is closed, so the unmatched-header and
`not-allowed` paths are not reachable here.

### `multiple-invalid.md` — expected 4 diagnostics

Sequence of headers, in document order, with matched rule by first-match-wins
(3.2):

1. `Release nope` → `release`, capture `nope`
2. `Release nope` → `release`, capture `nope`
3. `Count 9223372036854775808` → `count`, capture `9223372036854775808`
4. `Flag TRUE` → `flag`, capture `TRUE`

| # | id | target | multiplicity | anchor | spec |
|---|---|---|---|---|---|
| 1 | `invalid-value` | H("Release nope") | **2** | line 1 col 1 and line 3 col 1 | `nope` fails the SemVer lexical requirement (2.4) |
| 2 | `invalid-value` | H("Count 9223372036854775808") | 1 | line 5 col 1 | signed-64-bit bound exceeded; 2.4 consequences table names this exact literal |
| 3 | `invalid-value` | H("Flag TRUE") | 1 | line 7 col 1 | `match_case: false` lets the regex match, but the captured source is the case-preserving `TRUE`, and the header `bool` spelling is lowercase-only (2.4 table: header `bool` `True` → `invalid-value`) |

**Multiplicity note.** Both `Release nope` headers are siblings with the same
visible text, so their header paths are byte-identical and the two entries are
indistinguishable. Two entries therefore require exactly two diagnostics
(README "Comparison"): one or three is a failure. This is the point of the
duplicated header.

Total: exactly 4 entries. No dependent diagnostic exists to suppress — the
rules declare no `order`, and no constraint reads these captures.

---

## Group `typed-frontmatter-captures`

Five frontmatter captures: `version` (`$.release.version`, `semver`,
required), `count` (default path `$['count']`, `int`, required), `draft`
(`bool`, optional), `label` (`text`, optional), `last_item` (`$.items[-1]`,
`text`, required).

Capture evaluation follows 2.3: no result node or one null result node is
**absent** — `missing-value` exactly when `required: true`; one non-null
scalar of the required YAML kind is parsed per 2.4; a scalar of another kind,
a mapping, or a sequence is `invalid-value`.

### `pass.md` (block lines 1–8) — expected `[]`

| Capture | Query result | Spec verdict |
|---|---|---|
| `version` | string `1.0.0-rc.1` | YAML string kind required by `semver`; valid |
| `count` | integer `2` | YAML integer kind required by `int`; valid |
| `draft` | `True` | 2.4: frontmatter `bool` written `True` is valid when the YAML core resolver yields boolean true |
| `label` | string `42` | quoted, so the YAML kind is string; `text` preserves it verbatim |
| `last_item` | string `tail` | `$.items[-1]` counts back from the array end (4.6) |

### `multiple-invalid.md` (block lines 1–8) — expected 4 diagnostics

All four are kind failures, not coercions ("No implicit coercion occurs", 2.4).

| id | target | multiplicity | anchor | spec |
|---|---|---|---|---|
| `invalid-value` | F(8,"/release/version") | 1 | line 3 col 3 (the `version` key) | unquoted `1.2` is a YAML float; `semver` accepts only a YAML string (2.4, which names this exact mistake) |
| `invalid-value` | F(8,"/count") | 1 | line 4 col 1 | `"2"` is a YAML string; `int` accepts only a YAML integer |
| `invalid-value` | F(8,"/draft") | 1 | line 5 col 1 | `"true"` is a YAML string; `bool` accepts only a YAML boolean |
| `invalid-value` | F(8,"/label") | 1 | line 6 col 1 | `42` is a YAML integer; 2.4 table: frontmatter `text` whose YAML kind is integer is `invalid-value`, not coercion |

`last_item` resolves to `tail` and is valid, so it contributes nothing.
Anchors are the failing entry, named by its key for a mapping member (6.2).

### `missing-values.md` (block lines 1–5) — expected 2 diagnostics

| id | target | multiplicity | anchor | spec |
|---|---|---|---|---|
| `missing-value` | F(5,"/release/version") | 1 | line 2 col 1 (`release` key) | `release` is `{}`, so the singular query selects no node: absence, and the capture is required |
| `missing-value` | F(5,"/count") | 1 | line 3 col 1 | one **null** result node is absence (2.3), not a value of the wrong kind, so this is `missing-value` and not `invalid-value` |

Anchors follow 6.1: the pointer names the intended absent path, while the
anchor is the **deepest resolving positioned ancestor** of that path — the
`release` key for `/release/version`, and `/count`'s own written entry for
`/count`.

Suppressed/absent: `draft` and `label` are optional and absent, which 2.3 calls
valid and unbound — no diagnostic. `last_item` resolves to `tail`.

### `missing-negative-index.md` (block lines 1–6) — expected 1 diagnostic

| id | target | multiplicity | anchor | spec |
|---|---|---|---|---|
| `missing-value` | F(6,**no-pointer**) | 1 | line 5 col 1 (`items` key) | `items` is `[]`, so `$.items[-1]` selects nothing; 6.1 permits omitting `missing-value.pointer` exactly when no normalized absent path exists, and names "a negative index into an empty sequence" as the case |

**Pointer omission is load-bearing.** 6.1: absence of `pointer` and
`"pointer": ""` are different — `""` is the root pointer naming the mapping
itself, while no member at all means the diagnostic is about no value in the
block. An implementation MUST omit rather than emit `null`.

Anchor: `items` is the deepest resolving positioned ancestor of the addressed
path, so line 5 col 1; the block's first line is the floor 6.2 guarantees and
is acceptable only where no positioned ancestor exists.

`version` and `count` are valid here, so this document isolates the
pointer-omission case.

### `no-frontmatter.md` — expected `[]`

`frontmatter.required` is left at its `false` default, so an absent block is
not `missing-frontmatter`. 2.3: "When the document has no frontmatter block …
captures are not evaluated and produce neither `missing-value` nor
`invalid-value`." This holds **even though `version`, `count`, and
`last_item` are each declared `required: true`** — `required` on a capture
speaks about a block that exists.

---

## Group `typed-value-boundaries`

Six optional frontmatter captures — `$.int`, `$.bool`, `$.date`, `$.semver`,
`$.dotted`, `$.text` — and six regex rules `/Int (?<value>.+)/` and so on, one
per type. Every rule takes the `0..n` default and no capture is required, so
the only diagnostics reachable are `invalid-value`.

### `header-valid.md` — expected `[]`

| Header | Type | Why valid (2.4) |
|---|---|---|
| `Int -9223372036854775808` | `int` | exactly the signed-64-bit minimum |
| `Int 9223372036854775807` | `int` | exactly the signed-64-bit maximum |
| `Bool false` | `bool` | lowercase header spelling |
| `Date 0000-01-01` | `date` | years `0000`–`9999` are valid; `0000` is ISO 8601 astronomical year numbering |
| `Date 2024-02-29` | `date` | 2024 is a proleptic-Gregorian leap year |
| `Semver 1.0.0-rc.1` | `semver` | pre-release without build metadata |
| `Semver 18446744073709551615.0.0` | `semver` | major is exactly the unsigned-64-bit maximum |
| `Dotted 1.02.4294967295` | `dotted` | leading zeros allowed; last component is exactly the unsigned-32-bit maximum |
| `Text anything at all` | `text` | any string |

### `header-invalid.md` — expected exactly 8 diagnostics

One `invalid-value` per header, each targeting that header; all eight header
texts are distinct, so all eight entries are distinct. Anchor is each header's
own line, column 1.

| Header | Spec paragraph |
|---|---|
| `Int 9223372036854775808` | signed-64-bit maximum plus one; 2.4 consequences table |
| `Int -9223372036854775809` | signed-64-bit minimum minus one |
| `Bool True` | header `bool` is lowercase-only; 2.4 consequences table |
| `Date 2023-02-29` | 2023 is not a leap year; 2.4 consequences table |
| `Semver 1.0.0+build` | `+` build metadata is rejected, and the message must identify that suffix |
| `Semver 18446744073709551616.0.0` | major exceeds the unsigned-64-bit bound |
| `Semver 1.0.0-18446744073709551616` | the bound applies to **numeric pre-release identifiers** as well as major/minor/patch (2.4) |
| `Dotted 4294967296` | component exceeds the unsigned-32-bit bound; 2.4 consequences table |

### `frontmatter-valid.md` (block lines 1–8) — expected `[]`

`int: -9223372036854775808` (YAML integer, signed-64-bit minimum);
`bool: True` (YAML core resolver yields boolean true); `date`, `semver`,
`dotted`, `text` all written as **quoted** YAML strings, which is the kind
2.4 requires for those four types. `text: "7"` is quoted, so its YAML kind is
string and not integer.

### `frontmatter-no-coercion.md` (block lines 1–8) — expected exactly 6

Every entry is the right *spelling* for its type and the wrong YAML *kind*.
2.4: "No implicit coercion occurs."

| id | target | anchor | why |
|---|---|---|---|
| `invalid-value` | F(8,"/int") | line 2 col 1 | `"1"` is a string; `int` needs a YAML integer |
| `invalid-value` | F(8,"/bool") | line 3 col 1 | `"true"` is a string; `bool` needs a YAML boolean |
| `invalid-value` | F(8,"/date") | line 4 col 1 | `20230229` is an integer; `date` needs a YAML string |
| `invalid-value` | F(8,"/semver") | line 5 col 1 | unquoted `1.2` is a float; `semver` needs a YAML string |
| `invalid-value` | F(8,"/dotted") | line 6 col 1 | unquoted `1.2` is a float; `dotted` needs a YAML string |
| `invalid-value` | F(8,"/text") | line 7 col 1 | `7` is an integer; `text` needs a YAML string |

### `frontmatter-invalid-values.md` (block lines 1–8) — expected exactly 4

Here the YAML **kind** is right everywhere and four values fail a lexical,
calendar, SemVer, or bound requirement instead. `bool: false` and `text: "ok"`
are valid and contribute nothing, which is what makes the count exactly four.

| id | target | anchor | why |
|---|---|---|---|
| `invalid-value` | F(8,"/int") | line 2 col 1 | YAML integer of the right kind, outside the signed-64-bit bound; 1.6 requires the exact mathematical value be preserved, and 2.4 requires it be rejected anyway |
| `invalid-value` | F(8,"/date") | line 4 col 1 | string `2023-02-29`; not a valid proleptic-Gregorian date |
| `invalid-value` | F(8,"/semver") | line 5 col 1 | string `1.0.0+build`; build metadata rejected |
| `invalid-value` | F(8,"/dotted") | line 6 col 1 | string `4294967296`; component exceeds the unsigned-32-bit bound |

---

## Group `typed-order`

Eight rules, each with one `order` entry except `series`, which carries the
ordered `point` rule in its child scope:

| Rule | Capture type | `dir` | `strict` | Cardinality |
|---|---|---|---|---|
| `number` | `int` | asc (default) | false (default) | `0..n` |
| `flag` | `bool` | asc | false | `0..n` |
| `date` | `date` | asc | false | `0..n` |
| `version` | `semver` | **desc** | false | `0..n` |
| `dotted` | `dotted` | asc | **true** | `0..n` |
| `text` | `text` | asc | false | `0..n` |
| `bounded` | `int` | asc | false | **`0..2`** |
| `series` → `point` | `int` | asc | false | `0..n` each |

3.8 requires every ordered rule's effective maximum to exceed one, which all
eight satisfy; `bounded`'s `0..2` is the tightest bound that stays legal and is
chosen so `too-many-sections` and value ordering can be observed together.

### `pass-interleaved.md` — expected `[]`

Every sequence below is formed per 3.8: the headers whose first matching rule
is this rule, in document order. Headers matched by **other** rules and the
unmatched `Noise` heading do not belong to the sequence and do not break
adjacency, which is what this document exists to pin.

| Rule | Sequence (parsed values) | Adjacent comparisons |
|---|---|---|
| `number` | `-01`→−1, `-1`→−1, `0`→0 | −1 ≤ −1 ✓ (typed equality: 2.4 makes `-01` equal `-1`), −1 ≤ 0 ✓ |
| `flag` | false, true | false ≤ true ✓ (2.4: `false < true`) |
| `date` | 2024-02-29, 2024-03-01 | chronological ✓ |
| `version` | 2.0.0, 1.0.0 | descending needs A ≥ B: 2.0.0 ≥ 1.0.0 ✓ |
| `dotted` | 1.2, 1.2.0 | strict needs A < B: 2.4 says an equal prefix sorts first, so 1.2 < 1.2.0 ✓ |
| `text` | `B`, `a` | Unicode code-point order, unaffected by `match_case`: U+0042 ≤ U+0061 ✓ |
| `bounded` | 1, 2 | 1 ≤ 2 ✓; count 2 is within `0..2`, so no `too-many-sections` |
| `point` in `Series A` | 1, 2 | 1 ≤ 2 ✓ |

`Noise` matches no rule; the root scope is open (no `strict: true`), so it is
legal and produces no `unexpected-section`.

### `multiple-adjacent.md` — expected exactly 2 diagnostics

`number` sequence: 3, 2, 1 — `Noise` (unmatched) and `Flag true` (a different
rule) sit between occurrences and are not in the sequence.

| Pair | Comparison | Result |
|---|---|---|
| (3, 2) | 3 ≤ 2 fails | `order-violation` at the pair's **second** header |
| (2, 1) | 2 ≤ 1 fails | `order-violation` at the pair's second header |

| id | target | multiplicity | anchor |
|---|---|---|---|
| `order-violation` | H("Number 2") | 1 | line 5 col 1 |
| `order-violation` | H("Number 1") | 1 | line 9 col 1 |

3.8: "One misplaced value can therefore produce two diagnostics." `Flag true`
is a single occurrence, so its entry forms no pair.

### `typed-comparators.md` — expected exactly 4 diagnostics

One violating pair per comparator, each targeting the pair's second header.

| Rule | Sequence | Comparison | Diagnostic |
|---|---|---|---|
| `flag` | true, false | asc needs true ≤ false; `false < true` (2.4) so it fails | `order-violation` → H("Flag false") |
| `date` | 2024-03-01, 2024-02-29 | asc fails chronologically | `order-violation` → H("Date 2024-02-29") |
| `version` | 1.0.0-rc.1, 1.0.0 | desc needs A ≥ B; SemVer precedence puts a pre-release **before** its release, so it fails | `order-violation` → H("Version 1.0.0") |
| `text` | `a`, `B` | asc needs U+0061 ≤ U+0042; fails, and `match_case: false` does not fold it (2.4) | `order-violation` → H("Text B") |

Anchors: lines 3, 7, 11, 15 respectively, column 1.

### `strict-typed-equality.md` — expected exactly 1 diagnostic

`dotted` sequence: `1.02`, `1.2`. 2.4: a `dotted` compares its components
numerically, so `1.02` **equals** `1.2`. The entry declares `strict: true`,
which 3.8 says replaces `≤` with `<` and therefore "also requires uniqueness
under typed equality", naming this exact adjacent spelling pair.

| id | target | multiplicity | anchor |
|---|---|---|---|
| `order-violation` | H("Dotted 1.2") | 1 | line 3 col 1 |

Neither header is `invalid-value`: both spellings parse.

### `per-ancestor.md` — expected exactly 1 diagnostic

3.8: "When an ancestor repeats, each concrete ancestor instance supplies a
separate sequence; occurrences are never flattened across instances." The
`series` rule matches twice, opening two child scopes (3.1).

| Scope | `point` sequence | Comparisons |
|---|---|---|
| `Series A` | 2, 1 | 2 ≤ 1 fails |
| `Series B` | 1, 2 | 1 ≤ 2 ✓ |

| id | target | multiplicity | anchor |
|---|---|---|---|
| `order-violation` | H("Series A","Point 1") | 1 | line 5 col 1 |

A flattening implementation would additionally compare `Series A`'s last point
(1) with `Series B`'s first (1) — no violation — and, worse, would compare 1
with 2 across the boundary; the asymmetric sequences here make any flattening
visible as a wrong count or a wrong target path.

### `excess-still-ordered.md` — expected exactly 3 diagnostics

`bounded` is `0..2` and matches three headers: 3, 2, 1.

| id | target | multiplicity | anchor | spec |
|---|---|---|---|---|
| `order-violation` | H("Bounded 2") | 1 | line 3 col 1 | pair (3, 2) fails |
| `too-many-sections` | H("Bounded 1") | 1 | line 5 col 1 | 3.5/6.2: the target is the **first header in excess** of the bound, i.e. the third occurrence |
| `order-violation` | H("Bounded 1") | 1 | line 5 col 1 | pair (2, 1) fails |

**Why the third occurrence still orders.** 3.8: "Headers beyond the rule's
cardinality maximum remain in it, so `too-many-sections` does not suppress
value ordering." 4.4 makes the same point explicitly — value ordering does not
depend on the cardinality bound holding, unlike a locator descent. The two
entries targeting `H("Bounded 1")` carry different ids, so they are distinct
entries, not a multiplicity.

---

## Group `typed-order-suppression`

One repeatable `group` rule whose child `item` rule declares **two** captures
from one match — `version` (`semver`) and `rank` (`int`) — and **two**
independent ascending `order` entries, one per capture. Both named groups are
mandatory-participation under 2.2: neither sits under an alternation nor under
a repetition whose minimum is zero.

### `one-entry-suppressed.md` — expected exactly 3 diagnostics

`Group A`'s `item` sequence and its two parsed captures:

| # | Header | `version` | `rank` |
|---|---|---|---|
| 1 | `Item 1.0.0 rank=3` | 1.0.0 ✓ | 3 ✓ |
| 2 | `Item bad rank=2` | `bad` **invalid** | 2 ✓ |
| 3 | `Item 2.0.0 rank=1` | 2.0.0 ✓ | 1 ✓ |

| id | target | multiplicity | anchor | reasoning |
|---|---|---|---|---|
| `invalid-value` | H("Group A","Item bad rank=2") | 1 | line 5 col 1 | `bad` fails the SemVer requirement; the primary diagnostic always stands (3.8: "primary `invalid-value` diagnostics are unaffected") |
| `order-violation` | H("Group A","Item bad rank=2") | 1 | line 5 col 1 | from the **`rank`** entry, pair (3, 2) |
| `order-violation` | H("Group A","Item 2.0.0 rank=1") | 1 | line 7 col 1 | from the **`rank`** entry, pair (2, 1) |

**`version` entry: fully suppressed.** 3.8: "If any selected capture in a
sequence is invalid, the corresponding order entry produces no
`order-violation` in that scope … Skipping only the invalid element would
invent an adjacency." The spec's worked example is exactly this shape and
states the implementation must **not** compare the first and third values as
if adjacent. Here that would be 1.0.0 against 2.0.0 — ascending and therefore
silently passing, which is why the versions are arranged so a flattening
implementation produces *no* extra diagnostic rather than an obvious one; the
case is pinned by the count being exactly three.

**`rank` entry: not suppressed.** Suppression is per order entry and per
scope. Every `rank` value parses, so the entry evaluates normally and both
adjacent pairs violate.

### `scope-isolation.md` — expected exactly 2 diagnostics

| Scope | `version` sequence | `rank` sequence |
|---|---|---|
| `Group A` | `bad` (invalid), 1.0.0 | 1, 2 |
| `Group B` | 2.0.0, 1.0.0 | 1, 2 |

| id | target | multiplicity | anchor | reasoning |
|---|---|---|---|---|
| `invalid-value` | H("Group A","Item bad rank=1") | 1 | line 3 col 1 | primary diagnostic for `bad` |
| `order-violation` | H("Group B","Item 1.0.0 rank=2") | 1 | line 11 col 1 | `version` entry in `Group B`: pair (2.0.0, 1.0.0) fails ascending |

`Group A`'s `version` entry is suppressed in that scope only; its ranks 1, 2
ascend, so it contributes nothing further. `Group B` has no invalid capture, so
its `version` entry evaluates — the point of the fixture is that suppression is
scoped to the concrete parent instance, not to the rule or the document. Its
ranks 1, 2 ascend and contribute nothing.

### `disabled-inline.md` — expected `[]`

`Group A`'s items: `2.0.0 rank=1`, `bad rank=2`, `1.0.0 rank=3`, with
`<!-- outlint-disable invalid-value -->` on the line immediately preceding the
invalid item's header.

| Diagnostic that would arise | Fate |
|---|---|
| `invalid-value` on `Item bad rank=2` | anchored to that header, so the per-header comment of 6.3 filters it |
| `order-violation` from the `version` entry | **never produced**: 3.8 computes the dependency from typed validity *before* `outlint-disable` filtering |
| `order-violation` from the `rank` entry | none: ranks 1, 2, 3 ascend |

**What this fixture forbids.** If an implementation filtered `invalid-value`
first and then decided suppression, the `version` sequence would read
`2.0.0`, `1.0.0` as adjacent and emit an `order-violation` — a diagnostic the
document does not deserve, produced *because* a diagnostic was hidden. 6.3
states this directly: "Suppressing `invalid-value` therefore never re-enables a
dependent `order-violation`." Expected `[]`, and exactly `[]`.

### `disabled-file.md` — expected `[]`

The same semantic case with `<!-- outlint-disable-file invalid-value -->`
instead. The file-wide form of 6.3 reaches the diagnostic wherever it is
anchored, and the dependency ordering is identical. Both forms are pinned
because they are separate filtering paths.

---

## Group `locator-positions`

Rules: `group` (`/Group .+/`, `0..n`) with optional child `ready`
(exact `Ready`, `0..1`), and root `fallback` (exact `Fallback`, `0..1`).

Constraints, all at the schema root, so every violation targets `document`
under the sugar's single-h1 document voice (6.2 — this schema declares
`title: null`, so no h1 exists to own the scope) and anchors at line 1:

| # | Constraint |
|---|---|
| C1 | `requires: { if: "group[0]", then: "group[1]" }` |
| C2 | `conflicts: { if: "group[0].ready", then_not: "group[1].ready" }` |
| C3 | `any_of: ["group[184467440737095516160]", fallback]` |

Load-time notes: `group[0]` and `group[1]` are not duplicate locators under
5.4, which compares declared rule steps **together with** their positional
subscripts; `any_of` has the two locators 5.4 requires; and each `[i]` makes
its step singular, satisfying 4.4's singular-non-terminal rule for the
`.ready` descents. 4.5: a locator ending in a rule id is satisfied iff its
terminal node list is non-empty, and "positional narrowing does not change
that definition".

### `one-group.md` — expected exactly 1 diagnostic

One `Group A` containing `Ready`; no second group; `Fallback` present.

| Constraint | `if` | `then` / `then_not` | Verdict |
|---|---|---|---|
| C1 | `group[0]` → [Group A], satisfied | `group[1]` → [], unsatisfied | **violated** |
| C2 | `group[0].ready` → [Ready], satisfied | `group[1].ready` → [] (descent from an empty list is empty), unsatisfied | satisfied — `conflicts` holds when `then_not` is unsatisfied |
| C3 | — | `group[huge]` → [] ; `fallback` → [Fallback], satisfied | satisfied — `any_of` needs at least one |

| id | target | multiplicity | anchor |
|---|---|---|---|
| `requires` | D | 1 | line 1 col 1 |

### `two-ready.md` — expected exactly 1 diagnostic

Two groups, both containing `Ready`; `Fallback` present.

| Constraint | Verdict |
|---|---|
| C1 | `group[1]` → [Group B], satisfied → constraint satisfied |
| C2 | both `if` and `then_not` satisfied → **violated** |
| C3 | `fallback` satisfied |

| id | target | multiplicity | anchor |
|---|---|---|---|
| `conflicts` | D | 1 | line 1 col 1 |

Cardinality: `ready` is `0..1` and matches once **in each** group's own scope
(3.1 binds scopes per parent), so no `too-many-sections`. `group` is `0..n`.

### `position-pass.md` — expected `[]`

Two groups; only the **second** contains `Ready`; `Fallback` present.

| Constraint | Verdict |
|---|---|
| C1 | `group[1]` non-empty → satisfied |
| C2 | `group[0].ready` → Group A has no `Ready` → [] → `if` unsatisfied → satisfied |
| C3 | `fallback` satisfied |

This is the discriminating case for position semantics: an implementation that
ignored `[0]` and descended through every group would find `Ready` under Group
B, satisfy C2's `if` and its `then_not`, and report a `conflicts` the document
does not deserve.

### `huge-out-of-range.md` — expected exactly 1 diagnostic

Neither `Group` nor `Fallback` exists; the lone `Notes` heading matches no rule
and is legal in the open root scope.

| Constraint | Verdict |
|---|---|
| C1 | `group[0]` → [] → `if` unsatisfied → satisfied |
| C2 | `if` unsatisfied → satisfied |
| C3 | `group[184467440737095516160]` → [] ; `fallback` → [] → **violated** |

| id | target | multiplicity | anchor |
|---|---|---|---|
| `any_of` | D | 1 | line 1 col 1 |

No `missing-section`: `fallback` is `0..1` and `group` is `0..n`, so a count of
zero satisfies both minima.

**Raw-record requirements for the huge index** (4.4, 11.3), to hand-review at
activation:

- the constraint's `references` entry for that locator has `kind: "rule"` and a
  `positions` array aligned with `path`, carrying `184467440737095516160` as a
  **JSON integer**, not a string and not a float;
- the index selects nothing rather than erroring — magnitude is never an error;
- evaluation is proportional to the spelling, never to the numeric value.

---

## Group `locator-cardinality-suppression`

Rules: `group` (`/Group .+/`, **`required: false`** → `0..1`, hence statically
singular) with optional child `ready`; root `notice` (`0..1`).

| # | Constraint |
|---|---|
| C1 | `requires: { if: group.ready, then: notice }` — **unnarrowed** descent |
| C2 | `requires: { if: "group[0].ready", then: notice }` — **narrowed** descent |

Both bind at load: `group`'s effective maximum is one, so the bare `group` step
is statically singular under 4.4. The two locators differ in their positional
subscripts, so they are not duplicates under 5.4 (and they sit in separate
constraints in any case). Both constraints target `document` and anchor at
line 1, for the reason given under `locator-positions`.

### `single.md` — expected exactly 2 diagnostics

One `Group A` containing `Ready`; `Notice` absent. `group` matches once, within
its `0..1` bound, so no cardinality failure and nothing is suppressed.

| Constraint | `if` | `then` | Verdict |
|---|---|---|---|
| C1 | `group.ready` → [Ready], satisfied | `notice` → [], unsatisfied | violated |
| C2 | `group[0].ready` → [Ready], satisfied | unsatisfied | violated |

| id | target | multiplicity | anchor |
|---|---|---|---|
| `requires` | D | **2** | line 1 col 1 (both) |

**Multiplicity note.** Two distinct constraints produce two byte-identical
portable entries; the portable projection keeps only `{id, target}` and does
not carry the schema location that distinguishes them. Two entries therefore
require exactly two diagnostics. The raw version 3 records must differ in
`schema_node` (`kind: "constraint"`, different `index`) and in `references`,
and that is where the distinction is reviewed.

### `multiple-first-ready.md` — expected exactly 2 diagnostics

Two groups; the **first** contains `Ready`; `Notice` absent.

| id | target | multiplicity | anchor | reasoning |
|---|---|---|---|---|
| `too-many-sections` | H("Group B") | 1 | line 5 col 1 | `group` is `0..1` and matched twice; 6.2 targets the first header in excess of the bound |
| `requires` | D | 1 | line 1 col 1 | from **C2** only |

**C1 is suppressed.** 4.4: "an unnarrowed non-terminal locator step may be
statically singular because its rule has effective maximum one. If that rule
nevertheless matches several headers in a cardinality-violating concrete
scope, `too-many-sections` stands and every constraint evaluation that depends
on descending through that step is suppressed in that scope; it emits no
constraint diagnostic." 5.3 adds that suppression applies to the whole boolean
constraint without three-valued short-circuiting.

**C2 is not.** 4.4: "A step narrowed with `[i]` does not depend on the rule's
cardinality holding and remains evaluable." `group[0]` is Group A, whose
`Ready` satisfies the `if`; `notice` is absent, so it violates.

If both constraints reported, the count would be 3; if suppression were applied
by rule rather than per locator, it would be 1. The expected count is exactly
2, and the two entries carry different ids, so they are distinct.

### `multiple-second-ready.md` — expected exactly 1 diagnostic

Two groups; only the **second** contains `Ready`; `Notice` absent.

| id | target | multiplicity | anchor |
|---|---|---|---|
| `too-many-sections` | H("Group B") | 1 | line 3 col 1 |

C1 is suppressed for the same reason as above. C2 evaluates and is
**satisfied**: `group[0]` is Group A, which has no `Ready`, so the `if` is
unsatisfied and `requires` holds vacuously. This separates position-zero
semantics from mere evaluability — the previous fixture shows C2 firing, this
one shows it correctly not firing.

### `multiple-disabled.md` — expected exactly 1 diagnostic

`multiple-first-ready.md` again, with
`<!-- outlint-disable-file too-many-sections -->` at the top of the file.

| id | target | multiplicity | anchor |
|---|---|---|---|
| `requires` | D | 1 | line 1 col 1 |

The cardinality diagnostic is filtered. C1 remains suppressed and C2 remains
evaluable: 4.4 says "This dependency is decided before the `outlint-disable`
filtering of 6.3, so hiding `too-many-sections` does not make the descent
evaluable", and 6.3 repeats it. Hiding a diagnostic must not change a verdict —
if C1 re-entered, this document would report two `requires` and the fixture
would fail on multiplicity alone.

This document has no frontmatter block, so the disable comment can sit on the
first line; the file-wide form of 6.3 applies from anywhere in the file
regardless.

---

## Group `frontmatter-jsonpath`

Six root constraints and one nested constraint, each pairing one JSONPath
proposition with an optional proof heading. Every query stays inside the 4.6
**guaranteed core**: child segments carrying exactly one name, index, or
wildcard selector. Filters, slices, descendant segments, multi-selectors, and
extension functions are vendor-tier under 4.6 and carry no conformance
guarantee, so they are excluded from the portable corpus by construction.

| # | Constraint | Form under test |
|---|---|---|
| C1 | `fm[$.flag]` → `bool-proof` | bare typed **boolean read** |
| C2 | `fm[$.values[*]]=x` → `values-proof` | wildcard selector, existential equality |
| C3 | `fm[$['decision-makers']]=ada` → `quoted-proof` | quoted member name, case-folded string equality |
| C4 | `fm[$.items[-1]]=tail` → `last-proof` | negative index |
| C5 | `fm[$.number]=1` → `number-proof` | integer literal, no coercion |
| C6 | `fm[$.nothing]=null` → `null-proof` | equality with `null` |
| C7 (inside `area`) | `fm[$.nested]` → `nested-proof` | the same forms resolve from a nested scope |

Every root violation is `requires` → D anchored at line 1; C7's is `requires` →
H("Area") anchored at the `Area` header's line (6.2: a constraint targets its
scope's parent section).

Each document below declares only the keys its own case needs, so every other
constraint's `if` is unsatisfied against an absent member and holds vacuously.
That is what keeps each expected count at exactly one — or zero.

| Document | Frontmatter | Active constraint | Expected |
|---|---|---|---|
| `boolean-true.md` | `flag: true` | C1: one result node is boolean `true` → satisfied; `Bool Proof` absent | `requires` → D ×1 |
| `boolean-false.md` | `flag: false` | C1: boolean `false` is **unsatisfied** (4.6: a bare read is a typed boolean read, not a presence test) → `if` unsatisfied | `[]` |
| `wildcard-existential.md` | `values: [null, y, x]` | C2: wildcard selects all three; equality is existential over **non-null** nodes and `"x"` matches → satisfied; `Values Proof` absent | `requires` → D ×1 |
| `negative-index.md` | `items: [head, tail]` | C4: `[-1]` counts back from the end → `"tail"` → satisfied; `Last Proof` absent | `requires` → D ×1 |
| `quoted-name-casefold.md` | `decision-makers: Ada` | C3: bracket notation reaches the hyphenated name; 4.6 makes string equality follow `options.match_case`, which is `false`, so `Ada` equals `ada` → satisfied; `Quoted Proof` absent | `requires` → D ×1 |
| `integer-equality.md` | `number: 1` | C5: literal `1` resolves as a YAML core **integer** and the node is an integer of the same value → satisfied; `Number Proof` absent | `requires` → D ×1 |
| `no-coercion.md` | `number: "1"` | C5: the node is a **string**; 4.6 gives no cross-type coercion → unsatisfied | `[]` |
| `null-equality.md` | `nothing: null` | C6: 4.6 — `fm[query]=null` is **always false**; equality ranges over non-null nodes only | `[]` |
| `wrong-container.md` | `values: 7`, `items: {}`, `number: []` | C2: a wildcard on a scalar selects nothing. C4: an index selector on an object selects nothing. C5: a sequence never equals the literal. All unsatisfied | `[]` |
| `nested-scope.md` | `nested: true` | C7: satisfied inside the concrete `Area` scope; `Nested Proof` absent | `requires` → H("Area") ×1 |

**Why no `invalid-value` appears anywhere in this group.** 4.6 says every
non-boolean, non-null result node of a *bare* read produces `invalid-value`
and suppresses the containing constraint. C1 and C7 are the only bare reads,
and in every document above their queries select either nothing or a boolean.
`wrong-container.md` deliberately gives `values`, `items`, and `number`
non-boolean values but leaves `flag` and `nested` absent, so the wrong-kind
nodes are only ever reached by **equality** propositions, which have no
`invalid-value` path — they simply do not match. The invalid-read case is
covered by `frontmatter-query-suppression`, where it is the subject.

**`nested-scope.md` raw-record notes.** The diagnostic's `schema_node` is
`kind: "constraint"` with the `area` rule's scope coordinates, and its
`references` entry is `kind: "frontmatter_query"` with `query: "$.nested"` and
no `equals` member. Equality constraints instead carry `equals` with `type`
before `value`; C5's `type` is `integer` with a canonical **string** value,
C3's and C4's are `string`, and C1/C7 carry no `equals` at all (11.3).

---

## Group `frontmatter-query-suppression`

One constraint: `any_of: ["fm[$.flags[*]]", fallback]`. The bare read is the
first operand and `fallback` the second, so the fixture can show that an
already-satisfiable `any_of` is still suppressed.

### `multiple-invalid.md` (block lines 1–10) — expected exactly 4 diagnostics

`$.flags[*]` selects all seven elements. Per 4.6, each result node is
classified independently:

| Index | Value | Classification |
|---|---|---|
| 0 | `true` | boolean → **satisfies** the read |
| 1 | `bad` | string → non-boolean, non-null → `invalid-value` |
| 2 | `7` | integer → `invalid-value` |
| 3 | `{x: 1}` | mapping → `invalid-value` |
| 4 | `[false]` | sequence → `invalid-value` |
| 5 | `false` | boolean → valid, unsatisfied |
| 6 | `null` | null → unsatisfied, **no** `invalid-value` |

| id | target | multiplicity | anchor |
|---|---|---|---|
| `invalid-value` | F(10,"/flags/1") | 1 | line 4 col 5 |
| `invalid-value` | F(10,"/flags/2") | 1 | line 5 col 5 |
| `invalid-value` | F(10,"/flags/3") | 1 | line 6 col 5 |
| `invalid-value` | F(10,"/flags/4") | 1 | line 7 col 5 |

Anchors are the sequence **elements** themselves (6.2: at the key for a mapping
member, at the element itself for a sequence element). The block is written as
a two-space-indented block sequence, so each element's first byte is column 5.

**No `any_of` diagnostic.** 4.6: the whole containing constraint is suppressed,
and "a true sibling result or another already-true operand does not
short-circuit that suppression". Both escape hatches are present here on
purpose — element 0 is boolean `true`, which would satisfy the read outright,
and `Fallback` is present, which would satisfy the `any_of` by its second
operand — and neither may rescue the constraint. 5.3: suppression applies to
the whole boolean constraint without three-valued short-circuiting.

**Implementations must evaluate the complete result.** 4.6 forbids silent
truncation; stopping at the first satisfying node (index 0) would emit no
`invalid-value` at all, and stopping at the first invalid node would emit one.
Exactly four is the discriminating count.

### `false-and-null.md` — expected exactly 1 diagnostic

`flags: [false, null]`. Boolean `false` is valid and unsatisfied; `null` is
unsatisfied and produces no `invalid-value`. Nothing is suppressed, so the
constraint evaluates: neither operand is satisfied and `Fallback` is absent.

| id | target | multiplicity | anchor |
|---|---|---|---|
| `any_of` | D | 1 | line 1 col 1 |

### `absent.md` — expected exactly 1 diagnostic

No frontmatter block and no `Fallback`. 4.6: "If the block is absent, the query
produces an **empty result**: a bare boolean read is unsatisfied." An empty
result is falsity, not suppression — the distinction this document exists to
pin, since a suppressing implementation would report `[]`.

| id | target | multiplicity | anchor |
|---|---|---|---|
| `any_of` | D | 1 | line 1 col 1 |

No `missing-frontmatter`: this schema declares no `frontmatter` object, so
`required` is `false` by default.

### `invalid-frontmatter.md` (block lines 1–3) — expected exactly 1 diagnostic

A top-level YAML **sequence** where 1.6 requires a mapping.

| id | target | multiplicity | anchor |
|---|---|---|---|
| `invalid-frontmatter` | F(3,**no-pointer**) | 1 | line 1 col 1 |

The target names the block, not a value in it, so `pointer` is omitted
entirely — 6.1 distinguishes that from `"pointer": ""`, which would name the
mapping itself. 4.6: "If the block is `invalid-frontmatter`, the query is
unevaluated and the entire containing constraint is suppressed", so no `any_of`
accompanies it even though `Fallback` is also absent.

### `disabled-invalid.md` — expected `[]`

`flags: [bad]`, no `Fallback`, and a file-wide `invalid-value` suppression.
The `invalid-value` for `/flags/0` is produced and then filtered; the
constraint stays suppressed because 6.3 decides dependency suppression before
the comment filters anything: "suppressing `invalid-value` … never re-enables a
dependent constraint". A filter-first implementation would report an `any_of`
here — a diagnostic conjured by hiding another one.

The disable comment sits **after** the frontmatter block: 1.6 requires the
opening `---` to be the very first line of the file, so a comment above it
would destroy the block this fixture is about. The file-wide form of 6.3 works
from anywhere in the file.

---

## Group `frontmatter-capture-propositions`

`frontmatter.required: true` and three captures: `draft` (`bool`, optional),
`version` (`semver`, **required**), `label` (`text`, optional). Three root
constraints `requires: {if: fm.<name>, then: <name>-proof}` and one nested
constraint inside `area` requiring `Nested Proof` when `fm.label` is bound.

4.6 governs `fm.<name>`: satisfied iff the capture is valid and bound, "except
that a bound `bool` capture contributes its boolean value: a valid bound
`false` is unsatisfied". Optional absence is ordinary falsity. An invalid
value, a missing required capture, invalid frontmatter, or an absent required
block suppresses the entire containing constraint **after** its primary
diagnostic.

| Document | State | Expected |
|---|---|---|
| `bool-true.md` | `draft: true`, version valid, `Version Proof` present, no `Draft Proof` | `requires` → D ×1 |
| `bool-false.md` | as above with `draft: false` | `[]` — a bound `false` is unsatisfied, so the `if` does not fire |
| `version-present.md` | version valid, no `Version Proof` | `requires` → D ×1 |
| `text-empty.md` | `label: ""`, version valid + proof, no `Label Proof` | `requires` → D ×1 — empty `text` is valid and **bound**, which is exactly the case a truthiness-based reading would get wrong |
| `nested-text.md` | version + label bound, both root proofs present, `Area` without `Nested Proof` | `requires` → H("Area") ×1 |
| `optional-absent.md` | version valid + proof; `draft` and `label` absent | `[]` — optional absence is ordinary falsity, not an error |

Anchors: root violations at line 1 col 1; `nested-text.md`'s at the `Area`
header, line 10 col 1.

### Failure and suppression cases

| Document | id | target | multiplicity | anchor | reasoning |
|---|---|---|---|---|---|
| `invalid-version.md` (lines 1–3) | `invalid-value` | F(3,"/version") | 1 | line 2 col 1 | unquoted `1.2` is a YAML float; `semver` needs a YAML string (2.4). The `fm.version` constraint is suppressed after this primary diagnostic (4.6), so no `requires` accompanies it even though `Version Proof` is absent |
| `missing-version.md` (lines 1–3) | `missing-value` | F(3,"/version") | 1 | line 1 col 1 | the block is a valid mapping without `version`; the required capture is absent (2.3). Its constraint is likewise suppressed. The anchor is the deepest resolving positioned ancestor of `/version` — here the root mapping, whose anchor 6.2 gives as the block's first line |
| `no-frontmatter.md` | `missing-frontmatter` | F(**absent**) | 1 | line 1 col 1 | `required: true` with no block. 6.1: `line_range` is absent **exactly** when the document has no frontmatter block at all, so this is the only target shape in the corpus carrying neither member |
| `invalid-frontmatter.md` (lines 1–3) | `invalid-frontmatter` | F(3,no-pointer) | 1 | line 1 col 1 | top-level sequence |
| `disabled-invalid.md` | — | — | 0 | — | invalid `version` with file-wide `invalid-value` suppression |

**Why the absent and invalid block cases carry no capture diagnostics.** 2.3:
"When the document has no frontmatter block, or its block is
`invalid-frontmatter`, captures are not evaluated and produce neither
`missing-value` nor `invalid-value`. The block-level diagnostic, when one is
required, is sufficient." So `no-frontmatter.md` reports one diagnostic and
**not** an additional `missing-value` for the required `version`, and
`invalid-frontmatter.md` likewise reports exactly one. All three constraints
are suppressed in both, per 4.6.

**`disabled-invalid.md` is the filtering-order case.** The `invalid-value` is
produced and filtered file-wide; the `fm.version` constraint remains suppressed
because 6.3 decides dependency suppression first. Expected exactly `[]`: a
filter-first implementation would find `version` unbound, read that as ordinary
falsity, and — since the `if` would then be unsatisfied — also report `[]`
here by luck. What it would get wrong is the *reason*, which
`invalid-version.md` pins instead by requiring exactly one diagnostic where a
naive reading gives two.

---

## Audit of the two already-respelled legacy groups

Neither group is modified; their `expected.json` files are re-derived here from
the spec to confirm they remain correct under Typed Values, and are re-run
against the post-4A/4B CLI at activation to confirm they are byte-for-byte
unchanged.

### `frontmatter-refs`

Spellings in use: `fm[$.status]=deprecated` and `fm[$.semver]=major` — the
`fm[...]` JSONPath equality form of 4.6, not the `fm.<name>` capture form. The
schema declares no `frontmatter` object, so no capture exists and no
`missing-frontmatter` is reachable.

This schema does **not** set `ordered_sections: false`, so its root scope is
ordered (3.7). Its two rules are `migration` then `breaking-changes`, and the
only document containing both, `pass-satisfied.md`, spells them in that order,
so no `ordered` diagnostic arises.

| Document | Derivation | Committed |
|---|---|---|
| `pass-inert.md` | no block → both queries produce empty results → both `if`s unsatisfied → both constraints hold vacuously | `[]` ✓ |
| `pass-satisfied.md` | C1 `if` satisfied and `Migration` present; C2 `if` satisfied and `fm[$.semver]=major` satisfied | `[]` ✓ |
| `fail-missing-migration.md` | C1 `if` satisfied, `Migration` absent → violated; C2 `if` unsatisfied | `requires` → D ×1 ✓ |
| `fail-semver.md` | C1 `if` unsatisfied; C2 `if` satisfied, `semver: minor` ≠ `major` → violated | `requires` → D ×1 ✓ |

### `frontmatter-ref-typed-equality`

Spellings in use: `fm[$.count]=1` and `fm[$.checked]=true`. One constraint:
`requires: { if: "fm[$.count]=1", then: "fm[$.checked]=true" }`. 4.6 resolves
each literal as one YAML 1.2 core scalar and requires the result node to have
"the same resolved scalar type **and** value".

| Document | `count` node vs literal integer `1` | `checked` node vs literal boolean `true` | Committed |
|---|---|---|---|
| `pass-typed.md` | integer 1 = integer 1 → `if` satisfied | boolean true = boolean true → `then` satisfied | `[]` ✓ |
| `pass-string-count.md` | string `"1"` vs integer → type differs → `if` unsatisfied | not reached | `[]` ✓ |
| `pass-float-count.md` | float `1.0` vs integer → type differs → `if` unsatisfied | not reached | `[]` ✓ |
| `fail-quoted-bool.md` | integer 1 → `if` satisfied | string `"true"` vs boolean → type differs → `then` unsatisfied | `requires` → D ×1 ✓ |

Both groups agree with the specification as re-derived. Their `expected.json`
files are correct and MUST NOT be changed.
