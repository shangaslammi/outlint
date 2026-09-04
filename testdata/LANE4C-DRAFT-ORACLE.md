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
