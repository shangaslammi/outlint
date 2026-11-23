//! Pure line-script grammar. Payload file names are returned for the caller to load.
use super::{Op, Placement};

/// A payload supplied inline, through a relative file, or through stdin.
#[derive(Debug)]
pub enum Input {
    /// Consume stdin once.
    Stdin,
    /// Quoted, inline or heredoc content.
    Literal(String),
    /// Load before resolving the batch.
    File(String),
}
/// A checked operation signature before the shell has loaded its payloads.
pub struct Instruction {
    /// One-based script line.
    pub line: usize,
    /// Raw selector; a bad selector gets path-specific recovery.
    pub path: String,
    /// Operation keyword.
    pub verb: String,
    /// Positional parameters preceding payloads.
    pub args: Vec<String>,
    /// Payloads in argument order.
    pub inputs: Vec<Input>,
}
/// A malformed instruction with one-based UTF-8 byte column.
#[derive(Debug)]
pub struct ScriptError {
    /// Instruction or offending token line.
    pub line: usize,
    /// First offending byte or end of line.
    pub column: usize,
    /// Grammar expectation.
    pub message: String,
}
/// Construct the shared typed operation after loading its inputs.
pub fn operation(verb: &str, args: &[String], payloads: &[String]) -> Option<Op> {
    match (verb, args, payloads) {
        ("rename", [name], []) => Some(Op::Rename(name.clone())),
        ("delete", [], []) => Some(Op::Delete),
        ("replace", [], [p]) => Some(Op::Replace(p.clone())),
        ("replace-text", [], [a, b]) => Some(Op::ReplaceText(a.clone(), b.clone())),
        ("append-block", [], [p]) => Some(Op::AppendBlock(p.clone())),
        ("append-item", [], [p]) => Some(Op::AppendItem(p.clone())),
        ("insert-item", [n], [p]) => Some(Op::InsertItem(n.parse().ok()?, p.clone())),
        ("insert-before", [], [p]) => Some(Op::InsertBefore(p.clone())),
        ("insert-after", [], [p]) => Some(Op::InsertAfter(p.clone())),
        ("insert-section", [place, name], p) if p.len() <= 1 => Some(Op::InsertSection(
            match place.as_str() {
                "before" => Placement::Before,
                "after" => Placement::After,
                "append-child" => Placement::AppendChild,
                _ => return None,
            },
            name.clone(),
            p.first().cloned().unwrap_or_default(),
        )),
        _ => None,
    }
}
struct Word {
    value: String,
    col: usize,
    quoted: bool,
}
fn words(line: &str, n: usize) -> Result<Vec<Word>, ScriptError> {
    let mut iter = line.char_indices().peekable();
    let mut out = vec![];
    while let Some((i, ch)) = iter.next() {
        if matches!(ch, ' ' | '\t') {
            continue;
        }
        let err = |at, msg: &str| ScriptError {
            line: n,
            column: at + 1,
            message: msg.into(),
        };
        let quoted = ch == '"';
        let mut value = String::new();
        if quoted {
            loop {
                match iter.next() {
                    Some((_, '"')) => break,
                    Some((j, '\\')) => match iter.next() {
                        Some((_, v @ ('"' | '\\'))) => value.push(v),
                        _ => return Err(err(j, "invalid quoted escape")),
                    },
                    Some((_, c)) => value.push(c),
                    None => return Err(err(i, "unterminated quote")),
                }
            }
            if let Some(&(j, c)) = iter.peek() {
                if !matches!(c, ' ' | '\t') {
                    return Err(err(j, "expected whitespace after quote"));
                }
            }
        } else {
            value.push(ch);
            while let Some(&(_, c)) = iter.peek() {
                if matches!(c, ' ' | '\t') {
                    break;
                }
                value.push(c);
                iter.next();
            }
        }
        out.push(Word {
            value,
            col: i + 1,
            quoted,
        });
    }
    Ok(out)
}
fn instruction_line(raw: &str) -> &str {
    raw.strip_suffix("\r\n")
        .or_else(|| raw.strip_suffix('\n'))
        .unwrap_or(raw)
}
/// Parse a complete script without opening any files. Heredocs retain their line endings.
pub fn parse_script(s: &str, stdin_script: bool) -> Result<Vec<Instruction>, ScriptError> {
    let mut lines = s.split_inclusive('\n').enumerate();
    let mut used_stdin = stdin_script;
    let mut batch = vec![];
    while let Some((idx, raw)) = lines.next() {
        let line = instruction_line(raw);
        let n = idx + 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let tokens = words(line, n)?;
        let err = |column, message: &str| ScriptError {
            line: n,
            column,
            message: message.into(),
        };
        let [verb, path, rest @ ..] = tokens.as_slice() else {
            return Err(err(line.len() + 1, "expected operation and document path"));
        };
        let argc = match verb.value.as_str() {
            "rename" | "insert-item" => 1,
            "insert-section" => 2,
            _ => 0,
        };
        let (args, inputs) = rest
            .split_at_checked(argc)
            .ok_or_else(|| err(line.len() + 1, "missing operation argument"))?;
        let args: Vec<_> = args.iter().map(|w| w.value.clone()).collect();
        if operation(&verb.value, &args, &vec![String::new(); inputs.len()]).is_none() {
            return Err(err(verb.col, "invalid operation signature"));
        }
        let mut resolved = vec![];
        for word in inputs {
            let v = &word.value;
            resolved.push(
                if let Some(tag) = v.strip_prefix("<<").filter(|_| !word.quoted) {
                    if tag.is_empty() {
                        return Err(err(word.col, "empty heredoc sentinel"));
                    }
                    let mut body = String::new();
                    loop {
                        let (_, raw) = lines
                            .next()
                            .ok_or_else(|| err(word.col, "unterminated heredoc"))?;
                        if instruction_line(raw) == tag {
                            break;
                        }
                        body.push_str(raw);
                    }
                    Input::Literal(body)
                } else if let Some(name) = v.strip_prefix('@').filter(|_| !word.quoted) {
                    if name.is_empty() {
                        return Err(err(word.col, "empty payload filename"));
                    }
                    if name == "-" && std::mem::replace(&mut used_stdin, true) {
                        return Err(err(word.col, "stdin already used"));
                    }
                    if name == "-" {
                        Input::Stdin
                    } else {
                        Input::File(name.into())
                    }
                } else {
                    Input::Literal(v.clone())
                },
            );
        }
        batch.push(Instruction {
            line: n,
            path: path.value.clone(),
            verb: verb.value.clone(),
            args,
            inputs: resolved,
        });
    }
    Ok(batch)
}
