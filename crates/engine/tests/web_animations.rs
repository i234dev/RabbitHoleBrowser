//! Web Animations: script animations (`Element.animate`) and the
//! `CSSAnimation` / `CSSTransition` objects of CSS animations render and
//! reach computed styles.

use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Page, Viewport};

fn engine() -> Engine {
    let mut e = Engine::new_bundled_fonts_only();
    e.scripting = true;
    e
}

fn load(engine: &mut Engine, html: &str) -> Page {
    let url = Url::parse("https://example.test/").unwrap();
    let mut page = engine.load_html(html, url, Viewport { width: 400.0, height: 300.0 });
    engine.settle_for_snapshot(&mut page, std::time::Duration::from_millis(50));
    page
}

/// Console lines logged by the page.
fn logs(page: &mut Page) -> Vec<String> {
    page.take_console().into_iter().map(|(_, m)| m).collect()
}

/// RGB of the frame pixel at (x, y).
fn rgb(e: &Engine, page: &Page, x: u32, y: u32) -> (u8, u8, u8) {
    let f = e.snapshot(page);
    let i = ((y * f.width + x) * 4) as usize;
    (f.rgba[i], f.rgba[i + 1], f.rgba[i + 2])
}

const BOX: &str = "<!doctype html><style>body { margin: 0 } #t { width: 100px; height: 100px; background: green }</style><div id=t></div>";

#[test]
fn seeked_script_animation_reaches_computed_style_and_layout() {
    let mut e = engine();
    let mut p = load(
        &mut e,
        &format!(
            "{BOX}<script>
            const t = document.getElementById('t');
            const a = t.animate([{{ width: '0px', opacity: 0 }}, {{ width: '200px', opacity: 1 }}], 1000);
            a.pause();
            a.currentTime = 250;
            const cs = getComputedStyle(t);
            console.log(cs.width, cs.opacity, t.offsetWidth, a.playState);
            a.currentTime = 750;
            console.log(cs.width, cs.opacity, t.offsetWidth);
            a.cancel();
            console.log(cs.width, cs.opacity, a.playState);
            </script>"
        ),
    );
    assert_eq!(logs(&mut p), vec!["50px 0.25 50 paused", "150px 0.75 150", "100px 1 idle"]);
}

#[test]
fn filling_animation_renders_and_commits() {
    let mut e = engine();
    let mut p = load(
        &mut e,
        &format!(
            "{BOX}<script>
            const t = document.getElementById('t');
            const a = t.animate({{ backgroundColor: ['red', 'blue'] }}, {{ duration: 100, fill: 'forwards' }});
            a.finish();
            console.log(a.playState, getComputedStyle(t).backgroundColor);
            a.commitStyles();
            a.cancel();
            console.log(t.getAttribute('style'));
            const b = t.animate({{ opacity: [1, 0] }}, {{ duration: 1000, fill: 'both' }});
            b.pause();
            b.currentTime = 1000;
            </script>"
        ),
    );
    assert_eq!(logs(&mut p), vec!["finished rgb(0, 0, 255)", "background-color: rgb(0, 0, 255);"]);
    // opacity 0 over a blue box: white page.
    assert_eq!(rgb(&e, &p, 50, 50), (255, 255, 255));
}

#[test]
fn easing_composite_and_keyframe_offsets() {
    let mut e = engine();
    let mut p = load(
        &mut e,
        &format!(
            "{BOX}<script>
            const t = document.getElementById('t');
            const a = t.animate([{{ marginLeft: '0px' }}, {{ marginLeft: '100px', offset: 0.5 }}, {{ marginLeft: '0px' }}], {{ duration: 1000, easing: 'steps(2)' }});
            a.pause();
            a.currentTime = 600;
            const cs = getComputedStyle(t);
            console.log(cs.marginLeft, a.effect.getComputedTiming().progress);
            const b = t.animate({{ marginLeft: ['10px', '10px'] }}, {{ duration: 1000, composite: 'add' }});
            b.pause();
            console.log(cs.marginLeft, document.getAnimations().length, t.getAnimations()[1] === b);
            </script>"
        ),
    );
    assert_eq!(logs(&mut p), vec!["100px 0.5", "110px 2 true"]);
}

#[test]
fn css_animation_objects_control_rendering() {
    let mut e = engine();
    let mut p = load(
        &mut e,
        "<!doctype html><style>@keyframes slide { from { margin-left: 0px } to { margin-left: 100px } }
         #t { width: 10px; height: 10px; animation: slide 1000s linear }</style><div id=t></div><script>
        const t = document.getElementById('t');
        const [a] = t.getAnimations();
        console.log(a instanceof CSSAnimation, a.animationName, a.effect.getKeyframes().length, t.getAnimations()[0] === a);
        a.pause();
        a.currentTime = 500000;
        console.log(getComputedStyle(t).marginLeft, a.playState);
        a.cancel();
        console.log(getComputedStyle(t).marginLeft, t.getAnimations().length);
        </script>",
    );
    assert_eq!(logs(&mut p), vec!["true slide 2 true", "50px paused", "0px 0"]);
}

#[test]
fn replaced_filling_animations_are_removed() {
    let mut e = engine();
    let mut p = load(
        &mut e,
        &format!(
            "{BOX}<script>
            const t = document.getElementById('t');
            const a = t.animate({{ opacity: 0.5 }}, {{ duration: 1, fill: 'forwards' }});
            const b = t.animate({{ opacity: 0.25 }}, {{ duration: 1, fill: 'forwards' }});
            a.onremove = (e) => console.log('remove', a.replaceState, e.constructor.name);
            Promise.all([a.finished, b.finished]).then(() => requestAnimationFrame(() => requestAnimationFrame(() =>
              console.log(a.replaceState, b.replaceState, getComputedStyle(t).opacity, t.getAnimations().length))));
            </script>"
        ),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut out = Vec::new();
    while std::time::Instant::now() < deadline && out.len() < 2 {
        e.tick(&mut p);
        out.extend(logs(&mut p));
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(out, vec!["remove removed AnimationPlaybackEvent", "removed active 0.25 1"]);
}

#[test]
fn effects_on_elements_inserted_by_the_script_apply() {
    let mut e = engine();
    let mut p = load(
        &mut e,
        "<!doctype html><body><script>
        const d = document.createElement('div');
        document.body.appendChild(d);
        const a = d.animate([{ width: '0px' }, { width: '100px', easing: 'steps(2, start)' }, { width: '200px' }], { duration: 2000, fill: 'forwards' });
        const out = [];
        for (const t of [0, 999, 1100, 2000]) { a.currentTime = t; out.push(getComputedStyle(d).width); }
        console.log(out.join(' '));
        </script>",
    );
    assert_eq!(logs(&mut p), vec!["0px 99.9px 150px 200px"]);
}
