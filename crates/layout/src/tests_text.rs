//! CSS Text tests: justification, text-align-last, tab stops.

use crate::tree::{BoxKind, FragmentKind, LayoutBox, LayoutTree};
use crate::{layout, LayoutContext};
use rabbithole_css::{parse_stylesheet, MediaEnv};
use rabbithole_dom::Document;
use rabbithole_gfx::{FontContext, Rect};
use rabbithole_style::{style_tree, StyledTree};
use std::collections::HashMap;

struct Fx {
    doc: Document,
    styles: StyledTree,
    fonts: FontContext,
}

impl Fx {
    fn new(html: &str) -> Fx {
        let doc = rabbithole_html::parse(html);
        let env = MediaEnv { width: 800.0, height: 600.0, screen: true, scripting: true };
        let css = "html, body { margin: 0; padding: 0; } div { font: 20px/20px monospace; }";
        let styles = style_tree(&doc, &[parse_stylesheet(css)], &env);
        Fx { doc, styles, fonts: FontContext::new() }
    }

    fn layout(&self) -> LayoutTree {
        let images = HashMap::new();
        let bgs = HashMap::new();
        let scrolls = HashMap::new();
        let ctx = LayoutContext {
            doc: &self.doc,
            styles: &self.styles,
            fonts: &self.fonts,
            images: &images,
            background_images: &bgs,
            viewport_width: 800.0,
            viewport_height: 600.0,
            scroll_offsets: &scrolls,
            page_scroll: (0.0, 0.0),
        };
        layout(&ctx)
    }

    fn block<'t>(&self, tree: &'t LayoutTree, id: &str) -> &'t LayoutBox {
        let node = self.doc.element_by_id(id).unwrap_or_else(|| panic!("no #{id}"));
        fn find(b: &LayoutBox, n: usize) -> Option<&LayoutBox> {
            if b.node == Some(n) && b.kind != BoxKind::Text && b.pseudo.is_none() {
                return Some(b);
            }
            b.children.iter().find_map(|c| find(c, n))
        }
        find(&tree.root, node).unwrap_or_else(|| panic!("no box for #{id}"))
    }
}

/// (text, rect) of the text fragments of each line.
fn lines(b: &LayoutBox) -> Vec<Vec<(String, Rect)>> {
    b.lines
        .iter()
        .map(|l| {
            l.fragments
                .iter()
                .filter_map(|f| match &f.kind {
                    FragmentKind::Text { text, .. } => Some((text.to_string(), f.rect)),
                    _ => None,
                })
                .collect()
        })
        .collect()
}

#[test]
fn justify_expands_spaces_but_not_the_last_line() {
    let fx = Fx::new("<div id=d style='width:200px;text-align:justify'>aa bb cc dd ee ff gg</div>");
    let t = fx.layout();
    let ls = lines(fx.block(&t, "d"));
    assert!(ls.len() >= 2);
    // The first line reaches the right edge.
    let last = ls[0].last().unwrap().1;
    assert!((last.right() - 200.0).abs() < 0.5, "{:?}", ls[0]);
    // The last line is start-aligned (not stretched).
    let end = ls.last().unwrap().last().unwrap().1.right();
    assert!(end < 150.0, "{:?}", ls.last());
}

#[test]
fn text_align_last_applies_to_the_last_line() {
    let fx = Fx::new("<div id=d style='width:200px;text-align-last:right'>ab</div>");
    let t = fx.layout();
    let ls = lines(fx.block(&t, "d"));
    assert!((ls[0].last().unwrap().1.right() - 200.0).abs() < 0.5, "{:?}", ls);
}

#[test]
fn justify_all_stretches_the_last_line() {
    let fx = Fx::new("<div id=d style='width:200px;text-align:justify-all'>ab cd</div>");
    let t = fx.layout();
    let ls = lines(fx.block(&t, "d"));
    assert!((ls[0].last().unwrap().1.right() - 200.0).abs() < 0.5, "{:?}", ls);
    assert!(ls[0][0].1.x.abs() < 0.5);
}

#[test]
fn text_justify_none_disables_justification() {
    let fx = Fx::new("<div id=d style='width:200px;text-align:justify-all;text-justify:none'>ab cd</div>");
    let t = fx.layout();
    let ls = lines(fx.block(&t, "d"));
    assert!(ls[0].last().unwrap().1.right() < 150.0, "{:?}", ls);
}

#[test]
fn tabs_advance_to_tab_stops() {
    // Monospace 20px: a space is 12px wide (0.6em), tab-size 4 = 48px.
    let fx = Fx::new("<div id=d style='white-space:pre;tab-size:4'>a\tb\n\tc</div>");
    let t = fx.layout();
    let b = fx.block(&t, "d");
    let ls = lines(b);
    let x_of = |l: &Vec<(String, Rect)>, s: &str| l.iter().find(|f| f.0 == s).map(|f| f.1.x).unwrap();
    let stop = x_of(&ls[0], "b");
    assert!((stop - x_of(&ls[1], "c")).abs() < 0.5, "{:?}", ls);
    assert!(stop > 30.0 && stop < 60.0, "{:?}", ls);
}

#[test]
fn tab_size_length_and_half_ch_threshold() {
    // Stops every 10px; a stop closer than 0.5ch (6px) is skipped.
    let fx = Fx::new("<div id=d style='white-space:pre;tab-size:10px'>\tb</div>");
    let t = fx.layout();
    let ls = lines(fx.block(&t, "d"));
    let b = ls[0].iter().find(|f| f.0 == "b").unwrap().1.x;
    assert!((b - 10.0).abs() < 0.5, "{:?}", ls);
    let fx = Fx::new("<div id=d style='white-space:pre;tab-size:4px'>\tb</div>");
    let t = fx.layout();
    let ls = lines(fx.block(&t, "d"));
    let b = ls[0].iter().find(|f| f.0 == "b").unwrap().1.x;
    assert!((b - 8.0).abs() < 0.5, "{:?}", ls);
}

#[test]
fn soft_hyphen_shows_a_hyphen_at_the_break() {
    // 20px monospace: 12px per character; 6 characters fit.
    let fx = Fx::new("<div id=d style='width:72px'>abc\u{ad}defgh</div>");
    let t = fx.layout();
    let ls = lines(fx.block(&t, "d"));
    assert_eq!(ls.len(), 2, "{ls:?}");
    let first: String = ls[0].iter().map(|f| f.0.as_str()).collect();
    assert!(first == "abc\u{2010}" || first == "abc-", "{ls:?}");
    let fx = Fx::new("<div id=d style='width:72px;hyphens:none'>abc\u{ad}defgh</div>");
    let t = fx.layout();
    assert_eq!(lines(fx.block(&t, "d")).len(), 1);
}

#[test]
fn no_break_before_closing_punctuation_after_a_space() {
    // "aaaa !" does not fit in 5 characters but may not break before "!".
    let fx = Fx::new("<div id=d style='width:60px'>aaaa !</div>");
    let t = fx.layout();
    assert_eq!(lines(fx.block(&t, "d")).len(), 1);
    let fx = Fx::new("<div id=d style='width:60px'>aaaa bb</div>");
    let t = fx.layout();
    assert_eq!(lines(fx.block(&t, "d")).len(), 2);
}

#[test]
fn balanced_groups_between_forced_breaks() {
    let fx = Fx::new("<div id=d style='width:300px;text-wrap:balance'>aa bb cc dd ee ff gg hh ii jj kk ll mm<br>x</div>");
    let t = fx.layout();
    let ls = lines(fx.block(&t, "d"));
    // The first group is balanced (both lines about as wide), the second
    // keeps its single line.
    let w = |l: &Vec<(String, Rect)>| l.last().unwrap().1.right() - l[0].1.x;
    assert_eq!(ls.len(), 3, "{ls:?}");
    assert!((w(&ls[0]) - w(&ls[1])).abs() <= 40.0, "{ls:?}");
}

#[test]
fn letter_spacing_is_not_applied_at_the_line_end() {
    let fx = Fx::new("<div id=d style='letter-spacing:10px;width:300px;text-align:right'>ab</div>");
    let t = fx.layout();
    let ls = lines(fx.block(&t, "d"));
    assert!((ls[0].last().unwrap().1.right() - 300.0).abs() < 0.5, "{ls:?}");
}

#[test]
fn justified_lines_keep_whole_fragments_with_word_spacing() {
    let fx = Fx::new("<div id=d style='width:200px;text-align:justify'>aa bb cc dd ee ff gg</div>");
    let t = fx.layout();
    let b = fx.block(&t, "d");
    let first = &b.lines[0];
    let texts: Vec<_> = first.fragments.iter().filter_map(|f| match &f.kind {
        FragmentKind::Text { word_spacing, .. } => Some((*word_spacing, f.rect)),
        _ => None,
    }).collect();
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(texts[0].0 > 0.0);
    assert!((texts[0].1.right() - 200.0).abs() < 0.5, "{texts:?}");
}
