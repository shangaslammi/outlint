use super::*;
use proptest::prelude::*;

fn edit(source: &str, edits: &[Edit]) -> Result<String, &'static str> {
    candidate(source, edits)
        .map(|candidate| candidate.text)
        .map_err(|error| match error.reason {
            Error::Payload => "payload-form",
            Error::Structure => "candidate-structure",
            Error::Conflict(_) => "conflict",
            Error::Target => "target",
            Error::TextNotFound => "not-found",
            Error::TextAmbiguous(2) => "two-matches",
            other => panic!("{other:?}"),
        })
}

fn op(path: &str, op: Op) -> Edit {
    Edit {
        path: path.into(),
        op,
    }
}
fn one(s: &str, path: &str, action: Op) -> String {
    edit(s, &[op(path, action)]).unwrap()
}

#[test]
fn tight_and_loose_item_boundaries() {
    for gap in ["\n", "\n\n"] {
        let s = ["- one", "- two", "- three"].join(gap);
        for (i, remain) in [
            (0, ["- two", "- three"]),
            (1, ["- one", "- three"]),
            (2, ["- one", "- two"]),
        ] {
            let result = one(&s, &format!("$/list/item[{i}]"), Op::Delete);
            let mut expected = remain.join(gap);
            if i == 2 {
                expected.push('\n')
            }
            assert_eq!(result, expected);
        }
        assert_eq!(
            one(&s, "$/list/item[1]", Op::Replace("new".into())),
            s.replace("two", "new")
        );
        for i in 0..=3 {
            let mut items = vec!["- one", "- two", "- three"];
            items.insert(i, "- new");
            assert_eq!(
                one(
                    &s,
                    "$/list",
                    Op::InsertItem {
                        index: i,
                        payload: "new".into()
                    }
                ),
                items.join(gap)
            );
        }
    }
    assert_eq!(
        one("# T\n\n+ only\n\n## Child\n", "$/list/item", Op::Delete),
        "# T\n\n## Child\n"
    );
}
#[test]
fn item_content_and_marker_width() {
    let s = "  09) one\r\n      - nested\r\n  42) two\r\n";
    assert_eq!(
        one(s, "$/list/item", Op::Replace("new\n- inner".into())),
        "  09) new\r\n      - inner\r\n  42) two\r\n"
    );
    assert_eq!(
        one(s, "$/list", Op::AppendItem("three".into())),
        format!("{s}  42) three\r\n")
    );
}
#[test]
fn block_neighbors_and_section_anchors() {
    let s = "lead\n\n# T\n\n<!-- keep -->\n\nold\n\n## C\n";
    assert_eq!(one(s, "$/p[1]", Op::Delete), s.replace("old\n\n", ""));
    assert_eq!(one(s, "$", Op::Delete), "lead\n");
    assert_eq!(
        one(s, "$", Op::Rename("Title".into())),
        s.replace("# T", "# Title")
    );
    let empty = "# T\n## C";
    assert_eq!(
        one(empty, "$", Op::AppendBlock("body".into())),
        "# T\n\nbody\n\n## C"
    );
    assert_eq!(
        one(
            empty,
            "$",
            Op::InsertSection {
                placement: Placement::AppendChild,
                title: "D".into(),
                preamble: Some("body".into())
            }
        ),
        "# T\n## C\n\n## D\n\nbody"
    );
    assert_eq!(one(empty, "$.c", Op::Delete), "# T\n");
    for (place, expected) in [
        (Placement::Before, "# T\n\n## D\n\n## C"),
        (Placement::After, "# T\n## C\n\n## D"),
    ] {
        assert_eq!(
            one(
                empty,
                "$.c",
                Op::InsertSection {
                    placement: place,
                    title: "D".into(),
                    preamble: None
                }
            ),
            expected
        );
    }
    assert_eq!(one("", "$", Op::AppendBlock("body".into())), "body");
}
#[test]
fn whole_block_payloads_and_eof() {
    for (k, a, b) in [
        ("code", "    a", "~~~\nb\n~~~"),
        ("quote", "> a", "> b"),
        ("html", "<div>a</div>", "<div>b</div>"),
        ("break", "***", "___"),
        ("list", "+ a", "3. b"),
        ("p", "a", "b"),
    ] {
        for e in ["", "\r\n"] {
            let s = format!("{a}{e}");
            let expected = if e.is_empty() {
                b.to_owned()
            } else {
                b.replace('\n', e)
            };
            assert_eq!(
                one(&s, &format!("$/{k}"), Op::Replace(b.into())),
                expected + e
            );
        }
    }
    assert_eq!(one("a", "$/p", Op::InsertBefore("b".into())), "b\n\na");
    assert_eq!(
        one("a\r\n", "$/p", Op::InsertAfter("b\nc".into())),
        "a\r\n\r\nb\r\nc\r\n"
    );
}
#[test]
fn rejects_payloads_that_change_neighbors() {
    for (source, path, operation) in [
        ("- one", "$/list", Op::InsertAfter("- two".into())),
        ("> a\n***\nlazy", "$/break", Op::Delete),
    ] {
        assert_eq!(
            edit(source, &[op(path, operation)]),
            Err("candidate-structure")
        );
    }
}

#[test]
fn batches_use_original_paths_and_validate_together() {
    let s = "# T\n## A\n\nx\n";
    assert_eq!(
        edit(
            s,
            &[
                op("$.a", Op::Rename("B".into())),
                op("$.a/p", Op::Replace("y".into()))
            ]
        )
        .unwrap(),
        "# T\n## B\n\ny\n"
    );
    assert_eq!(
        edit(s, &[op("$.a", Op::Delete), op("$.a/p", Op::Delete)]),
        Err("conflict")
    );
    assert_eq!(
        edit(
            "x",
            &[
                op("$/p", Op::InsertAfter("a".into())),
                op("$/p", Op::InsertAfter("b".into()))
            ]
        )
        .unwrap(),
        "x\n\na\n\nb"
    );
    assert_eq!(
        edit(
            "x",
            &[
                op("$/p", Op::InsertAfter("- a".into())),
                op("$/p", Op::InsertAfter("- b".into()))
            ]
        ),
        Err("candidate-structure")
    );
}

proptest! {
    #[test]
    fn inserting_before_a_generated_block_preserves_surrounding_bytes(
        lines in prop::collection::vec(
            prop_oneof![
                "[^\r\n]{0,18}",
                prop::sample::select(vec!["# Title", "## Child", "- item", "~~~", "> quote", "<!-- note -->", "", "    code", "---"])
                    .prop_map(str::to_owned),
            ],
            0..16,
        ),
        crlf in any::<bool>(),
        pick in any::<usize>(),
    ) {
        let source = lines.join(if crlf { "\r\n" } else { "\n" });
        prop_assume!(!source.starts_with('\u{feff}'));
        let doc = parse(&source).unwrap();
        let entries = document_paths(&doc);
        let blocks: Vec<_> = entries.iter()
            .filter(|(_, node)| matches!(node, DocumentNode::Block(_))).collect();
        if let Some((path, node)) = blocks.get(pick % blocks.len().max(1)) {
            let start = model::line_span(&source, node.extent().unwrap()).unwrap().start;
            match edit(&source, &[op(&path.to_string(), Op::InsertBefore("edit-probe".into()))]) {
                Ok(result) => {
                    let inserted_end = start + result.len() - source.len();
                    prop_assert_eq!(result.get(..start), source.get(..start));
                    prop_assert_eq!(result.get(inserted_end..), source.get(start..));
                    prop_assert!(result.get(start..inserted_end).unwrap().contains("edit-probe"));
                }
                Err(reason) => prop_assert_eq!(reason, "candidate-structure"),
            }
        }
    }
}

#[test]
fn section_spelling_and_transparency() {
    assert_eq!(
        one("  ##  old  ###  \r\n", "$.old", Op::Rename("*new*".into())),
        "  ##  *new*  ###  \r\n"
    );
    assert_eq!(
        one("Title\n=====\n", "$", Op::Rename("New".into())),
        "New\n=====\n"
    );
    assert_eq!(
        one("  Title  \r\n=====\r\n", "$", Op::Rename("*new*".into())),
        "  *new*  \r\n=====\r\n"
    );
    assert_eq!(
        one(
            "# T\n\n<!-- keep -->\n\n## C",
            "$",
            Op::AppendBlock("new".into())
        ),
        "# T\n\n<!-- keep -->\n\nnew\n\n## C"
    );
    assert_eq!(
        one("# T\n## C\n\n<!-- tail -->\n", "$.c", Op::Delete),
        "# T\n<!-- tail -->\n"
    );
    assert_eq!(
        one("- a\n\n  more\n- b\n", "$/list", Op::AppendItem("c".into())),
        "- a\n\n  more\n- b\n- c\n"
    );
}

#[test]
fn release_list_gap_and_fenced_bodies() {
    assert_eq!(
        one(
            "### Added\n\n- a\n\n### Fixed\n",
            "$.added/list",
            Op::AppendItem("b".into())
        ),
        "### Added\n\n- a\n- b\n\n### Fixed\n"
    );
    assert_eq!(
        one(
            "  ~~~rs\r\na\r\n ~~~~\r\n",
            "$/code",
            Op::Replace("x\n".into())
        ),
        "  ~~~rs\r\nx\r\n ~~~~\r\n"
    );
    for source in ["    a", "~~~\na"] {
        assert_eq!(
            edit(source, &[op("$/code", Op::Replace("x\n".into()))]),
            Err("target")
        );
    }
    assert_eq!(
        one(
            "~~~old\na\n~~~\n",
            "$/code",
            Op::Replace("```new\nx\n```\n\n".into())
        ),
        "```new\nx\n```\n"
    );
    assert_eq!(
        one("- old", "$/list", Op::Replace("paragraph\n\n- new".into())),
        "paragraph\n\n- new"
    );
}

#[test]
fn gate_matches_surviving_anchors_once() {
    let schema=load_schema("version: 1\ntitle: null\nsections:\n  - match: Same\n    repeat: '0..n'\n    content:\n      - block: list\n        repeat: '0..1'\n        items: [{match: ok, repeat: '0..n'}]\n").unwrap().schema;
    let s = "## Same\n- bad\n\n## Same\n- other\n";
    let gate = |edits: &[Edit]| {
        candidate(s, edits)
            .unwrap()
            .introduced_diagnostics(&schema)
            .unwrap()
    };
    assert!(gate(&[op(
        "$.same[0]/list",
        Op::InsertItem {
            index: 0,
            payload: "ok".into()
        }
    )])
    .is_empty());
    let new = gate(&[
        op("$.same[0]/list/item", Op::Delete),
        op("$.same[1]/list", Op::AppendItem("third".into())),
    ]);
    assert_eq!(new.len(), 1);
    assert_eq!(new[0].id.as_str(), "unexpected-item");
    let both = "# Bad\n# Bad\n";
    let schema = load_schema("version: 1\noutline: []\n").unwrap().schema;
    assert!(
        candidate(both, &[op("$.bad[1]", Op::Rename("Other".into()))])
            .unwrap()
            .introduced_diagnostics(&schema)
            .unwrap()
            .is_empty()
    );
    let bad = "# Bad\n";
    let c = candidate(
        bad,
        &[
            op(
                "$",
                Op::ReplaceText {
                    old: "# Bad".into(),
                    new: "# Bad".into(),
                },
            ),
            op("$", Op::AppendBlock("New paragraph.".into())),
        ],
    )
    .unwrap();
    assert_ne!(c.text, bad);
    assert!(c.introduced_diagnostics(&schema).unwrap().is_empty());
}
#[test]
fn literal_scope_and_structural_verification() {
    let s = "# A\r\n\r\nold\r\n\r\n## B\r\n\r\nold\r\n";
    assert_eq!(
        edit(
            s,
            &[op(
                "$",
                Op::ReplaceText {
                    old: "old".into(),
                    new: "x".into()
                }
            )]
        ),
        Err("two-matches")
    );
    assert_eq!(
        edit(
            s,
            &[op(
                "$.b",
                Op::ReplaceText {
                    old: "absent".into(),
                    new: "x".into()
                }
            )]
        ),
        Err("not-found")
    );
    assert_eq!(
        one(
            s,
            "$.b/p",
            Op::ReplaceText {
                old: "old".into(),
                new: "x\ny".into()
            }
        ),
        s.replacen("## B\r\n\r\nold", "## B\r\n\r\nx\r\ny", 1)
    );
    assert_eq!(
        edit(
            s,
            &[op(
                "$.b",
                Op::ReplaceText {
                    old: "old".into(),
                    new: "# H".into()
                }
            )]
        ),
        Err("candidate-structure")
    );
}

#[test]
fn disjoint_literal_and_rename_share_a_heading() {
    assert_eq!(
        edit(
            "# old ###\n",
            &[
                op("$", Op::Rename("new".into())),
                op(
                    "$",
                    Op::ReplaceText {
                        old: "###".into(),
                        new: "####".into()
                    }
                )
            ]
        )
        .unwrap(),
        "# new ####\n"
    );
    assert_eq!(
        edit(
            "ababa",
            &[op(
                "$",
                Op::ReplaceText {
                    old: "aba".into(),
                    new: "x".into()
                }
            )]
        ),
        Err("two-matches")
    );
}

#[test]
fn document_voiced_absence_tracks_the_title() {
    let schema = load_schema("version: 1\ntitle: '*'\nsections: [{match: Needed}]\n")
        .unwrap()
        .schema;
    let c = candidate("intro\n\n# A\n", &[op("$", Op::Delete)]).unwrap();
    assert!(c
        .introduced_diagnostics(&schema)
        .unwrap()
        .iter()
        .any(|d| d.id == DiagnosticId::MissingSection));
}

#[test]
fn missing_item_tracks_a_list_when_its_first_marker_changes() {
    let schema = load_schema(
        "version: 1\ntitle: T\ncontent:\n  - block: list\n    items: [{match: needed}]\n",
    )
    .unwrap()
    .schema;
    let source = "# T\n\n- first\n- second\n";
    let before = validate(&schema, &parse(source).unwrap()).unwrap();
    assert!(before.iter().any(|d| d.id == DiagnosticId::MissingItem));

    let edited = candidate(source, &[op("$/list/item[0]", Op::Delete)]).unwrap();
    assert_eq!(edited.text, "# T\n- second\n");
    assert!(edited.introduced_diagnostics(&schema).unwrap().is_empty());
}

#[test]
fn adjacent_deletes_share_separator_lines() {
    for (source, paths, expected) in [
        ("a\n\nb\n\nc\n", ["$/p[1]", "$/p[2]"], "a\n\n"),
        ("- a\n\n- b\n", ["$/list/item[0]", "$/list/item[1]"], ""),
        (
            "# T\n\n## A\n\n## B\n\n## C\n",
            ["$.c", "$.b"],
            "# T\n\n## A\n\n",
        ),
    ] {
        let edits = paths.map(|path| op(path, Op::Delete));
        assert_eq!(edit(source, &edits).unwrap(), expected);
    }
}

#[test]
fn inserting_into_a_deleted_or_replaced_node_conflicts_at_both_edges() {
    for index in [0, 2] {
        let source = "para\n\n- a\n- b\n";
        for destructive in [Op::Replace("- new".into()), Op::Delete] {
            let edits = [
                op(
                    "$/list",
                    Op::InsertItem {
                        index,
                        payload: "x".into(),
                    },
                ),
                op("$/list", destructive),
            ];
            assert_eq!(edit(source, &edits), Err("conflict"));
        }
    }
    for operation in [
        Op::AppendBlock("body".into()),
        Op::InsertSection {
            placement: Placement::AppendChild,
            title: "Child".into(),
            preamble: None,
        },
    ] {
        assert_eq!(
            edit(
                "# T\n\n## B\n",
                &[op("$.b", Op::Delete), op("$.b", operation)]
            ),
            Err("conflict")
        );
    }
    assert_eq!(
        edit(
            "# T\n\n## A\n\n### Child\n",
            &[
                op("$.a", Op::Delete),
                op("$.a.child", Op::AppendBlock("body".into())),
            ]
        ),
        Err("conflict")
    );
}

#[test]
fn literal_gate_uses_the_same_heading_and_title_for_equivalent_candidates() {
    let schema = load_schema("version: 1\ntitle: '*'\nsections: [{match: Good}]\n")
        .unwrap()
        .schema;
    let source = "# Title\n\n## Bad\n";
    let original = parse(source).unwrap();
    let original_diagnostics = validate(&schema, &original).unwrap();
    assert!(original_diagnostics
        .iter()
        .any(|d| d.id == DiagnosticId::UnexpectedSection));
    assert!(original_diagnostics
        .iter()
        .any(|d| d.id == DiagnosticId::MissingSection));
    for (old, new) in [
        ("## Bad", "## Worse"),
        ("Bad", "Worse"),
        ("# Title", "# New"),
        ("Title", "New"),
    ] {
        let edits = [op(
            "$",
            Op::ReplaceText {
                old: old.into(),
                new: new.into(),
            },
        )];
        let candidate = candidate(source, &edits).unwrap();
        assert!(
            candidate
                .introduced_diagnostics(&schema)
                .unwrap()
                .is_empty(),
            "{old:?}"
        );
    }
}

#[test]
fn inserting_before_suppressed_nodes_keeps_the_directive_attached() {
    let source = "# T\n\n<!-- outlint-disable unexpected-section -->\n## Bad\n";
    let schema = load_schema("version: 1\ntitle: T\nsections: [{match: Good, repeat: '0..n'}]\n")
        .unwrap()
        .schema;
    for action in [
        op(
            "$.bad",
            Op::InsertSection {
                placement: Placement::Before,
                title: "Good".into(),
                preamble: None,
            },
        ),
        op("$", Op::AppendBlock("content".into())),
    ] {
        let edited = candidate(source, &[action]).unwrap();
        assert!(edited
            .text
            .contains("<!-- outlint-disable unexpected-section -->\n## Bad"));
        assert!(edited.introduced_diagnostics(&schema).unwrap().is_empty());
    }
    let source = "<!-- outlint-disable unexpected-block -->\n> old\n";
    assert_eq!(
        one(source, "$/quote", Op::InsertBefore("new".into())),
        "new\n\n<!-- outlint-disable unexpected-block -->\n> old\n"
    );
}

#[test]
fn a_fence_like_body_requires_its_own_closer_to_replace_the_whole_block() {
    assert_eq!(
        one(
            "```old\nold\n```\n",
            "$/code",
            Op::Replace("~~~\nx\n".into())
        ),
        "```old\n~~~\nx\n```\n",
    );
    assert_eq!(
        edit(
            "~~~\na\n~~~\n",
            &[op("$/code", Op::Replace("~~~\nx\n".into()))]
        ),
        Err("payload-form")
    );
    assert_eq!(
        one("~~~\na\n~~~\n", "$/code", Op::Replace("".into())),
        "~~~\n~~~\n"
    );
}

#[test]
fn rename_uses_the_core_atx_title_boundaries() {
    assert_eq!(
        one("# T\n\n## Old\t##\n", "$.old", Op::Rename("**New**".into())),
        "# T\n\n## **New**\n"
    );
    assert_eq!(one("#\n", "$", Op::Rename("New".into())), "# New\n");
    assert_eq!(one("# ###\n", "$", Op::Rename("New".into())), "# New ###\n");
    assert_eq!(
        edit("two\nlines\n=====\n", &[op("$", Op::Rename("New".into()))]),
        Err("target")
    );
}
