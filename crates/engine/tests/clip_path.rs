//! `clip-path`: basic shapes, reference boxes and `url()` clip paths clip
//! painting and hit testing; animations interpolate shapes.

use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Page, Viewport};

fn engine(scripting: bool) -> Engine {
    let mut e = Engine::new_bundled_fonts_only();
    e.scripting = scripting;
    e.loader.load_scripts = scripting;
    e
}

fn load(engine: &mut Engine, html: &str) -> Page {
    let url = Url::parse("https://example.test/").unwrap();
    engine.load_html(html, url, Viewport { width: 400.0, height: 300.0 })
}

/// RGB of the frame pixel at (x, y).
fn rgb(e: &Engine, page: &Page, x: u32, y: u32) -> (u8, u8, u8) {
    let f = e.snapshot(page);
    let i = ((y * f.width + x) * 4) as usize;
    (f.rgba[i], f.rgba[i + 1], f.rgba[i + 2])
}

const GREEN: (u8, u8, u8) = (0, 128, 0);
const WHITE: (u8, u8, u8) = (255, 255, 255);

fn page(e: &mut Engine, clip: &str) -> Page {
    load(e, &format!("<!doctype html><style>body {{ margin: 0 }} #t {{ width: 200px; height: 100px; padding: 10px; border: 10px solid green; background: green; clip-path: {clip} }}</style><div id=t></div>"))
}

#[test]
fn circle_clips_painting_and_hit_testing() {
    let mut e = engine(true);
    // Border box 240x140, center (120, 70), radius 40.
    let mut p = page(&mut e, "circle(40px)");
    assert_eq!(rgb(&e, &p, 120, 70), GREEN);
    assert_eq!(rgb(&e, &p, 120, 25), WHITE);
    assert_eq!(rgb(&e, &p, 5, 5), WHITE);
    assert_eq!(e.eval_in_page(&mut p, "document.elementFromPoint(120, 70).id").trim_matches(|c| c == '"' || c == '\''), "t");
    assert_eq!(e.eval_in_page(&mut p, "document.elementFromPoint(5, 5).tagName").trim_matches(|c| c == '"' || c == '\''), "BODY");
}

#[test]
fn inset_with_reference_box() {
    let mut e = engine(false);
    // content-box: (20, 20) 200x100; inset(10px) leaves (30, 30)-(210, 110).
    let p = page(&mut e, "inset(10px) content-box");
    assert_eq!(rgb(&e, &p, 35, 35), GREEN);
    assert_eq!(rgb(&e, &p, 25, 35), WHITE);
    assert_eq!(rgb(&e, &p, 205, 105), GREEN);
    assert_eq!(rgb(&e, &p, 215, 105), WHITE);
}

#[test]
fn polygon_evenodd_leaves_a_hole() {
    let mut e = engine(false);
    let p = page(
        &mut e,
        "polygon(evenodd, 0 0, 240px 0, 240px 140px, 0 140px, 0 20px, 200px 20px, 200px 120px, 40px 120px, 40px 20px, 0 20px)",
    );
    assert_eq!(rgb(&e, &p, 10, 10), GREEN);
    assert_eq!(rgb(&e, &p, 120, 70), WHITE);
}

#[test]
fn geometry_box_alone_clips_to_the_box() {
    let mut e = engine(false);
    let p = page(&mut e, "padding-box");
    assert_eq!(rgb(&e, &p, 5, 5), WHITE);
    assert_eq!(rgb(&e, &p, 15, 15), GREEN);
}

#[test]
fn url_references_an_svg_clip_path() {
    let mut e = engine(false);
    let p = load(
        &mut e,
        r#"<!doctype html><style>body { margin: 0 } #t { width: 100px; height: 100px; background: green; clip-path: url(#c) }</style>
        <div id=t></div><svg width=0 height=0><clipPath id=c clipPathUnits=objectBoundingBox><rect x=0.5 width=0.5 height=1 /></clipPath></svg>"#,
    );
    assert_eq!(rgb(&e, &p, 25, 50), WHITE);
    assert_eq!(rgb(&e, &p, 75, 50), GREEN);
}

#[test]
fn external_url_clips_nothing() {
    let mut e = engine(false);
    let p = page(&mut e, "url(other.svg#c)");
    assert_eq!(rgb(&e, &p, 5, 5), GREEN);
}

#[test]
fn clip_path_animation_interpolates_shapes() {
    let mut e = engine(true);
    let mut p = load(
        &mut e,
        r#"<!doctype html><style>body { margin: 0 }
        @keyframes k { from { clip-path: inset(0 0 0 0) } to { clip-path: inset(0 0 0 100px) } }
        #t { width: 100px; height: 100px; background: green; animation: k 1000s -500s linear }</style><div id=t></div>"#,
    );
    e.settle_for_snapshot(&mut p, std::time::Duration::from_millis(50));
    // Halfway: inset(0 0 0 50px).
    assert_eq!(rgb(&e, &p, 25, 50), WHITE);
    assert_eq!(rgb(&e, &p, 75, 50), GREEN);
}

#[test]
fn paused_animations_hold_their_time() {
    let mut e = engine(true);
    let mut p = load(
        &mut e,
        r#"<!doctype html><style>body { margin: 0 }
        @keyframes k { from { clip-path: inset(0 0 0 0) } to { clip-path: inset(0 0 0 100px) } }
        #t { width: 100px; height: 100px; background: green; animation: k 1s -0.5s linear paused }</style><div id=t></div>"#,
    );
    std::thread::sleep(std::time::Duration::from_millis(300));
    e.settle_for_snapshot(&mut p, std::time::Duration::from_millis(50));
    assert_eq!(rgb(&e, &p, 45, 50), WHITE);
    assert_eq!(rgb(&e, &p, 55, 50), GREEN);
}

#[test]
fn legacy_clip_applies_to_absolutely_positioned_boxes() {
    let mut e = engine(false);
    let p = load(
        &mut e,
        r#"<!doctype html><style>body { margin: 0 } div { width: 100px; height: 100px; background: green; clip: rect(10px, auto, auto, 50px) }
        #a { position: absolute; top: 0; left: 0 } #b { margin-top: 150px }</style><div id=a></div><div id=b></div>"#,
    );
    assert_eq!(rgb(&e, &p, 25, 50), WHITE);
    assert_eq!(rgb(&e, &p, 75, 5), WHITE);
    assert_eq!(rgb(&e, &p, 75, 50), GREEN);
    // Not absolutely positioned: not clipped.
    assert_eq!(rgb(&e, &p, 25, 200), GREEN);
}

#[test]
fn corner_shape_and_border_shape_clip_the_background() {
    let mut e = engine(false);
    let p = load(
        &mut e,
        r#"<!doctype html><style>body { margin: 0 } div { width: 100px; height: 100px; background: green }
        #a { border-radius: 50px; corner-shape: bevel } #b { border-shape: inset(20px) }</style><div id=a></div><div id=b></div>"#,
    );
    // Bevelled corners: a diamond.
    assert_eq!(rgb(&e, &p, 50, 50), GREEN);
    assert_eq!(rgb(&e, &p, 10, 10), WHITE);
    assert_eq!(rgb(&e, &p, 90, 90), WHITE);
    assert_eq!(rgb(&e, &p, 50, 150), GREEN);
    assert_eq!(rgb(&e, &p, 10, 110), WHITE);
}
