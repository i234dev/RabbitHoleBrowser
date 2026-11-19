//! Tests for the extended property coverage, pseudo-elements, dynamic
//! state and `@font-face` collection.

use crate::computed::*;
use crate::{style_tree, style_tree_with, FontFace, MatchState, MediaEnv, PseudoElement, StyleOptions, StyledTree};
use rabbithole_css::parse_stylesheet;
use rabbithole_dom::Document;

fn env() -> MediaEnv {
    MediaEnv { width: 1000.0, height: 800.0, screen: true, scripting: true }
}

fn styled(html: &str, css: &str) -> (Document, StyledTree) {
    let doc = rabbithole_html::parse(html);
    let sheets = vec![parse_stylesheet(css)];
    let tree = style_tree(&doc, &sheets, &env());
    (doc, tree)
}

fn style_of<'a>(doc: &Document, tree: &'a StyledTree, tag: &str) -> &'a ComputedStyle {
    let n = doc.find_element(tag).expect("element");
    tree.get(n).expect("styled")
}

fn one(css_decls: &str) -> ComputedStyle {
    let (doc, t) = styled("<div>x</div>", &format!("div {{ {css_decls} }}"));
    style_of(&doc, &t, "div").clone()
}

#[test]
fn flex_properties() {
    let s = one("display: flex; flex: 1; flex-wrap: wrap; justify-content: space-between; align-items: center; gap: 10px 20px; order: 3");
    assert_eq!(s.display, Display::Flex);
    assert_eq!((s.flex_grow, s.flex_shrink, s.flex_basis), (1.0, 1.0, Length::Percent(0.0)));
    assert_eq!(s.flex_wrap, FlexWrap::Wrap);
    assert_eq!(s.justify_content, ContentDistribution::SpaceBetween);
    assert_eq!(s.align_items, ItemAlign::Center);
    assert_eq!((s.row_gap, s.column_gap), (Length::Px(10.0), Length::Px(20.0)));
    assert_eq!(s.order, 3);
    let s = one("flex: 2 0 100px");
    assert_eq!((s.flex_grow, s.flex_shrink, s.flex_basis), (2.0, 0.0, Length::Px(100.0)));
    let s = one("flex: none");
    assert_eq!((s.flex_grow, s.flex_shrink, s.flex_basis), (0.0, 0.0, Length::Auto));
    let s = one("flex: auto; align-self: flex-end; place-items: center start");
    assert_eq!((s.flex_grow, s.flex_shrink, s.flex_basis), (1.0, 1.0, Length::Auto));
    assert_eq!(s.align_self, ItemAlign::FlexEnd);
    assert_eq!((s.align_items, s.justify_items), (ItemAlign::Center, ItemAlign::FlexStart));
    let s = one("display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 3; -webkit-flex: 1");
    // A clamped vertical -webkit-box is laid out as a block of lines.
    assert_eq!(s.display, Display::FlowRoot);
    assert_eq!(s.flex_direction, FlexDirection::Column);
    assert_eq!(s.line_clamp, Some(3));
    assert_eq!(s.flex_grow, 1.0);
}

#[test]
fn grid_properties() {
    let s = one("display: grid; grid-template-columns: [full-start] repeat(2, 1fr) minmax(100px, auto) [full-end]; grid-template-rows: 50px auto; gap: 8px");
    assert_eq!(s.display, Display::Grid);
    assert_eq!(s.grid_template_columns.len(), 4, "{:?}", s.grid_template_columns);
    match &s.grid_template_columns[0] {
        TrackListItem::Names(n) => assert_eq!(n, &vec!["full-start".to_string()]),
        other => panic!("{other:?}"),
    }
    match &s.grid_template_columns[1] {
        TrackListItem::Repeat { count: Some(2), auto_fit: false, tracks } => {
            assert_eq!(tracks.len(), 1);
            assert!(matches!(tracks[0], TrackListItem::Track { size: TrackSize::Fr(f), .. } if f == 1.0));
        }
        other => panic!("{other:?}"),
    }
    match &s.grid_template_columns[2] {
        TrackListItem::Track { names, size: TrackSize::MinMax(a, b) } => {
            assert!(names.is_empty());
            assert_eq!(**a, TrackSize::Length(Length::Px(100.0)));
            assert_eq!(**b, TrackSize::Length(Length::Auto));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(s.grid_template_columns[3], TrackListItem::Names(vec!["full-end".to_string()]));
    assert_eq!(s.grid_template_rows.len(), 2);
    assert_eq!(s.row_gap, Length::Px(8.0));

    let s = one("grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); grid-auto-rows: 100px; grid-auto-flow: column dense");
    assert!(matches!(&s.grid_template_columns[0], TrackListItem::Repeat { count: None, auto_fit: false, .. }));
    assert_eq!(s.grid_auto_rows, vec![TrackSize::Length(Length::Px(100.0))]);
    assert_eq!(s.grid_auto_flow, GridAutoFlow::ColumnDense);

    let s = one("grid-column: 1 / span 2; grid-row: 2 / -1; grid-template-areas: 'a b' 'c d'");
    assert_eq!(s.grid_column_start, GridLine::Line(1));
    assert_eq!(s.grid_column_end, GridLine::Span(2));
    assert_eq!(s.grid_row_start, GridLine::Line(2));
    assert_eq!(s.grid_row_end, GridLine::Line(-1));
    assert_eq!(s.grid_template_areas, vec![vec!["a".to_string(), "b".to_string()], vec!["c".to_string(), "d".to_string()]]);

    let s = one("grid-area: header");
    assert_eq!(s.grid_row_start, GridLine::Named("header".into(), false));
    assert_eq!(s.grid_column_end, GridLine::Named("header".into(), false));

    let s = one("grid-template: 'h h' 40px 'm s' 1fr / 2fr 1fr");
    assert_eq!(s.grid_template_areas.len(), 2);
    assert_eq!(s.grid_template_rows.len(), 2);
    assert!(matches!(&s.grid_template_rows[0], TrackListItem::Track { size: TrackSize::Length(Length::Px(40.0)), .. }));
    assert_eq!(s.grid_template_columns.len(), 2);
}

#[test]
fn background_layers_and_gradients() {
    let s = one("background: url(a.png) no-repeat center / cover, linear-gradient(to right, red, blue 50%) repeat-x, #333");
    assert_eq!(s.background_color, Color::rgb(0x33, 0x33, 0x33));
    assert_eq!(s.background_layers.len(), 2);
    let l0 = &s.background_layers[0];
    assert_eq!(l0.image, BackgroundImage::Url("a.png".into()));
    assert_eq!(l0.repeat, (BackgroundRepeat::NoRepeat, BackgroundRepeat::NoRepeat));
    assert_eq!(l0.position, (Length::Percent(50.0), Length::Percent(50.0)));
    assert_eq!(l0.size, BackgroundSize::Cover);
    let l1 = &s.background_layers[1];
    match &l1.image {
        BackgroundImage::LinearGradient { direction, stops, repeating, .. } => {
            assert_eq!(*direction, GradientDirection::Corner(1, 0));
            assert!(!repeating);
            assert_eq!(stops.len(), 2);
            assert_eq!(stops[0].color, Color::rgb(255, 0, 0));
            assert_eq!(stops[1].position, Some(Length::Percent(50.0)));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(l1.repeat, (BackgroundRepeat::Repeat, BackgroundRepeat::NoRepeat));
    assert_eq!(s.background_image.as_deref(), Some("a.png"));

    let s = one("background-image: linear-gradient(45deg, #000 0%, #fff 100%), radial-gradient(circle at top left, red, transparent)");
    assert_eq!(s.background_layers.len(), 2);
    match &s.background_layers[0].image {
        BackgroundImage::LinearGradient { direction: GradientDirection::Angle(a), .. } => assert!((a - 45f32.to_radians()).abs() < 1e-5),
        other => panic!("{other:?}"),
    }
    match &s.background_layers[1].image {
        BackgroundImage::RadialGradient { shape, position, .. } => {
            assert_eq!(*shape, RadialShape::Circle);
            assert_eq!(*position, (Length::Percent(0.0), Length::Percent(0.0)));
        }
        other => panic!("{other:?}"),
    }

    // longhands cycle over the layers defined by background-image
    let s = one("background-image: url(a.png), url(b.png); background-size: 10px 20px; background-position: right bottom; background-attachment: fixed; background-clip: content-box");
    assert_eq!(s.background_layers.len(), 2);
    for l in &s.background_layers {
        assert_eq!(l.size, BackgroundSize::Explicit(Length::Px(10.0), Length::Px(20.0)));
        assert_eq!(l.position, (Length::Percent(100.0), Length::Percent(100.0)));
        assert!(l.fixed);
        assert_eq!(l.clip, BoxArea::ContentBox);
    }
    let s = one("background: red");
    assert_eq!(s.background_color, Color::rgb(255, 0, 0));
    assert!(s.background_layers.is_empty());
}

#[test]
fn border_radius_and_resolved_radii() {
    let s = one("border-radius: 10px 20px / 5px");
    assert_eq!(s.border_radii[0], (Length::Px(10.0), Length::Px(5.0)));
    assert_eq!(s.border_radii[1], (Length::Px(20.0), Length::Px(5.0)));
    assert_eq!(s.border_radii[2], (Length::Px(10.0), Length::Px(5.0)));
    assert_eq!(s.border_radii[3], (Length::Px(20.0), Length::Px(5.0)));
    assert_eq!(s.border_radius, 20.0);
    let s = one("border-radius: 50%; border-top-left-radius: 4px 8px");
    assert_eq!(s.border_radii[0], (Length::Px(4.0), Length::Px(8.0)));
    assert_eq!(s.border_radii[1], (Length::Percent(50.0), Length::Percent(50.0)));
    let r = s.resolved_radii(100.0, 40.0);
    // 50% of 100 = 50 horizontally on two corners → sum 100 = width, fine;
    // vertically 50% of 40 = 20 per corner: 20+20 = 40 fits.
    assert!((r[1].0 - 50.0).abs() < 0.01 && (r[1].1 - 20.0).abs() < 0.01);
    let s = one("border-radius: 100px");
    let r = s.resolved_radii(50.0, 50.0);
    assert!((r[0].0 - 25.0).abs() < 0.01, "{r:?}");
}

#[test]
fn transforms_and_matrix() {
    let s = one("transform: translate(10px, 20%) rotate(45deg) scale(2); transform-origin: left top");
    assert_eq!(s.transform.len(), 3);
    assert_eq!(s.transform[0], TransformOp::Translate(Length::Px(10.0), Length::Percent(20.0)));
    assert_eq!(s.transform_origin, (Length::Percent(0.0), Length::Percent(0.0)));
    let m = transform_matrix(&s.transform, 100.0, 50.0, 16.0);
    let c = 45f32.to_radians().cos() * 2.0;
    let sn = 45f32.to_radians().sin() * 2.0;
    assert!((m[0] - c).abs() < 1e-4 && (m[1] - sn).abs() < 1e-4 && (m[2] + sn).abs() < 1e-4 && (m[3] - c).abs() < 1e-4);
    assert!((m[4] - 10.0).abs() < 1e-4 && (m[5] - 10.0).abs() < 1e-4);
    assert!(s.is_stacking_context());
    let s = one("-webkit-transform: translateX(5px); transform: none");
    assert!(s.transform.is_empty());
    let s = one("transform: matrix(1, 0, 0, 1, 30, 40)");
    assert_eq!(s.transform[0], TransformOp::Matrix([1.0, 0.0, 0.0, 1.0, 30.0, 40.0]));
    // rotate3d() about the z axis is rotate(); skewX()/skewY() stay distinct.
    let s = one("transform: rotate3d(0, 0, -1, 90deg) skewX(10deg) skewY(20deg)");
    assert!(matches!(s.transform[0], TransformOp::Rotate(a) if (a + std::f32::consts::FRAC_PI_2).abs() < 1e-5), "{:?}", s.transform);
    assert!(matches!(s.transform[1], TransformOp::SkewX(a) if (a - 10f32.to_radians()).abs() < 1e-5), "{:?}", s.transform);
    assert!(matches!(s.transform[2], TransformOp::SkewY(a) if (a - 20f32.to_radians()).abs() < 1e-5), "{:?}", s.transform);
}

#[test]
fn shadows_outline_filter() {
    let s = one("box-shadow: 0 2px 4px rgba(0,0,0,.5), inset 0 0 0 1px red; text-shadow: 1px 1px 2px black; outline: 2px dashed blue; outline-offset: 3px; opacity: 0.8; filter: opacity(0.5) blur(2px)");
    assert_eq!(s.box_shadow.len(), 2);
    assert_eq!(s.box_shadow[0].blur, 4.0);
    assert_eq!(s.box_shadow[0].color, Color::rgba(0, 0, 0, 128));
    assert!(s.box_shadow[1].inset && s.box_shadow[1].spread == 1.0);
    assert_eq!(s.text_shadow.len(), 1);
    assert_eq!(s.outline_width, 2.0);
    assert_eq!(s.outline_style, BorderStyle::Dashed);
    assert_eq!(s.outline_color, Some(Color::rgb(0, 0, 255)));
    assert_eq!(s.outline_offset, 3.0);
    // opacity() among other filter functions stays in the filter chain.
    assert!((s.opacity - 0.8).abs() < 1e-5);
    assert_eq!(s.filters, vec![FilterOp::Opacity(0.5), FilterOp::Blur(2.0)]);
    // A lone opacity() is folded into opacity.
    let s = one("opacity: 0.8; filter: opacity(0.5)");
    assert!((s.opacity - 0.4).abs() < 1e-5 && s.filters.is_empty());
    let s = one("outline: none; box-shadow: none");
    assert_eq!(s.outline_style, BorderStyle::None);
    assert!(s.box_shadow.is_empty());
}

#[test]
fn text_properties() {
    let s = one("text-overflow: ellipsis; word-break: break-all; word-wrap: break-word; text-decoration: underline dotted red 2px; direction: rtl; tab-size: 4; font-variant: small-caps");
    assert_eq!(s.text_overflow, TextOverflow::Ellipsis);
    assert_eq!(s.word_break, WordBreak::BreakAll);
    assert_eq!(s.overflow_wrap, OverflowWrap::BreakWord);
    assert!(s.text_decoration.underline);
    assert_eq!(s.text_decoration_style, TextDecorationStyle::Dotted);
    assert_eq!(s.text_decoration_color, Some(Color::rgb(255, 0, 0)));
    assert_eq!(s.text_decoration_thickness, Some(2.0));
    assert_eq!(s.direction, Direction::Rtl);
    assert_eq!(s.tab_size, 4.0);
    assert!(s.font_variant_small_caps);
    let s = one("text-decoration-line: line-through; text-decoration-color: blue");
    assert!(s.text_decoration.line_through && !s.text_decoration.underline);
    assert_eq!(s.text_decoration_color, Some(Color::rgb(0, 0, 255)));
}

#[test]
fn font_family_list_order() {
    let s = one("font-family: 'Fira Sans', Helvetica Neue, Arial, sans-serif");
    assert_eq!(s.font_family_list, vec!["Fira Sans".to_string(), "Helvetica Neue".to_string(), "Arial".to_string()]);
    assert_eq!(s.font_family, Family::Sans);
    let s = one("font: italic small-caps bold 14px/1.4 Georgia, 'Times New Roman', serif");
    assert_eq!(s.font_family_list, vec!["Georgia".to_string(), "Times New Roman".to_string()]);
    assert_eq!(s.font_family, Family::Serif);
    assert!(s.font_variant_small_caps);
    assert_eq!(s.font_size, 14.0);
    // inherited into children
    let (doc, t) = styled("<div><span>x</span></div>", "div { font-family: Roboto, sans-serif }");
    assert_eq!(style_of(&doc, &t, "span").font_family_list, vec!["Roboto".to_string()]);
}

#[test]
fn sizing_keywords_and_replaced() {
    let s = one("width: min-content; min-height: max-content; max-width: fit-content(200px); object-fit: cover; object-position: 20% bottom; aspect-ratio: 16 / 9");
    assert_eq!(s.width, Length::MinContent);
    assert_eq!(s.min_height, Length::MaxContent);
    // fit-content(<length>) keeps its argument for layout.
    assert_eq!(fit_content_arg(s.max_width).map(|a| a.eval(0.0)), Some(200.0));
    let s2 = one("height: stretch; min-width: -webkit-fill-available");
    assert!(is_stretch(s2.height) && is_stretch(s2.min_width));
    assert_eq!(s.object_fit, ObjectFit::Cover);
    assert_eq!(s.object_position, (Length::Percent(20.0), Length::Percent(100.0)));
    assert!((s.aspect_ratio.unwrap() - 16.0 / 9.0).abs() < 1e-5);
    let s = one("inline-size: 50%; inset-inline: 1px 2px");
    assert_eq!(s.width, Length::Percent(50.0));
    assert_eq!((s.inset.left, s.inset.right), (Length::Px(1.0), Length::Px(2.0)));
}

#[test]
fn content_and_counters() {
    let (doc, t) = styled(
        "<ol><li data-x='v'>a</li></ol><p>b</p><q>c</q>",
        "li::before { content: counter(item, upper-roman) '. ' attr(data-x) open-quote url(i.png); counter-increment: item 2 } ol { counter-reset: item 5 } p::before { } p::after { content: none } li { quotes: '«' '»' }",
    );
    let li = doc.find_element("li").unwrap();
    let before = t.pseudo(li, PseudoElement::Before).expect("::before generated");
    match &before.content {
        Content::Items(items) => {
            assert_eq!(items.len(), 5);
            assert_eq!(items[0], ContentItem::Counter("item".into(), ListStyleType::UpperRoman));
            assert_eq!(items[1], ContentItem::String(". ".into()));
            assert_eq!(items[2], ContentItem::Attr("data-x".into()));
            assert_eq!(items[3], ContentItem::OpenQuote);
            assert_eq!(items[4], ContentItem::Url("i.png".into()));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(before.counter_increment, vec![("item".to_string(), 2)]);
    // pseudo inherits from the originating element
    assert_eq!(before.quotes, vec![("«".to_string(), "»".to_string())]);
    assert_eq!(before.display, Display::Inline);
    let ol = doc.find_element("ol").unwrap();
    assert_eq!(t.get(ol).unwrap().counter_reset, vec![("item".to_string(), 5)]);
    let p = doc.find_element("p").unwrap();
    assert!(t.pseudo(p, PseudoElement::Before).is_none(), "no content → no box");
    assert!(t.pseudo(p, PseudoElement::After).is_none(), "content: none → no box");
    // UA: q::before gets open-quote
    let q = doc.find_element("q").unwrap();
    let qb = t.pseudo(q, PseudoElement::Before).expect("q::before");
    assert_eq!(qb.content, Content::Items(vec![ContentItem::OpenQuote]));
    // `content` is ignored on elements
    let (doc, t) = styled("<div>x</div>", "div { content: 'nope' }");
    assert_eq!(style_of(&doc, &t, "div").content, Content::Normal);
}

#[test]
fn pseudo_element_specificity_and_important() {
    let (doc, t) = styled(
        "<p class=c>x</p>",
        "p::before { content: 'a'; color: red } .c::before { color: blue } p::before { color: green !important } p::marker { color: purple }",
    );
    let p = doc.find_element("p").unwrap();
    let b = t.pseudo(p, PseudoElement::Before).unwrap();
    assert_eq!(b.color, Color::rgb(0, 128, 0));
    assert_eq!(b.content, Content::Items(vec![ContentItem::String("a".into())]));
    let m = t.pseudo(p, PseudoElement::Marker).unwrap();
    assert_eq!(m.color, Color::rgb(128, 0, 128));
    assert!(t.pseudo(p, PseudoElement::After).is_none());
}

#[test]
fn hover_state_matching() {
    let doc = rabbithole_html::parse("<div><a href=x><span>l</span></a><b>o</b></div>");
    let sheets = vec![parse_stylesheet("a:hover { color: red } a:hover span { color: blue } div:hover > b { color: green } #t:target { color: purple }")];
    let span = doc.find_element("span").unwrap();
    let a = doc.find_element("a").unwrap();
    let b = doc.find_element("b").unwrap();
    let state = MatchState { hovered: Some(span), ..Default::default() };
    let opts = StyleOptions { state: &state, resolve_url: &|_, u| Some(u.to_string()), document_base: "" };
    let t = style_tree_with(&doc, &sheets, &env(), &opts);
    assert_eq!(t.get(a).unwrap().color, Color::rgb(255, 0, 0));
    assert_eq!(t.get(span).unwrap().color, Color::rgb(0, 0, 255));
    assert_eq!(t.get(b).unwrap().color, Color::rgb(0, 128, 0));
    let plain = style_tree(&doc, &sheets, &env());
    assert_eq!(plain.get(a).unwrap().color, Color::rgb(0, 0, 0xee));
}

#[test]
fn font_stretch_property_and_shorthand() {
    let (doc, t) = styled(
        "<p>x</p><span>y</span><b>z</b><i>w</i>",
        "p { font-stretch: semi-expanded } span { font-width: 60% } b { font: condensed bold 12px serif } i { font-stretch: 75%; font: 10px serif } body { font-stretch: expanded }",
    );
    assert_eq!(style_of(&doc, &t, "p").font_stretch, 112);
    assert_eq!(style_of(&doc, &t, "span").font_stretch, 60);
    assert_eq!(style_of(&doc, &t, "b").font_stretch, 75);
    assert_eq!(style_of(&doc, &t, "b").font_weight, 700);
    assert_eq!(style_of(&doc, &t, "i").font_stretch, 100, "the shorthand resets the width");
    assert_eq!(style_of(&doc, &t, "body").font_stretch, 125);
}

#[test]
fn font_faces_collected_and_resolved() {
    let doc = rabbithole_html::parse("<p>x</p>");
    let mut sheet = parse_stylesheet(
        "@font-face { font-family: 'My Font'; src: local('x'), url(fonts/my.woff2) format('woff2'), url('my.woff') format(woff); font-weight: 300 700; font-style: italic; font-stretch: 125% condensed; font-display: swap; unicode-range: U+0000-00FF, U+4??; } \
         @media (min-width: 10px) { @font-face { font-family: Other; src: url(o.ttf) } } @keyframes spin { from { transform: rotate(0) } to { transform: rotate(360deg) } }",
    );
    sheet.base_url = Some("https://cdn.example/css/site.css".into());
    let state = MatchState::default();
    let resolve = |base: &str, raw: &str| -> Option<String> { Some(format!("{}/{raw}", base.rsplit_once('/').map(|(d, _)| d).unwrap_or(base))) };
    let opts = StyleOptions { state: &state, resolve_url: &resolve, document_base: "https://example.com/page" };
    let t = style_tree_with(&doc, &[sheet], &env(), &opts);
    assert_eq!(t.font_faces.len(), 2);
    let f: &FontFace = &t.font_faces[0];
    assert_eq!(f.family, "My Font");
    assert_eq!(f.sources.len(), 2, "local() skipped");
    assert_eq!(f.sources[0].url, "https://cdn.example/css/fonts/my.woff2");
    assert_eq!(f.sources[0].format.as_deref(), Some("woff2"));
    assert_eq!(f.weight, (300, 700));
    assert!(f.italic);
    assert_eq!(f.stretch, (75, 125));
    assert_eq!(f.display, "swap");
    assert_eq!(f.unicode_range, Some(vec![(0, 0xff), (0x400, 0x4ff)]));
    assert_eq!(t.font_faces[1].family, "Other");
    assert_eq!(t.font_faces[1].stretch, (100, 100));
    assert_eq!(t.keyframes.get("spin").map(|f| f.len()), Some(2));
    assert_eq!(t.root_font_size, 16.0);
}

#[test]
fn urls_resolved_against_sheet_base() {
    let doc = rabbithole_html::parse("<div>x</div>");
    let mut sheet = parse_stylesheet("div { background: url(img/bg.png) }");
    sheet.base_url = Some("https://cdn.example/css/a.css".into());
    let state = MatchState::default();
    let resolve = |base: &str, raw: &str| -> Option<String> { Some(format!("{}/{raw}", base.rsplit_once('/').map(|(d, _)| d).unwrap_or(base))) };
    let opts = StyleOptions { state: &state, resolve_url: &resolve, document_base: "https://example.com/" };
    let t = style_tree_with(&doc, &[sheet], &env(), &opts);
    let d = t.get(doc.find_element("div").unwrap()).unwrap();
    assert_eq!(d.background_image.as_deref(), Some("https://cdn.example/css/img/bg.png"));
}

#[test]
fn var_in_new_properties_and_wide_keywords() {
    let (doc, t) = styled(
        "<div><p>x</p></div>",
        ":root { --g: 12px; --c: red } div { gap: var(--g); box-shadow: 0 0 2px var(--c); border-collapse: collapse } p { gap: inherit; border-collapse: initial; flex-grow: 2; flex: initial }",
    );
    let d = style_of(&doc, &t, "div");
    assert_eq!(d.row_gap, Length::Px(12.0));
    assert_eq!(d.box_shadow[0].color, Color::rgb(255, 0, 0));
    assert!(d.border_collapse);
    let p = style_of(&doc, &t, "p");
    assert_eq!(p.row_gap, Length::Px(12.0));
    assert!(!p.border_collapse);
    assert_eq!(p.flex_grow, 0.0);
    // border-spacing inherits
    let (doc, t) = styled("<table><tr><td>x</td></tr></table>", "table { border-spacing: 4px 6px }");
    assert_eq!(style_of(&doc, &t, "td").border_spacing, (4.0, 6.0));
}

/// Custom property names are case-sensitive and must keep their case
/// (google.com's `:root{--xhUGwc:#fff}` backgrounds were transparent).
#[test]
fn custom_property_names_keep_case() {
    let (doc, t) = styled(
        "<div>x</div><p>y</p>",
        ":root { --xhUGwc: #fff; --xhugwc: #000 } div { background-color: var(--xhUGwc) } p { background-color: var(--xhugwc) }",
    );
    assert_eq!(style_of(&doc, &t, "div").background_color, Color::rgb(255, 255, 255));
    assert_eq!(style_of(&doc, &t, "p").background_color, Color::rgb(0, 0, 0));
}

#[test]
fn transitions_animations_misc() {
    let s = one("transition: opacity .3s ease-in 1s, transform 200ms; animation: spin 2s linear infinite alternate; pointer-events: none; user-select: none; appearance: none; resize: vertical; scroll-margin-top: 10px");
    assert_eq!(s.transitions.len(), 2);
    assert_eq!(s.transitions[0].property, "opacity");
    assert!((s.transitions[0].duration - 0.3).abs() < 1e-6 && (s.transitions[0].delay - 1.0).abs() < 1e-6);
    assert_eq!(s.transitions[0].timing, "ease-in");
    assert!((s.transitions[1].duration - 0.2).abs() < 1e-6);
    assert_eq!(s.animations.len(), 1);
    assert_eq!(s.animations[0].name, "spin");
    // Keyframes names keep their case in the shorthand.
    assert_eq!(one("animation: moveAnimation 2s").animations[0].name, "moveAnimation");
    assert_eq!(s.animations[0].iterations, None);
    assert_eq!(s.animations[0].direction, "alternate");
    assert_eq!(s.pointer_events, PointerEvents::None);
    assert_eq!(s.user_select, UserSelect::None);
    assert_eq!(s.appearance, Appearance::None);
    assert_eq!(s.resize, Resize::Vertical);
    assert_eq!(s.scroll_margin_top, 10.0);
    let s = one("transition-property: color, background-color; transition-duration: 1s");
    assert_eq!(s.transitions.len(), 2);
    assert_eq!(s.transitions[1].property, "background-color");
    assert_eq!(s.transitions[1].duration, 1.0);
}

#[test]
fn ua_form_controls_and_placeholder() {
    let ua = parse_stylesheet(crate::ua::UA_CSS);
    let mut rules = Vec::new();
    ua.active_rules(&env(), &mut rules);
    let placeholder = rules.iter().any(|r| {
        r.selectors.iter().any(|s| s.pseudo_element == Some(PseudoElement::Placeholder)) && r.declarations.iter().any(|d| d.name == "color")
    });
    assert!(placeholder, "UA sheet styles ::placeholder");
    let (doc, t) = styled("<input type=checkbox><button>b</button><textarea cols=20 rows=3></textarea><input size=10 id=s>", "");
    let cb = style_of(&doc, &t, "input");
    assert_eq!(cb.width, Length::Px(13.0));
    assert_eq!(cb.appearance, Appearance::Auto);
    let b = style_of(&doc, &t, "button");
    assert_eq!(b.display, Display::InlineBlock);
    assert!(b.cursor_pointer());
    let (cdoc, ct) = styled("<p></p><span></span><em></em>", "p { cursor: url(x.cur), nw-resize } span { cursor: help; cursor: bogus } em { cursor: default }");
    assert_eq!(style_of(&cdoc, &ct, "p").cursor, "nw-resize");
    assert_eq!(style_of(&cdoc, &ct, "span").cursor, "help");
    assert_eq!(style_of(&cdoc, &ct, "em").cursor, "default");
    // Square in CSS: the 2px rounding of the native look is painted by
    // layout while the UA colors are untouched (appearance: none and
    // author-styled buttons are square, like in Blink).
    assert_eq!(b.border_radius, 0.0);
    let ta = style_of(&doc, &t, "textarea");
    assert_eq!(ta.white_space, WhiteSpace::PreWrap);
    assert_eq!(ta.resize, Resize::Both);
    assert!(matches!(ta.width, Length::Px(w) if w > 100.0), "{:?}", ta.width);
    let sized = doc.element_by_id("s").unwrap();
    assert!(matches!(t.get(sized).unwrap().width, Length::Px(w) if w > 60.0 && w < 80.0));
}

#[test]
fn presentational_extras() {
    let (doc, t) = styled(
        "<body link=red background=bg.gif><a href=x>a</a><table border=1 cellspacing=0><tr><td>c</td></tr></table><img align=left src=i><ol type=A><li>x</li></ol><hr color=blue size=3></body>",
        "",
    );
    assert_eq!(style_of(&doc, &t, "a").color, Color::rgb(255, 0, 0));
    assert_eq!(style_of(&doc, &t, "body").background_image.as_deref(), Some("bg.gif"));
    let td = style_of(&doc, &t, "td");
    assert_eq!(td.border_width.top, 1.0);
    assert_eq!(style_of(&doc, &t, "table").border_spacing, (0.0, 0.0));
    assert_eq!(style_of(&doc, &t, "img").float, Float::Left);
    assert_eq!(style_of(&doc, &t, "ol").list_style_type, ListStyleType::UpperAlpha);
    let hr = style_of(&doc, &t, "hr");
    assert_eq!(hr.border_color.top, Color::rgb(0, 0, 255));
    assert_eq!(hr.height, Length::Px(3.0));
}

#[test]
fn stacking_context_detection() {
    assert!(one("opacity: .5").is_stacking_context());
    assert!(one("position: relative; z-index: 1").is_stacking_context());
    assert!(!one("position: relative").is_stacking_context());
    assert!(one("isolation: isolate").is_stacking_context());
    assert!(one("will-change: transform").is_stacking_context());
    assert!(one("position: fixed").is_stacking_context());
}

#[test]
fn wikipedia_grid_template_shorthand_and_areas() {
    let html = r#"<div class="inner"><div class="notice">n</div><div class="content">c</div></div>"#;
    let css = ".inner{display:grid;column-gap:24px;grid-template:min-content 1fr min-content / 12.25rem minmax(0,1fr);grid-template-areas:'siteNotice siteNotice' 'columnStart pageContent' 'footer footer'} .content{grid-area:pageContent} .notice{grid-area:siteNotice}";
    let doc = rabbithole_html::parse(html);
    let sheet = rabbithole_css::parse_stylesheet(css);
    let env = MediaEnv { width: 1280.0, height: 800.0, screen: true, scripting: true };
    let t = style_tree(&doc, &[sheet], &env);
    let inner = doc.find_element("div").unwrap();
    let s = t.get(inner).unwrap();
    eprintln!("rows={:?}\ncols={:?}\nareas={:?}", s.grid_template_rows, s.grid_template_columns, s.grid_template_areas);
    assert_eq!(s.grid_template_areas.len(), 3);
    assert_eq!(s.grid_template_rows.len(), 3, "rows: {:?}", s.grid_template_rows);
    assert_eq!(s.grid_template_columns.len(), 2, "cols: {:?}", s.grid_template_columns);
    let content = doc.descendants(0).find(|&n| doc.element(n).map(|e| e.has_class("content")).unwrap_or(false)).unwrap();
    let cs = t.get(content).unwrap();
    eprintln!("content: rs={:?} re={:?} cs={:?} ce={:?}", cs.grid_row_start, cs.grid_row_end, cs.grid_column_start, cs.grid_column_end);
    assert_eq!(cs.grid_row_start, GridLine::Named("pageContent".into(), false));
    assert_eq!(cs.grid_column_end, GridLine::Named("pageContent".into(), false));
}

#[test]
fn image_set_picks_supported_1x() {
    let s = one("background-image: image-set(url(a.jxl) type('image/jxl') 1x, url(b.avif) type('image/avif') 1x, url(c.png) 2x)");
    assert_eq!(s.background_layers[0].image, BackgroundImage::Url("b.avif".into()));
    let s = one("background-image: -webkit-image-set(url(hi.png) 2x, url(lo.png) 1x)");
    assert_eq!(s.background_layers[0].image, BackgroundImage::Url("lo.png".into()));
}

#[test]
fn shadow_tree_scoping_host_and_slotted() {
    use rabbithole_gfx::Color;
    let mut doc = rabbithole_html::parse("<html><body><x-card id=host><span class=t slot=title>T</span><em>body</em></x-card><p class=inner>doc p</p></body></html>");
    let host = doc.element_by_id("host").unwrap();
    let root = doc.attach_shadow(host);
    let h1 = doc.create_element("h1", vec![]);
    let slot_t = doc.create_element("slot", vec![("name".into(), "title".into())]);
    let p = doc.create_element("p", vec![("class".into(), "inner".into())]);
    let slot_d = doc.create_element("slot", vec![]);
    doc.append_child(root, h1);
    doc.append_child(h1, slot_t);
    doc.append_child(root, p);
    doc.append_child(root, slot_d);
    let doc_sheet = parse_stylesheet("p.inner { color: rgb(0, 0, 255) } x-card { color: rgb(0, 128, 0) } span { font-weight: 700 }");
    let mut shadow_sheet = parse_stylesheet(":host { display: block; color: rgb(255, 0, 0); padding-left: 3px } p { color: rgb(1, 2, 3) } ::slotted(em) { color: rgb(9, 9, 9) } h1 ::slotted(.t) { color: rgb(7, 7, 7) } :host(#host) h1 { margin-left: 5px }");
    shadow_sheet.shadow_scopes = vec![root];
    let state = MatchState::default();
    let opts = StyleOptions { state: &state, resolve_url: &|_, r| Some(r.to_string()), document_base: "https://t.test/" };
    let tree = style_tree_with(&doc, &[doc_sheet, shadow_sheet], &env(), &opts);
    let host_s = tree.get(host).unwrap();
    // Outer (document) normal rules beat :host rules; :host still applies.
    assert_eq!(host_s.color, Color::rgb(0, 128, 0));
    assert_eq!(host_s.display, Display::Block);
    assert_eq!(host_s.padding.left, Length::Px(3.0));
    // Document rules don't reach into the shadow tree, and vice versa.
    assert_eq!(tree.get(p).unwrap().color, Color::rgb(1, 2, 3));
    let doc_p = doc.descendants(Document::ROOT).find(|&n| doc.is_element(n, "p")).unwrap();
    assert_eq!(tree.get(doc_p).unwrap().color, Color::rgb(0, 0, 255));
    // ::slotted() with and without a slot-side compound; slotted content
    // inherits through the slot (flat tree) and keeps document rules.
    let span = doc.find_element("span").unwrap();
    let em = doc.find_element("em").unwrap();
    assert_eq!(tree.get(em).unwrap().color, Color::rgb(9, 9, 9));
    assert_eq!(tree.get(span).unwrap().color, Color::rgb(7, 7, 7));
    assert_eq!(tree.get(span).unwrap().font_weight, 700);
    // :host(...) as an ancestor compound.
    assert_eq!(tree.get(h1).unwrap().margin.left, Length::Px(5.0));
    assert_eq!(tree.get(slot_t).unwrap().display, Display::Contents);
}

#[test]
fn nesting_cascade_source_order_and_parent_specificity() {
    let (doc, tree) = styled("<div class=parent><span class=child>x</span></div>",
        ".parent, #absent { & > .child { color:green } } .parent { & {background:red} background:blue; } .parent > .child {color:red}");
    assert_eq!(style_of(&doc, &tree, "span").color, Color::rgb(0, 128, 0));
    assert_eq!(style_of(&doc, &tree, "div").background_color, Color::rgb(0, 0, 255));
}

#[test]
fn nesting_incremental_parent_class_and_dynamic_state() {
    use crate::restyle::StyleEngine;
    let mut doc = rabbithole_html::parse("<div><span>x</span></div>");
    let sheets = vec![parse_stylesheet("span {color:red} .active { > span {color:green} &:hover > span {color:blue} }")];
    let parent = doc.find_element("div").unwrap();
    let child = doc.find_element("span").unwrap();
    let env = env();
    let state = MatchState::default();
    let opts = StyleOptions { state: &state, resolve_url: &|_, s| Some(s.to_string()), document_base: "" };
    let mut engine = StyleEngine::new(&sheets, &env);
    let mut tree = engine.full(&doc, &sheets, &opts);
    assert_eq!(tree.get(child).unwrap().color, Color::rgb(255, 0, 0));
    doc.track_mutations(true);
    doc.set_attr(parent, "class", Some("active"));
    let log = doc.take_mutations();
    engine.update(&doc, &mut tree, &log, &opts, &sheets);
    assert_eq!(tree.get(child).unwrap().color, Color::rgb(0, 128, 0));
    let hovered = MatchState { hovered: Some(parent), ..Default::default() };
    let opts = StyleOptions { state: &hovered, ..opts };
    let log = doc.take_mutations();
    engine.update(&doc, &mut tree, &log, &opts, &sheets);
    assert_eq!(tree.get(child).unwrap().color, Color::rgb(0, 0, 255));
    doc.set_attr(parent, "class", None);
    let log = doc.take_mutations();
    engine.update(&doc, &mut tree, &log, &opts, &sheets);
    assert_eq!(tree.get(child).unwrap().color, Color::rgb(255, 0, 0));
}

#[test]
fn nth_of_membership_changes_restyle_forward_and_reverse_siblings() {
    use crate::restyle::StyleEngine;
    let mut doc = rabbithole_html::parse("<section class=group><i class=item id=a></i><i id=b></i><i class=item id=c></i></section>");
    let sheets = vec![parse_stylesheet("i {color:red; background:white} i:nth-child(2 of .group > .item) {color:green} i:nth-last-child(2 of .item) {background:blue}")];
    let a = doc.element_by_id("a").unwrap(); let b = doc.element_by_id("b").unwrap(); let c = doc.element_by_id("c").unwrap();
    let state = MatchState::default();
    let opts = StyleOptions { state: &state, resolve_url: &|_, s| Some(s.to_string()), document_base: "" };
    let mut engine = StyleEngine::new(&sheets, &env());
    let mut tree = engine.full(&doc, &sheets, &opts);
    assert_eq!(tree.get(c).unwrap().color, Color::rgb(0, 128, 0));
    assert_eq!(tree.get(a).unwrap().background_color, Color::rgb(0, 0, 255));
    doc.track_mutations(true);
    doc.set_attr(b, "class", Some("item"));
    let log = doc.take_mutations(); engine.update(&doc, &mut tree, &log, &opts, &sheets);
    assert_eq!(tree.get(b).unwrap().color, Color::rgb(0, 128, 0));
    assert_eq!(tree.get(c).unwrap().color, Color::rgb(255, 0, 0));
    assert_eq!(tree.get(a).unwrap().background_color, Color::rgb(255, 255, 255));
    assert_eq!(tree.get(b).unwrap().background_color, Color::rgb(0, 0, 255));
    doc.set_attr(a, "class", None);
    let log = doc.take_mutations(); engine.update(&doc, &mut tree, &log, &opts, &sheets);
    assert_eq!(tree.get(c).unwrap().color, Color::rgb(0, 128, 0));
    doc.set_attr(doc.find_element("section").unwrap(), "class", None);
    let log = doc.take_mutations(); engine.update(&doc, &mut tree, &log, &opts, &sheets);
    assert_eq!(tree.get(c).unwrap().color, Color::rgb(255, 0, 0));
}

#[test]
fn incremental_hover_removal_and_reinsertion_keep_old_ancestor_styles_current() {
    let mut doc=rabbithole_html::parse("<section id=left><div id=branch><button id=target>x</button></div><span class=marker>left</span></section><aside id=after></aside><section id=right><span class=marker>right</span></section>");
    let sheets=vec![parse_stylesheet("section{background:green}.marker,aside{color:green}section:hover{background:red}section:hover .marker{color:red}#left:hover+aside{color:red}")];
    let left=doc.element_by_id("left").unwrap();let right=doc.element_by_id("right").unwrap();
    let branch=doc.element_by_id("branch").unwrap();let target=doc.element_by_id("target").unwrap();
    let after=doc.element_by_id("after").unwrap();
    let state=MatchState{hovered:Some(target),..Default::default()};
    let opts=StyleOptions{state:&state,resolve_url:&|_,s|Some(s.to_string()),document_base:""};
    let mut engine=crate::StyleEngine::new(&sheets,&env());
    let mut tree=engine.full(&doc,&sheets,&opts);
    assert_eq!(tree.get(left).unwrap().background_color,Color::rgb(255,0,0));
    assert_eq!(tree.get(after).unwrap().color,Color::rgb(255,0,0));
    doc.track_mutations(true);
    doc.detach(branch);
    let log=doc.take_mutations();
    engine.update(&doc,&mut tree,&log,&opts,&sheets);
    assert_eq!(tree.get(left).unwrap().background_color,Color::rgb(0,128,0));
    assert_eq!(tree.get(after).unwrap().color,Color::rgb(0,128,0));
    let full=style_tree_with(&doc,&sheets,&env(),&opts);
    for node in doc.descendants(Document::ROOT).filter(|&node|doc.element(node).is_some()) {
        assert_eq!(tree.get(node),full.get(node),"after remove node={node}");
    }
    doc.append_child(right,branch);
    let log=doc.take_mutations();
    engine.update(&doc,&mut tree,&log,&opts,&sheets);
    assert_eq!(tree.get(left).unwrap().background_color,Color::rgb(0,128,0));
    assert_eq!(tree.get(right).unwrap().background_color,Color::rgb(255,0,0));
    let full=style_tree_with(&doc,&sheets,&env(),&opts);
    for node in doc.descendants(Document::ROOT).filter(|&node|doc.element(node).is_some()) {
        assert_eq!(tree.get(node),full.get(node),"after reinsert node={node}");
    }
}

#[test]
fn incremental_active_and_focus_within_reparent_without_changing_state_ids() {
    for focus in [false,true] {
        let mut doc=rabbithole_html::parse("<section id=left><input id=target></section><section id=right></section>");
        let sheets=vec![parse_stylesheet("section{background:green}section:active,section:focus-within{background:red}")];
        let left=doc.element_by_id("left").unwrap();let right=doc.element_by_id("right").unwrap();let target=doc.element_by_id("target").unwrap();
        let state=if focus {MatchState{focused:Some(target),..Default::default()}} else {MatchState{active:Some(target),..Default::default()}};
        let opts=StyleOptions{state:&state,resolve_url:&|_,s|Some(s.to_string()),document_base:""};
        let mut engine=crate::StyleEngine::new(&sheets,&env());
        let mut tree=engine.full(&doc,&sheets,&opts);
        assert_eq!(tree.get(left).unwrap().background_color,Color::rgb(255,0,0));
        doc.track_mutations(true);
        doc.append_child(right,target);
        let log=doc.take_mutations();
        engine.update(&doc,&mut tree,&log,&opts,&sheets);
        assert_eq!(tree.get(left).unwrap().background_color,Color::rgb(0,128,0),"focus={focus}");
        assert_eq!(tree.get(right).unwrap().background_color,Color::rgb(255,0,0),"focus={focus}");
        let full=style_tree_with(&doc,&sheets,&env(),&opts);
        for node in doc.descendants(Document::ROOT).filter(|&node|doc.element(node).is_some()) {
            assert_eq!(tree.get(node),full.get(node),"focus={focus} node={node}");
        }
    }
}

fn clip(css: &str) -> Option<ClipPath> {
    one(&format!("clip-path: {css}")).clip_path.map(|c| (*c).clone())
}

#[test]
fn clip_path_basic_shapes() {
    let px = Length::Px;
    let pc = Length::Percent;
    assert_eq!(clip("none"), None);
    assert!(one("clip-path: circle(10px)").creates_stacking_context);
    assert_eq!(clip("circle()"), Some(ClipPath::Shape(BasicShape::Circle { r: ShapeRadius::ClosestSide, at: (pc(50.0), pc(50.0)) }, ShapeBox::Border)));
    assert_eq!(
        clip("circle(farthest-side at right 10px top) padding-box"),
        Some(ClipPath::Shape(BasicShape::Circle { r: ShapeRadius::FarthestSide, at: (Length::Calc(-10.0, 100.0), pc(0.0)) }, ShapeBox::Padding))
    );
    assert_eq!(
        clip("content-box ellipse(20px 50% at 0 0)"),
        Some(ClipPath::Shape(BasicShape::Ellipse { rx: ShapeRadius::Len(px(20.0)), ry: ShapeRadius::Len(pc(50.0)), at: (px(0.0), px(0.0)) }, ShapeBox::Content))
    );
    let zero = (px(0.0), px(0.0));
    assert_eq!(
        clip("inset(10px 20% round 5px)"),
        Some(ClipPath::Shape(BasicShape::Inset { insets: [px(10.0), pc(20.0), px(10.0), pc(20.0)], radii: [(px(5.0), px(5.0)); 4] }, ShapeBox::Border))
    );
    assert_eq!(
        clip("rect(auto 10px 20px auto)"),
        Some(ClipPath::Shape(BasicShape::Rect { edges: [Length::Auto, px(10.0), px(20.0), Length::Auto], radii: [zero; 4] }, ShapeBox::Border))
    );
    assert_eq!(
        clip("polygon(evenodd, 0 0, 100% 0, 50% 100%)"),
        Some(ClipPath::Shape(BasicShape::Polygon { evenodd: true, points: vec![(px(0.0), px(0.0)), (pc(100.0), px(0.0)), (pc(50.0), pc(100.0))], round: 0.0 }, ShapeBox::Border))
    );
    assert!(matches!(clip("path(evenodd, 'M0 0 L10 0 L10 10 Z')"), Some(ClipPath::Shape(BasicShape::Path { evenodd: true, .. }, _))));
    assert!(matches!(
        clip("shape(from 0 0, line to 100% 0, hline by -50%, curve to 0 100% with 10px 10px, arc by 5px 5px of 10px cw large, close)"),
        Some(ClipPath::Shape(BasicShape::Shape { evenodd: false, .. }, ShapeBox::Border))
    ));
    assert_eq!(clip("margin-box"), Some(ClipPath::Box(ShapeBox::Margin)));
    assert_eq!(clip("url(#c)"), Some(ClipPath::Url("c".into())));
    assert_eq!(clip("url(other.svg#c)"), Some(ClipPath::Url(String::new())));
    // Invalid values are ignored.
    assert_eq!(clip("circle(-10px)"), None);
    assert_eq!(clip("ellipse(10px)"), None);
    assert_eq!(clip("circle(10px) border-box padding-box"), None);
    assert_eq!(clip("xywh(0 0 -1px 10px)"), None);
}

#[test]
fn basic_shape_geometry() {
    use crate::shapes::{basic_shape_path, basic_shape_rounded};
    use rabbithole_gfx::Rect;
    let rb = Rect::new(10.0, 20.0, 200.0, 100.0);
    let shape = |css: &str| match clip(css) {
        Some(ClipPath::Shape(s, _)) => s,
        other => panic!("{other:?}"),
    };
    // inset(): insets scale down when they exceed the box.
    let (r, _) = basic_shape_rounded(&shape("inset(10px 150px 20px 150px)"), rb).unwrap();
    assert_eq!((r.x, r.y, r.w, r.h), (110.0, 30.0, 0.0, 70.0));
    // Radius percentages resolve against the reference box.
    let (_, q) = basic_shape_rounded(&shape("inset(80% 0 0 round 8%)"), rb).unwrap();
    assert!((q[0].0 - 16.0).abs() < 1e-3 && (q[0].1 - 8.0).abs() < 1e-3, "{q:?}");
    // circle(): percentages of the normalized diagonal; side keywords.
    let (r, _) = basic_shape_rounded(&shape("circle(closest-side at 50px 50%)"), rb).unwrap();
    assert_eq!((r.x, r.y, r.w), (10.0, 20.0, 100.0));
    let (r, _) = basic_shape_rounded(&shape("circle(farthest-corner at 0 0)"), rb).unwrap();
    assert!((r.w / 2.0 - 200f32.hypot(100.0)).abs() < 1e-3);
    // xywh() / rect() rectangles.
    let (r, _) = basic_shape_rounded(&shape("xywh(10px 10% 50% 20px)"), rb).unwrap();
    assert_eq!((r.x, r.y, r.w, r.h), (20.0, 30.0, 100.0, 20.0));
    let (r, _) = basic_shape_rounded(&shape("rect(10px auto auto 20%)"), rb).unwrap();
    assert_eq!((r.x, r.y, r.w, r.h), (50.0, 30.0, 160.0, 90.0));
    // polygon() and path() are positioned in the box.
    let (p, rule) = basic_shape_path(&shape("polygon(evenodd, 0 0, 100% 0, 0 100%)"), rb);
    assert_eq!(rule, rabbithole_gfx::FillRule::EvenOdd);
    assert_eq!(p[1], rabbithole_gfx::PathSeg::LineTo(rabbithole_gfx::Point::new(210.0, 20.0)));
    let (p, _) = basic_shape_path(&shape("path('M5 5 L 10 5')"), rb);
    assert_eq!(p[0], rabbithole_gfx::PathSeg::MoveTo(rabbithole_gfx::Point::new(15.0, 25.0)));
    // shape(): relative commands accumulate from the current point.
    let (p, _) = basic_shape_path(&shape("shape(from 10px 10px, hline by 50%, vline to 100%, close)"), rb);
    assert_eq!(p[1], rabbithole_gfx::PathSeg::LineTo(rabbithole_gfx::Point::new(120.0, 30.0)));
    assert_eq!(p[2], rabbithole_gfx::PathSeg::LineTo(rabbithole_gfx::Point::new(120.0, 120.0)));
}

#[test]
fn legacy_clip_rect() {
    let px = Length::Px;
    assert_eq!(one("clip: rect(1px, auto, 3px, 4px)").clip_rect, Some([px(1.0), Length::Auto, px(3.0), px(4.0)]));
    assert_eq!(one("clip: rect(1px auto 3px -4px)").clip_rect, Some([px(1.0), Length::Auto, px(3.0), px(-4.0)]));
    assert_eq!(one("clip: auto").clip_rect, None);
    // Mixed separators and percentages are invalid.
    assert_eq!(one("clip: rect(1px 2px, 3px, 4px)").clip_rect, None);
    assert_eq!(one("clip: rect(1px, 2px, 3px, 10%)").clip_rect, None);
}

#[test]
fn border_shape_and_corner_shape() {
    assert!(matches!(one("border-shape: circle(50%) padding-box").border_shape.as_deref(), Some(ClipPath::Shape(BasicShape::Circle { .. }, ShapeBox::Padding))));
    assert!(matches!(one("border-shape: circle(50%)").border_shape.as_deref(), Some(ClipPath::Shape(_, ShapeBox::HalfBorder))));
    assert_eq!(one("border-shape: none").border_shape, None);
    assert_eq!(one("corner-shape: bevel").corner_shape, [CornerShape::Bevel; 4]);
    assert_eq!(
        one("corner-shape: notch scoop").corner_shape,
        [CornerShape::Notch, CornerShape::Scoop, CornerShape::Notch, CornerShape::Scoop]
    );
    assert_eq!(one("corner-shape: superellipse(0)").corner_shape, [CornerShape::Bevel; 4]);
    assert_eq!(one("corner-shape: round; corner-top-left-shape: square").corner_shape[0], CornerShape::Square);
}

#[test]
fn per_side_border_inherit_and_negative_sizes() {
    let (doc, t) = styled("<div><p>x</p></div>", "div { border-left: 7px solid rgb(0, 0, 255) } p { border-left: inherit; border-top-width: inherit }");
    let p = style_of(&doc, &t, "p");
    assert_eq!(p.border_width.left, 7.0);
    assert_eq!(p.border_style.left, BorderStyle::Solid);
    assert_eq!(p.border_color.left, rabbithole_gfx::Color::rgb(0, 0, 255));
    // The parent's top border has no style: its computed width is 0.
    assert_eq!(p.border_width.top, 0.0);

    let s = one("max-height: 10px; max-height: -1px; height: 5px; height: -5%; min-width: -2px");
    assert_eq!(s.max_height, Length::Px(10.0));
    assert_eq!(s.height, Length::Px(5.0));
    assert_eq!(s.min_width, Length::Auto);
}

#[test]
fn first_letter_style_is_computed() {
    let (doc, t) = styled("<p>Text</p>", "p::first-letter { font-size: 30px; float: left }");
    let n = doc.find_element("p").unwrap();
    let fl = t.pseudo(n, PseudoElement::FirstLetter).expect("::first-letter style");
    assert_eq!(fl.font_size, 30.0);
    assert_eq!(fl.float, Float::Left);
}

#[test]
fn monospace_alone_has_a_13px_medium() {
    let (doc, t) = styled(
        "<div><pre>a</pre><code>b</code><p>c</p><span>d</span></div><section><pre>e</pre></section>",
        "div { font-family: monospace } p { font-family: monospace, monospace } span { font-family: serif } section { font-size: 12px } code { font-size: 2em }",
    );
    assert_eq!(style_of(&doc, &t, "div").font_size, 13.0);
    assert_eq!(style_of(&doc, &t, "pre").font_size, 13.0);
    // A relative size follows the parent's used size.
    assert_eq!(style_of(&doc, &t, "code").font_size, 26.0);
    // Not the generic alone: the ordinary medium.
    assert_eq!(style_of(&doc, &t, "p").font_size, 16.0);
    assert_eq!(style_of(&doc, &t, "span").font_size, 16.0);
    // An explicit size is kept whatever the family.
    let n = doc.find_element("section").unwrap();
    let pre = doc.children(n).find(|&c| doc.is_element(c, "pre")).unwrap();
    assert_eq!(t.get(pre).unwrap().font_size, 12.0);
}

#[test]
fn font_kerning_none_is_inherited_and_reset_by_font() {
    let (doc, t) = styled("<div><p>a</p><span>b</span></div>", "div { font-kerning: none } span { font: 12px serif }");
    let kerning = |tag: &str| style_of(&doc, &t, tag).font_features.as_ref().and_then(|f| f.kerning);
    assert_eq!(kerning("div"), Some(false));
    assert_eq!(kerning("p"), Some(false));
    assert_eq!(kerning("span"), None);
}

#[test]
fn font_synthesis_small_caps() {
    assert!(one("").font_synthesis_small_caps);
    assert!(!one("font-synthesis-small-caps: none").font_synthesis_small_caps);
    assert!(!one("font-synthesis: weight style").font_synthesis_small_caps);
    assert!(one("font-synthesis: small-caps").font_synthesis_small_caps);
}

#[test]
fn paint_and_image_properties() {
    // Gradients: conic with a center, color hints, interpolation methods.
    let s = one("background-image: conic-gradient(from 90deg at 10px 20%, red, 30%, blue 0.5turn)");
    match &s.background_layers[0].image {
        BackgroundImage::ConicGradient { angle, position, stops, .. } => {
            assert!((angle - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
            assert_eq!(*position, (Length::Px(10.0), Length::Percent(20.0)));
            assert_eq!(stops.len(), 3);
            let pct = |l: Option<Length>| match l {
                Some(Length::Percent(p)) => p,
                other => panic!("{other:?}"),
            };
            assert!(stops[1].hint && (pct(stops[1].position) - 30.0).abs() < 1e-3);
            assert!((pct(stops[2].position) - 50.0).abs() < 1e-3);
        }
        other => panic!("{other:?}"),
    }
    let s = one("background-image: linear-gradient(to right in hsl longer hue, red, blue)");
    match &s.background_layers[0].image {
        BackgroundImage::LinearGradient { direction, interp, .. } => {
            assert_eq!(*direction, GradientDirection::Corner(1, 0));
            assert_eq!(*interp, crate::ColorInterp { space: crate::InterpSpace::Hsl, hue: crate::HueMethod::Longer });
        }
        other => panic!("{other:?}"),
    }
    // A hint must sit between two color stops.
    let s = one("background-image: linear-gradient(red, 10%)");
    assert!(s.background_layers.is_empty() || matches!(s.background_layers[0].image, BackgroundImage::None));
    // Edge offsets in positions.
    let s = one("background-position: right 10px bottom 5%");
    assert_eq!(s.background_layers[0].position, (Length::Calc(-10.0, 100.0), Length::Percent(95.0)));
    // Border images.
    let s = one("border-image: linear-gradient(red, blue) 10% 20 fill / 2 auto 3px / 1px round space");
    let bi = s.border_image.as_deref().expect("border image");
    assert!(matches!(bi.source, BackgroundImage::LinearGradient { .. }));
    assert_eq!(bi.slice, [Length::Percent(10.0), Length::Px(20.0), Length::Percent(10.0), Length::Px(20.0)]);
    assert!(bi.fill);
    assert_eq!(bi.width, [BorderImageWidth::Number(2.0), BorderImageWidth::Auto, BorderImageWidth::Length(Length::Px(3.0)), BorderImageWidth::Auto]);
    assert_eq!(bi.outset, [BorderImageWidth::Length(Length::Px(1.0)); 4]);
    assert_eq!(bi.repeat, (BorderImageRepeat::Round, BorderImageRepeat::Space));
    assert!(one("border-image: url(a.png) -1").border_image.is_none(), "negative slices are invalid");
    // overflow-clip-margin and image-orientation.
    let s = one("overflow-clip-margin: content-box 5px; image-orientation: none");
    assert_eq!(s.overflow_clip_margin, (BoxArea::ContentBox, 5.0));
    assert!(s.image_orientation_none);
    assert_eq!(one("overflow-clip-margin: -10px").overflow_clip_margin, (BoxArea::PaddingBox, 0.0));
}

#[test]
fn line_clamp_shorthand() {
    let s = one("line-clamp: 3 \" (more)\"");
    assert_eq!((s.line_clamp, s.line_clamp_auto, s.block_ellipsis.as_deref()), (Some(3), false, Some(" (more)")));
    let s = one("line-clamp: auto; max-height: 4lh; font: 16px/32px serif");
    assert_eq!((s.line_clamp, s.line_clamp_auto), (Some(u32::MAX), true));
    assert_eq!(s.max_height, Length::Px(128.0));
    let s = one("line-clamp: 2 no-ellipsis");
    assert_eq!((s.line_clamp, s.block_ellipsis.as_deref()), (Some(2), None));
    let s = one("line-clamp: 4 auto");
    assert_eq!((s.line_clamp, s.line_clamp_auto, s.block_ellipsis.as_deref()), (Some(4), false, Some("\u{2026}")));
    assert_eq!(one("line-clamp: 0").line_clamp, None);
}

#[test]
fn font_feature_properties() {
    let s = one("font-feature-settings: \"liga\" off, \"ss01\"; font-variant-numeric: tabular-nums slashed-zero; font-kerning: none");
    let f = s.font_features.as_deref().expect("features").resolved();
    assert_eq!(f, vec![(*b"kern", 0), (*b"tnum", 1), (*b"zero", 1), (*b"liga", 0), (*b"ss01", 1)]);
    // Contradictory keywords are invalid.
    assert!(one("font-variant-numeric: lining-nums oldstyle-nums").font_features.is_none());
    // Named alternates resolve through @font-feature-values of the family.
    let (doc, t) = styled(
        "<div>x</div>",
        "@font-feature-values Fancy { @styleset { curly: 2 4; } @swash { flowing: 1; } } div { font-family: Fancy, serif; font-variant-alternates: styleset(curly) swash(flowing) }",
    );
    let f = style_of(&doc, &t, "div").font_features.as_deref().expect("features").resolved();
    assert_eq!(f, vec![(*b"ss02", 1), (*b"ss04", 1), (*b"swsh", 1), (*b"cswh", 1)]);
    let s = one("font-size: 20px; font-size-adjust: ex-height 0.6; width: 1em");
    assert_eq!(s.font_size_adjust, Some((FontSizeAdjust::ExHeight, Some(0.6))));
    // `em` keeps the computed size.
    assert_eq!(s.width, Length::Px(20.0));
}

#[test]
fn object_view_box_and_background_clip_values() {
    assert_eq!(one("object-view-box: inset(10% 20px)").object_view_box, Some([Length::Percent(10.0), Length::Px(20.0), Length::Percent(10.0), Length::Px(20.0)]));
    // xywh(x y w h) → insets from each edge.
    assert_eq!(one("object-view-box: xywh(10px 20px 30% 40%)").object_view_box, Some([Length::Px(20.0), Length::Calc(-10.0, 70.0), Length::Calc(-20.0, 60.0), Length::Px(10.0)]));
    assert_eq!(one("object-view-box: rect(1px 50% 60% 2px)").object_view_box, Some([Length::Px(1.0), Length::Percent(50.0), Length::Percent(40.0), Length::Px(2.0)]));
    assert_eq!(one("object-view-box: xywh(0 0 -1px 5px)").object_view_box, None);
    let s = one("background: red linear-gradient(red, blue) border-area; background-attachment: local");
    let l = &s.background_layers[0];
    assert!(l.clip_border_area && !l.clip_text && l.local);
    let s = one("-webkit-background-clip: text; -webkit-text-fill-color: transparent");
    assert!(s.background_layers[0].clip_text);
    assert_eq!(s.text_fill_color, Some(rabbithole_gfx::Color::TRANSPARENT));
}

#[test]
fn transforms_3d_functions_and_properties() {
    let s = one("transform: translate3d(1px, 2px, 3px) rotateX(90deg) perspective(100px) matrix3d(1,0,0,0, 0,1,0,0, 0,0,1,0, 5,6,7,1)");
    assert_eq!(s.transform[0], TransformOp::Translate3d(Length::Px(1.0), Length::Px(2.0), 3.0));
    assert!(matches!(s.transform[1], TransformOp::Rotate3d(x, y, z, a) if x == 1.0 && y == 0.0 && z == 0.0 && (a - std::f32::consts::FRAC_PI_2).abs() < 1e-6));
    assert_eq!(s.transform[2], TransformOp::Perspective(Some(100.0)));
    assert!(matches!(&s.transform[3], TransformOp::Matrix3d(m) if m[12] == 5.0 && m[14] == 7.0));
    assert!(!s.transform.iter().all(TransformOp::is_2d));
    // translateZ() takes no percentage; perspective() no negative length.
    assert!(one("transform: translateZ(10%)").transform.is_empty());
    assert!(one("transform: perspective(-1px)").transform.is_empty());
    // Individual properties come first, in translate / rotate / scale order.
    let s = one("translate: 10px 20px 30px; rotate: y 90deg; scale: 2 3; transform: translateX(5px)");
    let t = s.transform3d.as_deref().unwrap();
    assert_eq!(t.translate, Some(TransformOp::Translate3d(Length::Px(10.0), Length::Px(20.0), 30.0)));
    assert!(matches!(t.rotate, Some(TransformOp::Rotate3d(x, y, z, _)) if x == 0.0 && y == 1.0 && z == 0.0));
    assert_eq!(t.scale, Some(TransformOp::Scale3d(2.0, 3.0, 1.0)));
    assert!(s.has_transform());
    let ops = s.transform_ops();
    assert_eq!(ops.len(), 4);
    assert!(matches!(ops[3], TransformOp::Translate(..)));
    assert!(one("rotate: 1 2 3 45deg").transform3d.is_some());
    assert!(one("rotate: 45deg x y").transform3d.is_none());
    assert!(one("scale: none").transform3d.is_none());
    // A transform-origin z length.
    let s = one("transform-origin: left top 12px");
    assert_eq!(s.transform3d.as_ref().map(|t| t.origin_z), Some(12.0));
}

#[test]
fn perspective_and_preserve_3d() {
    let s = one("perspective: 500px; perspective-origin: 10px bottom");
    assert_eq!(s.perspective(), Some(500.0));
    assert_eq!(s.transform3d.as_ref().unwrap().perspective_origin, (Length::Px(10.0), Length::Percent(100.0)));
    assert!(s.is_stacking_context());
    let s = one("transform-style: preserve-3d; backface-visibility: hidden");
    assert!(s.preserve_3d() && s.preserves_3d() && s.backface_hidden());
    assert!(s.is_stacking_context());
    // Grouping properties force a flat used value.
    assert!(!one("transform-style: preserve-3d; opacity: 0.5").preserves_3d());
    assert!(!one("transform-style: preserve-3d; overflow: hidden").preserves_3d());
    assert!(!one("transform-style: preserve-3d; isolation: isolate").preserves_3d());
    assert!(!one("perspective: none").is_stacking_context());
}
