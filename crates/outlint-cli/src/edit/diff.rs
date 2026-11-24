use similar::TextDiff;

/// Formats a unified diff using an already escaped filename for both headers.
/// Identical inputs produce an empty string.
pub(super) fn diff(file: &str, original: &str, candidate: &str) -> String {
    if original == candidate {
        return String::new();
    }

    // patch uses a tab to delimit filenames that contain spaces.
    let delimiter = if file.contains(' ') { "\t" } else { "" };
    TextDiff::from_lines(original, candidate)
        .unified_diff()
        .context_radius(3)
        .header(
            &format!("a/{file}{delimiter}"),
            &format!("b/{file}{delimiter}"),
        )
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::diff;

    #[test]
    fn unchanged_and_empty_original() {
        assert_eq!(diff("file", "same", "same"), "");
        assert_eq!(
            diff("file", "", "new\n"),
            "--- a/file\n+++ b/file\n@@ -0,0 +1 @@\n+new\n"
        );
    }

    #[test]
    fn splits_changes_separated_by_seven_lines() {
        assert_eq!(
            diff(
                "file",
                "old top\n1\n2\n3\n4\n5\n6\n7\nold bottom\n",
                "new top\n1\n2\n3\n4\n5\n6\n7\nnew bottom\n",
            ),
            concat!(
                "--- a/file\n+++ b/file\n",
                "@@ -1,4 +1,4 @@\n-old top\n+new top\n 1\n 2\n 3\n",
                "@@ -6,4 +6,4 @@\n 5\n 6\n 7\n-old bottom\n+new bottom\n",
            )
        );
    }

    #[test]
    fn merges_changes_separated_by_six_lines() {
        assert_eq!(
            diff(
                "file",
                "old top\n1\n2\n3\n4\n5\n6\nold bottom\n",
                "new top\n1\n2\n3\n4\n5\n6\nnew bottom\n",
            ),
            concat!(
                "--- a/file\n+++ b/file\n",
                "@@ -1,8 +1,8 @@\n-old top\n+new top\n",
                " 1\n 2\n 3\n 4\n 5\n 6\n-old bottom\n+new bottom\n",
            )
        );
    }

    #[test]
    fn marks_the_side_missing_a_final_newline() {
        assert_eq!(
            diff("file", "line", "line\n"),
            "--- a/file\n+++ b/file\n@@ -1 +1 @@\n-line\n\\ No newline at end of file\n+line\n"
        );
        assert_eq!(
            diff("file", "line\n", "line"),
            "--- a/file\n+++ b/file\n@@ -1 +1 @@\n-line\n+line\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn keeps_mixed_line_endings_in_changes_and_context() {
        let before = "0\r\n1\r\n2\n3\r\nold\r\n5\r\n6\n7\r\n8\r\n";
        assert_eq!(
            diff("file", before, &before.replace("old\r\n", "new\n")),
            "--- a/file\n+++ b/file\n@@ -2,7 +2,7 @@\n 1\r\n 2\n 3\r\n-old\r\n+new\n 5\r\n 6\n 7\r\n"
        );
    }
}
