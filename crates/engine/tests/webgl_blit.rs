//! Page presentation must use the default framebuffer after real offscreen blits.
use rabbithole_engine::{net::Url, Engine, Viewport};

#[test]
fn framebuffer_blits_reach_page_pixels_and_leave_offscreen_canvases_untouched() {
    let mut engine=Engine::new_bundled_fonts_only();engine.loader.load_images=false;
    let mut page=engine.load_html(include_str!("../../../tests/pages/webgl-blit.html"),Url::parse("https://example.test/").unwrap(),Viewport{width:320.,height:240.});
    engine.restyle(&mut page);
    let errors:Vec<_>=page.take_console().into_iter().filter(|(level,_)|level=="error").collect();assert!(errors.is_empty(),"{errors:?}");
    let scene=page.images.get(&page.doc.element_by_id("scene").unwrap()).unwrap().pixels();
    let at=|x:usize,y:usize|&scene[(y*4+x)*4..(y*4+x)*4+4];
    assert_eq!(at(0,0),[0,0,255,255]);assert_eq!(at(1,0),[255,255,255,255]);assert_eq!(at(3,0),[0,0,255,255]);
    assert_eq!(at(1,3),[0,255,0,255]);assert_eq!(at(3,3),[255,0,0,255]);
    let untouched=page.images.get(&page.doc.element_by_id("untouched").unwrap()).unwrap().pixels();assert!(untouched.chunks_exact(4).all(|p|p==[0,0,255,255]));
}
