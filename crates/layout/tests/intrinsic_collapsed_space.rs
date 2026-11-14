//! Collapsible spaces at min-content line breaks are not leading content.
use std::collections::HashMap;

use rabbithole_css::{parse_stylesheet, MediaEnv};
use rabbithole_dom::Document;
use rabbithole_gfx::FontContext;
use rabbithole_layout::{layout, BoxKind, LayoutBox, LayoutContext, LayoutTree};
use rabbithole_style::style_tree;

fn render(html: &str, extra_css: &str) -> (Document, LayoutTree) {
    let doc = rabbithole_html::parse(html);
    let css = format!("*{{font:20px/1 monospace}}div{{width:min-content}}.max{{width:max-content}}{extra_css}");
    let styles = style_tree(&doc, &[parse_stylesheet(&css)], &MediaEnv {
        width: 800., height: 600., screen: true, scripting: true,
    });
    let fonts = FontContext::new();
    let images = HashMap::new();
    let backgrounds = HashMap::new();
    let scrolls = HashMap::new();
    let tree = layout(&LayoutContext {
        doc: &doc, styles: &styles, fonts: &fonts, images: &images,
        background_images: &backgrounds, scroll_offsets: &scrolls,
        viewport_width: 800., viewport_height: 600., page_scroll: (0., 0.),
    });
    (doc, tree)
}

fn width(doc: &Document, tree: &LayoutTree, id: &str) -> f32 {
    fn find(b: &LayoutBox, node: usize) -> Option<&LayoutBox> {
        if b.node == Some(node) && b.kind != BoxKind::Text { return Some(b); }
        b.children.iter().find_map(|child| find(child, node))
    }
    find(&tree.root, doc.element_by_id(id).unwrap()).unwrap().content_rect.w
}

fn close(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 0.02, "{actual} != {expected}");
}

#[test]
fn collapsed_space_breaks_do_not_widen_either_word() {
    let (d, t) = render(r#"
      <div id=word>XXXXX</div>
      <div id=normal>XXXXX XXXXX</div>
      <div id=multiple>XXXXX     XXXXX</div>
      <div id=segment>XXXXX
XXXXX</div>
      <div id=inline>XXXXX <span>XXXXX</span></div>
      <div id=edges>  XXXXX XXXXX  </div>
      <div id=preline style="white-space:pre-line">XXXXX XXXXX</div>
    "#, "");
    let word = width(&d, &t, "word");
    assert!(word > 1.);
    for id in ["normal", "multiple", "segment", "inline", "edges", "preline"] {
        close(width(&d, &t, id), word);
    }
}

#[test]
fn max_content_and_nonwrapping_spaces_keep_the_separator() {
    let (d, t) = render(r#"
      <div id=word>XXXXX</div>
      <div id=max class=max>XXXXX XXXXX</div>
      <div id=nowrap style="white-space:nowrap">XXXXX XXXXX</div>
      <div id=nowrap-max class=max style="white-space:nowrap">XXXXX XXXXX</div>
      <div id=multiple class=max>XXXXX    XXXXX</div>
    "#, "");
    let max = width(&d, &t, "max");
    assert!(max > 2. * width(&d, &t, "word") + 1.);
    for id in ["nowrap", "nowrap-max", "multiple"] { close(width(&d, &t, id), max); }
}

#[test]
fn the_space_owner_controls_wrapping_across_inline_boundaries() {
    let (d, t) = render(r#"
      <div id=word>XXXXX</div>
      <div id=max class=max>XXXXX XXXXX</div>
      <div id=break>XXXXX <span style="white-space:nowrap">XXXXX</span></div>
      <div id=no-break>XXXXX<span style="white-space:nowrap"> </span>XXXXX</div>
    "#, "");
    close(width(&d, &t, "break"), width(&d, &t, "word"));
    close(width(&d, &t, "no-break"), width(&d, &t, "max"));
}

#[test]
fn nbsp_and_preserved_nonwrapping_spaces_remain_content() {
    let (d, t) = render(r#"
      <div id=normal-max class=max>XXXXX XXXXX</div>
      <div id=nbsp>XXXXX&nbsp;XXXXX</div>
      <div id=nbsp-max class=max>XXXXX&nbsp;XXXXX</div>
      <div id=pre style="white-space:pre">XXXXX  X</div>
      <div id=pre-max class=max style="white-space:pre">XXXXX  X</div>
      <div id=collapsed-max class=max>XXXXX  X</div>
    "#, "");
    close(width(&d, &t, "nbsp"), width(&d, &t, "nbsp-max"));
    close(width(&d, &t, "nbsp"), width(&d, &t, "normal-max"));
    close(width(&d, &t, "pre"), width(&d, &t, "pre-max"));
    assert!(width(&d, &t, "pre-max") > width(&d, &t, "collapsed-max") + 1.);
}

#[test]
fn pre_wrap_and_break_spaces_preserve_max_content_spaces() {
    // The longer last word determines min-content independently of the
    // existing trailing-preserved-space hanging implementation.
    let (d, t) = render(r#"
      <div id=word>XXXXX</div>
      <div id=pre-max class=max style="white-space:pre">X  XXXXX</div>
      <div id=prewrap style="white-space:pre-wrap">X  XXXXX</div>
      <div id=prewrap-max class=max style="white-space:pre-wrap">X  XXXXX</div>
      <div id=breakspaces style="white-space:break-spaces">X  XXXXX</div>
      <div id=breakspaces-max class=max style="white-space:break-spaces">X  XXXXX</div>
    "#, "");
    for id in ["prewrap", "breakspaces"] { close(width(&d, &t, id), width(&d, &t, "word")); }
    for id in ["prewrap-max", "breakspaces-max"] { close(width(&d, &t, id), width(&d, &t, "pre-max")); }
}

#[test]
fn forced_and_zero_width_breaks_do_not_carry_a_collapsed_space() {
    let (d, t) = render(r#"
      <div id=word>XXXXX</div>
      <div id=br>XXXXX <br>XXXXX</div>
      <div id=newline style="white-space:pre-line">XXXXX&#32;
XXXXX</div>
      <div id=zwsp>XXXXX&#x200b;XXXXX</div>
      <div id=wbr>XXXXX<wbr>XXXXX</div>
      <div id=br-max class=max>XXXXX <br>XXXXX</div>
      <div id=zwsp-max class=max>XXXXX&#x200b;XXXXX</div>
    "#, "");
    let word = width(&d, &t, "word");
    for id in ["br", "newline", "zwsp", "wbr", "br-max"] { close(width(&d, &t, id), word); }
    close(width(&d, &t, "zwsp-max"), word * 2.);
}

#[test]
fn a_later_hyphen_break_is_not_suppressed_by_the_previous_space() {
    let (d, t) = render(r#"
      <div id=word class=max>XXXXX-</div>
      <div id=hyphen>X XXXXX-XXXXX</div>
      <div id=nowrap style="white-space:nowrap">XXXXX-XXXXX</div>
      <div id=max class=max>XXXXX-XXXXX</div>
    "#, "");
    close(width(&d, &t, "hyphen"), width(&d, &t, "word"));
    close(width(&d, &t, "nowrap"), width(&d, &t, "max"));
}

#[test]
fn opening_inline_edges_after_the_space_stay_with_the_next_word() {
    let (d, t) = render(r#"
      <div id=ref class=max>X<br><span>XXXXX</span></div>
      <div id=target>X <span>XXXXX</span></div>
    "#, "span{margin-left:5px;padding-left:2px;border-left:1px solid;margin-right:7px;padding-right:4px;border-right:3px solid}");
    close(width(&d, &t, "target"), width(&d, &t, "ref"));
}

#[test]
fn closing_inline_edges_before_the_space_stay_with_the_previous_word() {
    let (d, t) = render(r#"
      <div id=ref class=max><span>XXXXX</span><br>X</div>
      <div id=inside><span>XXXXX </span>X</div>
      <div id=outside><span>XXXXX</span> X</div>
      <div id=trailing><span>XXXXX </span></div>
      <div id=nested><span>XXXXX <em></em></span>X</div>
      <div id=negative-ref class=max><span class=negative>XXXXX</span><br>X</div>
      <div id=negative><span class=negative>XXXXX </span>X</div>
    "#, "span{margin-left:5px;padding-left:2px;border-left:1px solid;margin-right:7px;padding-right:4px;border-right:3px solid}span.negative{margin-right:-12px}em{padding-left:1px;padding-right:2px}");
    for id in ["inside", "outside", "trailing", "nested"] {
        close(width(&d, &t, id), width(&d, &t, "ref"));
    }
    close(width(&d, &t, "negative"), width(&d, &t, "negative-ref"));
}

#[test]
fn atomic_consumption_does_not_leave_the_space_break_for_a_later_word() {
    let (d, t) = render(r#"
      <div id=word>XXXXXXXXXX</div>
      <div id=target>X <i></i>XXXXXXXXXX</div>
    "#, "i{display:inline-block;width:10px;height:10px}");
    close(width(&d, &t, "target"), width(&d, &t, "word"));
}
