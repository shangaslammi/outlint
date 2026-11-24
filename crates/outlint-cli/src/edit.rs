//! Edit command IO, presentation and atomic file replacement.
use crate::{
    args::{
        parse_environment_format, parse_read_search_format, EnvironmentFormat,
        ReadSearchFormat as Format,
    },
    diagnostics::{render_document_diagnostic, RenderedDiagnostic},
    render::{self, escape_compact, escape_human},
    schema_loading::{display_path, find_schema, read_and_load_schema},
};
use outlint_core::DocumentReadFailure;
mod destination;
mod engine;
mod script;
use destination::Destination;
use engine::{candidate, Edit, Error, Op, Refusal};
use script::{operation, parse_script, Input, Instruction, ScriptError};
use serde_json::{json, Value};
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

const HELP: &str = "Usage: outlint edit [OPTIONS] FILE OP PATH [ARGS]
       outlint edit [OPTIONS] FILE --script FILE|-

Edit one Markdown file by document path. PATH is required. The leading $ may
be omitted in a single command. Scripts require the leading $ and resolve all
paths in the original document.

Operations (shown with arguments after PATH):
  rename TITLE                               Rename a section heading
  delete                                     Delete a section, block or item
  insert-section before|after|append-child TITLE
                                             Insert a section with an optional preamble
  append-block                               Add one block to a section's preamble
  replace                                    Replace a block or item
  insert-before | insert-after               Insert blocks next to a block
  append-item | insert-item INDEX            Add an item to a list at a zero-based gap
  replace-text                               Replace literal text within any target

Payloads use --payload TEXT (or - for stdin) or --payload-file FILE.
Titles use one line of inline Markdown. Item payloads omit the list marker.
For replace-text, supply the old text with --old TEXT (or - for stdin)
or --old-file FILE.
The old text must have exactly one start position in the target's extent.
Overlapping starts count.
Scripts accept double-quoted text, @file and <<SENTINEL payloads.

Options:
  --schema FILE     Use FILE in place of the nearest discovered schema
  --ignore-schema   Disable schema validation for this edit
  --force           Allow and report new diagnostics
  --dry-run         Preview a unified diff without writing
  --format FORMAT   human, compact or json (default from OUTLINT_FORMAT)
  -h, --help        Show this help
";
const NEW_DIAGNOSTICS_ID: &str = "new-diagnostics";
const NEW_DIAGNOSTICS_MESSAGE: &str = "the edit introduces schema diagnostics";
type Failure = Box<dyn std::error::Error>;
#[derive(Default)]
struct Options {
    dry: bool,
    force: bool,
    ignore: bool,
    schema: Option<PathBuf>,
    script: Option<String>,
    old: Option<Input>,
    new: Option<Input>,
    pos: Vec<String>,
    format: Option<Format>,
}
fn options(args: &[String]) -> Result<Option<Options>, Failure> {
    let mut o = Options::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut val = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{arg} requires a value"))
        };
        match arg.as_str() {
            "--" => {
                o.pos.extend(it.cloned());
                break;
            }
            "-h" | "--help" => return Ok(None),
            "--dry-run" if !o.dry => o.dry = true,
            "--force" if !o.force => o.force = true,
            "--ignore-schema" if !o.ignore => o.ignore = true,
            "--schema" if o.schema.is_none() => o.schema = Some(val()?.into()),
            "--script" if o.script.is_none() => o.script = Some(val()?),
            "--format" => o.format = Some(parse_read_search_format(val()?, "--format")?),
            "--payload" | "--old" | "--payload-file" | "--old-file" => {
                let slot = if arg.starts_with("--old") {
                    &mut o.old
                } else {
                    &mut o.new
                };
                if slot.is_some() {
                    return Err("repeated payload source".into());
                }
                let v = val()?;
                *slot = Some(if arg.ends_with("-file") {
                    Input::File(v)
                } else if v == "-" {
                    Input::Stdin
                } else {
                    Input::Literal(v)
                });
            }
            s if s.starts_with('-') => return Err(format!("unknown or repeated option {s}").into()),
            _ => o.pos.push(arg.clone()),
        }
    }
    if o.ignore && o.schema.is_some() {
        return Err("choose --schema or --ignore-schema".into());
    }
    Ok(Some(o))
}
pub(crate) fn run(args: &[String], env: &EnvironmentFormat) -> u8 {
    let mut fmt = Format::Human;
    let result = (|| {
        let Some(mut o) = options(args)? else {
            return Ok(crate::write_stdout(HELP));
        };
        fmt = match o.format {
            Some(f) => f,
            None => parse_environment_format(env)?,
        };
        if o.pos.is_empty() {
            return Err("expected a destination file (see outlint edit --help)".into());
        }
        let file = o.pos.remove(0);
        execute(&file, o, fmt)
    })();
    match result {
        Ok(code) => code,
        Err(e) => {
            crate::write_stderr(&format!("outlint: {}\n", escape(fmt, &e.to_string())));
            2
        }
    }
}
fn escape(f: Format, s: &str) -> String {
    match f {
        Format::Human => escape_human(s),
        Format::Compact => escape_compact(s),
        Format::Json => s.into(),
    }
}
fn load(name: &str, base: &Path) -> io::Result<String> {
    if name == "-" {
        let mut s = String::new();
        io::stdin().read_to_string(&mut s)?;
        Ok(s)
    } else {
        fs::read_to_string(base.join(name))
    }
}
fn refusal(
    file: &str,
    fmt: Format,
    id: &str,
    message: &str,
    mut details: Value,
) -> Result<u8, Failure> {
    if fmt != Format::Json {
        return text_refusal(id, &escape(fmt, message));
    }
    details["message"] = json!(message);
    details["id"] = json!(id);
    let out = render::edit_json(file, &[("error", details)], None);
    Ok(if crate::write_stdout(&out) == 0 { 1 } else { 2 })
}

fn text_refusal(id: &str, detail: &str) -> Result<u8, Failure> {
    crate::write_stderr(&format!("{id}\t{detail}\n"));
    Ok(1)
}

fn single_instruction(o: &mut Options) -> Result<Instruction, Failure> {
    let [verb, path, args @ ..] = o.pos.as_slice() else {
        return Err("expected an operation and document path (see outlint edit --help)".into());
    };
    if (verb == "replace-text") != o.old.is_some() {
        return Err(if verb == "replace-text" {
            "replace-text requires --old or --old-file"
        } else {
            "--old and --old-file are only valid for replace-text"
        }
        .into());
    }
    let inputs: Vec<_> = o.old.take().into_iter().chain(o.new.take()).collect();
    if inputs
        .iter()
        .filter(|input| matches!(input, Input::Stdin))
        .count()
        > 1
    {
        return Err("stdin already used".into());
    }
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    let op = operation(verb, &args, inputs)
        .ok_or_else(|| format!("invalid arguments for {verb} (see outlint edit --help)"))?;
    Ok(Instruction {
        line: None,
        path: crate::args::path_spelling(path),
        op,
    })
}

fn instructions(
    o: &mut Options,
) -> Result<(Result<Vec<Instruction>, ScriptError>, PathBuf), Failure> {
    let cwd = Path::new(".");
    let Some(script) = &o.script else {
        return Ok((Ok(vec![single_instruction(o)?]), cwd.into()));
    };
    if !o.pos.is_empty() || o.old.is_some() || o.new.is_some() {
        return Err("--script cannot be combined with a single operation or payload flags".into());
    }
    let bytes =
        load(script, cwd).map_err(|error| format!("cannot read script {script}: {error}"))?;
    let base = Path::new(script).parent().unwrap_or(cwd).to_path_buf();
    Ok((parse_script(&bytes, script == "-"), base))
}

impl<T> Op<T> {
    fn map_inputs<U, E>(self, mut load: impl FnMut(T) -> Result<U, E>) -> Result<Op<U>, E> {
        Ok(match self {
            Self::ReplaceText { old, new } => Op::ReplaceText {
                old: load(old)?,
                new: load(new)?,
            },
            Self::Rename(title) => Op::Rename(title),
            Self::Delete => Op::Delete,
            Self::InsertSection {
                placement,
                title,
                preamble,
            } => Op::InsertSection {
                placement,
                title,
                preamble: preamble.map(load).transpose()?,
            },
            Self::AppendBlock(payload) => Op::AppendBlock(load(payload)?),
            Self::Replace(payload) => Op::Replace(load(payload)?),
            Self::InsertBefore(payload) => Op::InsertBefore(load(payload)?),
            Self::InsertAfter(payload) => Op::InsertAfter(load(payload)?),
            Self::AppendItem(payload) => Op::AppendItem(load(payload)?),
            Self::InsertItem { index, payload } => Op::InsertItem {
                index,
                payload: load(payload)?,
            },
        })
    }
}

fn load_input(input: Input, base: &Path) -> Result<String, Failure> {
    let mut text = match input {
        Input::Literal(text) => return Ok(text),
        Input::File(name) => {
            let path = base.join(name);
            fs::read_to_string(&path)
                .map_err(|error| format!("cannot read payload {}: {error}", path.display()))?
        }
        Input::Stdin => {
            load("-", base).map_err(|error| format!("cannot read payload from stdin: {error}"))?
        }
    };
    if text.starts_with('\u{feff}') {
        text.drain(..'\u{feff}'.len_utf8());
    }
    Ok(text)
}

fn edit_refusal(
    file: &str,
    fmt: Format,
    failure: Refusal,
    lines: &[Option<usize>],
) -> Result<u8, Failure> {
    let line_at = |index| lines.get(index).copied().flatten();
    let line = failure.operation.and_then(line_at);
    let path = failure.path;
    let (id, message, mut detail) = match failure.reason {
        Error::Payload => (
            "invalid-payload",
            "the payload does not have the required form".into(),
            json!({"reason": "payload-form"}),
        ),
        Error::Structure => (
            "invalid-payload",
            "the resulting Markdown changes the document structure unexpectedly".into(),
            json!({"reason": "candidate-structure"}),
        ),
        Error::BareCr => (
            "invalid-payload",
            "the payload or candidate contains a carriage return without a following line feed"
                .into(),
            json!({"reason": "bare-carriage-return"}),
        ),
        Error::Target => (
            "invalid-target",
            "the selected node or position does not support the requested operation".into(),
            json!({}),
        ),
        Error::Conflict(other) => {
            let other_line = line_at(other);
            let message = other_line.map_or_else(
                || "the operations conflict".to_owned(),
                |line| format!("the operation conflicts with the instruction on line {line}"),
            );
            (
                "conflicting-edits",
                message,
                json!({"other_line": other_line}),
            )
        }
        Error::TextNotFound => (
            "text-not-found",
            format!(
                "old text does not occur in `{}`",
                path.as_deref().unwrap_or_default()
            ),
            json!({}),
        ),
        Error::TextAmbiguous(count) => (
            "ambiguous-text",
            format!(
                "old text occurs {count} times in `{}`",
                path.as_deref().unwrap_or_default()
            ),
            json!({"count":count}),
        ),
        Error::BareCrSource => {
            return Err(format!(
                "destination {file} contains a carriage return without a following line feed"
            )
            .into())
        }
        Error::Path(error) => return path_refusal(file, fmt, &error, line),
        Error::Range => return Err(
            "cannot compute the edit because Markdown parsing or source ranges were inconsistent"
                .into(),
        ),
    };
    if let Some(path) = path {
        detail["path"] = json!(path);
        if let Some(line) = line {
            detail["line"] = json!(line);
            if fmt != Format::Json {
                return text_refusal(
                    id,
                    &format!(
                        "line {line}: {}: {}",
                        escape(fmt, &path),
                        escape(fmt, &message)
                    ),
                );
            }
        }
    }
    refusal(file, fmt, id, &message, detail)
}

fn path_refusal(
    file: &str,
    fmt: Format,
    failure: &DocumentReadFailure,
    line: Option<usize>,
) -> Result<u8, Failure> {
    let id = match failure {
        DocumentReadFailure::Syntax { .. } => "invalid-path",
        DocumentReadFailure::Ambiguous { .. } => "ambiguous-path",
        DocumentReadFailure::Unresolved { .. } => "unresolved-path",
        other => return Err(other.to_string().into()),
    };
    if fmt == Format::Json {
        let out = render::edit_json(file, &[], Some((failure, id, line)));
        return Ok(if crate::write_stdout(&out) == 0 { 1 } else { 2 });
    }
    let detail = render::read::error(failure, file, fmt);
    let prefix = line.map_or_else(String::new, |line| format!("line {line}: "));
    crate::write_stderr(&format!("{id}\t{prefix}{detail}"));
    Ok(1)
}

fn execute(file: &str, mut o: Options, fmt: Format) -> Result<u8, Failure> {
    let target = Path::new(file);
    let destination = Destination::read(target)
        .map_err(|error| format!("cannot open destination {file} as UTF-8: {error}"))?;
    let (instructions, base) = instructions(&mut o)?;
    let instructions = match instructions {
        Ok(instructions) => instructions,
        Err(error) if fmt == Format::Json => {
            return refusal(
                file,
                fmt,
                "invalid-script",
                &error.message,
                json!({"line":error.line,"column":error.column}),
            )
        }
        Err(error) => {
            return text_refusal(
                "invalid-script",
                &format!(
                    "{}:{} {}",
                    error.line,
                    error.column,
                    escape(fmt, &error.message)
                ),
            )
        }
    };
    let lines: Vec<_> = instructions
        .iter()
        .map(|instruction| instruction.line)
        .collect();
    let edits = instructions
        .into_iter()
        .map(|instruction| {
            Ok(Edit {
                path: instruction.path,
                op: instruction
                    .op
                    .map_inputs(|input| load_input(input, &base))?,
            })
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    let c = match candidate(&destination.source, &edits) {
        Ok(candidate) => candidate,
        Err(error) => return edit_refusal(file, fmt, error, &lines),
    };
    let schema = if o.ignore {
        None
    } else if o.schema.is_some() {
        o.schema
    } else {
        find_schema(target)?
    };
    let schema_label = schema.as_deref().map(display_path);
    let diagnostics = if let Some((path, name)) = schema.as_deref().zip(schema_label.as_deref()) {
        let loaded = read_and_load_schema(path, name)?
            .map_err(|invalid| format!("invalid schema {name}: {invalid}"))?;
        c.introduced_diagnostics(&loaded.schema)?
            .iter()
            .map(|diagnostic| render_document_diagnostic(file, diagnostic, &loaded))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if !diagnostics.is_empty() && fmt != Format::Json {
        if !o.force {
            crate::write_stderr(&format!(
                "{NEW_DIAGNOSTICS_ID}\t{NEW_DIAGNOSTICS_MESSAGE}\n"
            ));
        }
        for diagnostic in &diagnostics {
            crate::write_stderr(&format!("{}\n", candidate_diagnostic_text(diagnostic, fmt)));
        }
    }
    if !diagnostics.is_empty() && !o.force {
        if fmt == Format::Json {
            return refusal(
                file,
                fmt,
                NEW_DIAGNOSTICS_ID,
                NEW_DIAGNOSTICS_MESSAGE,
                json!({
                    "diagnostic_source": "candidate",
                    "diagnostics": diagnostic_values(&diagnostics),
                }),
            );
        }
        return Ok(1);
    }
    let changed = destination.source != c.text;
    let result = if changed { "changed" } else { "unchanged" };
    let preview = o.dry.then(|| {
        diff(
            &escape(
                if fmt == Format::Json {
                    Format::Human
                } else {
                    fmt
                },
                file,
            ),
            &destination.source,
            &c.text,
        )
    });
    if changed && !o.dry {
        destination
            .replace(&c.text)
            .map_err(|error| format!("cannot replace destination {file}: {error}"))?;
    }
    let out = if fmt == Format::Json {
        let mut fields = vec![
            ("result", json!(result)),
            ("paths", json!(c.paths)),
            ("schema", json!(schema_label)),
        ];
        if let Some(p) = preview {
            fields.push(("diff", json!(p)))
        }
        if !diagnostics.is_empty() {
            fields.push(("diagnostic_source", json!("candidate")));
            fields.push(("diagnostics", json!(diagnostic_values(&diagnostics))));
        }
        render::edit_json(file, &fields, None)
    } else if let Some(p) = preview {
        p
    } else {
        format!(
            "{}{}{result}\n",
            escape(fmt, file),
            if fmt == Format::Compact { "\t" } else { " " }
        )
    };
    Ok(crate::write_stdout(&out))
}

fn candidate_diagnostic_text(d: &RenderedDiagnostic, fmt: Format) -> String {
    let file = escape(fmt, &d.source_path);
    let message = escape(fmt, &d.message);
    let (line, column, id) = (d.line, d.column, &d.id);
    if fmt == Format::Compact {
        format!("candidate\t{file}\t{line}\t{column}\t{id}\t{message}")
    } else {
        format!("{file}:{line}:{column} (candidate) {id} {message}")
    }
}

fn diagnostic_values(diagnostics: &[RenderedDiagnostic]) -> Vec<Value> {
    diagnostics
        .iter()
        .map(render::json::diagnostic_json)
        .collect()
}
// A single contiguous change with up to three equal context lines on each side.
fn diff(file: &str, a: &str, b: &str) -> String {
    if a == b {
        return String::new();
    }
    let a: Vec<_> = a.split_inclusive('\n').collect();
    let b: Vec<_> = b.split_inclusive('\n').collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let start = prefix.saturating_sub(3);
    let end_a = a.len() - suffix;
    let end_b = b.len() - suffix;
    let context = suffix.min(3);
    let range = |end: usize| {
        let count = end + context - start;
        format!("{},{}", start + usize::from(count > 0), count)
    };
    let mut out = format!(
        "--- {file}\n+++ {file}\n@@ -{} +{} @@\n",
        range(end_a),
        range(end_b)
    );
    for (prefix, lines) in [
        (' ', &a[start..prefix]),
        ('-', &a[prefix..end_a]),
        ('+', &b[prefix..end_b]),
        (' ', &a[end_a..end_a + context]),
    ] {
        for line in lines {
            out.push(prefix);
            out.push_str(line);
            if !line.ends_with('\n') {
                out.push_str("\n\\ No newline at end of file\n");
            }
        }
    }
    out
}
