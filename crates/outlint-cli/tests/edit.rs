#![cfg(feature = "edit")]
mod common;
use common::{run, stdout, TempDir};
use std::fs;
#[test]
fn preview_apply_and_rejection() {
    let dir = TempDir::new("edit");
    dir.write("doc.md", "old");
    dir.write("payload", "new\n");
    let args = [
        "edit",
        "doc.md",
        "replace",
        "$/p",
        "--payload-file",
        "payload",
    ];
    let preview = run(&dir, &[&args[..], &["--dry-run"]].concat());
    assert!(preview.status.success());
    assert_eq!(stdout(&preview),"--- doc.md\n+++ doc.md\n@@ -1,1 +1,1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n");
    let file = dir.path().join("doc.md");
    assert_eq!(fs::read_to_string(&file).unwrap(), "old");
    assert!(run(&dir, &args).status.success());
    assert_eq!(
        run(
            &dir,
            &["edit", "doc.md", "replace", "$/p", "--payload", "# heading"]
        )
        .status
        .code(),
        Some(2)
    );
    assert_eq!(fs::read_to_string(file).unwrap(), "new");
}
