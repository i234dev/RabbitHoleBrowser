//! Preferred intrinsic main sizes participate in `flex-basis:auto`.
use std::collections::HashMap;
use rabbithole_css::{parse_stylesheet, MediaEnv};
use rabbithole_dom::Document;
use rabbithole_gfx::FontContext;
use rabbithole_layout::{layout, BoxKind, LayoutBox, LayoutContext, LayoutTree};
use rabbithole_style::style_tree;

fn render(html: &str, css: &str) -> (Document, LayoutTree) {
    let doc = rabbithole_html::parse(html);
    let sheets = [parse_stylesheet(css)];
    let styles = style_tree(&doc, &sheets, &MediaEnv { width:800., height:600., screen:true, scripting:true });
    let fonts = FontContext::new();
    let images = HashMap::new(); let backgrounds = HashMap::new(); let scrolls = HashMap::new();
    let tree = layout(&LayoutContext {
        doc:&doc, styles:&styles, fonts:&fonts, images:&images, background_images:&backgrounds,
        scroll_offsets:&scrolls, viewport_width:800., viewport_height:600., page_scroll:(0.,0.),
    });
    (doc, tree)
}
fn by_id<'a>(doc: &Document, tree: &'a LayoutTree, id: &str) -> &'a LayoutBox {
    fn find(b: &LayoutBox, node: usize) -> Option<&LayoutBox> {
        if b.node == Some(node) && b.kind != BoxKind::Text { return Some(b); }
        b.children.iter().find_map(|child| find(child,node))
    }
    find(&tree.root,doc.element_by_id(id).unwrap()).unwrap()
}
fn close(actual:f32, expected:f32) { assert!((actual-expected).abs()<0.02, "{actual} != {expected}"); }

#[test]
fn row_auto_retrieves_intrinsic_width_but_content_and_explicit_bases_do_not() {
    for direction in ["row", "row-reverse"] {
        let (d,t) = render(r#"
          <article id=min>XXXX XX</article><article id=max>XXXX XX</article>
          <section><i id=auto-min style="width:min-content">XXXX XX</i></section>
          <section><i id=auto-max style="width:max-content">XXXX XX</i></section>
          <section><i id=content style="width:min-content;flex-basis:content">XXXX XX</i></section>
          <section><i id=content-definite style="width:10px;flex-basis:content">XXXX XX</i></section>
          <section><i id=explicit style="width:min-content;flex-basis:50px">XXXX XX</i></section>
          <section><i id=basis-min style="width:max-content;flex-basis:min-content">XXXX XX</i></section>
          <section><i id=basis-max style="width:min-content;flex-basis:max-content">XXXX XX</i></section>
        "#, &format!("*{{font:10px/1 monospace}}#min{{width:min-content}}#max{{width:max-content}}section{{display:flex;flex-direction:{direction};width:300px}}i{{display:block;flex:0 0 auto;min-width:0}}"));
        let min = by_id(&d,&t,"min").content_rect.w;
        let max = by_id(&d,&t,"max").content_rect.w;
        assert!(max > min+1.);
        for id in ["auto-min", "basis-min"] { close(by_id(&d,&t,id).content_rect.w,min); }
        for id in ["auto-max", "content", "content-definite", "basis-max"] { close(by_id(&d,&t,id).content_rect.w,max); }
        close(by_id(&d,&t,"explicit").content_rect.w,50.);
        close(by_id(&d,&t,"auto-min").content_rect.h,20.);
        close(by_id(&d,&t,"content").content_rect.h,10.);
    }
}

#[test]
fn fit_content_uses_available_main_space_after_margin_padding_and_border() {
    let (d,t) = render(r#"
      <article id=min>XXXXX XXXXX</article><article id=max>XXXXX XXXXX</article>
      <section style="width:50px"><i id=middle>XXXXX XXXXX</i></section>
      <section style="width:20px"><i id=small>XXXXX XXXXX</i></section>
      <section style="width:300px"><i id=large>XXXXX XXXXX</i></section>
      <section style="width:50px"><i id=explicit style="width:max-content;flex-basis:fit-content">XXXXX XXXXX</i></section>
      <section style="width:50px"><i id=content style="flex-basis:content">XXXXX XXXXX</i></section>
    "#, "*{font:10px/1 monospace}#min{width:min-content}#max{width:max-content}section{display:flex}i{display:block;flex:0 0 auto;min-width:0;width:fit-content;margin-left:3px;margin-right:5px;padding-left:2px;padding-right:4px;border-left:1px solid;border-right:3px solid}");
    let min=by_id(&d,&t,"min").content_rect.w; let max=by_id(&d,&t,"max").content_rect.w;
    assert!(min<32. && max>32.);
    for id in ["middle", "explicit"] { close(by_id(&d,&t,id).content_rect.w,32.); }
    close(by_id(&d,&t,"small").content_rect.w,min);
    for id in ["large", "content"] { close(by_id(&d,&t,id).content_rect.w,max); }
}

#[test]
fn numeric_percentage_and_content_bases_stay_distinct_on_both_axes() {
    for direction in ["row", "row-reverse", "column", "column-reverse"] {
        let (d,t) = render(r#"
          <section><i id=preferred style="width:60px;height:70px"><b></b></i></section>
          <section><i id=percent style="width:40%;height:35%"><b></b></i></section>
          <section><i id=basis style="width:60px;height:70px;flex-basis:25%"><b></b></i></section>
          <section><i id=content style="width:60px;height:70px;flex-basis:content"><b></b></i></section>
        "#, &format!("section{{display:flex;flex-direction:{direction};width:200px;height:200px}}i{{display:block;flex:0 0 auto;min-width:0;min-height:0}}b{{display:block;width:50px;height:30px}}"));
        let row=direction.starts_with("row");
        let main=|id| {let b=by_id(&d,&t,id); if row {b.content_rect.w} else {b.content_rect.h}};
        close(main("preferred"),if row {60.} else {70.});
        close(main("percent"),if row {80.} else {70.});
        close(main("basis"),50.);
        close(main("content"),if row {50.} else {30.});
    }
}

#[test]
fn column_auto_reads_height_keywords_and_unresolved_percent_basis_uses_content() {
    for direction in ["column", "column-reverse"] {
        let (d,t) = render(r#"
          <section><i id=min style="height:min-content"><b></b></i></section>
          <section><i id=max style="height:max-content"><b></b></i></section>
          <section><i id=fit style="height:fit-content"><b></b></i></section>
          <section><i id=explicit style="height:min-content;flex-basis:50px"><b></b></i></section>
          <section class=indefinite><i id=percent style="height:100px;flex-basis:50%"><b></b></i></section>
        "#, &format!("section{{display:flex;flex-direction:{direction};width:40px;height:200px}}section.indefinite{{height:auto}}i{{display:block;width:40px;flex:0 0 auto;min-height:0}}b{{display:block;width:10px;height:80px}}"));
        for id in ["min", "max", "fit", "percent"] { close(by_id(&d,&t,id).content_rect.h,80.); }
        close(by_id(&d,&t,"explicit").content_rect.h,50.);
    }
}

#[test]
fn intrinsic_base_is_flexed_and_clamped_without_replacing_the_automatic_minimum() {
    let (d,t) = render(r#"
      <article id=min>XXXX XX</article><article id=max>XXXX XX</article>
      <section><i id=automatic>XXXX XX</i></section>
      <section><i id=zero style="min-width:0">XXXX XX</i></section>
      <section><i id=hidden style="overflow:hidden">XXXX XX</i></section>
      <aside><i id=minimum style="min-width:50px">XXXX XX</i></aside>
      <aside><i id=maximum style="width:max-content;max-width:30px;min-width:0">XXXX XX</i></aside>
    "#, "*{font:10px/1 monospace}#min{width:min-content}#max{width:max-content}section,aside{display:flex;width:1px}aside{width:300px}i{display:block;width:min-content;flex:0 1 auto}");
    let min=by_id(&d,&t,"min").content_rect.w; assert!(min>1.);
    close(by_id(&d,&t,"automatic").content_rect.w,min);
    for id in ["zero", "hidden"] { close(by_id(&d,&t,id).content_rect.w,1.); }
    close(by_id(&d,&t,"minimum").content_rect.w,50.);
    close(by_id(&d,&t,"maximum").content_rect.w,30.);
}
