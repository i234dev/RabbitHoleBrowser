use rabbithole_css::{parse_stylesheet, MediaEnv};
use rabbithole_style::{style_tree, ComputedStyle, GridLine};

fn computed(declarations: &str) -> ComputedStyle {
    let doc = rabbithole_html::parse("<div></div>");
    let sheets = [parse_stylesheet(&format!("div{{{declarations}}}"))];
    let styles = style_tree(&doc, &sheets, &MediaEnv { width: 800., height: 600., screen: true, scripting: true });
    styles.get(doc.find_element("div").unwrap()).unwrap().as_ref().clone()
}

#[test]
fn named_line_occurrences_keep_the_identifier_sign_and_order() {
    let s = computed("grid-column:y 5/y -2;grid-row:-3 Y/2 Y");
    assert_eq!(s.grid_column_start, GridLine::NamedNth("y".into(), 5, false));
    assert_eq!(s.grid_column_end, GridLine::NamedNth("y".into(), -2, false));
    assert_eq!(s.grid_row_start, GridLine::NamedNth("Y".into(), -3, false));
    assert_eq!(s.grid_row_end, GridLine::NamedNth("Y".into(), 2, false));
    let s = computed("grid-column:y 1;grid-row:y");
    assert_eq!(s.grid_column_start, GridLine::NamedNth("y".into(), 1, false));
    assert_eq!(s.grid_column_end, GridLine::Auto);
    assert_eq!(s.grid_row_start, GridLine::Named("y".into(), false));
    assert_eq!(s.grid_row_end, GridLine::Named("y".into(), false));
}

#[test]
fn named_spans_keep_their_count_in_each_component_order() {
    for value in ["span 2 y", "y span 2", "2 y span"] {
        let s = computed(&format!("grid-column:1/{value}"));
        assert_eq!(s.grid_column_start, GridLine::Line(1));
        assert_eq!(s.grid_column_end, GridLine::NamedNth("y".into(), 2, true));
    }
    assert_eq!(computed("grid-column-end:span y").grid_column_end, GridLine::Named("y".into(), true));
}

#[test]
fn invalid_grid_line_components_preserve_the_previous_declaration() {
    for invalid in ["y 0", "0", "y 1.5", "span -2 y", "span 0", "y y", "1 2 y", "span span y", "auto y", "auto 2", "inherit y", "default"] {
        let s = computed(&format!("grid-column:2/4;grid-column:{invalid}"));
        assert_eq!(s.grid_column_start, GridLine::Line(2), "{invalid}");
        assert_eq!(s.grid_column_end, GridLine::Line(4), "{invalid}");
    }
}
