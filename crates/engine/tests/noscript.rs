use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Viewport};

const HTML: &str = r#"<html><head><noscript><meta http-equiv="refresh" content="0;url=/nojs"></noscript></head>
<body><noscript id="n">no js</noscript><p>js</p></body></html>"#;

fn noscript_hidden(scripting: bool) -> (bool, bool) {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = scripting;
    engine.loader.load_scripts = scripting;
    let url = Url::parse("https://example.test/").unwrap();
    let mut page = engine.load_html(HTML, url, Viewport { width: 800.0, height: 600.0 });
    let n = page
        .doc
        .descendants(rabbithole_engine::dom::Document::ROOT)
        .find(|&n| page.doc.attr(n, "id") == Some("n"))
        .unwrap();
    let hidden = engine.computed_style_now(&mut page, n).map(|s| s.display.is_none()).unwrap_or(true);
    let nav = engine.take_page_navigation(&mut page).is_some();
    (hidden, nav)
}

#[test]
fn noscript_follows_scripting() {
    assert_eq!(noscript_hidden(true), (true, false));
    assert_eq!(noscript_hidden(false), (false, true));
}

/// Style sheets inside `<noscript>` are inert when scripting is enabled
/// (google.com ships `<noscript><style>table,div,span,p{display:none}`).
#[test]
fn noscript_styles_ignored_with_scripting() {
    const DOC: &str = r#"<html><head></head><body><noscript><style>div,span{display:none}</style></noscript><div id="d">content</div></body></html>"#;
    for scripting in [true, false] {
        let mut engine = Engine::new_bundled_fonts_only();
        engine.scripting = scripting;
        engine.loader.load_scripts = scripting;
        let url = Url::parse("https://example.test/").unwrap();
        let mut page = engine.load_html(DOC, url, Viewport { width: 800.0, height: 600.0 });
        let d = page.doc.element_by_id("d").unwrap();
        let hidden = engine.computed_style_now(&mut page, d).map(|s| s.display.is_none()).unwrap_or(true);
        assert_eq!(hidden, !scripting, "scripting={scripting}");
    }
}

fn eval(engine: &Engine, page: &mut rabbithole_engine::Page, js: &str) -> String {
    engine.eval_in_page(page, js).trim_matches(|c| c == '\'' || c == '"').to_string()
}

/// With scripting enabled the HTML parser treats `<noscript>` content as
/// raw text (HTML "in head" / "in body" insertion modes): no elements exist
/// inside it, so scripts cannot find fallback overlays or pixels there and
/// nothing inside loads. Auxiliary documents (DOMParser) have scripting
/// disabled and parse the markup.
#[test]
fn noscript_content_is_raw_text_with_scripting() {
    const DOC: &str = r#"<html><head><noscript><link rel="stylesheet" href="/nojs.css"><style>p{color:red}</style></noscript></head>
<body><noscript class="ov"><div id="overlay">Uh oh! This site does not run without JavaScript.</div><img id="pix" src="/pixel.gif"></noscript>
<div id="app">app</div></body></html>"#;
    let mut engine = Engine::new_bundled_fonts_only();
    let url = Url::parse("https://example.test/").unwrap();
    let mut page = engine.load_html(DOC, url, Viewport { width: 800.0, height: 600.0 });
    assert!(page.doc.element_by_id("overlay").is_none(), "noscript markup must not create elements");
    assert_eq!(eval(&engine, &mut page, "String(document.querySelector('#overlay, #pix, head link'))"), "null");
    assert_eq!(eval(&engine, &mut page, "var n = document.querySelector('noscript.ov'); n.childNodes.length + ':' + n.firstChild.nodeType"), "1:3");
    assert!(eval(&engine, &mut page, "document.querySelector('noscript.ov').textContent").contains("<div id=\"overlay\">"));
    assert_eq!(eval(&engine, &mut page, "document.querySelectorAll('noscript').length"), "2");
    // innerHTML on the scripted document: raw text too.
    assert_eq!(eval(&engine, &mut page, "var d = document.createElement('div'); d.innerHTML = '<noscript><p id=x>x</p></noscript>'; String(d.querySelector('#x')) + ' ' + d.firstChild.textContent"), "null <p id=x>x</p>");
    // DOMParser documents have scripting disabled: markup is parsed.
    assert_eq!(eval(&engine, &mut page, "new DOMParser().parseFromString('<noscript><p id=y>y</p></noscript>', 'text/html').querySelector('#y').textContent"), "y");
    // Without scripting the content is markup and is rendered.
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = false;
    engine.loader.load_scripts = false;
    let url = Url::parse("https://example.test/").unwrap();
    let page = engine.load_html(DOC, url, Viewport { width: 800.0, height: 600.0 });
    assert!(page.doc.element_by_id("overlay").is_some());
}

/// `navigator.userAgentData` and the other navigator identity values agree
/// with the User-Agent (and the `Sec-CH-UA` headers) of the browser
/// identity; the bot identity has no client hints.
#[test]
fn navigator_identity_is_consistent() {
    let mut engine = Engine::new_bundled_fonts_only();
    let url = Url::parse("https://example.test/").unwrap();
    let mut page = engine.load_html("<p>x</p>", url, Viewport { width: 800.0, height: 600.0 });
    let major = rabbithole_engine::net::CHROME_MAJOR.to_string();
    let ua = eval(&engine, &mut page, "navigator.userAgent");
    assert!(ua.contains(&format!("Chrome/{major}.")), "{ua}");
    assert_eq!(eval(&engine, &mut page, "navigator.userAgentData.brands.map(b => b.brand + '/' + b.version).join()"), format!("Chromium/{major},Google Chrome/{major},Not_A Brand/99"));
    assert_eq!(eval(&engine, &mut page, "navigator.userAgentData.mobile + ' ' + (navigator.userAgentData === navigator.userAgentData) + ' ' + typeof navigator.userAgentData.platform"), "false true string");
    assert_eq!(eval(&engine, &mut page, "Object.prototype.toString.call(navigator.userAgentData)"), "[object NavigatorUAData]");
    assert_eq!(eval(&engine, &mut page, "JSON.stringify(navigator.userAgentData).indexOf('brands') >= 0"), "true");
    assert_eq!(eval(&engine, &mut page, "navigator.vendor"), "Google Inc.");
    assert_eq!(eval(&engine, &mut page, "navigator.appVersion === navigator.userAgent.slice(8)"), "true");
    eval(&engine, &mut page, "navigator.userAgentData.getHighEntropyValues(['uaFullVersion','fullVersionList','bitness']).then(v => { window.he = v.uaFullVersion + '|' + v.fullVersionList[1].version + '|' + v.bitness + '|' + v.brands.length; })");
    assert_eq!(eval(&engine, &mut page, "window.he"), format!("{major}.0.0.0|{major}.0.0.0|64|3"));
    // The bot identity sends no client hints: no userAgentData either.
    let mut bot = Engine::new_bundled_fonts_only();
    bot.loader.fetcher.set_header_profile(rabbithole_engine::net::HeaderProfile::Bot);
    let url = Url::parse("https://example.test/").unwrap();
    let mut page = bot.load_html("<p>x</p>", url, Viewport { width: 800.0, height: 600.0 });
    assert_eq!(eval(&bot, &mut page, "typeof navigator.userAgentData + ' ' + navigator.vendor.length"), "undefined 0");
}

/// Template content keeps its leading comment anchors and its markup, also
/// when the template sits in the head (Svelte 5 hydration walks
/// `template.content` from a `<!---->` anchor with the Node getters).
#[test]
fn template_content_keeps_comments_and_head_templates() {
    let mut engine = Engine::new_bundled_fonts_only();
    let url = Url::parse("https://example.test/").unwrap();
    let html = "<html><head><template id=h><p>a</p>b<!--c--></template></head><body><div id=d>x</div></body></html>";
    let mut page = engine.load_html(html, url, Viewport { width: 800.0, height: 600.0 });
    assert_eq!(eval(&engine, &mut page, "Array.from(document.getElementById('h').content.childNodes, n => n.nodeName).join()"), "P,#text,#comment");
    assert_eq!(eval(&engine, &mut page, "document.body.textContent"), "x");
    let js = "var t = document.createElement('template'); t.innerHTML = '<!----> <span>s</span><!---->'; \
              var fc = Object.getOwnPropertyDescriptor(Node.prototype, 'firstChild').get, ns = Object.getOwnPropertyDescriptor(Node.prototype, 'nextSibling').get; \
              var c = fc.call(t.content); [c.nodeType, ns.call(c).nodeType, ns.call(ns.call(c)).nodeName, t.content.childNodes.length].join()";
    assert_eq!(eval(&engine, &mut page, js), "8,3,SPAN,4");
    assert_eq!(eval(&engine, &mut page, "var d = document.createElement('div'); d.innerHTML = '<!--a--><style>p{}</style><p>x</p>'; Array.from(d.childNodes, n => n.nodeName).join()"), "#comment,STYLE,P");
}
