use super::*;

fn doc_filter(markup: &str) -> FilterDef {
    let doc = rabbithole_html::parse(&format!("<!doctype html><body><svg>{markup}</svg>"));
    let svg = doc.find_element("svg").unwrap();
    let tree = crate::parse::build(&doc, svg, &|_, _| None).unwrap();
    tree.filters.get("f").expect("filter f").as_ref().clone()
}

fn env(w: f32, h: f32) -> FilterEnv<'static> {
    FilterEnv { m: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], bbox: Rect::new(10.0, 10.0, w, h), viewport: (100.0, 100.0), element: None, fonts: None }
}

#[test]
fn default_region_is_ten_percent_larger() {
    let f = doc_filter("<filter id=f><feFlood/></filter>");
    let r = f.region(Rect::new(10.0, 10.0, 50.0, 100.0), (300.0, 150.0)).unwrap();
    let near = |a: Rect, b: Rect| (a.x - b.x).abs() + (a.y - b.y).abs() + (a.w - b.w).abs() + (a.h - b.h).abs() < 1e-3;
    assert!(near(r, Rect::new(5.0, 0.0, 60.0, 120.0)), "{r:?}");
    let u = doc_filter("<filter id=f filterUnits=userSpaceOnUse x=0 y=0 width=50% height=20><feFlood/></filter>");
    assert_eq!(u.region(Rect::new(10.0, 10.0, 50.0, 100.0), (300.0, 150.0)).unwrap(), Rect::new(0.0, 0.0, 150.0, 20.0));
    // objectBoundingBox on an empty box renders nothing.
    assert!(f.region(Rect::new(0.0, 0.0, 0.0, 10.0), (300.0, 150.0)).is_none());
}

#[test]
fn flood_fills_the_subregion_in_srgb() {
    let f = doc_filter("<filter id=f x=0 y=0 width=1 height=1><feFlood flood-color=green flood-opacity=0.5 x=20 y=20 width=10 height=10 /></filter>");
    let mut fe = f.clone();
    fe.primitives[0].kind = fe.primitives[0].kind.clone();
    let src = Pixels::new(100, 100);
    let out = fe.apply(&src, &env(50.0, 50.0));
    // primitiveUnits default userSpaceOnUse: subregion 20..30.
    let at = |x: usize, y: usize| out.px[y * 100 + x];
    assert_eq!(at(19, 25)[3], 0.0);
    let c = at(25, 25);
    assert!((c[3] - 0.5).abs() < 1e-3);
    // Green stays 128/255 after the linearRGB round trip.
    assert!((c[1] / c[3] - 128.0 / 255.0).abs() < 2.0 / 255.0, "{c:?}");
    assert_eq!(at(31, 25)[3], 0.0);
}

#[test]
fn results_chain_by_name_and_merge() {
    let f = doc_filter(
        "<filter id=f x=0 y=0 width=1 height=1 color-interpolation-filters=sRGB>\
         <feFlood flood-color=red result=r />\
         <feOffset in=SourceGraphic dx=5 result=moved />\
         <feMerge><feMergeNode in=r /><feMergeNode in=moved /></feMerge></filter>",
    );
    assert_eq!(f.primitives.len(), 3);
    assert_eq!(f.primitives[2].inputs, vec![Input::Named("r".into()), Input::Named("moved".into())]);
    let mut src = Pixels::new(100, 100);
    src.px[20 * 100 + 20] = [0.0, 0.0, 1.0, 1.0];
    let out = f.apply(&src, &env(50.0, 50.0));
    assert_eq!(out.px[20 * 100 + 25], [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(out.px[20 * 100 + 20], [1.0, 0.0, 0.0, 1.0]);
    // Outside the filter region (bbox 10..60).
    assert_eq!(out.px[5 * 100 + 5], [0.0; 4]);
}

#[test]
fn primitive_units_object_bounding_box() {
    let f = doc_filter("<filter id=f primitiveUnits=objectBoundingBox><feOffset dx=0.1 /></filter>");
    assert!(f.prim_bbox);
    let mut src = Pixels::new(100, 100);
    src.px[20 * 100 + 20] = [1.0; 4];
    let out = f.apply(&src, &env(50.0, 50.0));
    // 0.1 × 50 = 5 px.
    assert_eq!(out.px[20 * 100 + 25], [1.0; 4]);
}

#[test]
fn colour_matrix_and_component_transfer_parse() {
    let f = doc_filter(
        "<filter id=f><feColorMatrix type=saturate values=0 /><feColorMatrix values='1 2 3' />\
         <feComponentTransfer><feFuncR type=table tableValues='1 0' /><feFuncA type=linear slope=0.5 /></feComponentTransfer></filter>",
    );
    assert!(matches!(f.primitives[0].kind, PrimKind::ColorMatrix(_)));
    // A matrix with the wrong number of values is the identity.
    assert_eq!(f.primitives[1].kind, PrimKind::PassThrough);
    let PrimKind::ComponentTransfer(t) = &f.primitives[2].kind else { panic!() };
    assert_eq!(t[0].apply(0.25), 0.75);
    assert_eq!(t[3].apply(1.0), 0.5);
    assert_eq!(Transfer::Discrete(vec![0.2, 0.8]).apply(0.6), 0.8);
    assert!((Transfer::Gamma(2.0, 2.0, 0.1).apply(0.5) - 0.6).abs() < 1e-6);
}

#[test]
fn colour_interpolation_inherits_from_the_filter() {
    let f = doc_filter("<filter id=f color-interpolation-filters=sRGB><feFlood /><feOffset color-interpolation-filters=linearRGB /></filter>");
    assert!(!f.primitives[0].linear);
    assert!(f.primitives[1].linear);
    let d = doc_filter("<filter id=f><feFlood /></filter>");
    assert!(d.primitives[0].linear);
}

#[test]
fn convolve_validation() {
    let ok = doc_filter("<filter id=f><feConvolveMatrix kernelMatrix='0 0 0 0 1 0 0 0 0' /></filter>");
    assert!(matches!(ok.primitives[0].kind, PrimKind::Convolve { order: (3, 3), target: (1, 1), .. }));
    let bad = doc_filter("<filter id=f><feConvolveMatrix order=2 kernelMatrix='1 1 1' /></filter>");
    assert_eq!(bad.primitives[0].kind, PrimKind::Transparent);
}

#[test]
fn filter_values_parse() {
    assert_eq!(parse_filter_value("url(#a)"), Some(vec![FilterItem::Url("a".into())]));
    assert_eq!(parse_filter_value("none"), Some(vec![]));
    let v = parse_filter_value("url(#a) blur(2px)").unwrap();
    assert_eq!(v.len(), 2);
    assert!(parse_filter_value("bogus(1)").is_none());
}

#[test]
fn empty_filter_renders_nothing_and_missing_refs_are_ignored() {
    let f = doc_filter("<filter id=f></filter>");
    let src = Pixels { w: 4, h: 4, px: vec![[1.0; 4]; 16] };
    let e = FilterEnv { m: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], bbox: Rect::new(0.0, 0.0, 4.0, 4.0), viewport: (4.0, 4.0), element: None, fonts: None };
    assert!(f.apply(&src, &e).px.iter().all(|c| c[3] == 0.0));
    let none = |_: &str| None;
    assert!(apply_chain(&[FilterItem::Url("x".into())], &none, src, &e, 1.0).is_none());
}

#[test]
fn mask_scales_by_luminance() {
    let doc = rabbithole_html::parse(
        "<!doctype html><body><svg width=20 height=10><mask id=m maskUnits=userSpaceOnUse x=0 y=0 width=10 height=10><rect width=20 height=10 fill=white /></mask>\
         <rect width=20 height=10 fill=blue mask='url(#m)' /></svg>",
    );
    let svg = doc.find_element("svg").unwrap();
    let tree = crate::SvgTree::from_dom(&doc, svg, &|_, _| None).unwrap();
    let fonts = rabbithole_gfx::FontContext::new();
    let img = tree.rasterize(&fonts, 20, 10);
    let px = |x: usize| &img.rgba[(5 * 20 + x) * 4..(5 * 20 + x) * 4 + 4];
    // Inside the mask region: blue; outside: transparent.
    assert_eq!(px(2), &[0, 0, 255, 255]);
    assert_eq!(px(15)[3], 0);
}

#[test]
fn svg_element_filter_floods_its_region() {
    let doc = rabbithole_html::parse(
        "<!doctype html><body><svg width=20 height=20><filter id=f x=0 y=0 width=1 height=1 color-interpolation-filters=sRGB><feFlood flood-color=lime /></filter>\
         <rect x=5 y=5 width=10 height=10 fill=red filter='url(#f)' /></svg>",
    );
    let svg = doc.find_element("svg").unwrap();
    let tree = crate::SvgTree::from_dom(&doc, svg, &|_, _| None).unwrap();
    assert!(tree.has_filters());
    let fonts = rabbithole_gfx::FontContext::new();
    let img = tree.rasterize(&fonts, 20, 20);
    let px = |x: usize, y: usize| &img.rgba[(y * 20 + x) * 4..(y * 20 + x) * 4 + 4];
    assert_eq!(px(10, 10), &[0, 255, 0, 255]);
    assert_eq!(px(2, 2)[3], 0);
}

#[test]
fn data_url_filters_resolve() {
    let url = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg'%3E%3Cfilter id='g'%3E%3CfeFlood/%3E%3C/filter%3E%3C/svg%3E#g";
    let (def, _) = crate::SvgTree::data_url_filter(url).expect("filter");
    assert_eq!(def.primitives.len(), 1);
    assert!(crate::SvgTree::data_url_filter("data:text/plain,hi#g").is_none());
    let v = parse_filter_value("url(\"data:image/svg+xml;base64,AAAA#g\")").unwrap();
    assert!(matches!(&v[0], FilterItem::Url(u) if u.starts_with("data:")));
}
