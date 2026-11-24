/// Formats a unified diff using an already escaped filename for both headers.
/// Identical inputs produce an empty string.
pub(super) fn diff(file: &str, a: &str, b: &str) -> String {
    if a == b {
        return String::new();
    }
    let a: Vec<_> = a.split_inclusive('\n').collect();
    let b: Vec<_> = b.split_inclusive('\n').collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a
        .iter()
        .rev()
        .take(a.len() - prefix)
        .zip(b.iter().rev().take(b.len() - prefix))
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
    for (marker, lines, offset, count) in [
        (' ', &a, start, prefix - start),
        ('-', &a, prefix, end_a - prefix),
        ('+', &b, prefix, end_b - prefix),
        (' ', &a, end_a, context),
    ] {
        for line in lines.iter().skip(offset).take(count) {
            out.push(marker);
            out.push_str(line);
            if !line.ends_with('\n') {
                out.push_str("\n\\ No newline at end of file\n");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::diff;

    #[test]
    fn empty_changes_and_missing_terminators() {
        assert_eq!(diff("file", "same", "same"), "");
        assert_eq!(
            diff("file", "", "new"),
            "--- file\n+++ file\n@@ -0,0 +1,1 @@\n+new\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn keeps_crlf_context_and_bounds_the_hunk() {
        let before = "0\r\n1\r\n2\r\n3\r\nold\r\n5\r\n6\r\n7\r\n8\r\n";
        assert_eq!(
            diff("file", before, &before.replace("old", "new")),
            "--- file\n+++ file\n@@ -2,7 +2,7 @@\n 1\r\n 2\r\n 3\r\n-old\r\n+new\r\n 5\r\n 6\r\n 7\r\n"
        );
    }
}
