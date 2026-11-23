# Outlint Edit — Specification

Status: Normative public specification of an experimental extension of
Outlint, layered on the validator's document model. This command is outside
the stability promises of the validator and its specification and may
change or be removed in any release. It is optional and provided by the
CLI's `edit` feature (`full` includes it).

Sections 1 through 7 are normative. Section 8 is a non-normative example.

---

## 1. Command line

1.1. The command accepts one destination and either one operation or one
script.

```text
outlint edit [OPTIONS] FILE OP PATH [ARGS]
outlint edit [OPTIONS] FILE --script FILE|-
```

`FILE` names a regular Markdown file. Standard input is reserved for a
script or payload. Use `--` before a filename starting with `-`, including
a file named `-`. Options may precede or follow positional arguments until
`--`. Option values are separate arguments. `outlint edit -h` and
`outlint edit --help` print help to stdout.

1.2. Both modes require an explicit target for every operation. Paths use
the grammar, resolution, canonical spelling, and source extents of the
[document-path specification](document-path-spec.md). Single-operation
mode accepts the leading-`$` abbreviation of [read §1.2](read-spec.md#1-command-line).
Scripts require the anchored spelling. Targets are resolved from the
original text under the default Markdown parsing options of
[the validator specification](outlint-spec.md) §7, regardless of schema
selection. Reported successful operation paths use canonical spelling.

1.3. In single-operation mode, `--payload TEXT` supplies literal text,
`--payload -` reads stdin, and `--payload-file FILE` reads a UTF-8 file.
`replace-text` takes the old text through `--old TEXT`, `--old -`, or
`--old-file FILE` and the new text through the payload options. Each
operand takes at most one source. A command may consume stdin at most
once. Relative file paths are relative to the working directory, and a
`-file` option treats `-` as a literal filename.

1.4. The remaining options are `--dry-run`, `--schema FILE`,
`--ignore-schema`, `--force`, and `--format FORMAT`. Except for
`--format`, each option that applies to the entire invocation may occur
at most once. `--script` excludes a positional operation and all
single-operation payload options. Unknown options, extra arguments,
missing required arguments, and invalid operation signatures in
single-operation mode are usage errors. Format selection and repetition
follow [search §7](search-spec.md#7-shared-output-format-rules).

## 2. Scripts

2.1. A script is UTF-8, read from the named file or from stdin with
`--script -`. Each instruction starts on its own LF- or CRLF-delimited
line and follows the grammar below. The final instruction line may be
unterminated. Blank lines and lines whose first non-whitespace character
is `#` are skipped outside heredocs. A `#` on an instruction line after
its first word has ordinary token syntax.

```text
rename PATH TITLE
delete PATH
insert-section PATH PLACEMENT TITLE [PAYLOAD]
append-block PATH PAYLOAD
replace PATH PAYLOAD
insert-before PATH PAYLOAD
insert-after PATH PAYLOAD
append-item PATH PAYLOAD
insert-item PATH INDEX PAYLOAD
replace-text PATH OLD NEW
```

`PLACEMENT` is `before`, `after`, or `append-child`. `INDEX` is an
unsigned decimal integer with an implementation-defined maximum. Leading
zeroes and an optional leading `+` are accepted. Single-operation mode
uses the same signatures, with payload operands supplied as in §1.3.

2.2. ASCII spaces and tabs separate words. A bare word extends up to the
next separator. A word starting with `"` ends at a closing `"` and must
then be followed by a separator or the end of the line. The only escapes
inside a quoted word are `\"` (a quote) and `\\` (a backslash).
The delimiter quotes are removed. Bare words have no escape syntax.
Arguments and payloads undergo no interpolation.

2.3. In a payload position, a quoted word or an ordinary bare word supplies
literal text. A bare `@FILE` reads a UTF-8 file, relative to the containing
script directory or to the working directory for a stdin script. A bare
`@-` consumes stdin and may occur once in a file script. A bare `<<TAG`
with a nonempty tag takes every following line up to the first line
equal to `TAG` after removing its LF or CRLF. The terminator itself is
excluded. Body bytes retain their original endings until payload processing
in §3. Multiple heredocs on an instruction are consumed left to right.

2.4. Invalid instruction grammar, including an invalid signature or missing
heredoc terminator, is `invalid-script` with a one-based script line and
UTF-8 byte column. A malformed or unresolvable selector has its own path
refusal and includes the instruction line.

## 3. Operations and source ownership

3.1. Source outside the edited byte spans is preserved verbatim. A node's
physical line span starts at the beginning of the first line of its
document-path extent and extends through the last spanned line, including
its terminator if present. For line-oriented deletion, the span also
consumes one adjacent blank line if present, preferring the following line.
A blank line may contain spaces or tabs.

3.2. New text uses the document's line-ending style, selected as in
[read §2.3](read-spec.md#2-content-mode). LF and CRLF in new payload text
are converted to that style. Bare carriage returns in a payload or
introduced by an edit cause `invalid-payload`. Line-oriented operations
remove trailing line endings from payloads before assembling the edit.
Whole-line replacements keep the terminated or unterminated state of the
replaced span. Insertions at EOF preserve the presence or absence of a
final newline in the original. The amount and placement of synthesized
separator whitespace, within these rules and the structural checks in §4,
are implementation-defined.

3.3. The following operations target a section or the root.

| Operation | Source ownership and result |
| --- | --- |
| `rename TITLE` | Replaces the title spelling of a single-line ATX or Setext heading, preserving its level and style, including a Setext underline. Existing indentation, surrounding spacing, closing hashes, and terminators are kept. Spaces may be synthesized when adding a title to an empty ATX heading. A multiline Setext heading is an invalid target. |
| `delete` | Removes the section's physical line span and adjacent blank line as in §3.1, including descendants and any transparent content inside the span. |
| `insert-section PLACEMENT TITLE` | Inserts an ATX section before the target's heading line, after its subtree's last line, or after its last line as a new last child. The new level is one greater than its physical parent's level (a virtual parent is level zero). Level seven is unavailable. An optional payload is the new section's preamble Markdown and may contain multiple blocks, but no child heading. |
| `append-block` | Inserts one visible direct block before the first child heading line, or after the target's last line if it has no children. |

A title supplies literal heading text under standalone Markdown parsing.
A title containing a line break is `invalid-payload`. The implementation
may escape punctuation to represent it in the chosen heading style.
When the sole H1 is merged into `$`, these operations use that H1 as the
physical target, leaving any source before it outside its span except for
the adjacent-blank deletion rule. Without a merged H1, `append-block`
uses the virtual root's preamble (EOF if there are no sections), and
`insert-section append-child` adds a level-one section at EOF. A virtual
root cannot be renamed or deleted. Sibling placement at `$` is unavailable.

3.4. Operations on a visible direct block (`p`, `list`, `code`,
`quote`, `html`, or `break`) use its physical line span.

| Operation | Source ownership and result |
| --- | --- |
| `delete` | Removes the block, including its nested source, with the adjacent-blank rule in §3.1. |
| `insert-before`, `insert-after` | Inserts one or more visible direct blocks, of any kind, at the corresponding edge of the span. |
| `replace` | Substitutes one or more visible direct blocks, of any kind, for the span. A code target uses the code-specific rule below. |

Block payloads cannot introduce document headings.
For code replacement, a payload consisting of a single fenced code block
spanning the entire normalized payload replaces the entire block, fences
and info string included. Otherwise the payload replaces only the body of
an explicitly closed fenced target. Both existing fence lines are kept.
A nonempty body must include its final line ending and must not close the
fence early. An empty body clears it. An indented or unclosed target is
unavailable for body-only replacement. To write a body that is itself
one whole fenced block, supply the complete replacement block with an
enclosing fence.

3.5. A list also accepts `append-item` and `insert-item INDEX`. The
latter inserts at a zero-based gap through the original direct item count,
inclusive. The last gap is append, at the end of the last item's line
span or the end of the list span. Other gaps are before the indexed item's
marker line. Appending before later document content retains the list's
existing outer separator. A direct item accepts `replace` and `delete`.
Replacement takes the item's physical lines, including nested material,
and leaves external blank lines in place. Deletion follows §3.1, and
deleting the final item removes the list.

Item payloads supply Markdown content without the outer list marker. Inline
markup is accepted. Continuation lines are re-indented to the generated
content column while retaining their relative indentation. Replacement
copies the target marker. Insertion copies the following marker or, at
the end, the preceding marker. The marker includes its original bullet
or ordered numeral and delimiter. Existing sibling markers are preserved.
Synthesized marker padding and item separation are implementation-defined.

3.6. `replace-text` accepts any resolved node.
The old text must have exactly one start position in the target's extent.
The old text is matched literally, including line endings. Empty old text
is `invalid-payload`. Overlapping starts count. The operation owns only the
matched bytes. The new text can be
empty or multiline, and any final line endings it supplies are kept in
the style of §3.2. The extent at `$` is the whole source, which also
allows literals in transparent content such as reference definitions.
Transparent content has no separate structural edit target.

## 4. Batches and structure verification

4.1. A batch is all or nothing. Each operation uses the original document
for its addresses, positions, and literal searches. A rename retains
its old address throughout the batch. Inserted nodes become addressable on
a later invocation. An empty script is an unchanged edit.

4.2. Intersecting nonempty source spans and insertions strictly inside
a nonempty span are `conflicting-edits`. Touching spans are permitted.
Insertions at the start of such a span precede its replacement and those
at the end follow it. Insertions at the same offset are ordered by
instruction order, subject to the structural checks below.

4.3. Before gating, the command MUST parse the assembled candidate and
verify the requested changes against the physical document structure,
including headings that validation would prune for skipped levels. The
verification includes the order, kinds, parentage, and source boundaries
of headings, visible direct blocks, and direct items. The path-root
projection may change. Untouched nodes must retain their structure and
source. A literal replacement may change spelling inside its matching
span without changing the physical structure established by the other
operations. A payload that absorbs a neighbor, merges lists intended
as separate blocks, adds an unexpected heading, or reparents a later
section fails with `invalid-payload`.

The structural check concerns the addressable document model, including
preservation of the frontmatter boundary when present. It does not
guarantee identical rendering, reference resolution, or suppression
attachment for unchanged bytes. The exact equivalence check at ambiguous
parser boundaries is implementation-defined.

## 5. Schema selection and gate

5.1. By default the command uses the discovery walk of
[validator §11.2](outlint-spec.md#112-document-checking-and-schema-selection).
If no schema is found, the edit is ungated. `--schema FILE` selects an
explicit schema and `--ignore-schema` disables the gate. These options
are mutually exclusive. An unreadable or invalid selected schema or
linked resource is an operational error, including with `--force`.

5.2. With a schema, the gate validates both versions under that schema's
Markdown options and the normal suppression rules. Identical text can be
validated once. A candidate diagnostic is new unless it can be paired
with one unused original occurrence of the same violation on the same
subject. Pairing uses the diagnostic id,
responsible schema node when present, target kind, and relevant operands.
Concrete subjects are matched by survival through the edit, including
explicitly renamed headings and byte-identical replacements. An absence
uses its surviving owner and matcher. A frontmatter target uses the
document and pointer when present.
Displayed messages, shifted positions, and shifted sibling ordinals do
not by themselves make a diagnostic new. Unproven correspondence is
treated as new. Correspondence in cases where diagnostic data does not
identify the subject or operands precisely is implementation-defined.
In particular, changed frontmatter may cause delegated JSON Schema or
frontmatter-dependent findings to be counted as new.

5.3. Existing findings may persist or disappear. New findings cause a
`new-diagnostics` refusal unless `--force` is supplied. Force reports
the new diagnostics and bypasses only this comparison. Preview and apply
use the same candidate and gate.

## 6. File IO

6.1. The destination must be valid UTF-8. A leading UTF-8 BOM is excluded
from Markdown parsing and path coordinates and preserved on output. An
input document with bare carriage returns is an operational error.
Symlink destinations and other non-regular files are refused as
operational errors.

6.2. Applying a changed candidate writes a temporary sibling, copies the
original file's permissions, closes the temporary file, and replaces the
destination atomically by rename. If this fails before replacement, the
temporary file is removed and the original is untouched. Preview and
byte-identical results leave the destination in place. Crash durability
and coordination with concurrent writers are outside this contract.

## 7. Output and exits

7.1. Formats follow [search §7](search-spec.md#7-shared-output-format-rules)
for selection, escaping, no color, and JSON framing. Successful human
apply prints the escaped filename, one space, and `changed` or
`unchanged`. Compact apply uses a tab between the two fields. Each ends
with LF. Successful text preview prints a unified diff, or empty stdout
when unchanged. The diff body preserves line bytes and marks a missing
final newline. Only variable filenames in the headers are escaped. Diff
hunk selection and context length are implementation-defined.

7.2. A successful JSON response has these members in order: `version`
(integer 1), `file`, `result` (`changed` or `unchanged`), `paths`
(canonical original targets in operation order), and `schema` (path or
null). Preview adds `diff` (a string, including the empty string when
unchanged), with human escaping for its filenames. If force admits new
diagnostics, the following members are `diagnostic_source` and
`diagnostics`.

7.3. Refusals in human and compact output go to stderr with empty stdout.
The first record begins with the refusal id, a tab, and written detail.
For path failures the detail and continuation lines use
[read §4](read-spec.md#4-path-failures-and-suggestions). For a script
syntax error the detail starts with `LINE:COLUMN`, and for a selector
failure in a script it starts with `line LINE:`. A JSON refusal has
`version`, `file`, and `error`, in that order, on stdout with empty
stderr. The error object includes `id` and a written `message`.
Non-path errors order their members lexicographically by name. Path
errors emit `id`, a written `message` for resolution errors, an optional
script `line`, then the error members of [read §5](read-spec.md#5-json-shapes)
in their documented order. For a syntax error, those shared members
supply the message.

| Refusal id | Condition | Additional details |
| --- | --- | --- |
| `invalid-path` | Malformed document selector | Syntax detail, including byte offset and suggestions |
| `unresolved-path` | No node resolves at a selector step | Resolved prefix, failing step, shallow tree, and suggestions |
| `ambiguous-path` | Multiple matching sections at an unindexed step | Resolved prefix, failing step, all canonical candidates, and empty suggestions |
| `invalid-script` | Malformed script instruction | `line` and `column` |
| `invalid-target` | Operation, heading style, or insertion position unavailable at the target | |
| `invalid-payload` | Payload or candidate violates the operation's requirements | |
| `conflicting-edits` | Incompatible batch spans | |
| `text-not-found` | Old text has no start position in the extent | |
| `ambiguous-text` | Old text has more than one start position in the extent | `count` in JSON and in the text message |
| `new-diagnostics` | Schema gate finds new diagnostics and force is absent | Candidate diagnostics |

7.4. On a gate refusal or forced success, diagnostic document positions
refer to the candidate, including when it was never written. Human
diagnostic lines on stderr use `FILE:LINE:COLUMN (candidate) ID MESSAGE`.
Compact diagnostic records on stderr use six tab-separated fields
`candidate`, `FILE`, `LINE`, `COLUMN`, `ID`, and `MESSAGE`.
Line and column use the validator's location convention. JSON includes
`diagnostic_source: "candidate"` and a `diagnostics` array in the error
object on refusal or at the top level on forced success. Array entries
use the diagnostic objects of validator §11.3. Any schema location still
refers to the schema. Diagnostic order is implementation-defined.

7.5. Exit 0 means success, including help and an unchanged result. Exit 1
means the edit was refused and the file was untouched. Exit 2 means a
usage or operational failure such as invalid UTF-8, schema loading,
file IO, or output failure. Those errors are written as text to stderr
in every format. A usage error before format selection uses human text.
A reporting failure after atomic replacement may produce exit 2 after
the change has been written.

## 8. Worked example (non-normative)

Given this `guide.md` and a discovered `guide.outlint.yml` that allows
a `Setup` section with no child sections:

```markdown
# Guide

## Setup
- Install Outlint.
- Open a terminal.
```

Use the list path from `outlint read --tree --blocks guide.md` to preview
adding an item.

```sh
outlint edit guide.md append-item '$.setup/list[0]' --payload 'Run **outlint check**.' --dry-run
```

```diff
--- guide.md
+++ guide.md
@@ -3,3 +3,4 @@
 ## Setup
 - Install Outlint.
 - Open a terminal.
+- Run **outlint check**.
```

Omit `--dry-run` to apply the same change. With the original file still on disk,
an attempt to add a child section under `Setup` is refused:

```sh
outlint edit guide.md insert-section '$.setup' append-child 'Troubleshooting' --dry-run
```

The refusal reports `new-diagnostics` and locates the unexpected section
at `guide.md:7:1 (candidate)`.
