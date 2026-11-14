//! `content` and `auto` must survive shorthand expansion and CSS-wide copies.
use rabbithole_css::{parse_stylesheet, MediaEnv};
use rabbithole_style::{style_tree, ComputedStyle, Length};

fn computed(declarations: &str) -> ComputedStyle {
    let doc = rabbithole_html::parse("<section><div id=target></div></section>");
    let sheets = [parse_stylesheet(&format!("section{{flex:2 0 content}}#target{{{declarations}}}"))];
    let tree = style_tree(&doc, &sheets, &MediaEnv {
        width: 800., height: 600., screen: true, scripting: true,
    });
    tree.get(doc.element_by_id("target").unwrap()).unwrap().as_ref().clone()
}

#[test]
fn content_basis_is_distinct_from_auto_and_is_not_inherited_by_default() {
    for declaration in ["flex-basis:content", "flex:content", "flex:0 1 content", "-webkit-flex-basis:content", "-ms-flex:0 1 content"] {
        let s = computed(declaration);
        assert_eq!(s.flex_basis, Length::Auto, "{declaration}");
        assert!(s.flex_basis_content, "{declaration}");
    }
    for declaration in ["", "flex-basis:auto", "flex:auto", "flex:none"] {
        let s = computed(declaration);
        assert_eq!(s.flex_basis, Length::Auto, "{declaration}");
        assert!(!s.flex_basis_content, "{declaration}");
    }
}

#[test]
fn replacing_content_resets_the_flag_for_all_basis_forms() {
    for declaration in ["flex-basis:auto", "flex-basis:20px", "flex-basis:25%", "flex-basis:min-content", "flex-basis:max-content", "flex-basis:fit-content", "flex:auto", "flex:none", "flex:1", "flex:2 0 30px", "flex:0 1 0", "-webkit-flex:auto"] {
        assert!(!computed(&format!("flex-basis:content;{declaration}")).flex_basis_content, "{declaration}");
    }
}

#[test]
fn invalid_basis_preserves_content_and_the_entire_shorthand_tuple() {
    for declaration in ["flex-basis:-10px", "flex-basis:bogus", "flex:4 1 -20px", "flex:4 1 content 20px"] {
        let s = computed(&format!("flex:2 0 content;{declaration}"));
        assert_eq!((s.flex_grow, s.flex_shrink, s.flex_basis), (2., 0., Length::Auto), "{declaration}");
        assert!(s.flex_basis_content, "{declaration}");
    }
}

#[test]
fn wide_values_copy_or_reset_the_basis_metadata_with_its_value() {
    for declaration in ["flex-basis:inherit", "flex:inherit", "-webkit-flex-basis:inherit", "-ms-flex-preferred-size:inherit", "-webkit-flex:inherit", "-ms-flex:inherit"] {
        assert!(computed(declaration).flex_basis_content, "{declaration}");
    }
    let s = computed("flex:inherit");
    assert_eq!((s.flex_grow, s.flex_shrink), (2., 0.));
    for declaration in ["flex-basis:initial", "flex-basis:unset", "flex:initial", "flex:unset", "-webkit-flex-basis:initial", "-ms-flex-preferred-size:unset", "-webkit-flex:initial", "-ms-flex:unset"] {
        let s = computed(&format!("flex:3 0 content;{declaration}"));
        assert_eq!(s.flex_basis, Length::Auto, "{declaration}");
        assert!(!s.flex_basis_content, "{declaration}");
    }
}
