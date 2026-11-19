//! CSS animations of elements styled when the page loads apply from the
//! first frame, below `!important` declarations.

use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Page, Viewport};

fn load(engine: &mut Engine, html: &str) -> Page {
    let url = Url::parse("https://example.test/").unwrap();
    let mut page = engine.load_html(html, url, Viewport { width: 400.0, height: 300.0 });
    engine.settle_for_snapshot(&mut page, std::time::Duration::from_millis(50));
    page
}

fn engine() -> Engine {
    let mut e = Engine::new_bundled_fonts_only();
    e.scripting = true;
    e
}

/// RGB of the frame pixel at (x, y).
fn rgb(e: &Engine, page: &Page, x: u32, y: u32) -> (u8, u8, u8) {
    let f = e.snapshot(page);
    let i = ((y * f.width + x) * 4) as usize;
    (f.rgba[i], f.rgba[i + 1], f.rgba[i + 2])
}

const STYLE: &str = "body { margin: 0 } @keyframes k { from { opacity: 0 } to { opacity: 0 } } #t { width: 100px; height: 100px; background: green; animation: k 1000s -500s linear }";

/// The overlay used to need a later restyle to get a base style.
#[test]
fn animations_present_at_load_apply() {
    let mut e = engine();
    let p = load(&mut e, &format!("<!doctype html><style>{STYLE}</style><div id=t></div>"));
    assert_eq!(rgb(&e, &p, 50, 50), (255, 255, 255));
}

#[test]
fn important_declarations_override_animations() {
    let mut e = engine();
    let p = load(&mut e, &format!("<!doctype html><style>{STYLE} #t {{ opacity: 1 !important }}</style><div id=t></div>"));
    assert_eq!(rgb(&e, &p, 50, 50), (0, 128, 0));
}
