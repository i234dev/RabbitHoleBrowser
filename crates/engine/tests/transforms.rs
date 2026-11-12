//! Pages that pan and zoom a big scene with one CSS transform on a
//! container (maps, timelines, canvases of cards): painting, offsets and
//! `wheel` input.

use rabbithole_engine::gfx::Canvas;
use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Modifiers, Page, Viewport};

fn engine(scripting: bool) -> Engine {
    let mut e = Engine::new_bundled_fonts_only();
    e.scripting = scripting;
    e.loader.load_scripts = scripting;
    e
}

fn load(engine: &mut Engine, html: &str) -> Page {
    let url = Url::parse("https://example.test/").unwrap();
    engine.load_html(html, url, Viewport { width: 800.0, height: 600.0 })
}

fn eval(engine: &Engine, page: &mut Page, js: &str) -> String {
    engine.eval_in_page(page, js).trim_matches(|c| c == '\'' || c == '"').to_string()
}

/// RGB of the canvas pixel at (x, y).
fn rgb(c: &Canvas, x: u32, y: u32) -> (u8, u8, u8) {
    let p = c.data[(y * c.width + x) as usize];
    (((p >> 16) & 0xff) as u8, ((p >> 8) & 0xff) as u8, (p & 0xff) as u8)
}

/// Children laid out far outside the viewport inside a scaled-down
/// container are painted where the transform puts them: culling used to
/// compare their untransformed rects with the viewport (layout and
/// rasterizer), so only what sat near the origin showed.
#[test]
fn scaled_container_paints_children_laid_out_beyond_the_viewport() {
    let mut e = engine(false);
    let page = load(
        &mut e,
        r#"<!doctype html><style>
        html, body { margin: 0; height: 100%; overflow: hidden; background: #fff }
        #stage { position: fixed; inset: 0; overflow: hidden }
        #scene { position: absolute; left: 0; top: 0; transform-origin: 0 0; transform: translate(10px, 10px) scale(0.05) }
        .item { position: absolute; width: 1000px; height: 1000px; background: rgb(255, 0, 0) }
        .clip { position: absolute; width: 1000px; height: 1000px; background: rgb(0, 0, 255); border-radius: 30px; overflow: hidden; contain: paint }
        .over { position: absolute; width: 1000px; height: 1000px; background: rgb(0, 128, 0) }
        .over::after { content: ''; position: absolute; inset: 0; background: rgb(255, 165, 0) }
        </style>
        <div id="stage"><div id="scene">
          <div class="item" style="left: 0; top: 0"></div>
          <div class="item" style="left: 4000px; top: 0"></div>
          <div class="item" style="left: 14000px; top: 0"></div>
          <div class="clip" style="left: 14000px; top: 4000px"></div>
          <div class="over" style="left: 14000px; top: 8000px"></div>
        </div></div>"#,
    );
    let mut c = Canvas::new(800, 600);
    e.render(&page, &mut c, 0.0, 0.0);
    // Each 1000px item is 50px on screen at 10 + left / 20.
    assert_eq!(rgb(&c, 35, 35), (255, 0, 0));
    assert_eq!(rgb(&c, 235, 35), (255, 0, 0));
    assert_eq!(rgb(&c, 735, 35), (255, 0, 0));
    assert_eq!(rgb(&c, 735, 235), (0, 0, 255));
    assert_eq!(rgb(&c, 735, 435), (255, 165, 0));
    // Between items: background.
    assert_eq!(rgb(&c, 400, 35), (255, 255, 255));
}

/// `offsetParent` is the nearest positioned (or transformed) ancestor, a
/// table cell for static elements, or the body; offsets are measured from
/// its padding edge (from the page for a static body). It used to be the
/// body for every element, so offsets inside positioned containers were
/// page offsets.
#[test]
fn offset_parent_and_offsets_follow_positioned_ancestors() {
    let mut e = engine(true);
    let mut page = load(
        &mut e,
        r#"<!doctype html><style>
        body { margin: 8px }
        #outer { position: absolute; left: 100px; top: 50px; width: 600px; height: 400px; border: 5px solid black }
        #mid { margin: 20px; padding: 10px }
        #inner { position: relative; top: 7px; height: 30px }
        #moved { position: absolute; left: 300px; top: 300px; transform: scale(0.5); width: 100px; height: 100px }
        #deep { position: absolute; left: 12px; top: 34px; width: 10px; height: 10px }
        </style>
        <p id="first">x</p>
        <div id="outer"><div id="mid"><div id="inner">a</div></div><div id="moved"><div id="deep"></div></div></div>
        <table><tr><td id="cell"><span id="incell">c</span></td></tr></table>"#,
    );
    let r = eval(
        &e,
        &mut page,
        "var q = (id) => document.getElementById(id); \
         [q('inner').offsetParent.id, q('inner').offsetTop, q('inner').offsetLeft, \
          q('mid').offsetParent.id, q('mid').offsetTop, \
          q('deep').offsetParent.id, q('deep').offsetTop, q('deep').offsetLeft, \
          q('outer').offsetParent.tagName, q('outer').offsetTop, q('outer').offsetLeft, \
          q('first').offsetParent.tagName, q('first').offsetTop, q('first').offsetLeft, \
          q('incell').offsetParent.id, String(document.body.offsetParent), document.body.offsetTop].join()",
    );
    assert_eq!(r, "outer,37,30,outer,20,moved,34,12,BODY,50,100,BODY,16,8,cell,null,0");
    // Offsets ignore transforms; getBoundingClientRect() applies them
    // (#moved is scaled by half around its center at 455,405).
    let r = eval(&e, &mut page, "var b = document.getElementById('deep').getBoundingClientRect(); [b.x, b.y, b.width, b.height].join()");
    assert_eq!(r, "436,397,5,5");
}

/// A `wheel` listener sees the wheel (with its deltas, position and
/// modifiers) and can cancel it: zoomable widgets take over the wheel
/// instead of the page scrolling.
#[test]
fn wheel_events_reach_listeners_and_can_be_cancelled() {
    let mut e = engine(true);
    let mut page = load(
        &mut e,
        r#"<!doctype html><style>body { margin: 0 } #map { width: 400px; height: 300px } #tall { height: 3000px }</style>
        <div id="map"></div><div id="tall"></div>
        <script>
        window.log = [];
        document.getElementById('map').addEventListener('wheel', (e) => {
          log.push([e.constructor.name, e.deltaX, e.deltaY, e.deltaMode, e.clientX, e.clientY, e.ctrlKey, e.target.id, e.cancelable].join(':'));
          e.preventDefault();
        }, { passive: false });
        window.addEventListener('wheel', (e) => { log.push('window:' + e.defaultPrevented); });
        </script>"#,
    );
    let (prevented, _) = e.wheel(&mut page, 100.0, 120.0, 0.0, 53.0, Modifiers { ctrl: true, ..Default::default() });
    assert!(prevented);
    assert_eq!(eval(&e, &mut page, "log.join('|')"), "WheelEvent:0:53:0:100:120:true:map:true|window:true");
    // Outside the widget nobody cancels: the page scrolls as usual.
    let (prevented, _) = e.wheel(&mut page, 100.0, 500.0, 0.0, 40.0, Modifiers::default());
    assert!(!prevented);
    assert_eq!(e.wheel_listeners(&page), (true, true));
}
