use rabbithole_css::{parse_stylesheet, MediaEnv};
use rabbithole_style::{style_tree, ComputedStyle, Length, SubgridLineNames, TrackListItem, TrackSize};

fn computed(declarations: &str) -> ComputedStyle {
    let doc = rabbithole_html::parse("<div id=target></div>");
    let sheets = [parse_stylesheet(&format!("#target{{{declarations}}}"))];
    let tree = style_tree(&doc, &sheets, &MediaEnv { width:800., height:600., screen:true, scripting:true });
    tree.get(doc.element_by_id("target").unwrap()).unwrap().as_ref().clone()
}

#[test]
fn subgrid_requests_keep_each_axis_distinct_from_none() {
    let columns = computed("grid-template-columns:subgrid;grid-template-rows:none");
    assert_eq!(columns.grid_template_columns, [TrackListItem::Subgrid { line_names:vec![] }]);
    assert!(columns.grid_template_rows.is_empty());
    let rows = computed("grid-template-rows:subgrid;grid-template-columns:50px");
    assert_eq!(rows.grid_template_rows, [TrackListItem::Subgrid { line_names:vec![] }]);
    assert!(matches!(rows.grid_template_columns[0], TrackListItem::Track { size:TrackSize::Length(Length::Px(50.)), .. }));
    let both = computed("grid-template:subgrid/subgrid");
    assert_eq!(both.grid_template_columns, columns.grid_template_columns);
    assert_eq!(both.grid_template_rows, rows.grid_template_rows);
}

#[test]
fn subgrid_named_lines_and_repeats_preserve_boundaries_without_expansion() {
    let style = computed("grid-template-columns:subgrid [start] [] repeat(1000000,[x] [y z]) repeat(auto-fill,[more]) [end]");
    assert_eq!(style.grid_template_columns, [TrackListItem::Subgrid { line_names:vec![
        SubgridLineNames::Names(vec!["start".into()]), SubgridLineNames::Names(vec![]),
        SubgridLineNames::Repeat { count:Some(1000000), names:vec![vec!["x".into()],vec!["y".into(),"z".into()]] },
        SubgridLineNames::Repeat { count:None, names:vec![vec!["more".into()]] },
        SubgridLineNames::Names(vec!["end".into()]),
    ] }]);
}

#[test]
fn invalid_subgrid_names_do_not_overwrite_a_valid_track_list() {
    let previous = computed("grid-template-columns:25px 50px").grid_template_columns;
    for invalid in ["subgrid 10px","subgrid [unclosed","subgrid [span]","subgrid repeat(0,[x])","subgrid repeat(1.5,[x])","subgrid repeat(auto-fit,[x])","subgrid repeat(2,10px)","subgrid repeat(2,repeat(2,[x]))","subgrid repeat(auto-fill,[x]) repeat(auto-fill,[y])"] {
        let style = computed(&format!("grid-template-columns:25px 50px;grid-template-columns:{invalid}"));
        assert_eq!(style.grid_template_columns,previous,"{invalid}");
    }
    assert!(computed("grid-template-columns:subgrid;grid-template-columns:none").grid_template_columns.is_empty());
}

#[test]
fn normal_gaps_keep_their_origin_without_changing_ordinary_used_lengths() {
    let normal=computed("");
    assert!(normal.row_gap_normal&&normal.column_gap_normal);
    assert_eq!((normal.row_gap,normal.column_gap),(Length::ZERO,Length::ZERO));
    let mixed=computed("gap:0 normal");
    assert!(!mixed.row_gap_normal&&mixed.column_gap_normal);
    assert_eq!((mixed.row_gap,mixed.column_gap),(Length::ZERO,Length::ZERO));
    let mixed=computed("gap:normal 0;grid-row-gap:0;column-gap:normal");
    assert!(!mixed.row_gap_normal&&mixed.column_gap_normal);
    let reset=computed("gap:0;row-gap:initial;column-gap:unset");
    assert!(reset.row_gap_normal&&reset.column_gap_normal);
}

#[test]
fn inherited_gap_metadata_follows_the_same_property_as_its_length() {
    let doc=rabbithole_html::parse("<div id=parent><div id=target></div></div>");
    let sheets=[parse_stylesheet("#parent{gap:normal 0}#target{gap:inherit}")];
    let tree=style_tree(&doc,&sheets,&MediaEnv{width:800.,height:600.,screen:true,scripting:true});
    let child=tree.get(doc.element_by_id("target").unwrap()).unwrap();
    assert!(child.row_gap_normal&&!child.column_gap_normal);
    assert_eq!((child.row_gap,child.column_gap),(Length::ZERO,Length::ZERO));
}
