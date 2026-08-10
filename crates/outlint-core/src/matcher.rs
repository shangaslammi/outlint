//! Shared compilation for anchored regex-backed matchers.

use regex::{Regex, RegexBuilder};

pub(crate) fn compile_glob_pattern(glob: &str, match_case: bool) -> Result<Regex, regex::Error> {
    let body = glob
        .split('*')
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join(".*");
    compile_anchored_pattern(&body, match_case, true)
}

pub(crate) fn compile_anchored_pattern(
    body: &str,
    match_case: bool,
    dot_matches_new_line: bool,
) -> Result<Regex, regex::Error> {
    let anchored = format!(r"\A(?:{body})\z");
    RegexBuilder::new(&anchored)
        .case_insensitive(!match_case)
        .dot_matches_new_line(dot_matches_new_line)
        .build()
}
