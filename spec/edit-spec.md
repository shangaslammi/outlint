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
`--`. Option values are separate arguments. `-h` and `--help` print help to
stdout when given as options.

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
`-file` option treats `-` as a literal filename. Payloads read from a
file or stdin discard one leading UTF-8 BOM.

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
`--script -`. One leading UTF-8 BOM is discarded. Each instruction starts
on its own LF- or CRLF-delimited line and follows the grammar below. The
final instruction line may be
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
ASCII decimal integer in the canonical index spelling of
[document paths §2.2](document-path-spec.md#2-input-grammar), with an
implementation-defined maximum. Single-operation mode
uses the same signatures, with payload operands supplied as in §1.3.

2.2. ASCII spaces and tabs separate words. A bare word extends up to the
next separator. A word starting with `"` ends at a closing `"` and must
then be followed by a separator or the end of the line. The only escapes
inside a quoted word are `\"` (a quote) and `\\` (a backslash).
The delimiter quotes are removed. Bare words have no escape syntax.
Arguments and payloads undergo no interpolation.

2.3. In a payload position, a quoted word or an ordinary bare word supplies
literal text. `TITLE` is an ordinary word even when it starts with `@` or
`<<`. A bare `@FILE` reads a UTF-8 file, relative to the containing
script directory or to the working directory for a stdin script. The
BOM rule in §1.3 also applies to these payloads. A bare
`@-` consumes stdin and may occur once in a file script. A bare `<<TAG`
with a nonempty tag takes every following line up to the first line
equal to `TAG` after removing its LF or CRLF. The terminator itself is
excluded. Body bytes retain their original endings until payload processing
in §3, except that each `replace-text` heredoc drops one final LF or
CRLF. Multiple heredocs on an instruction are consumed left to right.

2.4. Invalid instruction grammar, including an invalid signature or missing
heredoc terminator, is `invalid-script` with a one-based script line and
UTF-8 byte column. A malformed or unresolvable selector has its own path
refusal and includes the instruction line.

## 3. Operations and source ownership

3.1. An edit computes owned source spans from the original document.
Spans are half-open and use physical lines, including a line's terminator
when present. Sibling spans are disjoint. Container spans include their
descendants. Read extents remain as defined by the document-path specification.

| Target | Physical source owned by the target |
| --- | --- |
| Section | Its leading gap, attached suppression lines, heading lines, and descendants through the end of its last child or preamble block. |
| Direct block other than a list | Its leading gap, attached suppression lines, and all its physical lines, including nested source. |
| List | Its outer leading gap and the spans of its direct items. Suppression lines shared by the list and its first item belong to the first item's span and are removed when that item is deleted. |
| Direct list item | Its marker line and all content lines, including nested source. A tight item has no leading gap. In a loose list, each item after the first owns the blank-line run before it, if present. An attached suppression run follows the item it precedes, including when indented as part of the preceding item. The first item's attached suppression run is also at the start of the list body. |

A leading gap is the entire contiguous run of blank lines, including
space- or tab-only lines, immediately before the node and its attached
suppression lines. When an inline suppression is attached to a node under
validator §6.3, the node owns the entire contiguous run of standalone
inline suppression comments ending on the previous line. Other transparent
source outside the span remains verbatim. A gap cannot extend across a
transparent comment or reference definition. Leading blank lines at file
start and blank lines trailing at EOF are unowned and stay verbatim. The
root's frontmatter also remains outside structural edits.

The first node at file start and the first item in a list have no leading
gap. If that node is deleted, the first surviving node in the sequence
loses its leading gap. The outer gap of a surviving list stays with the
list. Deleting every item removes the list and its outer gap. A loose
list can become tight when its separating blank lines are deleted.

Deletion removes the owned span and replacement retains its gap and
attached suppression lines. An insertion before a node goes at the start
of its span. An insertion after a node goes at the end of its span.
Inserted content copies the following node's gap at a shared boundary,
or the last sibling's gap at the insertion depth at the end of a sequence.
At file start, an insertion before the first node copies the next sibling's
gap and places it after the inserted content. Consecutive inserts may each
copy the same gap. A gap is widened when the new neighbours need a
separator to keep the requested structure or to keep a standalone inline
directive on the left detached. An empty gap stays empty when the new
neighbours still parse separately with the intended attachments. If
deletion leaves the successor with too small a gap, retain a sufficient
gap from the deleted span when available, or synthesize
the separator. For example, deleting a code block or thematic break
between paragraphs must keep the paragraphs distinct. A separator includes
the terminator of the preceding line before any whitespace-only blank line.
Structural verification can still refuse a seam that Markdown cannot
express as the requested nodes.

3.2. Payload text, and both operands of `replace-text`, use the document's
line-ending style selected as in [read §2.3](read-spec.md#2-content-mode).
LF and CRLF are converted to that style. Bare carriage returns in a payload
or introduced by an edit cause `invalid-payload`. Block payloads and section
preambles discard outer blank lines. Line-oriented replacements keep the
terminated or unterminated state of the replaced span. Insertions at EOF
preserve the presence or absence of a final newline in the original.
Deletion can expose a preceding line's existing terminator at EOF.
Synthesized separator whitespace is implementation-defined within these
rules and the structural guarantees in §4.

3.3. The following operations target a section or the root.

| Operation | Source ownership and result |
| --- | --- |
| `rename TITLE` | Replaces the title spelling of a single-line ATX or Setext heading, preserving its level and style, including a Setext underline. Existing indentation, surrounding spacing, closing hashes, and terminators are kept. Spaces may be synthesized when adding a title to an empty ATX heading. A multiline Setext heading is an invalid target. |
| `delete` | Removes the section's owned span (§3.1), including descendants and any transparent content inside the span. |
| `insert-section PLACEMENT TITLE` | Inserts an ATX section before or after the target's subtree at the target heading's level, or as its last child one level deeper. A virtual parent is level zero. Level seven is unavailable. An optional payload is the new section's preamble Markdown and may contain multiple blocks, but no child heading. |
| `append-block` | Inserts one or more visible direct blocks in the preamble's last slot, before the first child's owned span if present. |

A title is a single line of inline Markdown, inserted as supplied. It must
form the heading's title in the selected heading syntax. Escape Markdown
punctuation in the supplied title when needed, for example to keep
trailing hashes as ATX title text. Inline rendering, including link
references, uses the candidate's surrounding document.
When the sole H1 is merged into `$`, these operations use that H1 as the
physical target. Source before that heading's owned span stays in place.
Without a merged H1, `append-block` uses the virtual root's preamble. With no sections, it appends at the end
of the source before any trailing blank lines. The virtual root's
`insert-section append-child` adds a level-one section at the end of the
source before any trailing blank lines. A virtual root cannot be renamed
or deleted. Sibling placement at `$` is unavailable.

3.4. Operations on a visible direct block (`p`, `list`, `code`,
`quote`, `html`, or `break`) use its owned span.

| Operation | Source ownership and result |
| --- | --- |
| `delete` | Removes the block’s owned span, including its nested source (§3.1). |
| `insert-before`, `insert-after` | Inserts one or more visible direct blocks, of any kind, at the corresponding edge of the span. |
| `replace` | Substitutes one or more visible direct blocks, of any kind, for the block body. A code target uses the code-specific rule below. |

Block payloads cannot introduce document headings. They may include
transparent content such as reference definitions and suppression comments
alongside visible blocks. Added suppression comments are subject to §5.3.
For code replacement, a single explicitly closed fenced code block spanning
the normalized payload replaces the entire block, fences and info string
included. Otherwise the payload replaces only the body of
an explicitly closed fenced target. Both existing fence lines are kept.
A nonempty body must include its final line ending and must not close the
fence early. An empty body clears it. An indented or unclosed target is
unavailable for body-only replacement. To write a body that is itself
one whole fenced block, supply the complete replacement block with an
enclosing fence.

3.5. A list also accepts `append-item` and `insert-item INDEX`. The
latter inserts at a zero-based gap through the original direct item count,
inclusive. The last gap appends inside the list, after its last item.
Other gaps precede the indexed item's owned span. Inserting at gap zero
in a list whose first item has an attached suppression run is
`invalid-target`. This preserves the existing directive and avoids
splitting the list. A direct item accepts `replace` and `delete`.
Replacement takes the item's physical content, including nested material,
and retains its gap. Deletion follows §3.1.

Item payloads supply Markdown content without the outer list marker. Inline
markup is accepted. Continuation lines are re-indented to the generated
content column while retaining their relative indentation. Replacement
copies the target marker and its existing padding. Insertion copies the
following marker or, at the end, the preceding marker. The marker includes its original bullet
or ordered numeral and delimiter. Existing sibling markers are preserved.
An insertion can copy the indexed item’s gap, the last item’s gap on
append, or the next inter-item gap when inserting at gap zero. In a tight
list only the line terminator separates adjacent items.

3.6. `replace-text` accepts any resolved node.
The old text must have exactly one start position in the target's extent.
Both operands undergo line-ending conversion (§3.2) before the old text
is matched literally. Empty old text is `invalid-payload`. Overlapping
starts count. For a non-root target, the document-path and validator
range rules determine whether the last line's terminator is part of the
extent. The operation owns only the matched bytes. The new text can be
empty or multiline, and any final line endings it supplies are kept in the
style of §3.2. The extent at `$` is the whole source, which also
allows literals in transparent content such as reference definitions.
Transparent content has no separate structural edit target.

## 4. Batches and structure verification

4.1. A batch is all or nothing. Each operation uses the original document
for its addresses, positions, and literal searches. A rename retains
its old address throughout the batch. Inserted nodes become addressable on
a later invocation. An empty script is an unchanged edit.

4.2. Conflicts are intersections of the original owned edit spans.
Structural deletion and replacement use the target's owned span, rename
uses the title span, and literal replacement uses the matched bytes.
Two renames of the same empty title conflict. Touching spans are
permitted. A zero-width insertion conflicts when its slot lies inside a
deleted or replaced node, including at a shared byte offset for the
opening or closing boundary. Insertions beside a deleted or replaced
target are permitted. Choosing separators for the resulting seams leaves
the conflict test unchanged.

View the physical tree as opening and closing boundaries in document
order, with a boundary between a section's preamble and child sections,
even when either is empty. The root also has a preamble boundary.
Unowned transparent source can separate neighbouring slots. Every
insertion belongs to a slot between consecutive boundaries. That slot
fixes the insertion's parent. Insertions follow slot order, with script
order within a slot. For example, after the last preamble block of A,
`insert-after` on that block shares a slot with `append-block` on A.
This slot precedes an `append-child` insertion and a sibling inserted
after A, even when all use the same byte offset.

4.3. The result must have the requested physical structure, including
headings that validation would prune for skipped levels. The check compares
the parent of every inserted heading, block, and item with the parent
selected by its slot, including for inserted subtrees. Untouched nodes
retain their order, kinds, parentage, source boundaries, and spelling.
The path-root projection may change. Structural edits preserve the
suppression attachments on untouched nodes. A literal replacement may
change spelling inside its matched span while preserving the physical
structure established by the other operations, including adding blank lines
after a node's last line. An operation that absorbs a neighbour, merges
lists intended as separate blocks, adds an unexpected
heading, or reparents a later section fails with `invalid-payload` and
reason `candidate-structure`, including when the operation has no payload.

The check includes the frontmatter boundary. Inline rendering and
reference resolution may change when the underlying Markdown changes.
Equivalence at ambiguous parser boundaries is implementation-defined.

## 5. Schema selection and gate

5.1. By default the command uses the discovery walk of
[validator §11.2](outlint-spec.md#112-document-checking-and-schema-selection).
If no schema is found, the edit skips the schema gate. `--schema FILE`
selects an explicit schema and `--ignore-schema` skips the gate.
These options are mutually exclusive. An unreadable or invalid selected schema or
linked resource is an operational error, including with `--force`.

5.2. With a schema, the gate validates both versions under that schema's
Markdown options and the normal suppression rules. A candidate
diagnostic is new unless it can be paired
with one unused original occurrence of the same violation on the same
subject. Pairing uses the diagnostic id,
responsible schema node when present, target kind, and relevant operands.
Concrete subjects are matched by survival through the edit, including
explicitly renamed headings, nodes preserved by literal edits, and
byte-identical replacements. An absence uses its surviving owner and
matcher. A frontmatter target uses the document and pointer when present.
Displayed messages, shifted positions, and shifted sibling ordinals do
not by themselves make a diagnostic new. Unproven correspondence is
treated as new. Correspondence in cases where diagnostic data does not
identify the subject or operands precisely is implementation-defined.
In particular, changed frontmatter may cause delegated JSON Schema or
frontmatter-dependent findings to be counted as new.

5.3. When the schema gate runs, an `outlint-disable` or `outlint-disable-file`
comment in the candidate counts as a new diagnostic unless paired with an unused original
comment with the same scope and diagnostic ids. Inline comment pairing
requires correspondence through the edit. File-wide comments may pair
across locations. Repeated comments count separately, even when the extra
copies would suppress the same findings. The diagnostic id is
`added-suppression`, with the candidate comment's position, a document
target, and no schema location.

5.4. Existing findings may persist or disappear. New findings cause a
`new-diagnostics` refusal unless `--force` is supplied. Force reports
the new diagnostics and leaves structural verification, destination checks,
and schema loading in effect. Preview and apply use the same candidate
and gate.

## 6. File IO

6.1. The destination must be valid UTF-8. A leading UTF-8 BOM is excluded
from Markdown parsing and path coordinates and preserved on output. An
input document with bare carriage returns is an operational error.
Both preview and apply require write access to the destination and refuse
read-only files, symlinks, non-regular files, and files with multiple hard
links as operational errors. Read-only and multiple-link refusals name
the destination and its condition.

6.2. A changed candidate is applied by atomic replacement, retaining
the original permissions. If replacement fails, the original stays
untouched and temporary files are cleaned up. Preview and byte-identical
results leave the destination in place. Crash durability and coordination
with concurrent writers are outside this contract.

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
(canonical original targets in operation order), and `schema` (a path
displayed as in `outlint check`, or null). Preview adds `diff` (a string,
including the empty string when unchanged), with human escaping for its
filenames. If force admits new diagnostics, the following members are
`diagnostic_source` and `diagnostics`.

7.3. Refusals in human and compact output go to stderr with empty stdout.
The first record begins with the refusal id, a tab, and written detail.
For path failures the detail and continuation lines use
[read §4](read-spec.md#4-path-failures-and-suggestions). For a script
syntax error the detail starts with `LINE:COLUMN`, and for a refusal
attributable to one script instruction it starts with `line LINE:`. A JSON refusal has
`version`, `file`, and `error`, in that order, on stdout with empty
stderr. The error object includes `id` and a written `message`.
Path errors emit `id`, a written `message` for resolution errors, an optional
script `line`, then the error members of [read §5](read-spec.md#5-json-shapes)
in their documented order. For a syntax error, those shared members
supply the message. Other refusals tied to one operation include its
canonical `path` and, in a script, its `line`. A two-operation conflict
identifies the later instruction and includes the earlier `other_line`.
Failures attributable only to the batch omit the individual attribution.

| Refusal id | Condition | Additional details |
| --- | --- | --- |
| `invalid-path` | Malformed document selector | Syntax detail, including byte offset and suggestions |
| `unresolved-path` | No node resolves at a selector step | Resolved prefix, failing step, shallow tree, and suggestions |
| `ambiguous-path` | Multiple matching sections at an unindexed step | Resolved prefix, failing step, all canonical candidates, and empty suggestions |
| `invalid-script` | Malformed script instruction | `line` and `column` |
| `invalid-target` | Operation, heading style, or insertion position unavailable at the target | |
| `invalid-payload` | Payload or contextual structure violates the operation's requirements | `reason` is `payload-form`, `candidate-structure`, or `bare-carriage-return` |
| `conflicting-edits` | Incompatible batch edits under §4.2 | |
| `text-not-found` | Old text has no start position in the extent | |
| `ambiguous-text` | Old text has more than one start position in the extent | `count` in JSON and in the text message |
| `new-diagnostics` | The gate finds new diagnostics or added suppressions and force is absent | Candidate diagnostics |

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
file IO, or output failure. Those errors are written as text to stderr with
the `outlint:` prefix in every format. Failed stderr reporting preserves the
original exit status. A usage error before format selection uses human text.
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
at `guide.md:6:1 (candidate)`.
