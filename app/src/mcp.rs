//! Model Context Protocol server: JSON-RPC 2.0 over stdio, one JSON object
//! per line. Exposes the engine as tools (`browse`, `links`, `screenshot`,
//! `fetch_html`, `dom`, `cookies`, and the interactive `evaluate`, `click`,
//! `fill`) so LLM agents can read and use the web through RabbitHole.

use std::io::{BufRead, Write};

use rabbithole_engine::net::{decode_text, Body, Request, Url};
use rabbithole_engine::{Loaded, Page, Viewport};
use serde_json::{json, Value};

use crate::cli::{capture, cookie_lines, encode_png, CaptureOptions, Session};

const PROTOCOL_VERSION: &str = "2025-06-18";
const SERVER_NAME: &str = "rabbithole";

/// Long-lived server state (the engine keeps fonts and caches; the cookie
/// jar is the persistent profile jar).
pub struct ServerState {
    session: Option<Session>,
    initialized: bool,
    /// The page the interactive tools (`evaluate`, `click`, `fill`) act on.
    current: Option<Page>,
    /// JavaScript options of the current call (`javascript`, `wait_ms`).
    js: (bool, u64),
}

impl Default for ServerState {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerState {
    pub fn new() -> ServerState {
        ServerState { session: None, initialized: false, current: None, js: (true, 1500) }
    }

    /// The engine is created lazily so protocol-only interactions (and
    /// tests) do not pay for font loading.
    fn session(&mut self) -> &mut Session {
        let (js, wait) = self.js;
        let session = self.session.get_or_insert_with(|| Session::new(true));
        session.engine.scripting = js;
        session.engine.loader.load_scripts = js;
        session.wait_ms = if js { wait } else { 0 };
        session
    }

    /// Record the `javascript` / `wait_ms` tool arguments; they are applied
    /// whenever the session is used.
    fn configure_js(&mut self, args: &Value) {
        self.js = (arg_bool(args, "javascript", true), arg_u64(args, "wait_ms", 1500).min(30_000));
    }

    /// The page for an interactive tool: `url` (loaded fresh when it differs
    /// from the current page or when `reload` is true) or the current page.
    fn page_for(&mut self, args: &Value) -> Result<(), String> {
        let url = args.get("url").and_then(Value::as_str).map(str::to_string);
        let reload = arg_bool(args, "reload", false);
        let need_load = match (&url, &self.current) {
            (Some(u), Some(p)) => reload || Url::parse(u).map(|u| u.without_fragment() != p.url.without_fragment()).unwrap_or(true),
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => return Err("no current page: pass `url`".into()),
        };
        if need_load {
            let u = url.unwrap_or_default();
            let session = self.session();
            session.engine.loader.load_images = false;
            session.engine.loader.load_fonts = false;
            let page = crate::cli::load_text_page(session, &u)?;
            self.current = Some(page);
        }
        Ok(())
    }

    /// Engine session and current page, borrowed together.
    fn parts(&mut self) -> (&Session, &mut Page) {
        self.session();
        (self.session.as_ref().expect("session"), self.current.as_mut().expect("current page"))
    }

    /// Navigate the current page with a request (after a click/submit),
    /// letting scripts settle.
    fn navigate_current(&mut self, req: &Request) -> Result<(), String> {
        let session = self.session();
        let loaded = session.engine.loader.load_request(req, DEFAULT_VIEWPORT).map_err(|e| e.to_string())?;
        let mut page = match loaded {
            Loaded::Page(p) => session.engine.build_page(p, DEFAULT_VIEWPORT),
            Loaded::Download(s) => return Err(format!("{} is a download ({}), not a page", s.url, s.content_type)),
        };
        let wait = session.wait_ms;
        if let Some(nav) = crate::scripting::settle(&session.engine, &mut page, wait) {
            if let Ok(Loaded::Page(p)) = session.engine.loader.load_request(&nav.request, DEFAULT_VIEWPORT) {
                page = session.engine.build_page(p, DEFAULT_VIEWPORT);
                let _ = crate::scripting::settle(&session.engine, &mut page, wait);
            }
        }
        session.flush_console(&mut page);
        session.save();
        self.current = Some(page);
        Ok(())
    }

    /// Text summary of the current page.
    fn current_text(&mut self, max_chars: usize) -> String {
        match &self.current {
            Some(p) => format!("# {}\nURL: {}\n\n{}", p.title.trim(), p.url, truncate(&p.text(), max_chars.max(1))),
            None => "(no page)".into(),
        }
    }
}

/// Run the server until stdin closes.
pub fn serve() -> Result<(), String> {
    let mut state = ServerState::new();
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    eprintln!("rabbithole mcp: ready (protocol {PROTOCOL_VERSION})");
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| format!("stdin error: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle_message(&mut state, &line) {
            let mut out = stdout.lock();
            out.write_all(reply.as_bytes())
                .and_then(|_| out.write_all(b"\n"))
                .and_then(|_| out.flush())
                .map_err(|e| format!("stdout error: {e}"))?;
        }
    }
    if let Some(s) = &state.session {
        s.save();
    }
    Ok(())
}

/// Handle one JSON-RPC message. Returns the serialized response, or None
/// for notifications.
pub fn handle_message(state: &mut ServerState, line: &str) -> Option<String> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Some(error_response(Value::Null, -32700, &format!("parse error: {e}")).to_string()),
    };
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let method = match msg.get("method").and_then(Value::as_str) {
        Some(m) => m.to_string(),
        None => {
            if msg.get("result").is_some() || msg.get("error").is_some() {
                return None;
            }
            return Some(error_response(id, -32600, "invalid request: missing method").to_string());
        }
    };
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let is_notification = msg.get("id").is_none();
    let result = dispatch(state, &method, &params);
    if is_notification {
        return None;
    }
    Some(
        match result {
            Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
            Err((code, message)) => error_response(id, code, &message),
        }
        .to_string(),
    )
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn dispatch(state: &mut ServerState, method: &str, params: &Value) -> Result<Value, (i64, String)> {
    match method {
        "initialize" => {
            state.initialized = true;
            let requested = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION);
            Ok(json!({
                "protocolVersion": requested,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
                "instructions": "RabbitHole renders web pages with its own engine. Use `browse` to read a page as text (GET, or POST a form with `method`/`form`), `links` to list its links, `screenshot` to see it rendered, `fetch_html` for raw source, `dom` for the parsed tree and `cookies` to inspect stored cookies. Cookies persist between calls."
            }))
        }
        "notifications/initialized" | "notifications/cancelled" => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).ok_or((-32602, "missing tool name".to_string()))?;
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            Ok(match call_tool(state, name, &args) {
                Ok(content) => json!({ "content": content, "isError": false }),
                Err(message) => json!({ "content": [{ "type": "text", "text": message }], "isError": true }),
            })
        }
        "resources/list" => Ok(json!({ "resources": [] })),
        "prompts/list" => Ok(json!({ "prompts": [] })),
        _ => Err((-32601, format!("method not found: {method}"))),
    }
}

/// JSON schemas of the exposed tools.
fn tool_definitions() -> Vec<Value> {
    let url_prop = json!({ "type": "string", "description": "Absolute URL (http, https, file, data or about)" });
    let js_prop = json!({ "type": "boolean", "description": "Run the page's JavaScript (default true)", "default": true });
    let wait_prop = json!({ "type": "integer", "description": "Milliseconds to let page timers run after load so script-rendered content appears (default 1500)", "default": 1500 });
    let mut tools = vec![
        json!({
            "name": "browse",
            "description": "Load a web page with the RabbitHole engine and return its readable text (title, final URL, headings, paragraphs, lists and links). Optionally submit a form with method POST and `form` fields.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": url_prop,
                    "max_chars": { "type": "integer", "description": "Truncate the text to this many characters (default 20000)", "default": 20000 },
                    "images": { "type": "boolean", "description": "Download images while loading (default false)", "default": false },
                    "method": { "type": "string", "description": "HTTP method: GET (default) or POST", "enum": ["GET", "POST"] },
                    "form": { "type": "object", "description": "Form fields sent as application/x-www-form-urlencoded (POST) or as the query string (GET)", "additionalProperties": { "type": "string" } }
                },
                "required": ["url"]
            }
        }),
        json!({
            "name": "links",
            "description": "List the hyperlinks of a page as `text — href` lines (max 500).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": url_prop,
                    "filter": { "type": "string", "description": "Only links whose text or href contains this substring (case-insensitive)" }
                },
                "required": ["url"]
            }
        }),
        json!({
            "name": "screenshot",
            "description": "Render a page with the RabbitHole engine and return a PNG screenshot.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": url_prop,
                    "width": { "type": "integer", "description": "Viewport width in CSS px (default 1280)", "default": 1280 },
                    "height": { "type": "integer", "description": "Viewport height in CSS px (default 800)", "default": 800 },
                    "scale": { "type": "number", "description": "Device scale factor (default 1)", "default": 1 },
                    "full_page": { "type": "boolean", "description": "Capture the entire page height (max 20000px)", "default": false },
                    "scroll_y": { "type": "number", "description": "Vertical scroll offset before capturing", "default": 0 }
                },
                "required": ["url"]
            }
        }),
        json!({
            "name": "fetch_html",
            "description": "Fetch a URL and return the raw (decoded) HTML source without rendering.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": url_prop,
                    "max_chars": { "type": "integer", "description": "Truncate to this many characters (default 50000)", "default": 50000 }
                },
                "required": ["url"]
            }
        }),
        json!({
            "name": "dom",
            "description": "Load a page and return its parsed DOM tree as an indented dump.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": url_prop,
                    "max_chars": { "type": "integer", "description": "Truncate to this many characters (default 50000)", "default": 50000 }
                },
                "required": ["url"]
            }
        }),
        json!({
            "name": "cookies",
            "description": "List the stored cookies that apply to a URL (name=value, domain, path, expiry, flags).",
            "inputSchema": {
                "type": "object",
                "properties": { "url": url_prop },
                "required": ["url"]
            }
        }),
    ];
    // Every page-loading tool accepts the JavaScript options.
    for t in tools.iter_mut() {
        if t["name"] != "cookies" && t["name"] != "fetch_html" {
            t["inputSchema"]["properties"]["javascript"] = js_prop.clone();
            t["inputSchema"]["properties"]["wait_ms"] = wait_prop.clone();
        }
    }
    let url_opt = json!({ "type": "string", "description": "Page to load first. Omit to act on the current page (the one left by the previous evaluate/click/fill call)." });
    tools.push(json!({
        "name": "evaluate",
        "description": "Evaluate a JavaScript expression in a page (after its own scripts ran) and return the result as text. The page becomes the current page for click/fill.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "url": url_opt,
                "expression": { "type": "string", "description": "JavaScript to evaluate, e.g. `document.title` or `[...document.querySelectorAll('h2')].map(h => h.textContent)`" },
                "reload": { "type": "boolean", "description": "Reload even if `url` is the current page", "default": false },
                "wait_ms": wait_prop
            },
            "required": ["expression"]
        }
    }));
    tools.push(json!({
        "name": "click",
        "description": "Click the first element matching a CSS selector (dispatching mousedown/mouseup/click to page scripts, then following links or submitting forms like a user would) and return the resulting page text.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "url": url_opt,
                "selector": { "type": "string", "description": "CSS selector of the element to click, e.g. `a[href*=login]` or `button.submit`" },
                "max_chars": { "type": "integer", "description": "Truncate the returned text (default 20000)", "default": 20000 },
                "wait_ms": wait_prop
            },
            "required": ["selector"]
        }
    }));
    tools.push(json!({
        "name": "click_at",
        "description": "Click at viewport coordinates (CSS pixels from the top-left of the visible page, as in a screenshot) like a real mouse: hit-tests what is painted there, dispatches pointer/mouse events and the click, then follows links or submits forms. Returns the clicked element, which viewport regions visibly changed (when the page did not navigate) and the resulting page text.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "url": url_opt,
                "x": { "type": "number", "description": "Viewport x in CSS pixels" },
                "y": { "type": "number", "description": "Viewport y in CSS pixels" },
                "max_chars": { "type": "integer", "description": "Truncate the returned text (default 20000)", "default": 20000 },
                "wait_ms": wait_prop
            },
            "required": ["x", "y"]
        }
    }));
    tools.push(json!({
        "name": "fill",
        "description": "Fill form fields (text inputs, textareas, checkboxes with true/false, selects by value or label) firing input/change events, optionally submit the form, and return the resulting page text.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "url": url_opt,
                "fields": { "type": "object", "description": "Map of CSS selector → value", "additionalProperties": { "type": "string" } },
                "submit": { "type": "boolean", "description": "Submit the form of the first field afterwards (default false)", "default": false },
                "max_chars": { "type": "integer", "description": "Truncate the returned text (default 20000)", "default": 20000 },
                "wait_ms": wait_prop
            },
            "required": ["fields"]
        }
    }));
    tools
}

fn arg_str<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args.get(name).and_then(Value::as_str).filter(|s| !s.trim().is_empty()).ok_or_else(|| format!("missing required argument `{name}`"))
}

fn arg_u64(args: &Value, name: &str, default: u64) -> u64 {
    args.get(name).and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f.max(0.0) as u64))).unwrap_or(default)
}

fn arg_f64(args: &Value, name: &str, default: f64) -> f64 {
    args.get(name).and_then(Value::as_f64).unwrap_or(default)
}

fn arg_bool(args: &Value, name: &str, default: bool) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(default)
}

/// Truncate to `max_chars` characters, appending a marker when cut.
fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push_str("\n…[truncated]");
    out
}

fn text_content(text: String) -> Vec<Value> {
    vec![json!({ "type": "text", "text": text })]
}

const DEFAULT_VIEWPORT: Viewport = Viewport { width: 1280.0, height: 800.0 };

/// Build the request for `browse`: GET (optionally with form fields in the
/// query) or POST with a urlencoded body.
pub fn browse_request(url: &str, args: &Value) -> Result<Request, String> {
    let mut parsed = Url::parse(url).ok_or_else(|| format!("invalid URL: {url}"))?;
    let method = args.get("method").and_then(Value::as_str).unwrap_or("GET").to_ascii_uppercase();
    let fields: Vec<(String, String)> = args
        .get("form")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), match v { Value::String(s) => s.clone(), other => other.to_string() }))
                .collect()
        })
        .unwrap_or_default();
    match method.as_str() {
        "POST" => Ok(Request::post(parsed, Body::form(&fields))),
        "GET" => {
            if !fields.is_empty() {
                let q = fields
                    .iter()
                    .map(|(k, v)| format!("{}={}", rabbithole_engine::net::form_encode(k), rabbithole_engine::net::form_encode(v)))
                    .collect::<Vec<_>>()
                    .join("&");
                parsed.query = Some(q);
            }
            Ok(Request::get(parsed))
        }
        other => Err(format!("unsupported method: {other}")),
    }
}

fn call_tool(state: &mut ServerState, name: &str, args: &Value) -> Result<Vec<Value>, String> {
    if name != "cookies" && name != "fetch_html" {
        state.configure_js(args);
    }
    match name {
        "browse" => {
            let url = arg_str(args, "url")?;
            let max_chars = arg_u64(args, "max_chars", 20_000) as usize;
            let req = browse_request(url, args)?;
            let session = state.session();
            session.engine.loader.load_images = arg_bool(args, "images", false);
            session.engine.loader.load_fonts = false;
            let page = session.load_request(&req, DEFAULT_VIEWPORT)?;
            let body = truncate(&page.text(), max_chars.max(1));
            Ok(text_content(format!("# {}\nURL: {}\n\n{}", page.title.trim(), page.url, body)))
        }
        "links" => {
            let url = arg_str(args, "url")?;
            let filter = args.get("filter").and_then(Value::as_str).map(|f| f.to_lowercase());
            let session = state.session();
            let page = crate::cli::load_text_page(session, url)?;
            let mut lines = Vec::new();
            for link in page.links() {
                let text = link.text.split_whitespace().collect::<Vec<_>>().join(" ");
                let href = link.href.to_string();
                if let Some(f) = &filter {
                    if !text.to_lowercase().contains(f) && !href.to_lowercase().contains(f) {
                        continue;
                    }
                }
                lines.push(format!("{text} — {href}"));
                if lines.len() >= 500 {
                    lines.push("…[more links omitted]".to_string());
                    break;
                }
            }
            if lines.is_empty() {
                lines.push("(no links)".to_string());
            }
            Ok(text_content(lines.join("\n")))
        }
        "screenshot" => {
            let url = arg_str(args, "url")?;
            let opts = CaptureOptions {
                width: arg_u64(args, "width", 1280).clamp(1, 8192) as u32,
                height: arg_u64(args, "height", 800).clamp(1, 8192) as u32,
                scale: arg_f64(args, "scale", 1.0).clamp(0.25, 4.0) as f32,
                full_page: arg_bool(args, "full_page", false),
                scroll_y: arg_f64(args, "scroll_y", 0.0) as f32,
                images: true,
            };
            let session = state.session();
            session.engine.loader.load_fonts = true;
            let cap = capture(session, url, &opts)?;
            let png = encode_png(&cap)?;
            Ok(vec![
                json!({ "type": "image", "data": base64_encode(&png), "mimeType": "image/png" }),
                json!({ "type": "text", "text": format!("{} — {} ({}x{} px)", cap.title.trim(), cap.url, cap.width, cap.height) }),
            ])
        }
        "fetch_html" => {
            let url = arg_str(args, "url")?;
            let max_chars = arg_u64(args, "max_chars", 50_000) as usize;
            let parsed = Url::parse(url).ok_or_else(|| format!("invalid URL: {url}"))?;
            let session = state.session();
            let response = session.engine.loader.fetcher.request(&Request::get(parsed)).map_err(|e| e.to_string())?;
            session.save();
            let text = decode_text(&response.body, response.charset.as_deref());
            Ok(text_content(format!(
                "URL: {}\nStatus: {}\nContent-Type: {}\n\n{}",
                response.url,
                response.status,
                response.content_type,
                truncate(&text, max_chars.max(1))
            )))
        }
        "dom" => {
            let url = arg_str(args, "url")?;
            let max_chars = arg_u64(args, "max_chars", 50_000) as usize;
            let session = state.session();
            let page = crate::cli::load_text_page(session, url)?;
            Ok(text_content(truncate(&page.dom_dump(), max_chars.max(1))))
        }
        "cookies" => {
            let url = arg_str(args, "url")?;
            let parsed = Url::parse(url).ok_or_else(|| format!("invalid URL: {url}"))?;
            let session = state.session();
            let jar = session.cookies.lock().map_err(|_| "cookie jar poisoned".to_string())?;
            let lines = cookie_lines(&jar, &parsed);
            Ok(text_content(if lines.is_empty() { "(no cookies)".to_string() } else { lines.join("\n") }))
        }
        "evaluate" => {
            let expr = arg_str(args, "expression")?.to_string();
            state.page_for(args)?;
            let (session, page) = state.parts();
            let result = session.engine.eval_in_page(page, &expr);
            let _ = crate::scripting::settle(&session.engine, page, 0);
            let console: Vec<String> = page.take_console().into_iter().map(|(l, m)| format!("[console.{l}] {m}")).collect();
            let mut text = result;
            if !console.is_empty() {
                text.push_str("\n\n");
                text.push_str(&console.join("\n"));
            }
            Ok(text_content(text))
        }
        "click" => {
            let selector = arg_str(args, "selector")?.to_string();
            let max_chars = arg_u64(args, "max_chars", 20_000) as usize;
            state.page_for(args)?;
            let (session, page) = state.parts();
            let node = crate::scripting::query_selector(&page.doc, &selector)?.ok_or_else(|| format!("no element matches `{selector}`"))?;
            let engine = &session.engine;
            let wait = session.wait_ms;
            let before = engine.snapshot(page);
            let request = crate::scripting::click_element(engine, page, node);
            let change = match request {
                Some(req) => {
                    state.navigate_current(&req)?;
                    String::new()
                }
                None => {
                    let (session, page) = state.parts();
                    match crate::scripting::settle(&session.engine, page, wait) {
                        Some(nav) => {
                            state.navigate_current(&nav.request)?;
                            String::new()
                        }
                        None => visual_change(&before, &session.engine.snapshot(page)),
                    }
                }
            };
            Ok(text_content(format!("{change}{}", state.current_text(max_chars))))
        }
        "click_at" => {
            let x = args.get("x").and_then(Value::as_f64).ok_or("`x` must be a number")? as f32;
            let y = args.get("y").and_then(Value::as_f64).ok_or("`y` must be a number")? as f32;
            let max_chars = arg_u64(args, "max_chars", 20_000) as usize;
            state.page_for(args)?;
            let (session, page) = state.parts();
            let engine = &session.engine;
            let wait = session.wait_ms;
            let before = engine.snapshot(page);
            let (clicked, request) = crate::scripting::click_at(engine, page, x, y);
            let what = match clicked {
                Some(n) => format!("Clicked <{}>", crate::scripting::describe_node(&page.doc, n)),
                None => format!("Nothing clickable at ({x}, {y})"),
            };
            let change = match request {
                Some(req) => {
                    state.navigate_current(&req)?;
                    String::new()
                }
                None => {
                    let (session, page) = state.parts();
                    match crate::scripting::settle(&session.engine, page, wait) {
                        Some(nav) => {
                            state.navigate_current(&nav.request)?;
                            String::new()
                        }
                        None => visual_change(&before, &session.engine.snapshot(page)),
                    }
                }
            };
            Ok(text_content(format!("{what}\n{change}\n{}", state.current_text(max_chars))))
        }
        "fill" => {
            let max_chars = arg_u64(args, "max_chars", 20_000) as usize;
            let fields = args.get("fields").and_then(Value::as_object).ok_or("`fields` must be an object of selector → value")?.clone();
            let submit = arg_bool(args, "submit", false);
            state.page_for(args)?;
            let (session, page) = state.parts();
            let engine = &session.engine;
            let mut first = None;
            for (selector, value) in &fields {
                let value = match value {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                let node = crate::scripting::query_selector(&page.doc, selector)?.ok_or_else(|| format!("no element matches `{selector}`"))?;
                let control = page.control_of(node).unwrap_or(node);
                crate::scripting::fill_control(engine, page, control, &value)?;
                first.get_or_insert(control);
            }
            if submit {
                let control = first.ok_or("no fields given")?;
                let form = page.form_of(control).ok_or("the first field is not inside a form")?;
                let out = engine.dispatch_event(page, form, "submit", rabbithole_engine::scripting::plain_init(true));
                let req = match out.navigation {
                    Some(nav) => Some(nav.request),
                    None if out.default_prevented => None,
                    None => {
                        let submitter = page.default_submitter(form);
                        page.submit_request(form, submitter, &|_| Vec::new())
                    }
                };
                if let Some(req) = req {
                    state.navigate_current(&req)?;
                }
            }
            Ok(text_content(state.current_text(max_chars)))
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

/// Describe what changed on screen between two viewport snapshots (empty
/// line when nothing did), so an agent knows whether its action had a
/// visible effect and where.
fn visual_change(before: &rabbithole_engine::diff::Frame, after: &rabbithole_engine::diff::Frame) -> String {
    let regions = rabbithole_engine::diff::changed_regions(before, after);
    if regions.is_empty() {
        return "No visible change in the viewport.\n".to_string();
    }
    let list: Vec<String> = regions.iter().take(12).map(|r| format!("({}, {}, {}x{})", r.x, r.y, r.w, r.h)).collect();
    let more = if regions.len() > 12 { format!(" and {} more", regions.len() - 12) } else { String::new() };
    format!("Visible change in {} region(s) of the viewport (x, y, w×h): {}{more}\n", regions.len(), list.join(", "))
}

/// Standard base64 (RFC 4648) with padding.
pub fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> ServerState {
        ServerState::new()
    }

    fn parse(reply: Option<String>) -> Value {
        serde_json::from_str(&reply.expect("expected a response")).unwrap()
    }

    #[test]
    fn initialize_shape() {
        let mut s = state();
        let reply = parse(handle_message(
            &mut s,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
        ));
        assert_eq!(reply["id"], 1);
        assert_eq!(reply["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(reply["result"]["serverInfo"]["name"], "rabbithole");
        assert!(reply["result"]["capabilities"]["tools"].is_object());
        assert!(s.initialized);
        assert!(handle_message(&mut s, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    }

    #[test]
    fn tools_list_has_all_tools() {
        let mut s = state();
        let reply = parse(handle_message(&mut s, r#"{"jsonrpc":"2.0","id":"abc","method":"tools/list"}"#));
        assert_eq!(reply["id"], "abc");
        let tools = reply["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["browse", "links", "screenshot", "fetch_html", "dom", "cookies", "evaluate", "click", "click_at", "fill"]);
        for t in tools.iter().take(6) {
            assert_eq!(t["inputSchema"]["required"][0], "url");
            assert!(t["description"].as_str().unwrap().len() > 10);
        }
    }

    #[test]
    fn errors() {
        let mut s = state();
        let reply = parse(handle_message(&mut s, r#"{"jsonrpc":"2.0","id":2,"method":"nope"}"#));
        assert_eq!(reply["error"]["code"], -32601);
        let reply = parse(handle_message(&mut s, "{not json"));
        assert_eq!(reply["error"]["code"], -32700);
        assert!(reply["id"].is_null());
        let reply = parse(handle_message(&mut s, r#"{"jsonrpc":"2.0","id":3}"#));
        assert_eq!(reply["error"]["code"], -32600);
        let reply = parse(handle_message(&mut s, r#"{"jsonrpc":"2.0","id":4,"method":"ping"}"#));
        assert!(reply["result"].is_object());
    }

    #[test]
    fn tool_call_argument_validation() {
        let mut s = state();
        let reply = parse(handle_message(
            &mut s,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"browse","arguments":{}}}"#,
        ));
        assert_eq!(reply["result"]["isError"], true);
        assert!(reply["result"]["content"][0]["text"].as_str().unwrap().contains("url"));
        let reply = parse(handle_message(
            &mut s,
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"bogus","arguments":{"url":"x"}}}"#,
        ));
        assert_eq!(reply["result"]["isError"], true);
        assert!(s.session.is_none(), "no engine should be created for invalid calls");
    }

    #[test]
    fn browse_request_building() {
        let args = json!({ "method": "POST", "form": { "q": "a b", "n": 1 } });
        let req = browse_request("https://example.com/s", &args).unwrap();
        assert_eq!(req.method, "POST");
        let body = req.body.unwrap();
        assert_eq!(body.content_type, "application/x-www-form-urlencoded");
        let text = String::from_utf8(body.data).unwrap();
        assert!(text.contains("q=a+b") && text.contains("n=1"));
        let req = browse_request("https://example.com/s", &json!({ "form": { "q": "x" } })).unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.url.query.as_deref(), Some("q=x"));
        assert!(browse_request("https://example.com/", &json!({ "method": "PUT" })).is_err());
    }

    #[test]
    fn base64_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(&[0xff, 0x00, 0x10]), "/wAQ");
    }

    #[test]
    fn truncation() {
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("abcdef", 3), "abc\n…[truncated]");
    }
}
