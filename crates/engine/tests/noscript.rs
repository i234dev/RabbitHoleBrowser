use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Viewport};

const HTML: &str = r#"<html><head><noscript><meta http-equiv="refresh" content="0;url=/nojs"></noscript></head>
<body><noscript id="n">no js</noscript><p>js</p></body></html>"#;

fn noscript_hidden(scripting: bool) -> (bool, bool) {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = scripting;
    engine.loader.load_scripts = scripting;
    let url = Url::parse("https://example.test/").unwrap();
    let mut page = engine.load_html(HTML, url, Viewport { width: 800.0, height: 600.0 });
    let n = page
        .doc
        .descendants(rabbithole_engine::dom::Document::ROOT)
        .find(|&n| page.doc.attr(n, "id") == Some("n"))
        .unwrap();
    let hidden = engine.computed_style_now(&mut page, n).map(|s| s.display.is_none()).unwrap_or(true);
    let nav = engine.take_page_navigation(&mut page).is_some();
    (hidden, nav)
}

#[test]
fn noscript_follows_scripting() {
    assert_eq!(noscript_hidden(true), (true, false));
    assert_eq!(noscript_hidden(false), (false, true));
}
