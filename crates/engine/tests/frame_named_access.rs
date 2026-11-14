//! Named child windows are live WindowProxy values, independent of a
//! preceding read of `frames` or `length`.

use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Page, Viewport};

fn load(html: &str) -> (Engine, Page) {
    let mut engine = Engine::new_bundled_fonts_only();
    let mut page = engine.load_html(html, Url::parse("https://example.test/").unwrap(), Viewport { width: 800.0, height: 600.0 });
    settle(&engine, &mut page);
    (engine, page)
}

fn settle(engine: &Engine, page: &mut Page) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut quiet = 0;
    while std::time::Instant::now() < deadline && quiet < 5 {
        let loading = page.frames_loading();
        engine.tick(page);
        if loading { quiet = 0; } else { quiet += 1; }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn assert_js(engine: &Engine, page: &mut Page, script: &str) {
    assert_eq!(engine.eval_in_page(page, script), "true", "{script}");
}

#[test]
fn named_child_is_available_before_reading_frames_and_delivers_messages() {
    let (engine, mut page) = load(r#"<!doctype html><script>
        var replies = [];
        addEventListener('message', function (e) {
            replies.push(e.data + ':' + (e.source === window.chat));
        });
    </script><iframe name="chat" srcdoc="<script>addEventListener('message',function(e){parent.postMessage('reply-'+e.data,'*');});</script>"></iframe>"#);
    // In the old implementation this threw the reported undefined /
    // postMessage TypeError until somebody first read `frames`.
    assert_js(&engine, &mut page, "typeof window.chat.postMessage === 'function' && window.chat === document.querySelector('iframe').contentWindow");
    assert_js(&engine, &mut page, "window.chat.postMessage('named', '*'); true");
    settle(&engine, &mut page);
    assert_js(&engine, &mut page, "replies.length === 1 && replies[0] === 'reply-named:true'");
}

#[test]
fn named_child_precedes_element_ids_and_uses_the_native_window_getter() {
    let (engine, mut page) = load(r#"<!doctype html><div id="chat"></div>
        <iframe name="chat" id="frame" srcdoc="<p>child</p>"></iframe>
        <iframe name="same" id="same" srcdoc="<p>same</p>"></iframe>
        <iframe id="idOnly" srcdoc="<p>id only</p>"></iframe>
        <form name="ordinaryForm"></form>"#);
    assert_js(&engine, &mut page, r#"(function () {
        var f = document.getElementById('frame'), w = f.contentWindow;
        if (window.chat !== w || window.same !== document.getElementById('same').contentWindow) return false;
        if (window.idOnly !== document.getElementById('idOnly') || window.ordinaryForm.tagName !== 'FORM') return false;
        Object.defineProperty(f, 'contentWindow', {value: 17, configurable: true});
        // Named-window resolution must not use a page-modified function
        // .call, Reflect.apply, or the iframe's public type properties.
        var calls = 0, originalApply = Reflect.apply;
        var methods = [
            Object.getOwnPropertyDescriptor(HTMLIFrameElement.prototype, 'contentWindow').get,
            Object.getOwnPropertyDescriptor(Element.prototype, 'localName').get,
            Object.getOwnPropertyDescriptor(Element.prototype, 'namespaceURI').get,
            Document.prototype.getElementsByName, Document.prototype.getElementById
        ];
        for (var method of methods) Object.defineProperty(method, 'call', {configurable: true, value: function () { calls++; throw new Error('page call'); }});
        Reflect.apply = function () { calls++; throw new Error('page apply'); };
        Object.defineProperty(f, 'localName', {value: 'div', configurable: true});
        Object.defineProperty(f, 'namespaceURI', {get: function () { calls++; throw new Error('page namespace'); }, configurable: true});
        try {
            return f.contentWindow === 17 && window.chat === w && frames.chat === w && window.idOnly === document.querySelector('#idOnly') && calls === 0;
        } finally {
            Reflect.apply = originalApply;
            for (var method of methods) delete method.call;
        }
    })()"#);
}

#[test]
fn named_child_rename_removal_and_expandos_are_live() {
    let (engine, mut page) = load("<!doctype html><div id=first></div>");
    assert_js(&engine, &mut page, r#"(function () {
        var fallback = document.getElementById('first'), f = document.createElement('iframe');
        f.name = 'first'; document.body.appendChild(f);
        var w = f.contentWindow;
        if (window.first !== w || Object.prototype.hasOwnProperty.call(window, 'first')) return false;
        f.name = 'second';
        if (window.first !== fallback || window.second !== w) return false;
        window.second = {manual: true};
        f.name = 'third';
        if (!window.second.manual || window.third !== w) return false;
        // A frames read must not install an own getter that outlives the
        // current name or shadows the element fallback after removal.
        if (frames.third !== w || Object.prototype.hasOwnProperty.call(window, 'third')) return false;
        f.remove();
        return typeof window.third === 'undefined' && window.first === fallback && window.second.manual;
    })()"#);
}
