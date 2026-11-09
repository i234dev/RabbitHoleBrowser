//! Tests for the v0.2 graphics features.

use crate::canvas::{Canvas, Color, ColorStop, FillRule, LineCap, Paint, PathSeg, Point, Rect, StrokeStyle};
use crate::font::{Family, FontContext, FontId};
use crate::font_ext::sfnt;
use crate::path::{ellipse_path, parse_svg_path, rounded_rect_path};

fn px(c: &Canvas, x: u32, y: u32) -> Color {
    Color::from_u32(c.data[(y * c.width + x) as usize])
}

/// Sum of "ink" (255 - red channel) over the canvas, in pixel units.
fn ink(c: &Canvas) -> f32 {
    c.data.iter().map(|&p| (255 - ((p >> 16) & 0xff)) as f32 / 255.0).sum()
}

#[test]
fn fill_triangle_and_rules() {
    let mut c = Canvas::new(40, 40);
    let tri = [PathSeg::MoveTo(Point::new(0.0, 0.0)), PathSeg::LineTo(Point::new(40.0, 0.0)), PathSeg::LineTo(Point::new(0.0, 40.0)), PathSeg::Close];
    c.fill_path(&tri, FillRule::NonZero, &Paint::Solid(Color::BLACK));
    let area = ink(&c);
    assert!((area - 800.0).abs() < 8.0, "triangle area {area}");
    assert_eq!(px(&c, 2, 2), Color::BLACK);
    assert_eq!(px(&c, 38, 38), Color::WHITE);
    // Even-odd donut: outer square + inner square (same orientation) leaves a hole.
    let mut c = Canvas::new(40, 40);
    let donut = [
        PathSeg::MoveTo(Point::new(0.0, 0.0)),
        PathSeg::LineTo(Point::new(40.0, 0.0)),
        PathSeg::LineTo(Point::new(40.0, 40.0)),
        PathSeg::LineTo(Point::new(0.0, 40.0)),
        PathSeg::Close,
        PathSeg::MoveTo(Point::new(10.0, 10.0)),
        PathSeg::LineTo(Point::new(30.0, 10.0)),
        PathSeg::LineTo(Point::new(30.0, 30.0)),
        PathSeg::LineTo(Point::new(10.0, 30.0)),
        PathSeg::Close,
    ];
    c.fill_path(&donut, FillRule::EvenOdd, &Paint::Solid(Color::BLACK));
    assert_eq!(px(&c, 20, 20), Color::WHITE);
    assert_eq!(px(&c, 5, 5), Color::BLACK);
    let mut c2 = Canvas::new(40, 40);
    c2.fill_path(&donut, FillRule::NonZero, &Paint::Solid(Color::BLACK));
    assert_eq!(px(&c2, 20, 20), Color::BLACK);
    // Circle area within 2%
    let mut c = Canvas::new(100, 100);
    let r = 40.0;
    c.fill_path(&ellipse_path(Point::new(50.0, 50.0), r, r), FillRule::NonZero, &Paint::Solid(Color::BLACK));
    let area = ink(&c);
    let expected = std::f32::consts::PI * r * r;
    assert!((area - expected).abs() / expected < 0.02, "circle area {area} vs {expected}");
    // Smooth edge: a partial pixel exists along the arc
    let partial = (0..100).any(|x| (0..100).any(|y| matches!(px(&c, x, y).r, 30..=225)));
    assert!(partial);
}

#[test]
fn strokes() {
    // Square outline stroked 4px wide: horizontal profile through the middle shows two 4px bands.
    let mut c = Canvas::new(60, 60);
    let sq = crate::path::rect_path(Rect::new(10.0, 10.0, 40.0, 40.0));
    c.stroke_path(&sq, &StrokeStyle { width: 4.0, ..Default::default() }, &Paint::Solid(Color::BLACK));
    let row: Vec<bool> = (0..60).map(|x| px(&c, x, 30).r < 128).collect();
    let dark = row.iter().filter(|b| **b).count();
    assert_eq!(dark, 8, "{row:?}");
    assert!(row[8] && row[11] && !row[12] && !row[47] && row[48] && row[51]);
    // Mitre corner is filled
    assert_eq!(px(&c, 8, 8), Color::BLACK);
    // Dashed stroke leaves gaps
    let mut c = Canvas::new(100, 10);
    let line = [PathSeg::MoveTo(Point::new(0.0, 5.0)), PathSeg::LineTo(Point::new(100.0, 5.0))];
    c.stroke_path(&line, &StrokeStyle { width: 2.0, dashes: vec![6.0, 4.0], ..Default::default() }, &Paint::Solid(Color::BLACK));
    let painted = (0..100).filter(|&x| px(&c, x, 5).r < 128).count();
    assert!(painted > 50 && painted < 70, "{painted}");
    assert!(px(&c, 2, 5).r < 128 && px(&c, 8, 5).r > 128, "gap missing");
    // Round caps extend beyond the end points
    let mut c = Canvas::new(40, 20);
    let seg = [PathSeg::MoveTo(Point::new(10.0, 10.0)), PathSeg::LineTo(Point::new(30.0, 10.0))];
    c.stroke_path(&seg, &StrokeStyle { width: 6.0, cap: LineCap::Round, ..Default::default() }, &Paint::Solid(Color::BLACK));
    assert!(px(&c, 8, 10).r < 128);
    assert!(px(&c, 5, 10).r > 200);
}

#[test]
fn gradients() {
    let mut c = Canvas::new(100, 10);
    let paint = Paint::Linear {
        start: Point::new(0.0, 0.0),
        end: Point::new(100.0, 0.0),
        stops: vec![ColorStop { offset: 0.0, color: Color::rgb(255, 0, 0) }, ColorStop { offset: 1.0, color: Color::rgb(0, 0, 255) }],
        repeat: false,
    };
    c.fill_rounded_rect_radii(Rect::new(0.0, 0.0, 100.0, 10.0), [(0.0, 0.0); 4], &paint);
    let l = px(&c, 0, 5);
    let r = px(&c, 99, 5);
    let m = px(&c, 50, 5);
    assert!(l.r > 240 && l.b < 15, "{l:?}");
    assert!(r.b > 240 && r.r < 15, "{r:?}");
    assert!((m.r as i32 - 128).abs() < 10 && (m.b as i32 - 128).abs() < 10, "{m:?}");
    // Radial: centre is first stop, far corner the last
    let mut c = Canvas::new(50, 50);
    let paint = Paint::Radial {
        center: Point::new(25.0, 25.0),
        rx: 25.0,
        ry: 25.0,
        stops: vec![ColorStop { offset: 0.0, color: Color::WHITE }, ColorStop { offset: 1.0, color: Color::BLACK }],
        repeat: false,
    };
    c.fill_path(&crate::path::rect_path(Rect::new(0.0, 0.0, 50.0, 50.0)), FillRule::NonZero, &paint);
    assert!(px(&c, 25, 25).r > 230);
    assert!(px(&c, 0, 0).r < 10);
    assert!(px(&c, 25, 2).r < 60);
    // Transparent stops interpolate in premultiplied space (no darkening).
    let lut = crate::paint::build_lut(&[ColorStop { offset: 0.0, color: Color::rgb(255, 0, 0) }, ColorStop { offset: 1.0, color: Color::rgba(0, 0, 0, 0) }]);
    let mid = lut[512];
    assert!(mid.r > 240 && mid.a > 100 && mid.a < 150, "{mid:?}");
}

#[test]
fn rounded_radii_clamp_and_clip() {
    let mut c = Canvas::new(40, 20);
    // Huge radii clamp to an ellipse-like pill shape.
    c.fill_rounded_rect_radii(Rect::new(0.0, 0.0, 40.0, 20.0), [(100.0, 100.0); 4], &Paint::Solid(Color::BLACK));
    assert_eq!(px(&c, 20, 10), Color::BLACK);
    assert!(px(&c, 0, 0).r > 200);
    assert!(px(&c, 39, 19).r > 200);
    assert_eq!(px(&c, 20, 0), Color::BLACK);
    // Per-corner: only top-left rounded
    let mut c = Canvas::new(20, 20);
    c.fill_rounded_rect_radii(Rect::new(0.0, 0.0, 20.0, 20.0), [(10.0, 10.0), (0.0, 0.0), (0.0, 0.0), (0.0, 0.0)], &Paint::Solid(Color::BLACK));
    assert!(px(&c, 0, 0).r > 200);
    assert_eq!(px(&c, 19, 0), Color::BLACK);
    assert_eq!(px(&c, 19, 19), Color::BLACK);
    assert_eq!(px(&c, 0, 19), Color::BLACK);
    // Rounded clip restricts a full fill
    let mut c = Canvas::new(20, 20);
    c.push_clip_rounded(Rect::new(0.0, 0.0, 20.0, 20.0), [(10.0, 10.0); 4]);
    c.fill_rect(Rect::new(0.0, 0.0, 20.0, 20.0), Color::BLACK);
    c.pop_clip();
    assert!(px(&c, 0, 0).r > 200);
    assert_eq!(px(&c, 10, 10), Color::BLACK);
    // Path clip: triangle
    let mut c = Canvas::new(40, 40);
    let tri = [PathSeg::MoveTo(Point::new(0.0, 0.0)), PathSeg::LineTo(Point::new(40.0, 0.0)), PathSeg::LineTo(Point::new(0.0, 40.0)), PathSeg::Close];
    c.push_clip_path(&tri, FillRule::NonZero);
    c.fill_rect(Rect::new(0.0, 0.0, 40.0, 40.0), Color::BLACK);
    c.draw_line(0.0, 39.0, 40.0, 39.0, 2.0, Color::BLACK);
    c.pop_clip();
    assert_eq!(px(&c, 2, 2), Color::BLACK);
    assert_eq!(px(&c, 38, 38), Color::WHITE);
    assert_eq!(px(&c, 30, 39), Color::WHITE);
    // Clip rect after pop restored
    c.fill_rect(Rect::new(38.0, 38.0, 2.0, 2.0), Color::BLACK);
    assert_eq!(px(&c, 38, 38), Color::BLACK);
}

#[test]
fn borders_with_radii_and_styles() {
    let red = Color::rgb(255, 0, 0);
    let blue = Color::rgb(0, 0, 255);
    let mut c = Canvas::new(40, 40);
    c.stroke_rect_radii(Rect::new(0.0, 0.0, 40.0, 40.0), [4.0; 4], [red, blue, red, blue], [0; 4], [(8.0, 8.0); 4]);
    assert_eq!(px(&c, 20, 1), red);
    assert_eq!(px(&c, 1, 20), blue);
    assert_eq!(px(&c, 20, 20), Color::WHITE);
    assert!(px(&c, 0, 0).g > 200, "corner outside radius is empty");
    // Mitred corner without radius: diagonal split between red (top) and blue (left)
    let mut c = Canvas::new(20, 20);
    c.stroke_rect_radii(Rect::new(0.0, 0.0, 20.0, 20.0), [6.0; 4], [red, blue, red, blue], [0; 4], [(0.0, 0.0); 4]);
    assert_eq!(px(&c, 4, 1), red);
    assert_eq!(px(&c, 1, 4), blue);
    // Double style: three bands (ink, gap, ink)
    let mut c = Canvas::new(30, 30);
    c.stroke_rect_radii(Rect::new(0.0, 0.0, 30.0, 30.0), [6.0; 4], [Color::BLACK; 4], [3; 4], [(0.0, 0.0); 4]);
    assert_eq!(px(&c, 15, 0), Color::BLACK);
    assert_eq!(px(&c, 15, 3), Color::WHITE);
    assert_eq!(px(&c, 15, 5), Color::BLACK);
    // Inset: top darker than bottom
    let mut c = Canvas::new(30, 30);
    let grey = Color::rgb(128, 128, 128);
    c.stroke_rect_radii(Rect::new(0.0, 0.0, 30.0, 30.0), [4.0; 4], [grey; 4], [4; 4], [(0.0, 0.0); 4]);
    assert!(px(&c, 15, 1).r < px(&c, 15, 28).r);
    // Dotted rounded border paints something but not everything along the top
    let mut c = Canvas::new(60, 30);
    c.stroke_rect_radii(Rect::new(0.0, 0.0, 60.0, 30.0), [3.0; 4], [Color::BLACK; 4], [2; 4], [(6.0, 6.0); 4]);
    let painted = (10..50).filter(|&x| px(&c, x, 1).r < 128).count();
    assert!(painted > 5 && painted < 35, "{painted}");
}

#[test]
fn box_shadow_falloff() {
    let mut c = Canvas::new(80, 80);
    c.draw_box_shadow(Rect::new(20.0, 20.0, 40.0, 40.0), [(4.0, 4.0); 4], (0.0, 0.0), 12.0, 0.0, Color::BLACK, false);
    // Inside the box nothing is painted (knocked out).
    assert_eq!(px(&c, 40, 40), Color::WHITE);
    // Outside: darkness decreases monotonically away from the edge.
    let vals: Vec<u8> = (60..80).map(|x| px(&c, x, 40).r).collect();
    assert!(vals[0] < 200, "shadow should be visible next to the box: {vals:?}");
    for w in vals.windows(2) {
        assert!(w[1] >= w[0], "not monotonic: {vals:?}");
    }
    assert!(vals[19] > 240);
    // Offset shadow appears only on the offset side
    let mut c = Canvas::new(80, 80);
    c.draw_box_shadow(Rect::new(20.0, 20.0, 40.0, 40.0), [(0.0, 0.0); 4], (10.0, 10.0), 0.0, 0.0, Color::BLACK, false);
    assert_eq!(px(&c, 65, 65), Color::BLACK);
    assert_eq!(px(&c, 15, 15), Color::WHITE);
    assert_eq!(px(&c, 40, 40), Color::WHITE);
    // Inset shadow darkens the inside edge, not the centre
    let mut c = Canvas::new(80, 80);
    c.draw_box_shadow(Rect::new(20.0, 20.0, 40.0, 40.0), [(0.0, 0.0); 4], (0.0, 0.0), 10.0, 0.0, Color::BLACK, true);
    assert!(px(&c, 21, 40).r < px(&c, 40, 40).r);
    assert_eq!(px(&c, 10, 40), Color::WHITE);
    // Spread enlarges the shadow
    let mut c = Canvas::new(80, 80);
    c.draw_box_shadow(Rect::new(20.0, 20.0, 40.0, 40.0), [(0.0, 0.0); 4], (0.0, 0.0), 0.0, 10.0, Color::BLACK, false);
    assert_eq!(px(&c, 12, 40), Color::BLACK);
    assert_eq!(px(&c, 8, 40), Color::WHITE);
}

#[test]
fn transforms() {
    let mut c = Canvas::new(60, 60);
    // Rotate 90° clockwise about the origin then translate so the rect (0,0,20,10) lands at x∈[40,50], y∈[0,20].
    let a = std::f32::consts::FRAC_PI_2;
    c.push_transform([a.cos(), a.sin(), -a.sin(), a.cos(), 50.0, 0.0]);
    c.fill_rect(Rect::new(0.0, 0.0, 20.0, 10.0), Color::BLACK);
    c.pop_transform();
    assert_eq!(px(&c, 45, 10), Color::BLACK);
    assert_eq!(px(&c, 45, 25), Color::WHITE);
    assert_eq!(px(&c, 35, 10), Color::WHITE);
    assert_eq!(px(&c, 10, 5), Color::WHITE);
    // Scale + translate keeps axis alignment
    let mut c = Canvas::new(60, 60);
    c.push_transform([2.0, 0.0, 0.0, 2.0, 10.0, 10.0]);
    c.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), Color::BLACK);
    c.push_clip(Rect::new(0.0, 0.0, 5.0, 5.0));
    c.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), Color::rgb(255, 0, 0));
    c.pop_clip();
    c.pop_transform();
    assert_eq!(px(&c, 29, 29), Color::BLACK);
    assert_eq!(px(&c, 30, 30), Color::WHITE);
    assert_eq!(px(&c, 15, 15), Color::rgb(255, 0, 0));
    assert_eq!(px(&c, 25, 25), Color::BLACK);
    assert_eq!(c.map_point(Point::new(1.0, 1.0)), Point::new(1.0, 1.0));
    // Rotated image lands where expected
    let img = crate::image::Image::new(2, 2, vec![0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]);
    let mut c = Canvas::new(60, 60);
    c.push_transform([a.cos(), a.sin(), -a.sin(), a.cos(), 50.0, 0.0]);
    c.draw_image(&img, Rect::new(0.0, 0.0, 20.0, 10.0), 1.0);
    c.pop_transform();
    assert!(px(&c, 45, 10).r < 30);
    assert_eq!(px(&c, 45, 25), Color::WHITE);
    // Rotated text draws something in the rotated area
    let fonts = FontContext::new();
    let f = fonts.select(Family::Sans, 400, false);
    let mut c = Canvas::new(80, 80);
    c.push_transform([a.cos(), a.sin(), -a.sin(), a.cos(), 50.0, 10.0]);
    c.draw_text(&fonts, f, 16.0, "Hi", 0.0, 0.0, Color::BLACK);
    c.pop_transform();
    let dark_right = (50..65).flat_map(|x| (10..40).map(move |y| (x, y))).filter(|&(x, y)| px(&c, x, y).r < 128).count();
    assert!(dark_right > 10, "{dark_right}");
}

#[test]
fn layers_and_blur() {
    let mut c = Canvas::new(20, 20);
    c.begin_layer(Rect::new(0.0, 0.0, 20.0, 20.0));
    c.fill_rect(Rect::new(0.0, 0.0, 10.0, 20.0), Color::BLACK);
    c.end_layer(0.5);
    let p = px(&c, 5, 5);
    assert!((p.r as i32 - 128).abs() <= 2, "{p:?}");
    assert_eq!(px(&c, 15, 5), Color::WHITE);
    // Opaque layer is a no-op
    let mut c = Canvas::new(20, 20);
    c.begin_layer(Rect::new(0.0, 0.0, 20.0, 20.0));
    c.fill_rect(Rect::new(0.0, 0.0, 10.0, 20.0), Color::BLACK);
    c.end_layer(1.0);
    assert_eq!(px(&c, 5, 5), Color::BLACK);
    // Blur softens a hard edge
    let mut c = Canvas::new(40, 20);
    c.fill_rect(Rect::new(0.0, 0.0, 20.0, 20.0), Color::BLACK);
    c.blur_region(Rect::new(0.0, 0.0, 40.0, 20.0), 3.0);
    let p = px(&c, 20, 10);
    assert!(p.r > 40 && p.r < 220, "{p:?}");
}

#[test]
fn svg_path_parsing() {
    let d = "M10 10 h 20 v20 H10 Z m5 5 l5 0 L20 20 c 1 1 2 2 3 3 s 4 4 5 5 q 1 1 2 2 t 3 3 a 5 5 0 0 1 10 0 A5,5 0 1,0 60 60z";
    let p = parse_svg_path(d);
    assert!(matches!(p[0], PathSeg::MoveTo(Point { x, y }) if x == 10.0 && y == 10.0));
    assert!(matches!(p[1], PathSeg::LineTo(Point { x, y }) if x == 30.0 && y == 10.0));
    assert!(matches!(p[2], PathSeg::LineTo(Point { x, y }) if x == 30.0 && y == 30.0));
    assert!(matches!(p[3], PathSeg::LineTo(Point { x, y }) if x == 10.0 && y == 30.0));
    assert!(matches!(p[4], PathSeg::Close));
    // relative moveto after close is relative to the subpath start (10,10)
    assert!(matches!(p[5], PathSeg::MoveTo(Point { x, y }) if x == 15.0 && y == 15.0));
    assert!(matches!(p[6], PathSeg::LineTo(Point { x, y }) if x == 20.0 && y == 15.0));
    assert!(p.iter().any(|s| matches!(s, PathSeg::CubicTo(..))));
    assert!(p.iter().any(|s| matches!(s, PathSeg::QuadTo(..))));
    let arcs = p.iter().filter(|s| matches!(s, PathSeg::ArcTo { .. })).count();
    assert_eq!(arcs, 2);
    assert!(matches!(p.last(), Some(PathSeg::Close)));
    // Numbers with exponents and implicit repeats
    let p = parse_svg_path("M1e1-2.5e0 1 2 3 4");
    assert!(matches!(p[0], PathSeg::MoveTo(Point { x, y }) if x == 10.0 && y == -2.5));
    assert_eq!(p.len(), 3);
    // Arc renders a half circle with sensible area
    let mut c = Canvas::new(60, 40);
    c.fill_path(&parse_svg_path("M10 30 A20 20 0 0 1 50 30 Z"), FillRule::NonZero, &Paint::Solid(Color::BLACK));
    let area = ink(&c);
    let expected = std::f32::consts::PI * 20.0 * 20.0 / 2.0;
    assert!((area - expected).abs() / expected < 0.03, "{area} vs {expected}");
    assert_eq!(px(&c, 30, 15), Color::BLACK);
    assert_eq!(px(&c, 30, 35), Color::WHITE);
}

fn dejavu_sans() -> &'static [u8] {
    include_bytes!("../../../assets/fonts/DejaVuSans.ttf")
}

/// Build a WOFF 1.0 file from an SFNT.
fn make_woff(sfnt: &[u8]) -> Vec<u8> {
    let dir = sfnt::table_directory(sfnt, 0).unwrap();
    let flavor = sfnt::rd32(sfnt, 0).unwrap();
    let mut tables: Vec<([u8; 4], Vec<u8>, usize)> = Vec::new();
    for t in &dir {
        let data = &sfnt[t.offset as usize..(t.offset + t.length) as usize];
        let comp = miniz_oxide::deflate::compress_to_vec_zlib(data, 6);
        if comp.len() < data.len() {
            tables.push((t.tag, comp, data.len()));
        } else {
            tables.push((t.tag, data.to_vec(), data.len()));
        }
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"wOFF");
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes()); // length (patched)
    out.extend_from_slice(&(tables.len() as u16).to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(sfnt.len() as u32).to_be_bytes());
    out.extend_from_slice(&[0, 1, 0, 0]);
    out.extend_from_slice(&[0u8; 20]);
    let mut offset = 44 + 20 * tables.len();
    let mut dir_bytes = Vec::new();
    for (tag, comp, orig) in &tables {
        dir_bytes.extend_from_slice(tag);
        dir_bytes.extend_from_slice(&(offset as u32).to_be_bytes());
        dir_bytes.extend_from_slice(&(comp.len() as u32).to_be_bytes());
        dir_bytes.extend_from_slice(&(*orig as u32).to_be_bytes());
        dir_bytes.extend_from_slice(&0u32.to_be_bytes());
        offset += (comp.len() + 3) & !3;
    }
    out.extend_from_slice(&dir_bytes);
    for (_, comp, _) in &tables {
        out.extend_from_slice(comp);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    let len = out.len() as u32;
    out[8..12].copy_from_slice(&len.to_be_bytes());
    out
}

#[test]
fn woff_round_trip_and_registration() {
    let woff = make_woff(dejavu_sans());
    assert!(woff.len() < dejavu_sans().len());
    let fonts = FontContext::new();
    let id = fonts.add_font_data("Web Sans", 400, false, &woff).expect("woff decodes");
    assert!(id.0 >= 7);
    assert!(fonts.has_family("web sans"));
    let w = fonts.measure(id, 16.0, "Hello");
    let w_ref = fonts.measure(fonts.select(Family::Sans, 400, false), 16.0, "Hello");
    assert!((w - w_ref).abs() < 0.01, "{w} vs {w_ref}");
    let mut c = Canvas::new(80, 30);
    c.draw_text(&fonts, id, 16.0, "Hello", 2.0, 20.0, Color::BLACK);
    assert!(c.data.iter().any(|&p| (p & 0xff) < 100));
    // Raw TTF also registers; garbage does not.
    assert!(fonts.add_font_data("Raw", 700, true, dejavu_sans()).is_some());
    assert!(fonts.add_font_data("Bad", 400, false, b"not a font").is_none());
    // select_family finds the web font by name, case-insensitively, with weight matching
    let picked = fonts.select_family(&["\"RAW\"".to_string()], Family::Sans, 700, true);
    assert_eq!(fonts.family_of(picked), "Raw");
    assert!(!fonts.needs_synthetic_italic(picked, true));
    let picked = fonts.select_family(&["Web Sans".to_string()], Family::Sans, 700, true);
    assert_eq!(fonts.family_of(picked), "Web Sans");
    assert!(fonts.needs_synthetic_italic(picked, true));
    assert!(fonts.needs_synthetic_bold(picked, 700));
}

#[test]
fn woff2_glyf_untransform_and_varints() {
    use crate::font_ext::woff2;
    assert_eq!(woff2::read_255(&[200]), Some(200));
    assert_eq!(woff2::read_255(&[253, 0x12, 0x34]), Some(0x1234));
    assert_eq!(woff2::read_255(&[254, 10]), Some(516));
    assert_eq!(woff2::read_255(&[255, 10]), Some(263));
    assert_eq!(woff2::read_base128(&[0x7f]), Some(127));
    assert_eq!(woff2::read_base128(&[0x81, 0x00]), Some(128));
    assert_eq!(woff2::read_base128(&[0x80]), None);
    // Hand-built transformed glyf with one triangle glyph: (100,0) (100,50) (0,50)
    let mut d = Vec::new();
    d.extend_from_slice(&0u32.to_be_bytes()); // version
    d.extend_from_slice(&1u16.to_be_bytes()); // numGlyphs
    d.extend_from_slice(&0u16.to_be_bytes()); // indexFormat
    let n_contour = [0u8, 1];
    let n_points = [3u8];
    let flags = [11u8, 1, 10];
    let glyph = [100u8, 50, 100, 0];
    let composite: [u8; 0] = [];
    let bbox = [0u8, 0, 0, 0];
    let instr: [u8; 0] = [];
    for s in [n_contour.len(), n_points.len(), flags.len(), glyph.len(), composite.len(), bbox.len(), instr.len()] {
        d.extend_from_slice(&(s as u32).to_be_bytes());
    }
    d.extend_from_slice(&n_contour);
    d.extend_from_slice(&n_points);
    d.extend_from_slice(&flags);
    d.extend_from_slice(&glyph);
    d.extend_from_slice(&bbox);
    let (glyf, loca, bboxes, fmt, n) = woff2::reconstruct_glyf(&d).expect("decodes");
    assert_eq!((fmt, n), (1, 1));
    assert_eq!(bboxes, vec![[0, 0, 100, 50]]);
    assert_eq!(&loca[0..4], &0u32.to_be_bytes());
    assert_eq!(u32::from_be_bytes([loca[4], loca[5], loca[6], loca[7]]) as usize, glyf.len());
    // header: nContours=1, bbox, endPts=[2], instrLen=0
    assert_eq!(&glyf[0..2], &1i16.to_be_bytes());
    assert_eq!(&glyf[2..10], &[0, 0, 0, 0, 0, 100, 0, 50]);
    assert_eq!(&glyf[10..12], &2u16.to_be_bytes());
    assert_eq!(&glyf[12..14], &0u16.to_be_bytes());
    assert_eq!(&glyf[14..17], &[1, 1, 1]); // on-curve flags
    // x deltas 100, 0, -100 ; y deltas 0, 50, 0
    assert_eq!(&glyf[17..23], &[0, 100, 0, 0, 0xff, 0x9c]);
    assert_eq!(&glyf[23..29], &[0, 0, 0, 50, 0, 0]);
    // hmtx reconstruction with lsb from bboxes
    let hm = woff2::reconstruct_hmtx(&[1u8, 0, 200], 1, 1, Some(&bboxes)).unwrap();
    assert_eq!(hm, vec![0, 200, 0, 0]);
    // A WOFF2 header that is not a collection but has garbage brotli data fails cleanly.
    let mut bad = Vec::new();
    bad.extend_from_slice(b"wOF2");
    bad.extend_from_slice(&[0, 1, 0, 0]);
    bad.extend_from_slice(&[0; 40]);
    assert!(woff2::decode(&bad).is_none());
}

#[test]
fn name_table_parsing_and_selection() {
    let info = sfnt::parse_info(dejavu_sans(), 0).unwrap();
    assert_eq!(info.family, "DejaVu Sans");
    assert_eq!(info.weight, 400);
    assert!(!info.italic);
    let bold = sfnt::parse_info(include_bytes!("../../../assets/fonts/DejaVuSans-Bold.ttf"), 0).unwrap();
    assert_eq!(bold.weight, 700);
    let obl = sfnt::parse_info(include_bytes!("../../../assets/fonts/DejaVuSans-Oblique.ttf"), 0).unwrap();
    assert!(obl.italic);
    assert_eq!(obl.family, "DejaVu Sans");
    let fonts = FontContext::new();
    assert_eq!(fonts.select_family(&["DejaVu Serif".to_string()], Family::Sans, 700, false), FontId(crate::font::SERIF_BOLD));
    assert_eq!(fonts.select_family(&["Nope".to_string(), "dejavu sans mono".to_string()], Family::Sans, 400, false), FontId(crate::font::MONO));
    assert_eq!(fonts.select_family(&["Nope".to_string()], Family::Serif, 400, false), FontId(crate::font::SERIF));
    assert_eq!(fonts.select_family(&["monospace".to_string()], Family::Sans, 400, false), FontId(crate::font::MONO));
    // Weight matching: 300 → regular (closest below none, so above), 900 → bold
    assert_eq!(fonts.select_family(&["DejaVu Sans".to_string()], Family::Sans, 300, false), FontId(crate::font::SANS));
    assert_eq!(fonts.select_family(&["DejaVu Sans".to_string()], Family::Sans, 900, true), FontId(crate::font::SANS_BOLD_ITALIC));
    let faces = fonts.faces();
    assert_eq!(faces.len(), 7);
    assert!(faces.iter().any(|f| f.family == "DejaVu Serif" && f.weight == 700));
}

#[test]
fn system_fonts_and_fallback() {
    let fonts = FontContext::new();
    let before = fonts.face_count();
    let found = fonts.load_system_fonts();
    if std::path::Path::new("/usr/share/fonts").exists() {
        assert!(found >= 1, "expected system fonts on this machine");
        assert!(fonts.face_count() > before);
    }
    // Second call is a no-op
    assert_eq!(fonts.load_system_fonts(), 0);
    let sans = fonts.select(Family::Sans, 400, false);
    let _ = fonts.fallback_for_char('中', sans);
    let _ = fonts.fallback_for_char('😀', sans);
    let glyphs = fonts.shape(sans, 16.0, "a中b", 0.0);
    assert_eq!(glyphs.len(), 3);
    // Shaping with a bogus FontId does not panic and uses the bundled sans.
    let w = fonts.measure(FontId(9999), 16.0, "Hello");
    assert!(w > 20.0);
    // Subpixel variants render and are cached.
    let g = glyphs[0].glyph;
    let b0 = fonts.rasterize_subpixel(sans, g, 16.0, 0);
    let b2 = fonts.rasterize_subpixel(sans, g, 16.0, 2);
    assert_eq!(b2.width, b0.width + 1);
    assert!(std::rc::Rc::ptr_eq(&b2, &fonts.rasterize_subpixel(sans, g, 16.0, 2)));
}

#[test]
fn text_ext_small_caps_and_oblique() {
    let fonts = FontContext::new();
    let f = fonts.select(Family::Serif, 400, false);
    let mut c = Canvas::new(200, 40);
    let adv = c.draw_text_ext(&fonts, f, 20.0, "Hello", 2.0, 30.0, Color::BLACK, 0.0, true, true);
    assert!(adv > 40.0);
    assert!(c.data.iter().any(|&p| (p & 0xff) < 100));
    // Letter spacing widens the advance
    let mut c2 = Canvas::new(200, 40);
    let adv2 = c2.draw_text_ext(&fonts, f, 20.0, "Hello", 2.0, 30.0, Color::BLACK, 4.0, false, false);
    assert!(adv2 > adv + 10.0);
    // Shadow with blur paints soft pixels around the text
    let mut c3 = Canvas::new(200, 60);
    c3.draw_text_shadow(&fonts, f, 20.0, "Hello", 10.0, 40.0, 0.0, Color::BLACK, 6.0);
    let soft = c3.data.iter().filter(|&&p| matches!(p & 0xff, 40..=215)).count();
    assert!(soft > 50, "{soft}");
}

#[test]
fn tiled_images() {
    let img = crate::image::Image::new(2, 2, vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255]);
    let mut c = Canvas::new(40, 40);
    c.draw_image_tiled(&img, Rect::new(0.0, 0.0, 40.0, 40.0), Point::new(0.0, 0.0), (4.0, 4.0), true, true, 1.0);
    let p = px(&c, 0, 0);
    assert!(p.r > 200 && p.g < 60, "{p:?}");
    let p = px(&c, 3, 0);
    assert!(p.g > 200 && p.r < 60, "{p:?}");
    let p = px(&c, 36, 36);
    assert!(p.r > 200 && p.g < 60, "{p:?}");
    // No repeat: a single tile
    let mut c = Canvas::new(40, 40);
    c.draw_image_tiled(&img, Rect::new(0.0, 0.0, 40.0, 40.0), Point::new(10.0, 10.0), (4.0, 4.0), false, false, 1.0);
    let p = px(&c, 10, 10);
    assert!(p.r > 200 && p.g < 60, "{p:?}");
    assert_eq!(px(&c, 20, 20), Color::WHITE);
    // Region draws a sub-rectangle
    let mut c = Canvas::new(10, 10);
    c.draw_image_region(&img, Rect::new(1.0, 0.0, 1.0, 1.0), Rect::new(0.0, 0.0, 10.0, 10.0), 1.0);
    let p = px(&c, 5, 5);
    assert!(p.g > 200 && p.r < 60 && p.b < 60, "{p:?}");
}

#[test]
fn rounded_rect_path_matches_fast_path() {
    let r = Rect::new(5.0, 5.0, 50.0, 30.0);
    let radii = [(10.0, 6.0), (0.0, 0.0), (15.0, 15.0), (4.0, 4.0)];
    let mut a = Canvas::new(60, 40);
    a.fill_rounded_rect_radii(r, radii, &Paint::Solid(Color::BLACK));
    let mut b = Canvas::new(60, 40);
    b.fill_path(&rounded_rect_path(r, radii), FillRule::NonZero, &Paint::Solid(Color::BLACK));
    let diff: f32 = a.data.iter().zip(&b.data).map(|(&x, &y)| ((x & 0xff) as i32 - (y & 0xff) as i32).abs() as f32 / 255.0).sum();
    assert!(diff < 6.0, "fast path and path rasterizer disagree: {diff}");
}

#[test]
fn perf_smoke() {
    let t = std::time::Instant::now();
    let mut c = Canvas::new(1000, 1000);
    let paint = Paint::Linear {
        start: Point::new(0.0, 0.0),
        end: Point::new(1000.0, 1000.0),
        stops: vec![ColorStop { offset: 0.0, color: Color::rgb(255, 0, 0) }, ColorStop { offset: 1.0, color: Color::rgb(0, 0, 255) }],
        repeat: false,
    };
    c.fill_rounded_rect_radii(Rect::new(0.0, 0.0, 1000.0, 1000.0), [(40.0, 40.0); 4], &paint);
    let grad = t.elapsed();
    let t = std::time::Instant::now();
    c.draw_box_shadow(Rect::new(300.0, 300.0, 300.0, 300.0), [(8.0, 8.0); 4], (0.0, 10.0), 20.0, 0.0, Color::rgba(0, 0, 0, 128), false);
    let shadow = t.elapsed();
    let t = std::time::Instant::now();
    c.fill_path(&ellipse_path(Point::new(500.0, 500.0), 400.0, 300.0), FillRule::NonZero, &Paint::Solid(Color::rgba(0, 128, 0, 200)));
    let ell = t.elapsed();
    eprintln!("perf: gradient rounded rect {grad:?}, box shadow {shadow:?}, ellipse {ell:?}");
    assert!(grad.as_millis() < 500 && shadow.as_millis() < 500 && ell.as_millis() < 500);
}
