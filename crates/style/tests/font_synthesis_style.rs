use rabbithole_css::{parse_stylesheet, MediaEnv};
use rabbithole_style::{style_tree, ComputedStyle};

fn computed(parent: &str, child: &str) -> ComputedStyle {
    let doc = rabbithole_html::parse("<div id=parent><span id=target>Filler text</span></div>");
    let sheets = [parse_stylesheet(&format!("#parent{{{parent}}}#target{{{child}}}"))];
    let tree = style_tree(&doc, &sheets, &MediaEnv {
        width: 800., height: 600., screen: true, scripting: true,
    });
    tree.get(doc.element_by_id("target").unwrap()).unwrap().as_ref().clone()
}

#[test]
fn synthesis_style_longhand_inherits_and_resolves_css_wide_keywords() {
    assert!(computed("", "").font_synthesis_style);
    for parent in ["font-synthesis-style:none", "font-synthesis:none"] {
        for child in ["", "font-synthesis-style:inherit", "font-synthesis-style:unset"] {
            assert!(!computed(parent, child).font_synthesis_style, "parent={parent}; child={child}");
        }
        for child in ["font-synthesis-style:auto", "font-synthesis-style:initial"] {
            assert!(computed(parent, child).font_synthesis_style, "parent={parent}; child={child}");
        }
    }
    assert!(computed("font-synthesis-style:auto", "font-synthesis-style:none;font-synthesis-style:inherit").font_synthesis_style);
}

#[test]
fn synthesis_style_longhand_and_shorthand_follow_declaration_order() {
    for (declarations, expected) in [
        ("font-synthesis:style;font-synthesis-style:none", false),
        ("font-synthesis-style:none;font-synthesis:style", true),
        ("font-synthesis:none;font-synthesis-style:auto", true),
        ("font-synthesis-style:auto;font-synthesis:none", false),
        ("font-synthesis-style:none;font-synthesis-style:auto", true),
        ("font-synthesis-style:auto;font-synthesis-style:none", false),
    ] {
        assert_eq!(computed("", declarations).font_synthesis_style, expected, "{declarations}");
    }
}

#[test]
fn invalid_synthesis_style_values_preserve_the_previous_declaration() {
    for invalid in ["auto none", "none auto", "unknown", "'none'", "0", "none,auto", "none()"] {
        for (previous, expected) in [("none", false), ("auto", true)] {
            let declarations = format!("font-synthesis-style:{previous};font-synthesis-style:{invalid}");
            assert_eq!(computed("", &declarations).font_synthesis_style, expected, "{declarations}");
        }
    }
}
