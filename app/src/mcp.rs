//! Model Context Protocol server: JSON-RPC 2.0 over stdio, one JSON object
//! per line. Exposes the engine as tools (`browse`, `links`, `screenshot`,
//! `fetch_html`, `dom`) so LLM agents can read the web through RabbitHole.

use std::io::{BufRead, Write};

use rabbithole_engine::net::{decode_text, Fetcher, Url};
use rabbithole_engine::{Engine, Viewport};
use serde_json::{json, Value};

use crate::cli::{capture, encode_png, CaptureOptions};

const PROTOCOL_VERSION: &str = "2025-06-18";
const SERVER_NAME: &str = "rabbithole";

/// Long-lived server state (the engine keeps fonts and an image cache).
pub struct ServerState {
    engine: Engine,
    initialized: bool,
}

impl Default for ServerState {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerState {
    pub fn new() -> ServerState {
        ServerState {
            engine: Engine::new(),
            initialized: false,
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
                return None; // a response to something we sent; we never send requests
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
            let requested = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSION);
            Ok(json!({
                "protocolVersion": requested,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
                "instructions": "RabbitHole renders web pages with its own engine (no JavaScript). Use `browse` to read a page as text, `links` to list its links, `screenshot` to see it rendered, `fetch_html` for raw source and `dom` for the parsed tree."
            }))
        }
        "notifications/initialized" | "notifications/cancelled" => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or((-32602, "missing tool name".to_string()))?;
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            Ok(match call_tool(state, name, &args) {
                Ok(content) => json!({ "content": content, "isError": false }),
                Err(message) => json!({
                    "content": [{ "type": "text", "text": message }],
                    "isError": true
                }),
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
    vec![
        json!({
            "name": "browse",
            "description": "Load a web page with the RabbitHole engine and return its readable text (title, final URL, headings, paragraphs, lists and links). No JavaScript is executed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": url_prop,
                    "max_chars": { "type": "integer", "description": "Truncate the text to this many characters (default 20000)", "default": 20000 },
                    "images": { "type": "boolean", "description": "Download images while loading (default false)", "default": false }
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
                    "width": { "type": "integer", "description": "Viewport width in px (default 1280)", "default": 1280 },
                    "height": { "type": "integer", "description": "Viewport height in px (default 800)", "default": 800 },
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
    ]
}

fn arg_str<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args.get(name)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("missing required argument `{name}`"))
}

fn arg_u64(args: &Value, name: &str, default: u64) -> u64 {
    args.get(name)
        .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f.max(0.0) as u64)))
        .unwrap_or(default)
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

const DEFAULT_VIEWPORT: Viewport = Viewport {
    width: 1280.0,
    height: 800.0,
};

fn call_tool(state: &mut ServerState, name: &str, args: &Value) -> Result<Vec<Value>, String> {
    match name {
        "browse" => {
            let url = arg_str(args, "url")?;
            let max_chars = arg_u64(args, "max_chars", 20_000) as usize;
            state.engine.load_images = arg_bool(args, "images", false);
            let page = state.engine.load(url, DEFAULT_VIEWPORT).map_err(|e| e.to_string())?;
            let body = truncate(&page.text(), max_chars.max(1));
            Ok(text_content(format!("# {}\nURL: {}\n\n{}", page.title.trim(), page.url, body)))
        }
        "links" => {
            let url = arg_str(args, "url")?;
            let filter = args.get("filter").and_then(Value::as_str).map(|f| f.to_lowercase());
            state.engine.load_images = false;
            let page = state.engine.load(url, DEFAULT_VIEWPORT).map_err(|e| e.to_string())?;
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
                full_page: arg_bool(args, "full_page", false),
                scroll_y: arg_f64(args, "scroll_y", 0.0) as f32,
                images: true,
            };
            let cap = capture(&mut state.engine, url, &opts)?;
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
            let fetcher = Fetcher::default();
            let response = fetcher.fetch(&parsed).map_err(|e| e.to_string())?;
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
            state.engine.load_images = false;
            let page = state.engine.load(url, DEFAULT_VIEWPORT).map_err(|e| e.to_string())?;
            Ok(text_content(truncate(&page.dom_dump(), max_chars.max(1))))
        }
        other => Err(format!("unknown tool: {other}")),
    }
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
        // notifications get no reply
        assert!(handle_message(&mut s, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    }

    #[test]
    fn tools_list_has_all_tools() {
        let mut s = state();
        let reply = parse(handle_message(&mut s, r#"{"jsonrpc":"2.0","id":"abc","method":"tools/list"}"#));
        assert_eq!(reply["id"], "abc");
        let tools = reply["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["browse", "links", "screenshot", "fetch_html", "dom"]);
        for t in tools {
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
        // Missing url: reported as a tool error, not a protocol error, and no network is touched.
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
