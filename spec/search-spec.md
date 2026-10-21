# Outlint Search — Specification

Status: Normative public specification; may change before 1.0. This command
is optional and provided by the CLI's `search` feature (`full` includes it).
Normative keywords MUST / MUST NOT / SHOULD / MAY per RFC 2119.

Sections 1 through 9 are normative. Section 10 contains non-normative
implementation notes and examples. Section 7 also defines the output-format
rules shared with [read](read-spec.md).

---

## 1. Command line and search scope

1.1. The command synopsis is:

```text
outlint search [options] <WORD>...
```

Positional arguments MUST be joined with one ASCII space to form the query.
A query empty after whitespace trimming is a usage error. Options are
`--root <DIR>`, `--limit <N>`, `--format <FORMAT>`, and `-h` / `--help`.
They may precede or follow query arguments. `--` ends option recognition;
subsequent arguments belong to the query even if they start with `-`.
Option values MUST be separate arguments, not `--name=value`. `--root` and
`--limit` MUST each occur at most once. Help MUST print to stdout and exit
0 when encountered before an argument error. Unknown options and missing
values are usage errors. Format selection follows §7.

1.2. `--limit` MUST accept an unsigned decimal integer from 1 through 1000
inclusive, defaulting to 10. Leading zeroes and a leading `+` are accepted.
The limit MUST apply after the list exclusion in §3.3 and ranking in §4.
The reported `total` MUST count all matching units after that exclusion,
before the limit, within the active search scope.

1.3. The search root MUST be the current directory unless `--root` supplies
a directory, resolved relative to the current directory. Both directories
MUST be canonicalized, resolving symbolic links and parent components.
An explicit root that is not a directory is an operational error. Only
files below the search root MUST participate in refresh and matching,
including when the index also contains files elsewhere in its repository.

1.4. Files MUST have a case-insensitive ASCII extension `md` or `markdown`.
The recursive walk MUST skip symbolic-link entries and directories named
`.outlint`. Hidden entries MUST be skipped unless admitted by an ignore-rule
whitelist. Ignore files MUST be honoured as Git does. Files with a non-UTF-8
path MUST be skipped with a note, never indexed under a lossy path spelling.
Only regular files are eligible. Files larger than 4 MiB (4,194,304 bytes),
non-UTF-8 contents, and Markdown parse failures MUST be skipped as specified
in §6.4.

1.5. Hit file paths MUST use forward slashes and be relative to the
canonical current directory, not to `--root`. If a relative representation
is impossible, an absolute UTF-8 path MUST be emitted instead. Failure to
represent a hit path is an operational error. Document paths MUST be the
canonical addresses of [document-path §5](document-path-spec.md#5-canonical-enumeration-and-spelling).

## 2. Indexed units and text

2.1. Files MUST be decoded as UTF-8 without removing a leading BOM, and
parsed using the document model in
[the schema specification](outlint-spec.md) §1 and default Markdown options
in its §7. The root merge and addressable nodes MUST follow document-path
§§1–5. Search MUST NOT load a schema or perform validation.

2.2. Before lead-in folding (§2.5), each of the following MUST contribute
one unit at its canonical path:

| Node | Searchable body | Snippet source |
| --- | --- | --- |
| Root, only if it has a merged H1 or a frontmatter mapping | Merged title text, then frontmatter scalar values | Frontmatter scalar values only |
| Section | Its own case-preserving heading text | Visible text of its own preamble blocks |
| Visible direct block | Visible text of the complete block | Visible text of the complete block |
| Direct list item with nonempty normalized text | Visible text of the complete item | Visible text of the complete item |

The root does not absorb preamble text into its body. Sections do not absorb
descendant headings or body text. A list's body includes all its items;
an item's body includes nested content inside that item, even though the
nested content has no separate document path. Empty items MUST NOT
contribute item units; their list still contributes a block unit. Other
units may have empty bodies. GFM tables are not enabled: table-like source
is indexed as the CommonMark blocks it parses into.

2.3. Block and item bodies MUST contain their visible CommonMark text,
including inline code and code-block text, with whitespace normalized.
Link destinations, raw HTML, comments, and fenced-code info strings MUST
NOT contribute text. Inline markup may separate words for matching even
when a snippet preserves their adjacency.

2.4. Frontmatter MUST contribute string, number, and boolean values,
recursing through arrays in element order and mappings in sorted key order.
Mapping keys and null values MUST NOT contribute. Numbers and booleans use
their JSON spelling. Values MUST be separated by spaces. Frontmatter not
parsed as a mapping contributes no text.

2.5. A short introductory paragraph ending in `:`, or consisting only of
emphasis, MUST be searched together with the following sibling block it
introduces and have no hit of its own. Consecutive introductions MUST prefix that
block's body and snippet text in source order, separated by spaces. A
paragraph without a following sibling block remains a unit. A prefix
attached to a list MUST NOT also prefix its item units. Folding MUST NOT
change document paths, source ranges, or byte counts.

2.6. Each unit also has a context field. The root's context MUST be the
filename stem only. Other units' contexts MUST be the stem, any merged H1
text, then the enclosing section headings from outermost to innermost,
separated by ` / `. A section's own heading belongs to its body, not its
context. Directory components MUST NOT be included in the stem.

## 3. Query syntax and matching

3.1. Unqualified words MUST search unit bodies. Adjacent query terms MUST
be conjoined by default: every required term must match the same unit's
body. Matching a heading in a unit's context alone MUST NOT satisfy an
unqualified word query. Matching context may raise a body's relevance.

3.2. Body, context, and snippet text MUST use the same English analyzer:
split into maximal runs of Unicode alphanumeric characters; discard tokens
whose original UTF-8 length is 40 bytes or more; lowercase; then apply
English stemming. No stop-word filter is applied. Query words MUST use
that analyzer too. Exact punctuation, case, and unstemmed spelling
therefore need not be equal for a match; accent removal is not part of
this analyzer.

3.3. If any item of a list matches the complete query, that aggregate list
MUST be excluded from both the ranked results and `total`, even when the
matching item falls beyond the output limit. Matching items remain separate
hits. Otherwise a matching aggregate list remains eligible, including when
required words occur in different items or in a folded introduction.

3.4. Plain words joined by conjunction and matched with the analyzer of
§3.2 are the specified query language. The command MUST NOT reject a query
for query-language errors. Further syntax accepted by the reference
implementation is not part of this specification; see §10.2.

## 4. Ranking and totals

4.1. Results MUST be the first `min(limit, total)` units in descending
relevance-score order after §3.3. Equal scores MUST be ordered by
repository-relative UTF-8 file-path bytes, then canonical document-path
bytes, ascending. Ties at the limit boundary MUST use the same ordering;
selection MUST NOT depend on index-segment order. Presentation rebases paths
only after ranking.

4.2. For unqualified queries with analyzed terms, matching the query in the
context field MUST contribute a positive relevance boost in addition to
the required body match. No fixed score values, size-based ordering, or
score stability across corpus or engine changes is guaranteed. Scoring
internals in §10.3 are non-normative.

## 5. Snippets and size hints

5.1. Snippets MUST be excerpts of the source identified in §2.2, with the
lead-in prefixes of §2.5. Block and item snippets MUST preserve visible
inline adjacency and surround inline code with backticks. Line breaks and
block boundaries MUST separate text; whitespace MUST be collapsed to single
spaces. Raw HTML, comments, and link destinations MUST be omitted. A
section's snippet source MUST include only its own preamble blocks,
separated by spaces, excluding child sections.

5.2. Excerpt selection MUST prefer text around matching query terms,
falling back to the leading text when no matching fragment is available.
Empty source yields an empty snippet, even for a matching title or heading.

5.3. Excerpts MUST have `…` prepended or appended where they omit the
beginning or end of their source. No match highlight markup is added.
Exact choice between matching fragments is not guaranteed.

5.4. `bytes` MUST be the complete source extent length defined in
document-path §6. For a block or item inside an addressable section,
`section_bytes` MUST be that enclosing section's full extent length.
Root and section units, root preamble blocks/items, and blocks/items of a
merged H1 MUST have no enclosing-section size. Sizes MUST NOT measure
normalized text, folded introductions, snippet length, or newline-adjusted
`read` output. Presentation uses §7.5.

## 6. Index location and lifecycle

6.1. Starting at the canonical search root, the nearest ancestor, including
itself, with an existing `.git` entry MUST be the index repository. The
entry may be a file or directory; its contents are not validated. Without
such an entry the search root itself MUST be the index repository. The
index MUST live at `<repository>/.outlint/search/`. A repository index is
shared across its search roots. No separate user configuration selects it.

6.2. The index MUST be created on demand. If `.outlint/.gitignore` does not
exist, it MUST be created with a rule that ignores the index. An existing
file MUST remain unchanged.

6.3. An incompatible or unreadable index MUST be rebuilt rather than
causing the search to fail. Rebuilding discards records outside the current
scope too; the subsequent refresh indexes only the active scope. A missing
directory is creation, not rebuilding. Operational failures during creation,
rebuilding, or writing MUST exit 2.

6.4. Every search MUST refresh the index so results reflect the current
files in the active scope, subject to §6.5. New and changed files MUST be
reflected. Files larger than 4 MiB (4,194,304 bytes), files with invalid
UTF-8 contents, and files that cannot be read or parsed MUST be skipped with
a note and MUST NOT retain old searchable units.

6.5. After a complete walk, indexed files inside the active scope absent
from the walk MUST be removed, including newly ignored files. Walk or
metadata errors make a walk incomplete: removal of absent files MUST then
be suppressed. Refresh MUST leave indexed files outside the active scope
unchanged. If another process holds the writer lock, refresh MUST be
skipped and search MUST use the existing index with a note. Other index
read/write errors are operational errors. An ordinary successful refresh
prints no progress or count message.

6.6. Notes MUST go to stderr in every format, before results or an ensuing
operational error. Each has `outlint: ` prepended and LF appended. The
following message templates are normative; filesystem/engine cause text is
not stable:

| Event | Message |
| --- | --- |
| Created index | `created search index at <directory>` |
| Rebuilt index | `rebuilt search index at <directory> (<reason>)` |
| Walk error | `cannot walk: <cause>` |
| Metadata error | `cannot stat <path>: <cause>` |
| Skipped file | `skipping <path>: <cause>` |
| Busy writer | `search index is being refreshed by another process; searching the existing index` |

Rebuild reasons MUST be `format changed` when the stored index format
differs, otherwise `outlint version changed` when its version differs,
otherwise `index could not be opened`. Creation and rebuild notes
MUST name the index directory and be emitted only after successful creation
or rebuild. Fixed skip causes include `larger than 4 MiB`, `not valid UTF-8`,
and `file name is not valid UTF-8`. Notes MUST NOT themselves change a
successful search's exit code.

## 7. Shared output-format rules

7.1. `search` and `read` MUST accept `--format human|json|compact` with
case-sensitive values. A valid explicit `--format` overrides `OUTLINT_FORMAT`,
even when that environment value is invalid or non-Unicode. Repeated format
options are accepted; the last valid value wins, but any invalid occurrence
is a usage error. Without an explicit format, an unset or empty environment
value selects `human`; `human`, `json`, or `compact` selects that format;
any other value, including whitespace or non-Unicode data, is a usage error.
Neither command accepts `--color` or adds terminal color sequences.

7.2. JSON output MUST be one unindented JSON object followed by LF. Each
command defines its own envelope with integer `version` 1, independently of
the validation envelope in the schema specification §11. Member ordering
is part of these output contracts. Strings use JSON escaping; optional
values described as null MUST be present as null, not omitted. Operational
and usage diagnostics on stderr remain text, not JSON.

7.3. Compact records MUST use literal tab field separators and LF record
terminators, with no header, alignment padding, or blank records. Within a
text field, backslash MUST become `\\`, tab `\t`, LF `\n`, and every other
Unicode control scalar (General Category `Cc`, including CR and ESC)
`\u{h}`, where `h` is its lowercase hexadecimal value without leading
zeroes. All other scalars MUST remain literal. A parser MUST split records
and fields before decoding escapes. Backslash escapes are reversible;
printable Unicode separators and direction controls are not escaped by this
format. Compact `read` content is the raw-content exception in read §2.4.

7.4. Human structured text MUST escape LF, CR, tab, and ESC as `\n`, `\r`,
`\t`, and `\x1b`. Other control scalars, U+061C, U+200E–U+200F,
U+2028–U+202E, and U+2066–U+206F MUST use `\u{h}` as above. Printable
quotes and backslashes remain literal. Human `read` content is the
raw-content exception. Operational messages and index notes use human or
compact escaping in those modes; JSON mode leaves their stderr text
unescaped.

7.5. Display sizes MUST be decimal, without spaces: below 1000 bytes,
`<integer>B`; below 1,000,000 bytes, one decimal place and `kB`; otherwise
one decimal place and `MB`. For unit size `u` (1000 or 1,000,000), the number
of tenths MUST be `floor((bytes + u/20)/(u/10))`, with the addition saturated
at the unsigned 64-bit maximum. Unit choice MUST precede rounding. JSON
size fields MUST instead contain exact nonnegative integer byte counts.

## 8. Search output and no-hit diagnostics

8.1. Human output MUST emit for each hit, in ranked order:

```text
<path> <mdpath> <size>[ (section <section-size>)]
  <snippet>

```

The bracketed suffix appears only with an enclosing-section size. The
snippet line MUST be omitted when empty; the blank line after each hit
MUST remain. If fewer than `total` hits are shown, stdout MUST then include
`(<shown> of <total> matching blocks; use --limit to see more)` and LF.

8.2. Compact hits MUST contain exactly four fields: file path, canonical
document path, size, snippet. The size field MUST be `<size>` or
`<size>/<section-size>` when an enclosing-section size exists. The fourth
field MUST remain present even for an empty snippet. The truncation note
of §8.1 MUST go to stderr instead of stdout.

8.3. JSON results MUST have ordered members `version` (integer 1), `query`
(the joined query string), `total` (nonnegative integer), and `hits`
(array in ranked order). Each hit MUST have these members in order:

| Member | Type and meaning |
| --- | --- |
| `path` | String, file path from §1.5 |
| `mdpath` | String, canonical document path |
| `bytes` | Nonnegative integer, unit extent bytes |
| `section_bytes` | Nonnegative integer or null |
| `score` | JSON number, engine relevance score |
| `snippet` | String, possibly empty |

8.4. When there are no hits, JSON MUST additionally append `term_counts`
after `hits`. It MUST be an array only when the query is a nonempty sequence
of whitespace-separated terms composed entirely of Unicode alphanumeric
scalars, none exactly `AND`, `OR`, or `NOT`; otherwise it MUST be null.
Each array element MUST contain `term` (original string) then `blocks`
(nonnegative integer), in query order, retaining repeated terms. Counts
MUST use the same analyzer and active scope, exclude item units, and count
each matching aggregate list once. They count units, including heading and
root units, not just Markdown blocks. `term_counts` MUST be absent when
hits exist.

8.5. With nonempty term counts, human no-hit stderr MUST be:

```text
outlint: no block contains all of: <query>
  <term> <count>, <term> <count>, ...
  (a block must contain every word; drop or change the rarest words)
```

The count line contains exactly the actual terms, comma-space separated;
the ellipsis above is template notation. Compact no-hit stderr MUST instead
be one line `outlint: no block contains all words; ` followed by the same
comma-space-separated counts. Without term counts, human stderr MUST be
`outlint: no search hits for: <query>` and LF; compact stderr MUST be
`outlint: no search hits` and LF. Human and compact no-hit stdout MUST be
empty. JSON no-hit results MUST go to stdout without a corresponding
no-hit stderr note; lifecycle notes may still be present.

## 9. Exit codes

9.1. Exit codes MUST be 0 when hits are printed or help is shown, 1 when
there are no hits, and 2 for usage or operational errors, including failure
to write stdout. Operational errors MUST be text on stderr prefixed with
`outlint: ` and terminated by LF; no JSON error envelope is produced.
Failure to write stderr does not change the exit code.

## 10. Implementation notes and examples (non-normative)

10.1. **Indexed text (§2).** The reference implementation currently parses
block and item source slices as CommonMark without extensions, trims each
text or inline-code event payload, joins nonempty payloads with spaces, and
collapses Unicode whitespace. This can separate source-adjacent inline
text. Frontmatter scalar values are trimmed and space-joined, retaining
whitespace within string values. Colon-ended introductions qualify up to
120 Unicode scalars; emphasis-only introductions require a single emphasis
or strong span covering the entire inline body, not separate spans.

10.2. **Query syntax (§3).** The reference implementation passes the query
to its search engine's lenient query parser, so further syntax is accepted;
it is not part of this specification and may change. This includes boolean
operators, phrases, ranges, boosts, `*`, and field restrictions using names
such as `body`, `context`, `snippet`, `path`, and `mdpath`.

10.3. **Ranking (§4).** The reference engine uses BM25 relevance with a
context-query boost of 2.0. Scores depend on indexed corpus statistics,
including records outside the active search scope. These details are not a
stable scoring contract.

10.4. **Snippets (§5).** The reference implementation currently preserves
text and inline-code event adjacency, surrounds inline code with one
backtick on each side, and inserts spaces at breaks and block boundaries
only when adjoining text lacks whitespace. Emphasis, links, and images do
not insert spaces. It selects matching fragments using a separate lenient
query with the snippet as the default field and disjunction as the default
operator. Matching fragments have a 160-byte span budget at token boundaries
and widen at each end over up to three adjacent scalars that are neither
whitespace nor alphanumeric. Fallback excerpts take up to 160 Unicode
scalars, shortened to a word boundary when possible; a longer first word
is cut at the bound. Neither budget is a strict final output-byte limit.

10.5. Given only `docs/guide.md` below the search root, with the content in
[read §7](read-spec.md#7-example-non-normative), and an already refreshed
index, `outlint search install tool` prints the following stdout and no
stderr, with exit code 0. There is a blank line after the snippet.

```text
docs/guide.md $.setup/p[0] 18B (section 56B)
  Install the tool.

```

10.6. In the same directory, `outlint search --format json install absent`
prints the following stdout and no stderr, with exit code 1:

```json
{"version":1,"query":"install absent","total":0,"hits":[],"term_counts":[{"term":"install","blocks":1},{"term":"absent","blocks":0}]}
```
