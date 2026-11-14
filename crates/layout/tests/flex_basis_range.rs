use std::collections::HashMap;
use rabbithole_css::{parse_stylesheet, MediaEnv};
use rabbithole_dom::Document;
use rabbithole_gfx::FontContext;
use rabbithole_layout::{layout, BoxKind, LayoutBox, LayoutContext, LayoutTree};
use rabbithole_style::style_tree;

fn fixture() -> (Document, LayoutTree) {
    let doc = rabbithole_html::parse(r#"
      <section><div id=row class=default></div><div class=rest></div></section>
      <section><div id=prior class=prior></div><div class=rest></div></section>
      <section class=column><div id=column class=default></div><div class=rest></div></section>
      <section class=column><div id=column-prior class=prior></div><div class=rest></div></section>
      <section><div id=math class=math></div><div class=rest></div></section>
      <section><div id=var-math class=var-math></div><div class=rest></div></section>
    "#);
    let sheets = [parse_stylesheet(r#"
      section{display:flex;width:100px;height:100px}
      section>div{width:30px;height:30px}.rest{width:70px;height:70px}
      .column{flex-direction:column}
      .default{flex-basis:-50px}
      .prior{flex-basis:20px;flex-basis:-50px}
      .math{flex-basis:20px;flex-basis:calc(-50px)}
      .var-math{--basis:-50px;flex-basis:20px;flex-basis:calc(var(--basis))}
    "#)];
    let styles = style_tree(&doc, &sheets, &MediaEnv{width:800.,height:600.,screen:true,scripting:true});
    let fonts = FontContext::new();
    let images = HashMap::new(); let backgrounds = HashMap::new(); let scrolls = HashMap::new();
    let tree = layout(&LayoutContext {doc:&doc,styles:&styles,fonts:&fonts,images:&images,background_images:&backgrounds,scroll_offsets:&scrolls,viewport_width:800.,viewport_height:600.,page_scroll:(0.,0.)});
    (doc, tree)
}
fn by_id<'a>(doc:&Document, tree:&'a LayoutTree, id:&str)->&'a LayoutBox {
    fn find(b:&LayoutBox,node:usize)->Option<&LayoutBox>{
        if b.node==Some(node)&&b.kind!=BoxKind::Text{return Some(b);}
        b.children.iter().find_map(|child|find(child,node))
    }
    find(&tree.root,doc.element_by_id(id).unwrap()).unwrap()
}

#[test]
fn invalid_negative_basis_falls_back_to_preferred_main_size_on_both_axes() {
    let(doc,tree)=fixture();
    assert_eq!(by_id(&doc,&tree,"row").content_rect.w,30.);
    assert_eq!(by_id(&doc,&tree,"column").content_rect.h,30.);
}

#[test]
fn a_prior_valid_basis_survives_a_negative_literal_on_both_axes() {
    let(doc,tree)=fixture();
    assert_eq!(by_id(&doc,&tree,"prior").content_rect.w,20.);
    assert_eq!(by_id(&doc,&tree,"column-prior").content_rect.h,20.);
}

#[test]
fn negative_math_basis_is_valid_then_clamped_instead_of_falling_back() {
    let(doc,tree)=fixture();
    assert_eq!(by_id(&doc,&tree,"math").content_rect.w,0.);
    assert_eq!(by_id(&doc,&tree,"var-math").content_rect.w,0.);
}
