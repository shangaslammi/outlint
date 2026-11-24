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
        "# T\nbody\n## C"
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
        "# T\n## C\n## D\n\nbody"
    );
    assert_eq!(one(empty, "$.c", Op::Delete), "# T\n");
    for (place, expected) in [
        (Placement::Before, "# T\n## D\n## C"),
        (Placement::After, "# T\n## C\n## D"),
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
fn a_separate_inserted_list_cannot_merge_with_its_neighbor() {
    assert_eq!(
        edit("- one", &[op("$/list", Op::InsertAfter("- two".into()))]),
        Err("candidate-structure")
    );
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
        "# T\n\n<!-- tail -->\n"
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
fn inserting_before_suppressed_nodes_keeps_the_directive_attached() {
    let source = "# T\n\n<!-- outlint-disable unexpected-section -->\n## Bad\n";
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
    }
    let source = "<!-- outlint-disable unexpected-block -->\n> old\n";
    assert_eq!(
        one(source, "$/quote", Op::InsertBefore("new".into())),
        "new\n<!-- outlint-disable unexpected-block -->\n> old\n"
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

#[test]
fn insertion_order_follows_the_hierarchy_in_both_script_orders() {
    let source = "# T\n\n## A\n\npara\n\n## B\n";
    let append = || op("$.a", Op::AppendBlock("y".into()));
    let after = || op("$.a/p", Op::InsertAfter("y".into()));
    let sibling = || section("$.a", Placement::After, "Z");
    let child = || section("$.a", Placement::AppendChild, "K");
    for (source, mut edits, expected) in [
        (
            source,
            [sibling(), after()],
            "# T\n\n## A\n\npara\n\ny\n\n## Z\n\n## B\n",
        ),
        (
            source,
            [sibling(), append()],
            "# T\n\n## A\n\npara\n\ny\n\n## Z\n\n## B\n",
        ),
        (
            source,
            [sibling(), child()],
            "# T\n\n## A\n\npara\n\n### K\n\n## Z\n\n## B\n",
        ),
        (
            source,
            [child(), after()],
            "# T\n\n## A\n\npara\n\ny\n\n### K\n\n## B\n",
        ),
        (
            "## A\n## B\n",
            [section("$.b", Placement::Before, "N"), append()],
            "## A\ny\n## N\n## B\n",
        ),
        (
            "# T\n\n## A\n\npara\n",
            [section("$", Placement::AppendChild, "Z"), after()],
            "# T\n\n## A\n\npara\n\ny\n\n## Z\n",
        ),
        (
            "- a\n",
            [
                op("$/list", Op::InsertAfter("para".into())),
                op("$/list", Op::AppendItem("x".into())),
            ],
            "- a\n- x\n\npara\n",
        ),
    ] {
        assert_eq!(edit(source, &edits), Ok(expected.into()), "{edits:?}");
        edits.reverse();
        assert_eq!(edit(source, &edits), Ok(expected.into()), "{edits:?}");
    }
}

#[test]
fn equivalent_slots_preserve_script_order() {
    let source = "# T\n\n## A\n\npara\n\n## B\n";
    let mut edits = [
        op("$.a", Op::AppendBlock("first".into())),
        op("$.a/p", Op::InsertAfter("second".into())),
        section("$.b", Placement::Before, "N"),
        section("$.a", Placement::After, "Z"),
    ];
    assert_eq!(
        edit(source, &edits),
        Ok("# T\n\n## A\n\npara\n\nfirst\n\nsecond\n\n## N\n\n## Z\n\n## B\n".into())
    );
    edits.reverse();
    assert_eq!(
        edit(source, &edits),
        Ok("# T\n\n## A\n\npara\n\nsecond\n\nfirst\n\n## Z\n\n## N\n\n## B\n".into())
    );
}

#[test]
fn script_order_within_one_slot_can_request_unrepresentable_parentage() {
    let source = "# T\n### A\n## B\n";
    let mut edits = [
        section("$.b", Placement::Before, "N"),
        section("$.a", Placement::After, "Z"),
    ];
    assert_eq!(edit(source, &edits), Err("candidate-structure"));
    edits.reverse();
    assert_eq!(
        edit(source, &edits),
        Ok("# T\n### A\n### Z\n## N\n## B\n".into())
    );
}

#[test]
fn deletions_compose_with_beside_insertions_and_never_consume_the_list_separator() {
    for (source, mut edits, expected) in [
        (
            "a\n\nb\n",
            vec![
                op("$/p[1]", Op::Delete),
                op("$/p[1]", Op::InsertBefore("x".into())),
            ],
            "a\n\nx\n",
        ),
        (
            "# T\n\n## A\n\n## B\n",
            vec![op("$.a", Op::Delete), section("$.a", Placement::After, "X")],
            "# T\n\n## X\n\n## B\n",
        ),
        (
            "# T\n\n## A\n\n## B\n",
            vec![
                op("$.b", Op::Delete),
                section("$.b", Placement::Before, "X"),
            ],
            "# T\n\n## A\n\n## X\n",
        ),
        (
            "## H\n\n- Alpha\n- Alpha\n",
            vec![
                op("$.h/list/item[0]", Op::Delete),
                op(
                    "$.h/list",
                    Op::InsertItem {
                        index: 0,
                        payload: "Alpha".into(),
                    },
                ),
            ],
            "## H\n\n- Alpha\n- Alpha\n",
        ),
        (
            "para\n\n- a\n- b\n",
            vec![
                op("$/list", Op::Delete),
                op("$/list", Op::InsertBefore("x".into())),
                op("$/list", Op::InsertAfter("y".into())),
            ],
            "para\n\nx\n\ny\n",
        ),
        (
            "x\n\n- a\n- b\n\ny\n",
            vec![op("$/list/item[1]", Op::Delete)],
            "x\n\n- a\n\ny\n",
        ),
        (
            "x\n\n2. a\n3. b\n",
            vec![op("$/list/item[0]", Op::Delete)],
            "x\n\n3. b\n",
        ),
        (
            "x\n\n- a\n- b\n\npara\n",
            vec![
                op("$/list/item[0]", Op::Delete),
                op("$/list/item[1]", Op::Delete),
            ],
            "x\n\npara\n",
        ),
        (
            "a\n\n```\ncode\n```\nb\n",
            vec![op("$/code", Op::Delete)],
            "a\n\nb\n",
        ),
    ] {
        assert_eq!(edit(source, &edits), Ok(expected.into()), "{edits:?}");
        edits.reverse();
        assert_eq!(edit(source, &edits), Ok(expected.into()), "{edits:?}");
    }
}

#[test]
fn existing_suppressed_first_item_refuses_insertion_at_gap_zero() {
    assert_eq!(
        edit(
            "x\n\n<!-- outlint-disable unexpected-item -->\n- a\n- b\n",
            &[op(
                "$/list",
                Op::InsertItem {
                    index: 0,
                    payload: "c".into()
                }
            )]
        ),
        Err("target")
    );
}

#[test]
fn conflicting_renames_of_an_empty_title_do_not_reach_assembly() {
    assert_eq!(
        edit(
            "#",
            &[
                op("$", Op::Rename("first".into())),
                op("$", Op::Rename("second".into()))
            ]
        ),
        Err("conflict")
    );
    assert_eq!(
        edit(
            "#",
            &[
                op("$", Op::Rename("title".into())),
                op("$", Op::AppendBlock("body".into()))
            ]
        ),
        Ok("# title\nbody".into())
    );
}

fn section(path: &str, placement: Placement, title: &str) -> Edit {
    op(
        path,
        Op::InsertSection {
            placement,
            title: title.into(),
            preamble: None,
        },
    )
}

#[test]
fn comments_at_item_indentation_follow_their_suppressed_item() {
    for gap in ["", "\n", " \t\n\n"] {
        let source = format!("- a\n{gap}  <!-- outlint-disable unexpected-item -->\n  <!-- outlint-disable misplaced-item -->\n- b\n- c\n");
        let document = parse(&source).unwrap();
        let Some(Block::List(list)) = document.preamble.iter().next() else {
            panic!("expected a list");
        };
        assert!(list.items.rest[0].suppressions.contains("misplaced-item"));
        assert_eq!(one(&source, "$/list/item[1]", Op::Delete), "- a\n- c\n");
    }
}

#[test]
fn insertion_after_a_dangling_inline_directive_leaves_it_detached() {
    let directive = "<!-- outlint-disable unexpected-section -->";
    let source = format!("{directive}\n");
    assert_eq!(
        edit(&source, &[section("$", Placement::AppendChild, "N")]),
        Ok(format!("{directive}\n\n# N\n"))
    );
    let source = format!("# T\n- a\n  {directive}\n");
    assert_eq!(
        edit(&source, &[section("$", Placement::AppendChild, "N")]),
        Ok(format!("{source}\n## N\n"))
    );
}

#[test]
fn changed_seams_separate_lazy_continuations_only_when_needed() {
    for (source, expected) in [
        ("- a\n\n***\nTitle\n===\n", "- a\n\nTitle\n===\n"),
        ("> q\n\n***\nTitle\n===\n", "> q\n\nTitle\n===\n"),
        ("- a\n\n***\n<custom-tag>\n", "- a\n\n<custom-tag>\n"),
        ("> q\n\n***\n    code\n", "> q\n\n    code\n"),
        ("para\n\n***\n2. ordered\n", "para\n\n2. ordered\n"),
        ("para\n\n***\n- bullet\n", "para\n- bullet\n"),
    ] {
        assert_eq!(one(source, "$/break", Op::Delete), expected);
    }
    assert_eq!(
        one("> q\n## H\n", "$/quote", Op::InsertAfter("    code".into())),
        "> q\n\n    code\n## H\n"
    );
    assert_eq!(
        one(
            "- a\n## H\n",
            "$/list",
            Op::InsertAfter("<custom-tag>".into())
        ),
        "- a\n\n<custom-tag>\n\n## H\n"
    );
    let source = "para\n- a\n- b\n";
    assert_eq!(one(source, "$/list/item[0]", Op::Delete), "para\n- b\n");
    assert_eq!(
        one(
            source,
            "$/list",
            Op::InsertItem {
                index: 0,
                payload: "n".into()
            }
        ),
        "para\n- n\n- a\n- b\n"
    );
    let source = "a\n<!-- outlint-disable unexpected-block -->\nb\n";
    assert_eq!(
        one(source, "$/p[0]", Op::InsertAfter("new".into())),
        "a\n\nnew\n<!-- outlint-disable unexpected-block -->\nb\n"
    );
}

#[test]
fn insertion_gaps_come_from_siblings() {
    let source = "# T\n\npara\n\n- a\n- b\n";
    assert_eq!(
        edit(source, &[section("$", Placement::AppendChild, "S")]),
        Ok(format!("{source}\n## S\n"))
    );
    assert_eq!(
        one(source, "$/list", Op::InsertAfter("```\ncode\n```".into())),
        format!("{source}\n```\ncode\n```\n")
    );
    for gap in ["\n", " \t\n\n"] {
        let source = format!("## A\n\npara\n{gap}## B\n");
        assert_eq!(
            edit(&source, &[section("$.a", Placement::Before, "N")]),
            Ok(format!("## N\n{gap}{source}"))
        );
    }
    assert_eq!(
        one("a\n \t\n\nb", "$/p[0]", Op::InsertBefore("new".into())),
        "new\n \t\n\na\n \t\n\nb"
    );
}

#[test]
fn deleting_a_first_item_removes_its_shared_list_directive() {
    let source = "# T\n\n<!-- outlint-disable unexpected-block -->\n- a\n- b\n";
    assert_eq!(one(source, "$/list/item[0]", Op::Delete), "# T\n\n- b\n");
}

#[test]
fn a_literal_can_add_blank_lines_at_a_node_end() {
    for (source, expected) in [
        ("a\n\nb\n", "a\n\t\n\nb\n"),
        ("a\r\n\r\nb", "a\r\n\t\r\n\r\nb"),
        ("a\n", "a\n\t\n"),
    ] {
        assert_eq!(
            one(
                source,
                "$",
                Op::ReplaceText {
                    old: "a\n".into(),
                    new: "a\n\t\n".into()
                }
            ),
            expected
        );
    }
}

proptest! {
    #[test]
    fn generated_batches_preserve_bytes_and_put_nodes_in_expected_parents(
        selection in prop::collection::btree_map(0usize..12, any::<u16>(), 2..7),
        ending in prop_oneof![Just("\n"), Just("\r\n")],
        gap_lines in 1usize..4,
        loose in any::<bool>(),
        trailing in 0usize..4,
        transparent_comment in any::<bool>(),
        attached_directive in any::<bool>(),
        word in "[a-z]{1,8}",
    ) {
        let chosen = |id| selection.contains_key(&id);
        let gap = format!("{ending} \t{ending}{}", ending.repeat(gap_lines - 1));
        let inter_item = if loose { gap.as_str() } else { ending };
        let alpha = format!("Alpha {word}");
        let mut prefix = vec!["# T", "lead  "];
        if transparent_comment {
            prefix.push("<!-- note -->");
        }
        let directive = "<!-- outlint-disable unexpected-block -->";
        let mut source = prefix.join(&gap);
        source.push_str(&gap);
        source.push_str(&["## A", &alpha, "beta"].join(&gap));
        source.push_str(&gap);
        if attached_directive {
            source.push_str(directive);
            source.push_str(ending);
        }
        source.push_str("- one");
        source.push_str(inter_item);
        source.push_str("- two");
        for line in ["### Old", "## B", "end\t"] {
            source.push_str(&gap);
            source.push_str(line);
        }
        source.push_str(&ending.repeat(trailing));
        let operation = |id| match id {
            0 => op("$.a/p[0]", Op::Delete),
            1 => op("$.a/p[0]", Op::InsertAfter("extra".into())),
            2 => op("$.a/p[1]", Op::Replace("replacement".into())),
            3 => op("$.a/list/item[0]", Op::Delete),
            4 => op("$.a/list", Op::AppendItem("three".into())),
            5 => op("$.a", Op::AppendBlock("tail".into())),
            6 => section("$.a", Placement::AppendChild, "New"),
            7 => section("$.a", Placement::After, "Next"),
            8 => op("$.a.old", Op::Delete),
            9 => op("$.b", Op::Rename("Renamed".into())),
            10 => op("$.a/p[1]", Op::InsertBefore("before beta".into())),
            11 => op("$.a/list", Op::Delete),
            _ => unreachable!(),
        };
        let mut script: Vec<_> = selection.keys().copied().collect();
        script.sort_by_key(|id| selection[id]);
        let mut reordered = script.clone();
        reordered.reverse();
        // Both address the seam between the two paragraphs.
        if let (Some(a), Some(b)) = (script.iter().position(|id| *id == 1), script.iter().position(|id| *id == 10)) {
            reordered.swap(script.len() - 1 - a, script.len() - 1 - b);
        }
        let normal = edit(&source, &script.iter().copied().map(operation).collect::<Vec<_>>());
        let reverse = edit(&source, &reordered.into_iter().map(operation).collect::<Vec<_>>());
        prop_assert_eq!(&normal, &reverse);
        if chosen(11) && (chosen(3) || chosen(4)) {
            prop_assert_eq!(normal, Err("conflict"));
        } else {
            let mut lines = vec![("# T", None), ("lead  ", Some("# T")), ("## A", Some("# T"))];
            if !chosen(0) {
                lines.push((&alpha, Some("## A")));
            }
            for id in &script {
                match id {
                    1 => lines.push(("extra", Some("## A"))),
                    10 => lines.push(("before beta", Some("## A"))),
                    _ => {}
                }
            }
            lines.push((if chosen(2) { "replacement" } else { "beta" }, Some("## A")));
            if !chosen(11) {
                if !chosen(3) {
                    lines.push(("- one", Some("list")));
                }
                lines.push(("- two", Some("list")));
                if chosen(4) {
                    lines.push(("- three", Some("list")));
                }
            }
            if chosen(5) {
                lines.push(("tail", Some("## A")));
            }
            if !chosen(8) {
                lines.push(("### Old", Some("## A")));
            }
            if chosen(6) {
                lines.push(("### New", Some("## A")));
            }
            if chosen(7) {
                lines.push(("## Next", Some("# T")));
            }
            let b = if chosen(9) { "## Renamed" } else { "## B" };
            lines.extend([(b, Some("# T")), ("end\t", Some(b))]);
            let mut exact = String::new();
            let mut inside_list = false;
            for (line, parent) in &lines {
                let item = *parent == Some("list");
                if !exact.is_empty() {
                    exact.push_str(if item && inside_list { inter_item } else { &gap });
                }
                if *line == "## A" && transparent_comment {
                    exact.push_str("<!-- note -->");
                    exact.push_str(&gap);
                }
                if *line == "- one" && attached_directive {
                    exact.push_str(directive);
                    exact.push_str(ending);
                }
                exact.push_str(line);
                inside_list = item;
            }
            exact.push_str(&ending.repeat(trailing));
            let text = normal.unwrap();
            prop_assert_eq!(&text, &exact);
            let parsed = parse(&text).unwrap();
            let rows = model::rows(&text, &parsed).unwrap();
            let actual: Vec<_> = rows.iter().filter(|row| !model::is_list(row.token.kind)).map(|row| {
                let parent = row.token.parent.map(|index| {
                    let token = &rows[index].token;
                    if model::is_list(token.kind) { "list" } else { token.spelling.as_str() }
                });
                (row.token.spelling.as_str(), parent)
            }).collect();
            prop_assert_eq!(actual, lines);
        }
    }
}

#[test]
fn replacing_an_item_keeps_its_marker_and_padding() {
    assert_eq!(
        one(
            "09)   old\n10)  other\n",
            "$/list/item[0]",
            Op::Replace("new\n- nested".into())
        ),
        "09)   new\n      - nested\n10)  other\n"
    );
}

#[test]
fn deleting_the_first_node_keeps_only_the_original_file_prefix() {
    assert_eq!(one(" \t\na\n\n\nb\n", "$/p", Op::Delete), " \t\nb\n");
}

#[test]
fn an_append_block_payload_can_contain_multiple_visible_blocks() {
    assert_eq!(
        one(
            "# T\n\n## Child\n",
            "$",
            Op::AppendBlock("first\n\n> second".into())
        ),
        "# T\n\nfirst\n\n> second\n\n## Child\n"
    );
}

#[test]
fn a_whitespace_only_gap_does_not_terminate_the_inserted_paragraph() {
    let source = "a\n  \n\t\nb\n";
    let expected = "a\n  \n\t\ny\n  \n\t\nb\n";
    assert_eq!(one(source, "$/p[0]", Op::InsertAfter("y".into())), expected);
    assert_eq!(
        one(source, "$/p[1]", Op::InsertBefore("y".into())),
        expected
    );
}

#[test]
fn replacement_retains_its_existing_gap() {
    assert_eq!(
        edit(
            "a\n> b\n",
            &[op("$/quote", Op::Replace("paragraph".into()))]
        ),
        Err("candidate-structure")
    );
}
