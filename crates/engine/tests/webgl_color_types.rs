use rabbithole_engine::{net::Url, Engine, Viewport};

#[test]
fn boolean_vector_masks_and_constant_helpers_are_visible_on_both_canvases() {
    let mut engine=Engine::new_bundled_fonts_only();engine.loader.load_images=false;
    let mut page=engine.load_html(include_str!("../../../tests/pages/webgl-color-types.html"),Url::parse("https://example.test/").unwrap(),Viewport{width:16.,height:12.});
    engine.restyle(&mut page);
    let errors:Vec<_>=page.take_console().into_iter().filter(|(level,_)|level=="error").collect();assert!(errors.is_empty(),"{errors:?}");
    let frame=engine.snapshot(&page);
    for id in ["gl1","gl2"] {
        let node=page.doc.element_by_id(id).unwrap();let image=page.images.get(&node).unwrap();
        assert!(image.pixels().chunks_exact(4).all(|p|p==[128,96,255,255]),"{id}");
        let rect=page.rect_of(node).unwrap();let x=(rect.x+rect.w*0.5)as usize;let y=(rect.y+rect.h*0.5)as usize;
        let offset=(y*frame.width as usize+x)*4;assert_eq!(&frame.rgba[offset..offset+4],[128,96,255,255],"{id}");
    }
}
