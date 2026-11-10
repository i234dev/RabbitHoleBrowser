//! CSS error handling: invalid declarations are dropped as a whole, rules
//! with invalid selectors are dropped, and the parser recovers from
//! malformed blocks (CSS 2.1 §4.2, CSS Syntax 3). Cases from Acid2.

use crate::computed::*;
use crate::{style_tree, MediaEnv, StyledTree};
use rabbithole_css::parse_stylesheet;
use rabbithole_dom::Document;
use rabbithole_gfx::Color;

fn styled(html: &str, css: &str) -> (Document, StyledTree) {
    let doc = rabbithole_html::parse(html);
    let env = MediaEnv { width: 1000.0, height: 800.0, screen: true, scripting: true };
    let tree = style_tree(&doc, &[parse_stylesheet(css)], &env);
    (doc, tree)
}

fn div(css: &str) -> ComputedStyle {
    let (doc, t) = styled("<div class=parser>x</div>", css);
    (**t.get(doc.find_element("div").unwrap()).unwrap()).clone()
}

const YELLOW: Color = Color { r: 255, g: 255, b: 0, a: 255 };

#[test]
fn acid2_parser_rules() {
    let css = r".parser { margin: 0 5em 1em; padding: 0 1em; width: 2em; height: 1em; error: \}; background: yellow; }
        * html .parser { background: gray; }
        \.parser { padding: 2em; }
        .parser { m\argin: 2em; };
        .parser { height: 3em; }
        .parser { width: 200; }
        .parser { border: 5em solid red ! error; }
        .parser { background: red pink; }";
    let s = div(&format!("div {{ font-size: 12px }} {css}"));
    assert_eq!(s.background_color, YELLOW);
    assert_eq!(s.width, Length::Px(24.0));
    // `.parser { height: 3em }` follows a stray `;`: its selector is invalid.
    assert_eq!(s.height, Length::Px(12.0));
    assert_eq!(s.margin.top, Length::Px(0.0));
    assert_eq!(s.margin.right, Length::Px(60.0));
    assert_eq!(s.padding.left, Length::Px(12.0));
    assert_eq!(s.border_width.top, 0.0);
}

#[test]
fn invalid_values_drop_the_whole_declaration() {
    let s = div("div { color: blue; color: blue green; width: 10px; width: 10px 20px; border: 1px solid; border: 2px 3px solid; overflow: hidden; overflow: auto bogus; z-index: 3; z-index: 1.5; display: block; display: bogus }");
    assert_eq!(s.color, Color { r: 0, g: 0, b: 255, a: 255 });
    assert_eq!(s.width, Length::Px(10.0));
    assert_eq!(s.border_width.top, 1.0);
    assert_eq!(s.overflow_x, Overflow::Hidden);
    assert_eq!(s.z_index, Some(3));
    assert_eq!(s.display, Display::Block);
}

#[test]
fn background_shorthand_components_at_most_once() {
    let s = div("div { background: yellow; background: url(a.png) url(b.png); background: red pink; background: red, blue }");
    assert_eq!(s.background_color, YELLOW);
    assert!(s.background_layers.is_empty());
}

#[test]
fn important_with_whitespace_and_comment() {
    let (doc, t) = styled("<p id=x class=y>t</p>", "#x { color: red } .y { color: blue ! important } p { width: 5px !/**/important } #x { width: 1px }");
    let s = t.get(doc.find_element("p").unwrap()).unwrap();
    assert_eq!(s.color, Color { r: 0, g: 0, b: 255, a: 255 });
    assert_eq!(s.width, Length::Px(5.0));
}

#[test]
fn current_color_uses_the_final_color() {
    // `color` declared after `border`: the border is still the final color,
    // and an unset border color is currentcolor too.
    let s = div("div { color: maroon; border: solid; color: orange; }");
    assert_eq!(s.border_color.top, Color { r: 255, g: 165, b: 0, a: 255 });
    let s = div("div { color: orange; border-width: 1px; border-style: solid }");
    assert_eq!(s.border_color.left, Color { r: 255, g: 165, b: 0, a: 255 });
}

#[test]
fn malformed_rules_and_at_rules_recover() {
    let css = "@media screen { div { color: red } p } div { width: 7px } @bogus foo { div { width: 1px } } div { height: 3px } div { ; width: 8px } [class=second two] { height: 9px }";
    let s = div(css);
    assert_eq!(s.height, Length::Px(3.0));
    assert_eq!(s.width, Length::Px(8.0));
}
