use outlint_core::{ByteOffset, DocumentFrontmatter, FrontmatterLocation, TextRange};

#[test]
fn frontmatter_mapping_value_has_the_public_json_object_type() {
    let frontmatter = DocumentFrontmatter::Mapping {
        value: serde_json::Map::new(),
        location: FrontmatterLocation {
            range: TextRange {
                start: ByteOffset(0),
                end: ByteOffset(0),
            },
            start_line: 1,
            end_line: 1,
        },
    };
    let DocumentFrontmatter::Mapping { value, .. } = frontmatter else {
        panic!("constructed the mapping variant")
    };

    let _: serde_json::Map<String, serde_json::Value> = value;
}
