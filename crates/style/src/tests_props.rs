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
    assert_eq!(s.display, Display::Flex);
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
        BackgroundImage::LinearGradient { direction, stops, repeating } => {
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
    assert!((s.opacity - 0.4).abs() < 1e-5);
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
    assert_eq!(s.max_width, Length::FitContent);
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
fn font_faces_collected_and_resolved() {
    let doc = rabbithole_html::parse("<p>x</p>");
    let mut sheet = parse_stylesheet(
        "@font-face { font-family: 'My Font'; src: local('x'), url(fonts/my.woff2) format('woff2'), url('my.woff') format(woff); font-weight: 300 700; font-style: italic; font-display: swap; unicode-range: U+0000-00FF, U+4??; } \
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
    assert_eq!(f.display, "swap");
    assert_eq!(f.unicode_range, Some(vec![(0, 0xff), (0x400, 0x4ff)]));
    assert_eq!(t.font_faces[1].family, "Other");
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
    assert!(b.cursor_pointer);
    assert_eq!(b.border_radius, 2.0);
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
