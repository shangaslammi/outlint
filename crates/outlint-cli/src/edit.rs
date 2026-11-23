//! Flag-mode shell for the experimental edit engine.
use outlint_core::DocumentPath;
use outlint_edit::{Edit, Op, Placement};
use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
};
use tempfile::NamedTempFile;

const HELP:&str="Usage: outlint edit <FILE> <op> <path> [args] [--payload <s|-> | --payload-file <p>] [--dry-run]
Operations: rename TEXT, delete, insert-section before|after|append-child TEXT,
append-block, replace, insert-before, insert-after, append-item, insert-item INDEX.
";

pub(crate) fn run(args: &[String]) -> u8 {
    if matches!(args,[a] if a=="--help"||a=="-h") {
        return crate::write_stdout(HELP);
    }
    match execute(args) {
        Ok(s) => crate::write_stdout(&s),
        Err(e) => {
            crate::write_stderr(&format!("outlint edit: {e}\n"));
            2
        }
    }
}

fn execute(args: &[String]) -> Result<String, Box<dyn std::error::Error>> {
    let [file, verb, path, tail @ ..] = args else {
        return Err(HELP.into());
    };
    let path = DocumentPath::parse(path)?;
    let mut dry = false;
    let mut payload = None;
    let mut pos = vec![];
    let mut iter = tail.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--dry-run" if !dry => dry = true,
            "--payload" | "--payload-file" if payload.is_none() => {
                let value = iter.next().ok_or("missing payload argument")?;
                payload = Some(if arg == "--payload-file" {
                    fs::read_to_string(value)?
                } else if value == "-" {
                    let mut s = String::new();
                    io::stdin().read_to_string(&mut s)?;
                    s
                } else {
                    value.clone()
                });
            }
            s if s.starts_with("--") => return Err("unknown or repeated flag".into()),
            s => pos.push(s),
        }
    }
    let op = match (verb.as_str(), pos.as_slice(), payload) {
        ("rename", [text], None) => Op::Rename((*text).into()),
        ("delete", [], None) => Op::Delete,
        ("replace", [], Some(p)) => Op::Replace(p),
        ("insert-before", [], Some(p)) => Op::InsertBefore(p),
        ("insert-after", [], Some(p)) => Op::InsertAfter(p),
        ("append-block", [], Some(p)) => Op::AppendBlock(p),
        ("append-item", [], Some(p)) => Op::AppendItem(p),
        ("insert-item", [n], Some(p)) => Op::InsertItem(n.parse()?, p),
        ("insert-section", [place, text], p) => Op::InsertSection(
            match *place {
                "before" => Placement::Before,
                "after" => Placement::After,
                "append-child" => Placement::AppendChild,
                _ => return Err(HELP.into()),
            },
            (*text).into(),
            p.unwrap_or_default(),
        ),
        _ => return Err(HELP.into()),
    };
    let target = fs::canonicalize(file)?;
    let source = fs::read_to_string(&target)?;
    let result = outlint_edit::edit(&source, &[Edit { path, op }]).map_err(|e| match e {
        outlint_edit::Error::ConflictingEdits => "conflicting-edits".to_owned(),
        _ => format!("{e:?}"),
    })?;
    if dry {
        return Ok(diff(file, &source, &result));
    }
    if source != result {
        let mut temp = NamedTempFile::new_in(target.parent().unwrap_or(Path::new(".")))?;
        temp.write_all(result.as_bytes())?;
        temp.as_file()
            .set_permissions(fs::metadata(&target)?.permissions())?;
        temp.persist(target)?;
    }
    Ok(String::new())
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
