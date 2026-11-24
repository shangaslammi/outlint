//! Pure edit instruction grammar. The shell loads referenced inputs separately.
use super::engine::{Op, Placement};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Input {
    Stdin,
    Literal(String),
    File(String),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Instruction {
    pub line: Option<usize>,
    pub path: String,
    pub op: Op<Input>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ScriptError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

pub(super) fn operation<T>(verb: &str, args: &[&str], inputs: Vec<T>) -> Option<Op<T>> {
    let count = inputs.len();
    let mut inputs = inputs.into_iter();
    Some(match (verb, args, count) {
        ("rename", [title], 0) => Op::Rename((*title).into()),
        ("delete", [], 0) => Op::Delete,
        ("replace", [], 1) => Op::Replace(inputs.next()?),
        ("replace-text", [], 2) => Op::ReplaceText {
            old: inputs.next()?,
            new: inputs.next()?,
        },
        ("append-block", [], 1) => Op::AppendBlock(inputs.next()?),
        ("append-item", [], 1) => Op::AppendItem(inputs.next()?),
        ("insert-item", [index], 1) => Op::InsertItem {
            index: item_index(index)?,
            payload: inputs.next()?,
        },
        ("insert-before", [], 1) => Op::InsertBefore(inputs.next()?),
        ("insert-after", [], 1) => Op::InsertAfter(inputs.next()?),
        ("insert-section", [placement, title], 0..=1) => Op::InsertSection {
            placement: match *placement {
                "before" => Placement::Before,
                "after" => Placement::After,
                "append-child" => Placement::AppendChild,
                _ => return None,
            },
            title: (*title).into(),
            preamble: inputs.next(),
        },
        _ => return None,
    })
}

fn item_index(index: &str) -> Option<usize> {
    if (index.starts_with('0') && index.len() > 1)
        || !index.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    index.parse().ok()
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
/// Parse the whole batch before opening any payload sources.
pub(super) fn parse_script(
    source: &str,
    stdin_script: bool,
) -> Result<Vec<Instruction>, ScriptError> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let mut lines = source.split_inclusive('\n').enumerate();
    let mut used_stdin = stdin_script;
    let mut batch = vec![];
    while let Some((index, raw)) = lines.next() {
        let line = instruction_line(raw);
        let number = index + 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let tokens = words(line, number)?;
        let err = |column, message: &str| ScriptError {
            line: number,
            column,
            message: message.into(),
        };
        let [verb, path, rest @ ..] = tokens.as_slice() else {
            return Err(err(line.len() + 1, "expected operation and document path"));
        };
        let arg_count = match verb.value.as_str() {
            "rename" | "insert-item" => 1,
            "insert-section" => 2,
            _ => 0,
        };
        let (args, inputs) = rest
            .split_at_checked(arg_count)
            .ok_or_else(|| err(line.len() + 1, "missing operation argument"))?;
        let args: Vec<_> = args.iter().map(|word| word.value.as_str()).collect();
        let op = operation(&verb.value, &args, inputs.iter().collect())
            .ok_or_else(|| err(verb.col, "invalid operation signature"))?;
        let literal = matches!(op, Op::ReplaceText { .. });
        let op = op.map_inputs(|word| {
            let value = word.value.as_str();
            if word.quoted {
                return Ok(Input::Literal(value.into()));
            }
            if let Some(tag) = value.strip_prefix("<<") {
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
                if literal {
                    body.truncate(instruction_line(&body).len());
                }
                return Ok(Input::Literal(body));
            }
            if let Some(name) = value.strip_prefix('@') {
                return match name {
                    "" => Err(err(word.col, "empty payload filename")),
                    "-" if std::mem::replace(&mut used_stdin, true) => {
                        Err(err(word.col, "stdin already used"))
                    }
                    "-" => Ok(Input::Stdin),
                    _ => Ok(Input::File(name.into())),
                };
            }
            Ok(Input::Literal(value.into()))
        })?;
        batch.push(Instruction {
            line: Some(number),
            path: path.value.clone(),
            op,
        });
    }
    Ok(batch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bom_crlf_comments_escapes_and_typed_arguments() {
        let instructions = parse_script(
            "\u{feff}   # ignored\r\n\r\ninsert-section $.old after \"Use \\\"x\\\" and \\\\ here\" @body\r\n",
            false,
        ).unwrap();
        assert_eq!(
            instructions,
            [Instruction {
                line: Some(3),
                path: "$.old".into(),
                op: Op::InsertSection {
                    placement: Placement::After,
                    title: "Use \"x\" and \\ here".into(),
                    preamble: Some(Input::File("body".into())),
                },
            }]
        );
        let instructions = parse_script("rename $ <<TITLE\nreplace $/p \"@-\"", false).unwrap();
        assert_eq!(instructions[0].op, Op::Rename("<<TITLE".into()));
        assert_eq!(instructions[1].op, Op::Replace(Input::Literal("@-".into())));
    }

    #[test]
    fn heredocs_drop_only_one_ending_for_literal_operands() {
        let instructions = parse_script(
            "replace-text $ <<OLD <<NEW\r\nold\r\n\r\nOLD\r\nnew\r\nNEW\r\nreplace $/code <<BODY\r\nbody\r\nBODY",
            false,
        ).unwrap();
        assert_eq!(
            instructions[0].op,
            Op::ReplaceText {
                old: Input::Literal("old\r\n".into()),
                new: Input::Literal("new".into()),
            }
        );
        assert_eq!(
            instructions[1].op,
            Op::Replace(Input::Literal("body\r\n".into()))
        );
        assert_eq!(instructions[1].line, Some(7));
    }

    #[test]
    fn grammar_errors_point_to_the_responsible_byte() {
        let error = parse_script("  # comment\nreplace $/p \"é\\q\"", false).unwrap_err();
        assert_eq!(
            (error.line, error.column, error.message.as_str()),
            (2, 16, "invalid quoted escape")
        );
        for script in [
            "delete $ extra",
            "insert-item $/list +1 x",
            "insert-item $/list 01 x",
        ] {
            let error = parse_script(script, false).unwrap_err();
            assert_eq!(
                (error.line, error.column, error.message.as_str()),
                (1, 1, "invalid operation signature")
            );
        }
        assert_eq!(
            parse_script("replace $/p <<END\nmissing", false)
                .unwrap_err()
                .column,
            13
        );
    }

    #[test]
    fn stdin_can_be_used_only_once_across_the_batch() {
        let script = "replace $/p @-";
        assert_eq!(
            parse_script(script, false).unwrap()[0].op,
            Op::Replace(Input::Stdin)
        );
        assert_eq!(parse_script(script, true).unwrap_err().line, 1);
        assert_eq!(
            parse_script(&format!("{script}\n{script}"), false)
                .unwrap_err()
                .line,
            2
        );
    }
}
