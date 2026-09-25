//! Help text and hand-written command-line argument parsing.

#[cfg(not(any(feature = "search", feature = "read")))]
pub(crate) const TOP_HELP: &str = "Usage: outlint <command> [options]\n\
\n\
Commands:\n\
  check          Validate Markdown documents\n\
  schema check   Validate Outlint schema files\n\
\n\
Options:\n\
  -h, --help     Show help\n\
  -V, --version  Show version\n";

#[cfg(all(feature = "search", not(feature = "read")))]
pub(crate) const TOP_HELP: &str = "Usage: outlint <command> [options]\n\
\n\
Commands:\n\
  check          Validate Markdown documents\n\
  schema check   Validate Outlint schema files\n\
  search         Search Markdown blocks by keyword\n\
\n\
Options:\n\
  -h, --help     Show help\n\
  -V, --version  Show version\n";

#[cfg(all(feature = "read", not(feature = "search")))]
pub(crate) const TOP_HELP: &str = "Usage: outlint <command> [options]\n\
\n\
Commands:\n\
  check          Validate Markdown documents\n\
  schema check   Validate Outlint schema files\n\
  read           Print a Markdown node by document path\n\
\n\
Options:\n\
  -h, --help     Show help\n\
  -V, --version  Show version\n";

#[cfg(all(feature = "search", feature = "read"))]
pub(crate) const TOP_HELP: &str = "Usage: outlint <command> [options]\n\
\n\
Commands:\n\
  check          Validate Markdown documents\n\
  schema check   Validate Outlint schema files\n\
  search         Search Markdown blocks by keyword\n\
  read           Print a Markdown node by document path\n\
\n\
Options:\n\
  -h, --help     Show help\n\
  -V, --version  Show version\n";

pub(crate) const CHECK_HELP: &str = "Usage: outlint check <FILE>... [options]\n\
\n\
Validate individual Markdown files. Without --schema, each file discovers its\n\
schema separately: the nearest <stem>.outlint.yml (file name, extension\n\
removed) or .outlint.yml, specific name first in each ancestor directory.\n\
Standard input (-) requires --schema.\n\
\n\
Options:\n\
  -s, --schema <SCHEMA>       Use one schema for every input\n\
      --format human|json     Select output format (default: human)\n\
      --color auto|always|never\n\
                              Control human-output color (default: auto)\n\
  -h, --help                  Show help\n\
\n\
Exit codes: 0 valid, 1 validation diagnostics, 2 usage or operational error.\n";

pub(crate) const SCHEMA_HELP: &str = "Usage: outlint schema check <SCHEMA>... [options]\n\
\n\
Validate schema syntax, normalization, ids, matchers, cardinalities, constraints,\n\
and all other schema-load-time checks.\n\
\n\
Options:\n\
      --format human|json     Select output format (default: human)\n\
      --color auto|always|never\n\
                              Control human-output color (default: auto)\n\
  -h, --help                  Show help\n\
\n\
Exit codes: 0 valid, 1 validation diagnostics, 2 usage or operational error.\n";

#[cfg(feature = "search")]
pub(crate) const SEARCH_HELP: &str = "Usage: outlint search [options] <WORD>...\n\
\n\
Search the Markdown files under the search root (the current directory unless\n\
--root is given) and print the best-matching blocks. The index lives under\n\
.outlint/search/ in the enclosing Git repository, or in the search root when\n\
there is none. Hit paths are relative to the current directory and can be\n\
passed directly to `outlint read`.\n\
\n\
Options:\n\
      --root <DIR>            Search the files under DIR instead\n\
  -h, --help                  Show help\n\
\n\
Exit codes: 0 hits printed, 1 no hits, 2 usage or operational error.\n";

#[cfg(feature = "read")]
pub(crate) const READ_HELP: &str = "Usage: outlint read [options] <FILE> [MDPATH]\n\
\n\
Print the node of a Markdown file addressed by a document path (default: $,\n\
the whole file), or list the structure under it. A path not starting with $\n\
gets one prepended, so .a.b and /p[0] work unquoted.\n\
\n\
Options:\n\
      --tree                  List the paths under the node instead\n\
      --blocks                With --tree, also list blocks and list items\n\
      --depth <N>             Include N levels of subsections (default: all)\n\
  -h, --help                  Show help\n\
\n\
Exit codes: 0 printed, 1 path syntax or resolution error, 2 usage or\n\
operational error.\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputFormat {
    Human,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(Debug)]
pub(crate) struct CheckOptions {
    pub(crate) files: Vec<String>,
    pub(crate) schema: Option<String>,
    pub(crate) format: OutputFormat,
    pub(crate) color: ColorChoice,
}

#[derive(Debug)]
pub(crate) struct SchemaOptions {
    pub(crate) schemas: Vec<String>,
    pub(crate) format: OutputFormat,
    pub(crate) color: ColorChoice,
}

#[cfg(feature = "search")]
#[derive(Debug)]
pub(crate) struct SearchOptions {
    /// The search root as given, relative to the current directory.
    pub(crate) root: Option<String>,
    /// The search words joined by single spaces; never blank.
    pub(crate) words: String,
}

#[cfg(feature = "read")]
#[derive(Debug)]
pub(crate) struct ReadOptions {
    /// The Markdown file as given.
    pub(crate) file: String,
    /// The document path argument as given; `$` when omitted.
    pub(crate) path: Option<String>,
    /// List the structure under the node instead of printing its content.
    pub(crate) tree: bool,
    /// In tree mode, also list blocks and list items.
    pub(crate) blocks: bool,
    /// Levels of descendant sections to include; unlimited when `None`.
    pub(crate) depth: Option<usize>,
}

pub(crate) enum ParseOutcome<T> {
    Help,
    Run(T),
}

pub(crate) fn parse_check_args(args: &[String]) -> Result<ParseOutcome<CheckOptions>, String> {
    let mut files = Vec::new();
    let mut schema = None;
    let mut format = OutputFormat::Human;
    let mut color = ColorChoice::Auto;
    let mut positional_only = false;
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if positional_only {
            files.push(argument.clone());
        } else {
            match argument.as_str() {
                "--" => positional_only = true,
                "--help" | "-h" => return Ok(ParseOutcome::Help),
                "-s" | "--schema" => {
                    let value = option_value(args, &mut index, argument)?;
                    set_once(&mut schema, value, "--schema")?;
                }
                "--format" => {
                    format = parse_format(option_value(args, &mut index, argument)?)?;
                }
                "--color" => {
                    color = parse_color(option_value(args, &mut index, argument)?)?;
                }
                "-" => files.push(argument.clone()),
                value if value.starts_with('-') => {
                    return Err(format!("unknown option '{value}'"));
                }
                _ => files.push(argument.clone()),
            }
        }
        index += 1;
    }
    if files.is_empty() {
        return Err("at least one Markdown input is required".to_owned());
    }
    if files.iter().any(|file| file == "-") && schema.is_none() {
        return Err("standard input requires an explicit --schema".to_owned());
    }
    Ok(ParseOutcome::Run(CheckOptions {
        files,
        schema,
        format,
        color,
    }))
}

pub(crate) fn parse_schema_args(args: &[String]) -> Result<ParseOutcome<SchemaOptions>, String> {
    let mut schemas = Vec::new();
    let mut format = OutputFormat::Human;
    let mut color = ColorChoice::Auto;
    let mut positional_only = false;
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if positional_only {
            schemas.push(argument.clone());
        } else {
            match argument.as_str() {
                "--" => positional_only = true,
                "--help" | "-h" => return Ok(ParseOutcome::Help),
                "--format" => {
                    format = parse_format(option_value(args, &mut index, argument)?)?;
                }
                "--color" => {
                    color = parse_color(option_value(args, &mut index, argument)?)?;
                }
                value if value.starts_with('-') => {
                    return Err(format!("unknown option '{value}'"));
                }
                _ => schemas.push(argument.clone()),
            }
        }
        index += 1;
    }
    if schemas.is_empty() {
        return Err("at least one schema input is required".to_owned());
    }
    Ok(ParseOutcome::Run(SchemaOptions {
        schemas,
        format,
        color,
    }))
}

#[cfg(feature = "search")]
pub(crate) fn parse_search_args(args: &[String]) -> Result<ParseOutcome<SearchOptions>, String> {
    let mut words = Vec::new();
    let mut root = None;
    let mut positional_only = false;
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if positional_only {
            words.push(argument.clone());
        } else {
            match argument.as_str() {
                "--" => positional_only = true,
                "--help" | "-h" => return Ok(ParseOutcome::Help),
                "--root" => {
                    let value = option_value(args, &mut index, argument)?;
                    set_once(&mut root, value, "--root")?;
                }
                value if value.starts_with('-') => {
                    return Err(format!("unknown option '{value}'"));
                }
                _ => words.push(argument.clone()),
            }
        }
        index += 1;
    }
    let words = words.join(" ");
    if words.trim().is_empty() {
        return Err("missing search words".to_owned());
    }
    Ok(ParseOutcome::Run(SearchOptions { root, words }))
}

#[cfg(feature = "read")]
pub(crate) fn parse_read_args(args: &[String]) -> Result<ParseOutcome<ReadOptions>, String> {
    let mut positionals = Vec::new();
    let mut tree = false;
    let mut blocks = false;
    let mut depth = None;
    let mut positional_only = false;
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if positional_only {
            positionals.push(argument.clone());
        } else {
            match argument.as_str() {
                "--" => positional_only = true,
                "--help" | "-h" => return Ok(ParseOutcome::Help),
                "--tree" => tree = true,
                "--blocks" => blocks = true,
                "--depth" => {
                    let value = option_value(args, &mut index, argument)?;
                    set_once(&mut depth, value, "--depth")?;
                }
                value if value.starts_with('-') => {
                    return Err(format!("unknown option '{value}'"));
                }
                _ => positionals.push(argument.clone()),
            }
        }
        index += 1;
    }
    let depth = depth
        .map(|value| {
            value.parse::<usize>().map_err(|_| {
                format!("invalid --depth value '{value}' (expected a non-negative integer)")
            })
        })
        .transpose()?;
    if blocks && !tree {
        return Err("--blocks requires --tree".to_owned());
    }
    let mut positionals = positionals.into_iter();
    let Some(file) = positionals.next() else {
        return Err("a Markdown input is required".to_owned());
    };
    let path = positionals.next();
    if let Some(extra) = positionals.next() {
        return Err(format!("unexpected argument '{extra}'"));
    }
    Ok(ParseOutcome::Run(ReadOptions {
        file,
        path,
        tree,
        blocks,
        depth,
    }))
}

fn option_value(args: &[String], index: &mut usize, option: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .filter(|value| !value.is_empty())
        .cloned()
        .ok_or_else(|| format!("option '{option}' requires a value"))
}

fn set_once(slot: &mut Option<String>, value: String, option: &str) -> Result<(), String> {
    if slot.replace(value).is_some() {
        Err(format!("option '{option}' may only be specified once"))
    } else {
        Ok(())
    }
}

fn parse_format(value: String) -> Result<OutputFormat, String> {
    match value.as_str() {
        "human" => Ok(OutputFormat::Human),
        "json" => Ok(OutputFormat::Json),
        _ => Err(format!(
            "invalid --format value '{value}' (expected human or json)"
        )),
    }
}

fn parse_color(value: String) -> Result<ColorChoice, String> {
    match value.as_str() {
        "auto" => Ok(ColorChoice::Auto),
        "always" => Ok(ColorChoice::Always),
        "never" => Ok(ColorChoice::Never),
        _ => Err(format!(
            "invalid --color value '{value}' (expected auto, always, or never)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_check_args, ParseOutcome};

    #[test]
    fn stdin_requires_an_explicit_schema() {
        let args = vec!["-".to_owned()];
        assert!(parse_check_args(&args).is_err());
        let args = vec!["-".to_owned(), "--schema".to_owned(), "s.yml".to_owned()];
        assert!(matches!(parse_check_args(&args), Ok(ParseOutcome::Run(_))));
    }

    #[cfg(feature = "search")]
    #[test]
    fn search_takes_root_anywhere_and_joins_the_words() {
        use super::parse_search_args;
        let parse = |args: &[&str]| {
            let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
            match parse_search_args(&args) {
                Ok(ParseOutcome::Run(options)) => Ok((options.root, options.words)),
                Ok(ParseOutcome::Help) => Ok((None, "help".to_owned())),
                Err(message) => Err(message),
            }
        };
        assert_eq!(
            parse(&["--root", "docs", "rollback", "plan"]),
            Ok((Some("docs".to_owned()), "rollback plan".to_owned()))
        );
        assert_eq!(
            parse(&["rollback", "--root", "docs", "plan"]),
            Ok((Some("docs".to_owned()), "rollback plan".to_owned()))
        );
        assert_eq!(
            parse(&["--", "--root", "x"]),
            Ok((None, "--root x".to_owned()))
        );
        assert!(parse(&["--root", "docs"]).is_err());
        assert!(parse(&["--root"]).is_err());
        assert!(parse(&["-x", "word"]).is_err());
    }
    #[cfg(feature = "read")]
    #[test]
    fn read_takes_a_file_an_optional_path_and_tree_options() {
        use super::parse_read_args;
        let parse = |args: &[&str]| {
            let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
            match parse_read_args(&args) {
                Ok(ParseOutcome::Run(options)) => Ok((
                    options.file,
                    options.path,
                    options.tree,
                    options.blocks,
                    options.depth,
                )),
                Ok(ParseOutcome::Help) => Ok((String::new(), None, false, false, None)),
                Err(message) => Err(message),
            }
        };
        assert_eq!(
            parse(&["--tree", "--depth", "2", "--blocks", "a.md", ".x"]),
            Ok((
                "a.md".to_owned(),
                Some(".x".to_owned()),
                true,
                true,
                Some(2)
            ))
        );
        assert_eq!(
            parse(&["a.md"]),
            Ok(("a.md".to_owned(), None, false, false, None))
        );
        assert!(parse(&["--blocks", "a.md"]).is_err());
        assert!(parse(&["--depth", "-1", "a.md"]).is_err());
        assert!(parse(&["a.md", "$", "extra"]).is_err());
        assert!(parse(&[]).is_err());
    }
}
