use super::*;
use proptest::prelude::*;

fn op(path: &str, op: Op) -> Edit {
    Edit {
        path: DocumentPath::parse(path).unwrap(),
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
                one(&s, "$/list", Op::InsertItem(i, "new".into())),
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
            Op::InsertSection(Placement::AppendChild, "D".into(), "body".into())
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
                Op::InsertSection(place, "D".into(), "".into())
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
fn rejects_structural_collisions() {
    for (s, path, op_) in [
        ("one", "$/p", Op::Replace("# new".into())),
        ("- one", "$/list", Op::InsertAfter("- two".into())),
        (
            "~~~\na\n~~~\n\nkeep",
            "$/code",
            Op::Replace("~~~\nb".into()),
        ),
        (
            "~~~\na\n~~~",
            "$/code",
            Op::Replace("~~~\nb\n~~~\nescaped".into()),
        ),
        ("> a\n***\nlazy", "$/break", Op::Delete),
        (
            "# T\n#### A",
            "$.a",
            Op::InsertSection(Placement::Before, "B".into(), String::new()),
        ),
    ] {
        assert_eq!(edit(s, &[op(path, op_)]), Err(Error::Payload), "{s}")
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
        Err(Error::ConflictingEdits)
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
        Err(Error::Payload)
    );
}

proptest! {
    #[test]
    fn generated_splices_restore_original(
        parts in prop::collection::vec(prop_oneof![ "[^\r\n]{0,18}",prop::sample::select(vec!["# H","## H","- x","  - y","~~~","> lazy","<!-- c -->","", "    z","---"]).prop_map(str::to_owned)],0..16),
        crlf in any::<bool>(), trailing in any::<bool>(),pick in any::<usize>()
    ){
        let e=if crlf{"\r\n"}else{"\n"};
        let mut s=parts.join(e);if trailing{s.push_str(e);}
        let doc=parse(&s).unwrap();
        let paths=document_paths(&doc);
        let blocks:Vec<_>=paths.iter().filter(|(_,node)|matches!(node,DocumentNode::Block(_))).collect();
        if let Some((path,_))=blocks.get(pick%blocks.len().max(1)){
            let insert=Edit{path:DocumentPath::from(path),op:Op::InsertBefore("edit-probe".into())};
            match prepare(&s,&[insert]){
                Ok((out,changes))=>{
                    let [c]=changes.as_slice() else{unreachable!()};
                    prop_assert!(c.range.is_empty());
                    prop_assert_eq!(&out[..c.range.start],&s[..c.range.start]);
                    prop_assert_eq!(&out[c.range.start+c.replacement.len()..],&s[c.range.end..]);
                    let post=parse(&out).unwrap();
                    let entries=document_paths(&post);
                    let new_blocks:Vec<_>=entries.iter().filter(|(_,n)|matches!(n,DocumentNode::Block(_))).collect();
                    prop_assert_eq!(new_blocks.len(),blocks.len()+1);
                    let node=new_blocks[pick%blocks.len()].1;
                    prop_assert!(matches!(node,DocumentNode::Block(Block::Paragraph(_))));
                    let span=line_span(&out,node.extent().unwrap()).unwrap();
                    prop_assert_eq!(out[span].trim_end_matches(['\r','\n']),"edit-probe");
                }
                Err(err)=>prop_assert_eq!(err,Error::Payload),
            }
            let edit=Edit{path:DocumentPath::from(path),op:Op::Delete};
            match prepare(&s,&[edit]){
                Ok((out,changes))=>{
                    let [c]=changes.as_slice() else{unreachable!()};

                    prop_assert_eq!(&out[..c.range.start],&s[..c.range.start]);
                    prop_assert_eq!(&out[c.range.start..],&s[c.range.end..]);
                    let post=parse(&out).unwrap();
                    let block_count=|d:&Document|document_paths(d).iter().filter(|(_,n)|matches!(n,DocumentNode::Block(_))).count();
                    prop_assert_eq!(block_count(&post)+1,blocks.len());
                    // Try the public inverse with the physical node source, including indentation.
                    // Prefer the following sibling, then the preceding sibling, then append-block.
                    let global=pick%blocks.len();
                    let siblings:Vec<_>=blocks.iter().enumerate()
                        .filter(|(_,entry)|entry.0.sections()==path.sections()).collect();
                    let local=siblings.iter().position(|(idx,_)|*idx==global).unwrap();
                    let post_paths=document_paths(&post);
                    let remaining:Vec<_>=post_paths.iter().filter(|(p,n)|
                        p.sections()==path.sections() && matches!(n,DocumentNode::Block(_))).collect();
                    let raw=line_span(&s,blocks[global].1.extent().unwrap()).unwrap();
                    let payload=s[raw].to_owned();
                    let inverse=if let Some((p,_))=remaining.get(local){
                        Edit{path:DocumentPath::from(p),op:Op::InsertBefore(payload)}
                    }else if let Some((p,_))=remaining.last(){
                        Edit{path:DocumentPath::from(p),op:Op::InsertAfter(payload)}
                    }else{
                        Edit{path:DocumentPath::from(&path.prefix(path.sections().len())),op:Op::AppendBlock(payload)}
                    };
                    match prepare(&out,&[inverse]){
                        Ok((typed,inserts))=>{
                            let [ins]=inserts.as_slice() else{unreachable!()};
                            let j=ins.range.start;
                            let k=j.checked_add(c.range.len()).unwrap();
                            let exact=s.get(..j)==out.get(..j)
                                && s.get(j..k)==Some(ins.replacement.as_str())
                                && s.get(k..)==out.get(j..);
                            prop_assert_eq!(typed==s,exact);
                        }
                        Err(err)=>prop_assert_eq!(err,Error::Payload),
                    }
                    let mut restored=out;restored.insert_str(c.range.start,&s[c.range.clone()]);
                    prop_assert_eq!(restored,s);
                }
                Err(err)=>prop_assert_eq!(err,Error::Payload),
            }
        }
    }

    #[test]
    fn typed_delete_insert_inverse(a in "[a-z]{1,12}",b in "[a-z]{1,12}",c in "[a-z]{1,12}", gap in 1..5usize){
        let s=format!("{a}\n\n{b}{}{c}","\n".repeat(gap+1));
        let out=one(&s,"$/p[1]",Op::Delete);
        let restored=one(&out,"$/p[1]",Op::InsertBefore(b.clone()));
        // One following blank is owned. Insertion at the next node places all
        // surviving excess blank lines on the left of the reinserted node.
        prop_assert_eq!(restored==s,gap==1);
    }
}

#[test]
fn section_spelling_and_transparency() {
    assert_eq!(
        one("  ##  old  ###  \r\n", "$.old", Op::Rename("*new*".into())),
        "  ##  \\*new\\*  ###  \r\n"
    );
    assert_eq!(
        one("Title\n=====\n", "$", Op::Rename("New".into())),
        "New\n=====\n"
    );
    assert_eq!(
        one("  Title  \r\n=====\r\n", "$", Op::Rename("*new*".into())),
        "  \\*new\\*  \r\n=====\r\n"
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
            Err(Error::Target)
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
    assert!(gate(&[op("$.same[0]/list", Op::InsertItem(0, "ok".into()))]).is_empty());
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
            op("$", Op::ReplaceText("# Bad".into(), "# Bad".into())),
            op("$", Op::AppendBlock("New paragraph.".into())),
        ],
    )
    .unwrap();
    assert_ne!(c.text(), bad);
    assert!(c.introduced_diagnostics(&schema).unwrap().is_empty());
}
#[test]
fn literal_scope_and_structural_verification() {
    let s = "# A\r\n\r\nold\r\n\r\n## B\r\n\r\nold\r\n";
    assert_eq!(
        edit(s, &[op("$", Op::ReplaceText("old".into(), "x".into()))]),
        Err(Error::TextAmbiguous(DocumentPath::root(), 2))
    );
    assert_eq!(
        edit(
            s,
            &[op("$.b", Op::ReplaceText("absent".into(), "x".into()))]
        ),
        Err(Error::TextNotFound(DocumentPath::parse("$.b").unwrap()))
    );
    assert_eq!(
        one(s, "$.b/p", Op::ReplaceText("old".into(), "x\ny".into())),
        s.replacen("## B\r\n\r\nold", "## B\r\n\r\nx\r\ny", 1)
    );
    assert_eq!(
        edit(s, &[op("$.b", Op::ReplaceText("old".into(), "# H".into()))]),
        Err(Error::Payload)
    );
    assert_eq!(
        edit(
            "old\r\n",
            &[op("$", Op::ReplaceText("\n".into(), "".into()))]
        ),
        Err(Error::BareCr)
    );
}

#[test]
fn disjoint_literal_and_rename_share_a_heading() {
    assert_eq!(
        edit(
            "# old ###\n",
            &[
                op("$", Op::Rename("new".into())),
                op("$", Op::ReplaceText("###".into(), "####".into()))
            ]
        )
        .unwrap(),
        "# new ####\n"
    );
    assert_eq!(
        edit(
            "ababa",
            &[op("$", Op::ReplaceText("aba".into(), "x".into()))]
        ),
        Err(Error::TextAmbiguous(DocumentPath::root(), 2))
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
