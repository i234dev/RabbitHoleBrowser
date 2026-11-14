//! Insertion-point continuation across real tokenizer compaction.
use rabbithole_dom::Document;
use rabbithole_html::{Parser,ParseStatus};

fn run_written_stream(prefix:&str) {
    let mut parser=Parser::new(true);let mut doc=Document::new();
    parser.append(&format!("<!doctype html><body>{prefix}<script id=outer></script><p id=after>tail</p>"));
    assert!(matches!(parser.run(&mut doc),ParseStatus::Script(_)),"pause at original outer script");
    parser.push_insertion_point_here();
    parser.write("<p id=one>á");
    assert!(matches!(parser.run(&mut doc),ParseStatus::NeedInput));
    parser.write("•😀&am");
    assert!(matches!(parser.run(&mut doc),ParseStatus::NeedInput));
    parser.write("p;</p><script id=inner></script><p id=two>dos</p>");
    assert!(matches!(parser.run(&mut doc),ParseStatus::Script(_)),"pause at written nested script");
    parser.push_insertion_point_here();
    parser.write("<i id=inside>中</i>");
    assert!(matches!(parser.run(&mut doc),ParseStatus::NeedInput));
    parser.pop_insertion_point();
    assert!(matches!(parser.run(&mut doc),ParseStatus::NeedInput));
    parser.pop_insertion_point();
    parser.append("<p id=network>fin</p>");parser.close();
    assert!(matches!(parser.run(&mut doc),ParseStatus::Finished));
    let actual:Vec<_>=doc.child_elements(doc.body().unwrap()).filter_map(|node|doc.attr(node,"id").map(str::to_owned)).collect();
    assert_eq!(actual.join("|"),"outer|one|inner|inside|two|after|network");
    assert_eq!(doc.text_content(doc.element_by_id("one").unwrap()),"á•😀&");
    assert_eq!(doc.text_content(doc.element_by_id("inside").unwrap()),"中");
}

#[test]
fn inserted_chunks_and_nested_points_keep_network_tail_order() {run_written_stream("");}

#[test]
fn insertion_offsets_survive_compaction_with_unicode_and_nested_points() {run_written_stream(&"é".repeat(70_000));}
