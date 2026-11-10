//! Image loading after the first layout: script-set `src`, lazy loading,
//! srcset density choice, broken images, video posters.

use rabbithole_engine::dom::{Document, NodeId};
use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Page, Viewport};

const VP: Viewport = Viewport { width: 800.0, height: 600.0 };

fn base() -> Url {
    let dir = format!("{}/tests/fixtures/images/", env!("CARGO_MANIFEST_DIR"));
    Url::parse(&format!("file://{dir}page.html")).unwrap()
}

fn by_id(page: &Page, id: &str) -> NodeId {
    page.doc.descendants(Document::ROOT).find(|&n| page.doc.attr(n, "id") == Some(id)).unwrap()
}

/// Tick until background image loads have landed.
fn settle(engine: &Engine, page: &mut Page) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let _ = engine.tick(page);
        if page.images_in_flight() == 0 || std::time::Instant::now() > deadline {
            let _ = engine.tick(page);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn script_set_src_loads_and_fires_load() {
    let mut engine = Engine::new_bundled_fonts_only();
    let html = r#"<body><img id=i alt="x"><script>
        var i = document.getElementById('i');
        i.onload = function () { document.title = 'loaded ' + i.naturalWidth + 'x' + i.naturalHeight + ' ' + i.complete; };
        i.src = 'red_20x10.png';
        var pre = new Image();
        pre.onload = function () { document.body.setAttribute('data-pre', pre.naturalWidth); };
        pre.src = 'blue_40x20.png';
    </script></body>"#;
    let mut page = engine.load_html(html, base(), VP);
    settle(&engine, &mut page);
    let i = by_id(&page, "i");
    let img = page.images.get(&i).expect("dynamic src loaded");
    assert_eq!((img.width, img.height), (20, 10));
    assert_eq!(page.title, "loaded 20x10 true");
    let body = page.doc.body().unwrap();
    assert_eq!(page.doc.attr(body, "data-pre"), Some("40"));
    assert_eq!(page.rect_of(i).map(|r| (r.w, r.h)), Some((20.0, 10.0)));
}

#[test]
fn lazy_images_load_when_scrolled_near() {
    let mut engine = Engine::new_bundled_fonts_only();
    let html = r#"<body style="margin:0"><div style="height:5000px"></div>
        <img id=l loading=lazy src="red_20x10.png" width=20 height=10></body>"#;
    let mut page = engine.load_html(html, base(), VP);
    settle(&engine, &mut page);
    let l = by_id(&page, "l");
    assert!(!page.images.contains_key(&l), "far lazy image must not load");
    // The box keeps its attribute size while waiting (no layout shift).
    assert_eq!(page.rect_of(l).map(|r| (r.w, r.h)), Some((20.0, 10.0)));
    engine.set_page_scroll(&mut page, 0.0, 4000.0);
    settle(&engine, &mut page);
    assert!(page.images.contains_key(&l), "lazy image loads near the viewport");
}

#[test]
fn srcset_density_follows_device_pixel_ratio() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.set_device_pixel_ratio(2.0);
    let html = r#"<img id=i srcset="red_20x10.png 1x, blue_40x20.png 2x">"#;
    let mut page = engine.load_html(html, base(), VP);
    settle(&engine, &mut page);
    let i = by_id(&page, "i");
    let img = page.images.get(&i).unwrap();
    // 2x candidate: 40×20 pixels shown at 20×10 CSS px.
    assert_eq!((img.width, img.natural_width), (40, 20));
    assert_eq!(page.rect_of(i).map(|r| (r.w, r.h)), Some((20.0, 10.0)));
}

#[test]
fn width_descriptors_and_resize_reselect() {
    let mut engine = Engine::new_bundled_fonts_only();
    let html = r#"<img id=i style="width:100%" srcset="red_20x10.png 20w, blue_40x20.png 40w" sizes="(max-width: 500px) 20px, 40px">"#;
    let mut page = engine.load_html(html, base(), VP);
    settle(&engine, &mut page);
    let i = by_id(&page, "i");
    assert_eq!(page.images.get(&i).unwrap().width, 40);
    engine.relayout(&mut page, Viewport { width: 400.0, height: 600.0 });
    settle(&engine, &mut page);
    assert_eq!(page.images.get(&i).unwrap().width, 20);
}

#[test]
fn broken_image_keeps_size_and_shows_alt() {
    let mut engine = Engine::new_bundled_fonts_only();
    let html = r#"<img id=a src="nope.png" alt="Alt text" width=120 height=40><img id=b src="nope2.png" alt="Inline">"#;
    let mut page = engine.load_html(html, base(), VP);
    settle(&engine, &mut page);
    let a = by_id(&page, "a");
    assert!(page.images.get(&a).map(|i| i.broken).unwrap_or(false));
    assert_eq!(page.rect_of(a).map(|r| (r.w, r.h)), Some((120.0, 40.0)));
    use rabbithole_engine::layout::DisplayItem as D;
    let texts: Vec<&str> = page.display_list.items.iter().filter_map(|it| if let D::Text { text, .. } = it { Some(text.as_str()) } else { None }).collect();
    assert!(texts.contains(&"Alt text"), "{texts:?}");
    assert!(texts.iter().any(|t| t.contains("Inline")), "{texts:?}");
}

#[test]
fn aspect_ratio_from_attributes_before_load() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.loader.load_images = false;
    let html = r#"<div style="width:300px"><img id=i src="x.png" width=400 height=200 style="width:100%;height:auto" alt=""></div>"#;
    let page = engine.load_html(html, base(), VP);
    let i = by_id(&page, "i");
    assert_eq!(page.rect_of(i).map(|r| (r.w, r.h)), Some((300.0, 150.0)));
}

#[test]
fn video_poster_and_picture_type() {
    let mut engine = Engine::new_bundled_fonts_only();
    let html = r#"<video id=v poster="blue_40x20.png" controls></video>
        <picture><source type="image/jxl" srcset="nope.jxl"><source type="image/png" srcset="blue_40x20.png"><img id=p src="red_20x10.png"></picture>"#;
    let mut page = engine.load_html(html, base(), VP);
    settle(&engine, &mut page);
    let v = by_id(&page, "v");
    assert_eq!(page.images.get(&v).map(|i| i.width), Some(40));
    assert_eq!(page.rect_of(v).map(|r| (r.w, r.h)), Some((40.0, 20.0)));
    let p = by_id(&page, "p");
    assert_eq!(page.images.get(&p).map(|i| i.width), Some(40));
}
