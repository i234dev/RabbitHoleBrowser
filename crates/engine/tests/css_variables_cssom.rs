//! Custom properties and environment variables as pages read and use them:
//! `getComputedStyle(el).getPropertyValue('--x')` (theme and chart code
//! reads its design tokens this way) and `env()` insets in positioned
//! boxes.

use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Page, Viewport};

fn load(html: &str) -> (Engine, Page) {
    let mut e = Engine::new_bundled_fonts_only();
    e.scripting = true;
    e.loader.load_scripts = true;
    let page = e.load_html(html, Url::parse("https://example.test/").unwrap(), Viewport { width: 1000.0, height: 600.0 });
    (e, page)
}

fn eval(e: &Engine, page: &mut Page, js: &str) -> String {
    e.eval_in_page(page, js).trim_matches(|c| c == '\'' || c == '"').to_string()
}

#[test]
fn computed_style_serves_custom_properties() {
    let (e, mut page) = load(
        "<!doctype html><style>:root { --brand: #0a84ff; --gap:  8px ; --pad: calc(var(--gap) * 2) } .card { --brand: tomato; --bad: var(--missing) }</style><div class=card><p id=p>x</p></div>",
    );
    let r = eval(
        &e,
        &mut page,
        "const cs = getComputedStyle(document.documentElement), p = getComputedStyle(document.getElementById('p')); \
         [cs.getPropertyValue('--brand'), cs.getPropertyValue('--gap'), cs.getPropertyValue('--pad'), p.getPropertyValue('--brand'), p.getPropertyValue('--gap'), '[' + p.getPropertyValue('--bad') + ']', '[' + cs.getPropertyValue('--nope') + ']'].join('|')",
    );
    assert_eq!(r, "#0a84ff|8px|calc(8px * 2)|tomato|8px|[]|[]");
    // Live: a script change shows up in the next read.
    let r = eval(&e, &mut page, "document.documentElement.style.setProperty('--gap', '12px'); getComputedStyle(document.getElementById('p')).getPropertyValue('--gap')");
    assert_eq!(r, "12px");
}

#[test]
fn env_insets_position_fixed_boxes() {
    let (e, mut page) = load(
        "<!doctype html><style>:root { --sab: env(safe-area-inset-bottom, 50px); --sae: env(safe-area-inset-right, 50px) } \
         #a { position: fixed; width: 10px; height: 10px; right: max(30px, var(--sae)); bottom: max(20px, var(--sab)) } \
         #b { position: fixed; width: 10px; height: 10px; left: env(unknown-variable, 40px); top: env(safe-area-inset-top, 40px) }</style><div id=a></div><div id=b></div>",
    );
    let r = eval(&e, &mut page, "['a', 'b'].map(id => { const r = document.getElementById(id).getBoundingClientRect(); return r.left + ',' + r.top; }).join(' ')");
    assert_eq!(r, "960,570 40,0");
}
