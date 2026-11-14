use rabbithole_css::{parse_stylesheet, MediaEnv};
use rabbithole_style::{style_tree, ComputedStyle, Length};

fn computed(declarations: &str) -> ComputedStyle {
    let doc = rabbithole_html::parse("<div id=target></div>");
    let sheets = [parse_stylesheet(&format!("#target{{{declarations}}}"))];
    let tree = style_tree(&doc, &sheets, &MediaEnv {
        width: 800., height: 600., screen: true, scripting: true,
    });
    tree.get(doc.element_by_id("target").unwrap()).unwrap().clone()
}

#[test]
fn negative_basis_literals_are_ignored_without_replacing_a_valid_declaration() {
    for value in ["-50px", "-10%", "-2em", "-3vw"] {
        assert_eq!(computed(&format!("flex-basis:{value}")).flex_basis, Length::Auto, "{value}");
        assert_eq!(computed(&format!("flex-basis:20px;flex-basis:{value}")).flex_basis, Length::Px(20.), "{value}");
        assert_eq!(computed(&format!("flex-basis:25%;flex-basis:{value}")).flex_basis, Length::Percent(25.), "{value}");
    }
    assert_eq!(computed("flex-basis:20px;-webkit-flex-basis:-50px").flex_basis, Length::Px(20.));
    assert_eq!(computed("flex-basis:20px;flex-basis:-0px").flex_basis, Length::Px(0.));
}

#[test]
fn invalid_negative_shorthand_basis_preserves_the_entire_previous_tuple() {
    for value in ["-50px", "-10%"] {
        let s = computed(&format!("flex:2 0 20px;flex:4 1 {value}"));
        assert_eq!((s.flex_grow, s.flex_shrink, s.flex_basis), (2., 0., Length::Px(20.)));
    }
    let s = computed("flex:2 0 20px;-webkit-flex:4 1 -50px");
    assert_eq!((s.flex_grow, s.flex_shrink, s.flex_basis), (2., 0., Length::Px(20.)));
}

#[test]
fn negative_math_results_remain_valid_and_positive_variables_still_resolve() {
    for value in ["calc(-50px)", "calc(-20px - 30px)", "min(-50px, 20px)"] {
        let basis = computed(&format!("flex-basis:20px;flex-basis:{value}")).flex_basis;
        assert_eq!(basis.resolve(100.), Some(-50.), "{value}");
    }
    let s = computed("--basis:-50px;flex-basis:20px;flex-basis:calc(var(--basis))");
    assert_eq!(s.flex_basis.resolve(100.), Some(-50.));
    assert_eq!(computed("--basis:25px;flex-basis:var(--basis)").flex_basis, Length::Px(25.));
    let s = computed("flex:2 0 20px;flex:4 1 calc(-50px)");
    assert_eq!((s.flex_grow, s.flex_shrink), (4., 1.));
    assert_eq!(s.flex_basis.resolve(100.), Some(-50.));
}
