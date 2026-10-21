# Outlint Read — Specification

Status: Normative public specification; may change before 1.0. This command
is optional and provided by the CLI's `read` feature (`full` includes it).
Normative keywords MUST / MUST NOT / SHOULD / MAY per RFC 2119.

Sections 1 through 6 are normative. Section 7 is a non-normative example.

---

## 1. Command line

1.1. The command synopsis is:

```text
outlint read [options] <FILE> [MDPATH]
```

Exactly one file and at most one document path MUST be accepted. One leading
UTF-8 BOM (bytes EF BB BF), if present, MUST be removed before UTF-8 decoding.
The resulting text is the source for all parsing, extents, sizes, and content
output in this command. It MUST be parsed with the default Markdown options in
[the schema specification](outlint-spec.md) §7, using its §1 document model.
No schema is loaded and no validation is performed. `-` is not stdin:
after `--` it is a literal filename. A file error takes precedence over a
document-path error.

1.2. An absent path MUST mean `$`. If the supplied path does not start
with `$`, the command MUST prepend `$`, including for an empty argument.
Parsing, resolution, canonical addresses, and source extents MUST follow
[the document-path specification](document-path-spec.md).

1.3. Options are `--tree`, `--blocks`, `--depth <N>`, `--format <FORMAT>`,
and `-h` / `--help`. Options may precede or follow positional arguments;
`--` ends option recognition. Values MUST be separate arguments, not
`--name=value`. `--depth` MUST occur at most once and accept an unsigned
decimal integer fitting `usize`, including zero; a leading `+` and leading
zeroes are accepted. Its default is unlimited. Repeated `--tree` and
`--blocks` are accepted. `--blocks` without `--tree` is a usage error.
Help MUST print to stdout and exit 0 when encountered before an argument
error. Unknown options, missing option values, and extra positionals are
usage errors.

1.4. Format selection, `OUTLINT_FORMAT`, escaping, size notation, and common
wire conventions MUST follow [search §7](search-spec.md#7-shared-output-format-rules).

## 2. Content mode

2.1. Without `--tree`, the command MUST print the selected source content.
With unlimited depth this is the whole file for `$`, the complete section
extent for a section, the parsed block range for a block, or the parsed item
range for an item. Source syntax, list markers, indentation within the
range, and internal line endings MUST be retained. Transparent material
inside a selected range MUST remain present.

2.2. A finite depth counts descendant section edges, not heading-level
differences. Zero includes only the addressed root's or section's own
content; one includes its direct child sections' own content, and so on.
Depth MUST have no effect on a block or item selection. Content slices MUST
be selected in preorder as follows:

1. If the remaining depth covers the entire subtree, select its full
   extent as one slice; for the root, select the whole file.
2. Otherwise, for the root select byte zero through, but not including,
   the first child heading. A merged H1 belongs to the root, so this slice
   includes frontmatter, root preamble, the H1, and its own content.
3. Otherwise, for a section select from its heading start through the
   greatest end offset of its heading and its own visible preamble blocks.
4. If depth is positive, apply the same procedure to each child section
   with depth decreased by one.

2.3. Empty slices MUST be omitted. Before every nonfinal slice's separator,
all trailing complete LF or CRLF sequences MUST be removed; exactly two
line-ending sequences MUST then separate that slice from the next. The
last slice MUST remain verbatim. If the assembled content is nonempty and
does not end in LF, one line-ending sequence MUST be appended. Empty content
MUST remain empty. The line ending used for inserted separators is CRLF if
the first LF in the entire source is preceded by CR, otherwise LF; a source
without LF uses LF. Existing line endings MUST NOT otherwise be normalized.

2.4. Human and compact content output MUST be those bytes without labels,
escaping, or a wrapper. JSON content MUST contain the same text as a JSON
string. Its `bytes` field MUST count the UTF-8 bytes of the assembled
content, including inserted separators and the appended line ending.

## 3. Tree mode

3.1. `--tree` MUST emit canonical nodes in document-path enumeration order,
including the selected base itself. For a root or section base, only the
base and descendant sections are included by default. `--blocks` adds
their direct blocks and direct list items. Finite depth limits the number
of additional section steps; blocks and items consume no depth.

3.2. A block or item base MUST list itself even without `--blocks`. A list
base additionally lists its direct items only with `--blocks`; depth does
not affect this terminal-base listing.

3.3. Each row MUST have a canonical path, a size, and a label. Sizes MUST
count full source extents as defined in document-path §6, regardless of
tree depth. In particular they do not include an output newline appended
by content mode. Labels and machine fields are:

| Kind | Label | Additional JSON members |
| --- | --- | --- |
| `root` | Merged title's case-preserving heading text, or the supplied filename | `level`: 1 and `text`: title string when merged; otherwise both null |
| `section` | Case-preserving heading text | `level`: integer 1–6; `text`: heading string |
| `list` | `list (` followed by direct item count and ` items)` | `items`: nonnegative integer |
| `p` | `p: ` followed by preview | `preview`: string |
| `item` | `item: ` followed by preview | `preview`: string |
| `code`, `quote`, `html`, `break` | Kind keyword | `preview`: empty string |

3.4. A paragraph preview MUST use the first source line in its range,
retaining Markdown markup. An item preview MUST use its first direct
paragraph's diagnostic text when present. Otherwise it MUST use the item's
source after trimming leading whitespace, removing one leading unordered
marker (`-`, `*`, `+`) or an initial run of ASCII digits followed by `.` or
`)`, then trimming leading whitespace again. From either text take the first
LF- or CRLF-delimited line and at most 60 Unicode scalars; append `…` only
if that line was longer. Heading labels are not preview-truncated.

3.5. Human rows MUST be `<path><padding>  <size>  <label>` followed by LF.
Paths are left-aligned to the longest rendered path in the listing; sizes
are not padded. Compact rows MUST have exactly three tab-separated fields
`path`, `size`, `label`, followed by LF, with the escaping of search §7.

## 4. Path failures and suggestions

4.1. Syntax failures MUST report the spelling after `$` insertion, a
message, and a zero-based UTF-8 byte offset in that spelling. Resolution
failures MUST report the formatted selector (terminal indices explicit),
the failing step, and the deepest successfully resolved canonical prefix.
Named failing steps omit their leading dot; descendant steps retain `..`;
terminal steps omit their leading slash.

4.2. An unresolved failure MUST include a tree of the resolved prefix to
depth one. Blocks and items MUST be included if the failing step is a
terminal step, otherwise excluded. An ambiguous failure MUST instead list
all matching section candidates in canonical enumeration order, considering
direct children for a named step or all descendants for a descendant step.
These listings do not depend on the invocation's `--tree`, `--depth`, or
`--blocks` settings.

4.3. Syntax and unresolved failures MUST provide suggested canonical paths
when candidate selection finds suitable alternatives; the suggestion list
may be empty. Suggestions MUST be resolving canonical paths, with at most
five entries. Ambiguous and other failures have an empty suggestion list;
ambiguity is explained by its candidates instead. Each human or compact
suggestion MUST be a separate `did you mean <path>?` line after the failure
and any listing. JSON MUST always include the `suggestions` array, even
when empty. Candidate-selection and similarity heuristics are not normative.

4.4. Human and compact failure headers MUST use the following templates,
with variable text escaped according to search §7 and each header ending
in LF. Listings use the corresponding tree format of §3.5.

```text
outlint: invalid document path '<path>': <message> at byte <offset>
outlint: cannot resolve <path> in <file>: <step> not found under <resolved>
outlint: ambiguous document path <path> in <file>: <count> sections match '<step>'
outlint: cannot resolve <path> in <file>: <message>
```

The final form is reserved for other resolution failures.

## 5. JSON shapes

5.1. Members MUST appear in the following order. All `file`, `mdpath`,
`path`, `step`, `resolved`, `message`, and `content` values are strings;
`file` preserves the supplied filename spelling. Counts, byte lengths, and
offsets are nonnegative integers. `version` is the integer 1.

| Result | Ordered members |
| --- | --- |
| Content | `version`, `file`, `mdpath`, `bytes`, `content` |
| Tree | `version`, `file`, `mdpath`, `nodes` |
| Path failure | `version`, `error` |

Successful `mdpath` values MUST be canonical. `nodes` is an ordered array
of objects. Each node's ordered members are `mdpath`, `kind`, the additional
members in §3.3 (`level` before `text`), then `bytes`. Labels are not emitted
as separate JSON members.

5.2. The `error` object MUST have one of these ordered shapes. `kind` is
the literal string in the first column. `suggestions` is an array of
strings; `nodes` and `candidates` are arrays of the node objects in §5.1.
There is no `file` member in a path-failure object.

| `kind` | Ordered members |
| --- | --- |
| `syntax` | `kind`, `path`, `message`, `offset`, `suggestions` |
| `unresolved` | `kind`, `path`, `step`, `resolved`, `nodes`, `suggestions` |
| `ambiguous` | `kind`, `path`, `step`, `resolved`, `candidates`, `suggestions` |
| `other` | `kind`, `path`, `message`, `suggestions` |

## 6. Streams and exit codes

6.1. Successful content or tree output MUST go to stdout. Human and compact
path failures MUST go to stderr with no stdout. JSON path failures MUST go
to stdout with no corresponding stderr diagnostic. Operational errors MUST
go to stderr as `outlint: <message>` followed by LF, including in JSON mode;
they are not JSON envelopes. Usage errors MUST go to stderr as text.

6.2. Exit codes MUST be 0 for successful output or help, 1 for a path syntax
or resolution failure, and 2 for a usage or operational error, including
failure to write stdout. Failure to write stderr does not change the status.

## 7. Example (non-normative)

Given `docs/guide.md` with LF line endings and a final LF:

```markdown
# Guide

## Setup

Install the tool.

- first item
- second item

## FAQ

Answers.
```

`outlint read --format json docs/guide.md .setup/p` prints the following
stdout and no stderr, with exit code 0:

```json
{"version":1,"file":"docs/guide.md","mdpath":"$.setup/p[0]","bytes":18,"content":"Install the tool.\n"}
```
