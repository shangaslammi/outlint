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
        "# New\n"
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
