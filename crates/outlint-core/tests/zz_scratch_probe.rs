use outlint_core::{DocumentFrontmatter, MarkdownOptions, parse_markdown};

fn show(name: &str, src: &str) {
    let doc = parse_markdown(src, MarkdownOptions::default());
    match &doc.frontmatter {
        DocumentFrontmatter::Absent => println!("{name}: ABSENT"),
        DocumentFrontmatter::Mapping { value, .. } => println!("{name}: MAPPING {value}"),
        DocumentFrontmatter::Invalid { message, .. } => println!("{name}: INVALID {message}"),
    }
}

#[test]
fn probe() {
    show("dup_only", "---\na: 1\na: hello\n---\n\n# X\n");
    show(
        "bigint_then_dup",
        "---\nbig: 99999999999999999999999\na: 1\na: 2\n---\n\n# X\n",
    );
    show(
        "dup_then_bigint",
        "---\na: 1\na: 2\nbig: 99999999999999999999999\n---\n\n# X\n",
    );
    show("bigint_only", "---\nbig: 99999999999999999999999\n---\n\n# X\n");
    show(
        "nested_dup",
        "---\nouter:\n  a: 1\n  a: 2\n---\n\n# X\n",
    );
    show(
        "bigint_and_nested_dup",
        "---\nbig: 99999999999999999999999\nouter:\n  a: 1\n  a: 2\n---\n\n# X\n",
    );

    // raw serde_yaml behavior
    let r = serde_yaml::from_str::<serde_yaml::Value>("a: 1\na: hello\n");
    println!("serde_yaml dup: {:?}", r.map(|v| format!("{v:?}")));
    let r2 = serde_yaml::from_str::<serde_yaml::Value>("big: 99999999999999999999999\n");
    println!("serde_yaml bigint: {:?}", r2.map(|v| format!("{v:?}")));
    let r3 = serde_yaml::from_str::<serde_yaml::Value>("big: 99999999999999999999999\na: 1\na: 2\n");
    println!("serde_yaml bigint+dup: {:?}", r3.map(|v| format!("{v:?}")));
    let r4 = serde_yaml::from_str::<serde_yaml::Value>("a: 1\na: 2\nbig: 99999999999999999999999\n");
    println!("serde_yaml dup+bigint: {:?}", r4.map(|v| format!("{v:?}")));
}
