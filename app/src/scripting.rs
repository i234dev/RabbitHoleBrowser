//! Helpers shared by the GUI, the CLI and the MCP server for script-driven
//! pages: settling timers, querying elements by CSS selector, dispatching
//! synthetic events and keeping a console log.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use rabbithole_engine::css::selector::SelectorParser;
use rabbithole_engine::css::{matches, Tokenizer};
use rabbithole_engine::dom::{Document, NodeId};
use rabbithole_engine::net::Url;
use rabbithole_engine::scripting::{input_init, plain_init, PendingNavigation};
use rabbithole_engine::net::Request;
use rabbithole_engine::{ClickAction, ControlKind, Engine, Modifiers, Page};

/// Maximum number of console lines kept per page.
pub const CONSOLE_CAPACITY: usize = 500;

/// A ring buffer of console messages (level, text).
#[derive(Debug, Default, Clone)]
pub struct ConsoleLog {
    lines: VecDeque<(String, String)>,
}

impl ConsoleLog {
    pub fn push(&mut self, level: impl Into<String>, text: impl Into<String>) {
        if self.lines.len() >= CONSOLE_CAPACITY {
            self.lines.pop_front();
        }
        self.lines.push_back((level.into(), text.into()));
    }

    pub fn extend(&mut self, lines: Vec<(String, String)>) {
        for (l, t) in lines {
            self.push(l, t);
        }
    }

    pub fn lines(&self) -> impl DoubleEndedIterator<Item = &(String, String)> + ExactSizeIterator {
        self.lines.iter()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// Number of error-level lines.
    pub fn error_count(&self) -> usize {
        self.lines.iter().filter(|(l, _)| l == "error").count()
    }
}

/// First element of `doc` matching the CSS selector list.
pub fn query_selector(doc: &Document, selector: &str) -> Result<Option<NodeId>, String> {
    let toks = Tokenizer::tokenize(selector);
    let list = SelectorParser::new(&toks).parse_list().ok_or_else(|| format!("invalid selector: {selector}"))?;
    Ok(doc
        .descendants(Document::ROOT)
        .find(|&n| doc.element(n).is_some() && list.iter().any(|s| matches(s, doc, n))))
}

/// Run due timers and microtasks for up to `wait_ms`, sleeping between
/// timers like a browser would, so dynamically generated content settles.
/// Returns the last outcome with a navigation request, if any.
pub fn settle(engine: &Engine, page: &mut Page, wait_ms: u64) -> Option<PendingNavigation> {
    let deadline = Instant::now() + Duration::from_millis(wait_ms);
    let mut navigation = None;
    loop {
        let Some(due) = page.next_script_due() else { break };
        let now_ms = engine.start.elapsed().as_secs_f64() * 1000.0;
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        if due > now_ms {
            let wait = Duration::from_millis((due - now_ms).ceil() as u64).min(deadline.saturating_duration_since(now));
            if wait.is_zero() {
                break;
            }
            std::thread::sleep(wait);
            if Instant::now() >= deadline {
                break;
            }
        }
        let dbg = std::env::var_os("RH_DEBUG_SCRIPTS").is_some();
        let t = Instant::now();
        if dbg {
            eprintln!("[settle] tick start due={due:.0} now={now_ms:.0}");
        }
        let out = engine.tick(page);
        if dbg {
            eprintln!("[settle] tick done in {:?}", t.elapsed());
        }
        if out.navigation.is_some() {
            navigation = out.navigation;
            break;
        }
    }
    navigation
}

/// Turn a click's outcome into the request to navigate with, if any: a
/// script-requested navigation, a followed link, or a submitted form
/// (dispatching `submit` first).
pub fn click_request(engine: &Engine, page: &mut Page, action: ClickAction, navigation: Option<PendingNavigation>) -> Option<Request> {
    if let Some(nav) = navigation {
        return Some(nav.request);
    }
    match action {
        ClickAction::Navigate { url, .. } => Some(Request::get(url).with_referer(page.url.clone())),
        ClickAction::Submit { form, submitter } => {
            let out = engine.dispatch_event(page, form, "submit", plain_init(true));
            if out.default_prevented {
                return None;
            }
            if let Some(nav) = out.navigation {
                return Some(nav.request);
            }
            page.submit_request(form, submitter, &|_| Vec::new())
        }
        _ => None,
    }
}

/// Click an element like a user would (event sequence at its centre,
/// activation behaviour) and return the request to navigate with.
pub fn click_element(engine: &Engine, page: &mut Page, node: NodeId) -> Option<Request> {
    let (action, out) = engine.click_element(page, node, None, Modifiers::default());
    click_request(engine, page, action, out.navigation)
}

/// Click at viewport coordinates (hit test + full pointer event sequence).
/// Returns the clicked element and the request to navigate with.
pub fn click_at(engine: &Engine, page: &mut Page, x: f32, y: f32) -> (Option<NodeId>, Option<Request>) {
    let up = engine.click_at(page, x, y, Modifiers::default());
    let clicked = up.clicked;
    (clicked, click_request(engine, page, up.action, up.outcome.navigation))
}

/// Short description of an element for diagnostics: `tag#id.class`.
pub fn describe_node(doc: &Document, node: NodeId) -> String {
    let Some(el) = doc.element(node) else { return "#text".into() };
    let mut s = el.tag.clone();
    if let Some(id) = el.attr("id") {
        s.push('#');
        s.push_str(id);
    }
    if let Some(c) = el.attr("class") {
        for c in c.split_whitespace().take(3) {
            s.push('.');
            s.push_str(c);
        }
    }
    s
}

/// Set a control's value as a user would (typing, toggling, choosing an
/// option) and fire `input`/`change`.
pub fn fill_control(engine: &Engine, page: &mut Page, control: NodeId, value: &str) -> Result<(), String> {
    match page.control_kind(control) {
        ControlKind::Checkbox | ControlKind::Radio => {
            let want = matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes" | "checked");
            let is = page.doc.attr(control, "checked").is_some();
            if want != is {
                page.toggle_control(control);
            }
        }
        ControlKind::Select => {
            let options = page.select_options(control);
            let idx = options
                .iter()
                .position(|(label, v, _, _)| v == value || label == value)
                .or_else(|| options.iter().position(|(label, v, _, _)| v.eq_ignore_ascii_case(value) || label.eq_ignore_ascii_case(value)))
                .ok_or_else(|| format!("no option `{value}` in select"))?;
            page.select_option(control, idx);
        }
        ControlKind::Text | ControlKind::Password | ControlKind::TextArea | ControlKind::Other | ControlKind::Hidden => {
            let _ = engine.dispatch_event(page, control, "focus", rabbithole_engine::script::EventInit::simple(false, false));
            page.set_control_text(control, value);
        }
        other => return Err(format!("cannot fill a {other:?} control")),
    }
    engine.restyle(page);
    let _ = engine.dispatch_event(page, control, "input", input_init(Some(value)));
    let _ = engine.dispatch_event(page, control, "change", plain_init(false));
    Ok(())
}

/// Percent-decode and evaluate a `javascript:` URL in the page.
pub fn run_javascript_url(engine: &Engine, page: &mut Page, url: &Url) -> String {
    let code = rabbithole_engine::net::url::percent_decode(&url.path);
    engine.eval_in_page(page, &code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rabbithole_engine::Viewport;

    #[test]
    fn console_ring_buffer() {
        let mut c = ConsoleLog::default();
        for i in 0..(CONSOLE_CAPACITY + 10) {
            c.push(if i % 2 == 0 { "log" } else { "error" }, format!("m{i}"));
        }
        assert_eq!(c.len(), CONSOLE_CAPACITY);
        assert_eq!(c.lines().next().unwrap().1, "m10");
        assert_eq!(c.error_count(), CONSOLE_CAPACITY / 2);
        c.clear();
        assert!(c.is_empty());
    }

    #[test]
    fn selector_helper() {
        let doc = rabbithole_engine::html::parse("<div id=a><p class='x y'>one</p><p class=x>two</p></div>");
        let p = query_selector(&doc, "#a > p.x:not(.y)").unwrap().unwrap();
        assert_eq!(doc.text_content(p), "two");
        assert!(query_selector(&doc, "p.nope").unwrap().is_none());
        assert!(query_selector(&doc, "p[").is_err());
    }

    #[test]
    fn scripted_click_changes_dom() {
        let mut engine = Engine::new_bundled_fonts_only();
        engine.scripting = true;
        let html = r#"<html><body><button id="b">Go</button><p id="out">before</p>
            <script>
              document.getElementById('b').addEventListener('click', function (e) {
                e.preventDefault();
                document.getElementById('out').textContent = 'after';
                setTimeout(function () { document.getElementById('out').textContent += '!'; }, 10);
              });
              console.log('ready');
            </script></body></html>"#;
        let url = Url::parse("https://example.test/").unwrap();
        let mut page = engine.load_html(html, url, Viewport { width: 800.0, height: 600.0 });
        assert!(page.has_scripts());
        let logs = page.take_console();
        assert!(logs.iter().any(|(_, m)| m == "ready"), "{logs:?}");
        let button = query_selector(&page.doc, "#b").unwrap().unwrap();
        // The listener cancels the click: no navigation, page updated.
        assert!(click_element(&engine, &mut page, button).is_none());
        let p = query_selector(&page.doc, "#out").unwrap().unwrap();
        assert_eq!(page.doc.text_content(p), "after");
        settle(&engine, &mut page, 500);
        assert_eq!(page.doc.text_content(p), "after!");
        // Evaluate in the page.
        let r = engine.eval_in_page(&mut page, "document.querySelectorAll('p').length + 40");
        assert_eq!(r.trim(), "41");
    }

    #[test]
    fn fill_and_default_actions() {
        let mut engine = Engine::new_bundled_fonts_only();
        let html = r#"<html><body><form action="/go" method="get"><input id="q" name="q"><input type="checkbox" id="c" name="c"><select id="s" name="s"><option value="1">One</option><option value="2">Two</option></select><button id="go" type="submit">Go</button></form>
            <script>var n = 0; document.getElementById('q').addEventListener('input', function () { n++; });</script></body></html>"#;
        let url = Url::parse("https://example.test/").unwrap();
        let mut page = engine.load_html(html, url, Viewport { width: 800.0, height: 600.0 });
        let q = query_selector(&page.doc, "#q").unwrap().unwrap();
        fill_control(&engine, &mut page, q, "rabbits").unwrap();
        assert_eq!(page.control_text(q), "rabbits");
        assert_eq!(engine.eval_in_page(&mut page, "n").trim(), "1");
        let c = query_selector(&page.doc, "#c").unwrap().unwrap();
        fill_control(&engine, &mut page, c, "on").unwrap();
        assert!(page.doc.attr(c, "checked").is_some());
        let s = query_selector(&page.doc, "#s").unwrap().unwrap();
        fill_control(&engine, &mut page, s, "Two").unwrap();
        assert_eq!(engine.eval_in_page(&mut page, "document.getElementById('s').value").trim(), "'2'");
        let go = query_selector(&page.doc, "#go").unwrap().unwrap();
        let req = click_element(&engine, &mut page, go).expect("submission request");
        assert_eq!(req.method, "GET");
        assert!(req.url.query.as_deref().unwrap_or("").contains("q=rabbits"), "{:?}", req.url.query);
        assert!(req.url.query.as_deref().unwrap_or("").contains("s=2"));
    }
}
