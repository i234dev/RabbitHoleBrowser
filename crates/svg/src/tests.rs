//! Parsing and rendering tests.

use crate::SvgTree;
use rabbithole_gfx::{Canvas, Color, FontContext, Rect};

const ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><path d="M12 2L2 7l10 5 10-5-10-5zm0 13L2 10v7l10 5 10-5v-7l-10 5z" fill="#4285f4"/></svg>"##;

const LOGO: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd">
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 200 100">
  <defs>
    <linearGradient id="g" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0%" stop-color="red"/>
      <stop offset="100%" stop-color="blue"/>
    </linearGradient>
    <radialGradient id="r" cx="50%" cy="50%" r="50%">
      <stop offset="0" stop-color="#fff"/>
      <stop offset="1" stop-color="#000" stop-opacity="0.5"/>
    </radialGradient>
    <clipPath id="c"><circle cx="50" cy="50" r="30"/></clipPath>
  </defs>
  <rect x="10" y="10" width="80" height="80" fill="url(#g)" transform="rotate(10 50 50)"/>
  <g transform="translate(100,0)" opacity="0.8">
    <rect width="100" height="100" fill="url(#r)" clip-path="url(#c)"/>
  </g>
  <text x="100" y="95" text-anchor="middle" font-size="12" font-family="sans-serif" fill="#333">Logo</text>
</svg>"##;

const CHART: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="300" height="200">
  <style>
    .axis { stroke: #888; stroke-width: 1; }
    .series { fill: none; stroke: #e91e63; stroke-width: 2; stroke-linejoin: round; }
    text { font: 10px sans-serif; fill: #444; }
  </style>
  <line class="axis" x1="30" y1="170" x2="290" y2="170"/>
  <line class="axis" x1="30" y1="10" x2="30" y2="170"/>
  <polyline class="series" points="30,150 80,120 130,140 180,60 230,80 280,30"/>
  <polygon points="40,160 60,160 50,140" fill="#2196f3" stroke="black" stroke-dasharray="2 1"/>
  <text x="150" y="190" text-anchor="middle">Time</text>
  <ellipse cx="200" cy="100" rx="20" ry="10" fill="none" stroke="green"/>
</svg>"##;

const SPRITE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 100 100">
  <symbol id="dot" viewBox="0 0 10 10"><circle cx="5" cy="5" r="5"/></symbol>
  <use xlink:href="#dot" x="10" y="10" width="30" height="30" fill="green"/>
  <use href="#dot" x="60" y="60" width="30" height="30" fill="red"/>
  <g id="grp"><rect x="0" y="0" width="10" height="10" fill="orange"/></g>
  <use href="#grp" x="45" y="45"/>
  <use href="#nope"/>
</svg>"##;

fn fonts() -> FontContext {
    FontContext::new()
}

#[test]
fn parses_icon() {
    let t = SvgTree::parse(ICON).expect("icon parses");
    let (w, h, vb) = t.intrinsic_size();
    assert_eq!((w, h), (Some(24.0), Some(24.0)));
    let vb = vb.unwrap();
    assert_eq!((vb.x, vb.y, vb.w, vb.h), (0.0, 0.0, 24.0, 24.0));
    assert_eq!(t.resolve_size(None, None), (24.0, 24.0));
    assert_eq!(t.resolve_size(Some(48.0), None), (48.0, 48.0));
    assert!(t.node_count() >= 2);
}

#[test]
fn parses_logo_chart_sprite() {
    let logo = SvgTree::parse(LOGO).expect("logo parses");
    assert_eq!(logo.intrinsic_size().0, None);
    assert_eq!(logo.resolve_size(None, None), (300.0, 150.0));
    assert_eq!(logo.resolve_size(None, Some(50.0)), (100.0, 50.0));
    assert!(logo.node_count() >= 8, "{}", logo.node_count());
    let chart = SvgTree::parse(CHART).expect("chart parses");
    assert_eq!(chart.intrinsic_size(), (Some(300.0), Some(200.0), None));
    let sprite = SvgTree::parse(SPRITE).expect("sprite parses");
    assert!(sprite.node_count() >= 6);
}

#[test]
fn garbage_does_not_panic() {
    assert!(SvgTree::parse("hello").is_none());
    assert!(SvgTree::parse("<svg").is_some() || SvgTree::parse("<svg").is_none());
    let truncated = &LOGO[..LOGO.len() / 2];
    let _ = SvgTree::parse(truncated);
    let _ = SvgTree::parse("<svg viewBox='a b c d' width='-5'><path d='M 1e999 Q'/><circle r='x'/><use href='#self' id='self'/><rect width='1e40' height='1e40'/></svg>");
    let t = SvgTree::parse("<svg><g id='a'><use href='#b'/></g><g id='b'><use href='#a'/></g></svg>").unwrap();
    let f = fonts();
    let mut c = Canvas::new(20, 20);
    t.render(&mut c, &f, Rect::new(0.0, 0.0, 20.0, 20.0));
}

#[test]
fn preserve_aspect_ratio_placement() {
    // 100×50 viewBox into a 200×200 box, meet + centred: content spans y 50..150.
    let svg = r##"<svg viewBox="0 0 100 50"><rect width="100" height="50" fill="red"/></svg>"##;
    let t = SvgTree::parse(svg).unwrap();
    let img = t.rasterize(&fonts(), 200, 200);
    assert_eq!(img.pixel(100, 100)[3], 255);
    assert_eq!(img.pixel(100, 25)[3], 0, "above the content should be transparent");
    assert_eq!(img.pixel(100, 175)[3], 0);
    let p = img.pixel(100, 100);
    assert!(p[0] > 200 && p[1] < 50 && p[2] < 50, "{p:?}");
    let slice = r##"<svg viewBox="0 0 100 50" preserveAspectRatio="xMinYMin slice"><rect width="100" height="50" fill="red"/></svg>"##;
    let t = SvgTree::parse(slice).unwrap();
    let img = t.rasterize(&fonts(), 200, 200);
    assert_eq!(img.pixel(100, 25)[3], 255);
}

#[test]
fn renders_circle_with_transparency() {
    let svg = r##"<svg width="100" height="100"><circle cx="50" cy="50" r="40" fill="red"/></svg>"##;
    let t = SvgTree::parse(svg).unwrap();
    let img = t.rasterize(&fonts(), 100, 100);
    let center = img.pixel(50, 50);
    assert_eq!(center[3], 255);
    assert!(center[0] > 240 && center[1] < 20 && center[2] < 20, "{center:?}");
    assert_eq!(img.pixel(2, 2)[3], 0);
    assert_eq!(img.pixel(97, 97)[3], 0);
}

#[test]
fn renders_gradient_and_stroke() {
    let svg = r##"<svg width="100" height="20"><defs><linearGradient id="g"><stop offset="0" stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></linearGradient></defs><rect width="100" height="20" fill="url(#g)"/></svg>"##;
    let t = SvgTree::parse(svg).unwrap();
    let img = t.rasterize(&fonts(), 100, 20);
    let left = img.pixel(1, 10);
    let right = img.pixel(98, 10);
    assert!(left[0] > 200 && left[2] < 60, "{left:?}");
    assert!(right[2] > 200 && right[0] < 60, "{right:?}");

    let svg = r##"<svg width="100" height="100"><rect x="10" y="10" width="80" height="80" fill="none" stroke="black" stroke-width="4"/></svg>"##;
    let t = SvgTree::parse(svg).unwrap();
    let img = t.rasterize(&fonts(), 100, 100);
    assert_eq!(img.pixel(50, 50)[3], 0, "interior must stay transparent");
    assert!(img.pixel(10, 50)[3] > 200, "stroke on the left edge");
}

#[test]
fn use_symbol_opacity_display_none() {
    let t = SvgTree::parse(SPRITE).unwrap();
    let img = t.rasterize(&fonts(), 100, 100);
    let p = img.pixel(25, 25);
    assert!(p[3] > 200 && p[1] > 100 && p[0] < 50, "green symbol instance: {p:?}");
    let p = img.pixel(75, 75);
    assert!(p[3] > 200 && p[0] > 200, "red symbol instance: {p:?}");
    let p = img.pixel(50, 50);
    assert!(p[3] > 200 && p[0] > 200 && p[1] > 100, "orange group use: {p:?}");

    let svg = r##"<svg width="10" height="10"><rect width="10" height="10" fill="black" opacity="0.5"/></svg>"##;
    let img = SvgTree::parse(svg).unwrap().rasterize(&fonts(), 10, 10);
    let p = img.pixel(5, 5);
    assert!((100..=160).contains(&p[3]), "half opacity: {p:?}");

    let svg = r##"<svg width="10" height="10"><g display="none"><rect width="10" height="10" fill="black"/></g><g style="display:none"><rect width="10" height="10"/></g></svg>"##;
    let img = SvgTree::parse(svg).unwrap().rasterize(&fonts(), 10, 10);
    assert_eq!(img.pixel(5, 5)[3], 0);
}

#[test]
fn class_styles_clip_and_text() {
    let t = SvgTree::parse(CHART).unwrap();
    let img = t.rasterize(&fonts(), 300, 200);
    // The blue triangle around (50, 155).
    let p = img.pixel(50, 155);
    assert!(p[3] > 200 && p[2] > 150, "{p:?}");
    // Text "Time" near (150, 185) produces some dark pixels.
    let mut dark = 0;
    for x in 120..180 {
        for y in 178..192 {
            let px = img.pixel(x, y);
            if px[3] > 100 {
                dark += 1;
            }
        }
    }
    assert!(dark > 10, "text pixels: {dark}");

    let logo = SvgTree::parse(LOGO).unwrap();
    let img = logo.rasterize(&fonts(), 200, 100);
    // Clipped radial: outside the circle (at 105, 5) is transparent, centre (150, 50) painted.
    assert_eq!(img.pixel(105, 5)[3], 0);
    assert!(img.pixel(150, 50)[3] > 100);
    // Rotated gradient rect painted around its centre.
    assert!(img.pixel(50, 50)[3] > 200);
}

#[test]
fn inline_from_dom_with_page_styles() {
    let doc = rabbithole_html::parse(r##"<p>hi <svg class="ic" viewBox="0 0 10 10" width="10" height="10"><path d="M0 0h10v10H0z"/></svg></p>"##);
    let root = doc.find_element("svg").unwrap();
    let t = SvgTree::from_dom(&doc, root, &|_, prop| if prop == "fill" { Some("rgb(0, 128, 0)".into()) } else { None }).unwrap();
    let img = t.rasterize(&fonts(), 10, 10);
    let p = img.pixel(5, 5);
    assert!(p[1] > 100 && p[0] < 30, "page css fill applied: {p:?}");
}

#[test]
fn render_into_canvas_region() {
    let t = SvgTree::parse(ICON).unwrap();
    let f = fonts();
    let mut c = Canvas::new(60, 60);
    c.clear(Color::WHITE);
    t.render(&mut c, &f, Rect::new(10.0, 10.0, 40.0, 40.0));
    // Outside the dest rect stays white.
    assert_eq!(c.data[0], 0xffffff);
    // Something blue was painted inside.
    let painted = c.data.iter().any(|&p| p != 0xffffff);
    assert!(painted);
}

#[test]
fn renders_many_icons_quickly() {
    let mut svg = String::from(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1000 1000">"##);
    for i in 0..500 {
        let x = (i % 25) * 40;
        let y = (i / 25) * 40;
        svg.push_str(&format!(
            r##"<g transform="translate({x} {y}) scale(1.5)"><path d="M12 2L2 7l10 5 10-5-10-5zm0 13L2 10v7l10 5 10-5v-7l-10 5z" fill="#4285f4" stroke="#123" stroke-width="0.5"/></g>"##
        ));
    }
    svg.push_str("</svg>");
    let t = SvgTree::parse(&svg).unwrap();
    assert!(t.node_count() >= 1000);
    let f = fonts();
    let mut c = Canvas::new(1000, 1000);
    let start = std::time::Instant::now();
    t.render(&mut c, &f, Rect::new(0.0, 0.0, 1000.0, 1000.0));
    let elapsed = start.elapsed();
    assert!(elapsed.as_millis() < 2000, "took {elapsed:?}");
}
