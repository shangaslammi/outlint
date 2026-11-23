//! Edit command IO, presentation and atomic file replacement.
use crate::{
    args::{
        parse_environment_format, parse_read_search_format, EnvironmentFormat,
        ReadSearchFormat as Format,
    },
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

const HELP: &str =
    "Usage: outlint edit FILE OP PATH [ARGS] [--payload TEXT|- | --payload-file FILE]
       outlint edit FILE --script FILE|-
Options: --schema FILE | --ignore-schema, --force, --dry-run, --format human|compact|json
Operations: rename TITLE, delete, insert-section before|after|append-child TITLE,
append-block, replace, insert-before, insert-after, append-item, insert-item INDEX.
replace-text uses --old TEXT|- or --old-file FILE and --payload or --payload-file for new text.
Scripts use anchored paths, double quotes, @file and <<SENTINEL payloads.
";
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
            s if s.starts_with("--") => {
                return Err(format!("unknown or repeated option {s}").into())
            }
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
        if o.pos.is_empty() {
            return Err(HELP.into());
        }
        let file = o.pos.remove(0);
        fmt = match o.format {
            Some(f) => f,
            None => parse_environment_format(env)?,
        };
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
fn refusal(file: &str, fmt: Format, id: &str, mut details: Value, text: &str) -> u8 {
    details["id"] = json!(id);
    if fmt == Format::Json {
        let out = render::edit_json(file, &[("error", details)], None);
        if crate::write_stdout(&out) != 0 {
            return 2;
        }
    } else {
        crate::write_stderr(&format!("{id}\t{text}\n"));
    }
    1
}
fn execute(file: &str, o: Options, fmt: Format) -> Result<u8, Failure> {
    let target = Path::new(file);
    if !fs::symlink_metadata(target)?.is_file() {
        return Err("destination must be a regular file".into());
    }
    let source = fs::read_to_string(target)?;
    let clean = source.strip_prefix('\u{feff}').unwrap_or(&source);
    let doc = parse_markdown(clean, Default::default())?;
    let cwd = Path::new(".");
    let mut base = cwd;
    let instructions = if let Some(script) = &o.script {
        if !o.pos.is_empty() || o.old.is_some() || o.new.is_some() {
            return Err(HELP.into());
        }
        let s = load(script, cwd)?;
        if script != "-" {
            base = Path::new(script).parent().unwrap_or(cwd)
        }
        match parse_script(&s, script == "-") {
            Ok(v) => v,
            Err(e) => {
                return Ok(refusal(
                    file,
                    fmt,
                    "invalid-script",
                    json!({"line":e.line,"column":e.column,"message":e.message}),
                    &format!("{}:{} {}", e.line, e.column, e.message),
                ))
            }
        }
    } else {
        let [verb, path, args @ ..] = o.pos.as_slice() else {
            return Err(HELP.into());
        };
        if (verb == "replace-text") != o.old.is_some() {
            return Err(HELP.into());
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
                crate::write_stderr(&format!("{id}\t{text}"));
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
                Input::File(name) => fs::read_to_string(base.join(name)),
                Input::Stdin => load("-", base),
            })
            .collect::<io::Result<Vec<_>>>()?;
        let op = operation(&cmd.verb, &cmd.args, &inputs).ok_or(HELP)?;
        edits.push(Edit { path, op });
    }
    let c = match candidate(&source, &edits) {
        Ok(c) => c,
        Err(e) => {
            let mut detail = json!({});
            let id = match e {
                Error::Payload | Error::BareCr => "invalid-payload",
                Error::Target => "invalid-target",
                Error::ConflictingEdits => "conflicting-edits",
                Error::TextNotFound => "text-not-found",
                Error::TextAmbiguous(n) => {
                    detail["count"] = json!(n);
                    "ambiguous-text"
                }
                _ => return Err(format!("edit computation failed: {e:?}").into()),
            };
            return Ok(refusal(file, fmt, id, detail, &format!("{e:?}")));
        }
    };
    let mut diagnostics = vec![];
    if let Some(loaded) = loaded {
        for d in c.introduced_diagnostics(&loaded.schema)? {
            diagnostics.push(render::json::diagnostic_json(
                &crate::diagnostics::render_document_diagnostic(file, &d, &loaded),
            ));
        }
    }
    if !diagnostics.is_empty() {
        let summary = diagnostics
            .iter()
            .map(|d| {
                format!(
                    "{}:{} {} {}",
                    escape(fmt, file),
                    d["line"],
                    d["id"].as_str().unwrap_or(""),
                    escape(fmt, d["message"].as_str().unwrap_or(""))
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !o.force {
            return Ok(refusal(
                file,
                fmt,
                "new-diagnostics",
                json!({"diagnostics":diagnostics}),
                &summary,
            ));
        }
        if fmt != Format::Json {
            crate::write_stderr(&format!("{summary}\n"))
        }
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
        temp.as_file()
            .set_permissions(fs::metadata(target)?.permissions())?;
        temp.persist(target)?;
    }
    let out = if fmt == Format::Json {
        let mut fields = vec![
            ("result", json!(result)),
            ("paths", json!(paths)),
            ("schema", json!(schema)),
        ];
        if let Some(p) = preview {
            fields.push(("diff", json!(p)))
        }
        if !diagnostics.is_empty() {
            fields.push(("diagnostics", json!(diagnostics)))
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
