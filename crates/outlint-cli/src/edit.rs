//! Edit command IO, presentation and atomic file replacement.
use crate::{
    args::{
        parse_environment_format, parse_read_search_format, EnvironmentFormat,
        ReadSearchFormat as Format,
    },
    diagnostics::{render_document_diagnostic, RenderedDiagnostic},
    render::{self, escape_compact, escape_human},
    schema_loading::{find_schema, read_and_load_schema},
};
use outlint_core::{document_tree, parse_markdown, DocumentPath, DocumentReadFailure};
use outlint_edit::{
    candidate,
    script::{operation, parse_script, Input, Instruction},
    Edit, Error,
};
use serde_json::{json, Value};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

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
Item payloads omit the list marker. For replace-text, supply the old text
with --old TEXT (or - for stdin) or --old-file FILE.
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
fn options(args: &[String]) -> Result<Options, Failure> {
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
    Ok(o)
}
pub(crate) fn run(args: &[String], env: &EnvironmentFormat) -> u8 {
    if matches!(args,[a] if a=="--help"||a=="-h") {
        return crate::write_stdout(HELP);
    }
    let mut fmt = Format::Human;
    let result = (|| {
        let mut o = options(args)?;
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
            crate::write_stderr(&format!("outlint edit: {}\n", escape(fmt, &e.to_string())));
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
    writeln!(io::stderr().lock(), "{id}\t{detail}")?;
    Ok(1)
}

fn execute(file: &str, o: Options, fmt: Format) -> Result<u8, Failure> {
    let target = Path::new(file);
    let metadata = fs::symlink_metadata(target)
        .map_err(|e| format!("cannot inspect destination {file}: {e}"))?;
    if !metadata.is_file() {
        return Err(format!("destination {file} must be a regular file").into());
    }
    let source = fs::read_to_string(target)
        .map_err(|e| format!("cannot read destination {file} as UTF-8: {e}"))?;
    let clean = source.strip_prefix('\u{feff}').unwrap_or(&source);
    let doc = parse_markdown(clean, Default::default())?;
    let cwd = Path::new(".");
    let mut base = cwd;
    let instructions = if let Some(script) = &o.script {
        if !o.pos.is_empty() || o.old.is_some() || o.new.is_some() {
            return Err(
                "--script cannot be combined with a single operation or payload flags".into(),
            );
        }
        let s = load(script, cwd).map_err(|e| format!("cannot read script {script}: {e}"))?;
        if script != "-" {
            base = Path::new(script).parent().unwrap_or(cwd)
        }
        match parse_script(&s, script == "-") {
            Ok(v) => v,
            Err(e) if fmt == Format::Json => {
                return refusal(
                    file,
                    fmt,
                    "invalid-script",
                    &e.message,
                    json!({"line":e.line,"column":e.column}),
                )
            }
            Err(e) => {
                let detail = format!("{}:{} {}", e.line, e.column, escape(fmt, &e.message));
                return text_refusal("invalid-script", &detail);
            }
        }
    } else {
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
        let inputs: Vec<_> = o.old.into_iter().chain(o.new).collect();
        if inputs.iter().filter(|i| matches!(i, Input::Stdin)).count() > 1 {
            return Err("stdin already used".into());
        }
        vec![Instruction {
            verb: verb.clone(),
            path: crate::args::path_spelling(path),
            args: args.into(),
            inputs,
            line: 0,
        }]
    };
    let schema = if o.ignore {
        None
    } else if o.schema.is_some() {
        o.schema
    } else {
        find_schema(target)?
    };
    let loaded = schema
        .as_ref()
        .map(|p| -> Result<_, Failure> {
            let name = p.to_string_lossy();
            read_and_load_schema(p, &name)?
                .map_err(|invalid| format!("invalid schema {name}: {invalid}").into())
        })
        .transpose()?;
    let mut edits = vec![];
    let mut paths = vec![];
    for cmd in instructions {
        let canonical = match document_tree(clean, &doc, &cmd.path, Some(0), false) {
            Ok(tree) => tree.mdpath,
            Err(failure) => {
                let id = match failure.as_ref() {
                    DocumentReadFailure::Syntax { .. } => "invalid-path",
                    DocumentReadFailure::Ambiguous { .. } => "ambiguous-path",
                    DocumentReadFailure::Unresolved { .. } => "unresolved-path",
                    other => return Err(other.to_string().into()),
                };
                if fmt == Format::Json {
                    let out = render::edit_json(file, &[], Some((&failure, id, cmd.line)));
                    return Ok(if crate::write_stdout(&out) == 0 { 1 } else { 2 });
                }
                let mut text = render::read::error(&failure, file, fmt);
                if cmd.line != 0 {
                    text = format!("line {}: {text}", cmd.line)
                }
                io::stderr()
                    .lock()
                    .write_all(format!("{id}\t{text}").as_bytes())?;
                return Ok(1);
            }
        };
        let path = DocumentPath::from(&canonical);
        paths.push(canonical.to_string());
        let inputs = cmd
            .inputs
            .into_iter()
            .map(|i| match i {
                Input::Literal(s) => Ok(s),
                Input::File(name) => {
                    let path = base.join(name);
                    fs::read_to_string(&path)
                        .map_err(|e| format!("cannot read payload {}: {e}", path.display()))
                }
                Input::Stdin => {
                    load("-", base).map_err(|e| format!("cannot read payload from stdin: {e}"))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let op = operation(&cmd.verb, &cmd.args, &inputs).ok_or_else(|| {
            format!(
                "invalid arguments for {} (see outlint edit --help)",
                cmd.verb
            )
        })?;
        edits.push(Edit { path, op });
    }
    let c = match candidate(&source, &edits) {
        Ok(c) => c,
        Err(e) => {
            let (id, message, detail) = match e {
                Error::Payload => (
                    "invalid-payload",
                    "the payload or resulting Markdown does not have the required structure".into(),
                    json!({}),
                ),
                Error::BareCr => (
                    "invalid-payload",
                    "the payload or candidate contains a carriage return without a following line feed"
                        .into(),
                    json!({}),
                ),
                Error::Target => (
                    "invalid-target",
                    "the selected node or position does not support the requested operation".into(),
                    json!({}),
                ),
                Error::ConflictingEdits => (
                    "conflicting-edits",
                    "the operations edit overlapping source spans".into(),
                    json!({}),
                ),
                Error::TextNotFound(path) => (
                    "text-not-found",
                    format!("old text does not occur in `{path}`"),
                    json!({}),
                ),
                Error::TextAmbiguous(path, count) => (
                    "ambiguous-text",
                    format!("old text occurs {count} times in `{path}`"),
                    json!({"count": count}),
                ),
                Error::BareCrSource => {
                    let message = "contains a carriage return without a following line feed";
                    return Err(format!("destination {file} {message}").into());
                }
                Error::Path(error) => return Err(error.to_string().into()),
                Error::Range => {
                    return Err(
                        "cannot compute the edit because Markdown parsing or source ranges were inconsistent".into(),
                    )
                }
            };
            return refusal(file, fmt, id, &message, detail);
        }
    };
    let mut diagnostics = vec![];
    if let Some(loaded) = loaded {
        for d in c.introduced_diagnostics(&loaded.schema)? {
            diagnostics.push(render_document_diagnostic(file, &d, &loaded));
        }
    }
    if !diagnostics.is_empty() && fmt != Format::Json {
        let mut stderr = io::stderr().lock();
        if !o.force {
            writeln!(stderr, "{NEW_DIAGNOSTICS_ID}\t{NEW_DIAGNOSTICS_MESSAGE}")?;
        }
        for d in &diagnostics {
            writeln!(stderr, "{}", candidate_diagnostic_text(d, fmt))?;
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
    let changed = source != c.text();
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
            &source,
            c.text(),
        )
    });
    if changed && !o.dry {
        let mut temp = NamedTempFile::new_in(target.parent().unwrap_or(cwd))?;
        temp.write_all(c.text().as_bytes())?;
        temp.as_file().set_permissions(metadata.permissions())?;
        temp.into_temp_path().persist(target)?;
    }
    let out = if fmt == Format::Json {
        let mut fields = vec![
            ("result", json!(result)),
            ("paths", json!(paths)),
            (
                "schema",
                json!(schema.as_deref().map(Path::to_string_lossy)),
            ),
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
