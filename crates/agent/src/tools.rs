//! The agent tools (shared by the MCP server and the CLI): definitions,
//! argument handling and dispatch.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rabbithole_engine::css::selector::SelectorParser;
use rabbithole_engine::css::{matches, Tokenizer};
use rabbithole_engine::dom::{Document, NodeId};
use rabbithole_engine::net::{Request, Url};
use rabbithole_engine::scripting::ScriptOutcome;
use rabbithole_engine::DialogPolicy;
use serde_json::{json, Value};

use crate::ax::{clip_text, esc, Kind};
use crate::browser::{fence, Agent, Out};
use crate::refs::Ref;

/// Default snapshot budget (tokens).
pub const SNAPSHOT_TOKENS: usize = 2500;
/// Search windows are measured in original Unicode characters, including ellipses.
const FIND_CONTEXT: usize = 240;
const MAX_FIND_CONTEXT: usize = 4096;

/// Tool names, in the order they are listed.
pub const TOOLS: &[&str] = &["open", "snapshot", "read", "find", "click", "type", "fill", "scroll", "screenshot", "network", "tabs", "debug", "batch"];

/// JSON schemas of the tools (MCP `tools/list`).
pub fn definitions() -> Vec<Value> {
    let r = json!({ "type": "string", "description": "@eN or @fN" });
    let selector = json!({ "type": "string", "description": "Unique CSS, instead of ref" });
    let dialog = json!({ "type": "string", "description": "accept|dismiss|text" });
    // The descriptions say the shape (text or {selector:CSS}); the server
    // validates it, so the schema stays short in every tool that takes it.
    let until = json!({ "type": ["string", "object"] });
    let timeout = json!({ "type": "integer", "minimum": 0, "maximum": 30000 });
    vec![
        json!({ "name": "open", "description": "Load URL/history; default summary. find returns passages; summary=true adds overview; snapshot/full returns tree.",
            "inputSchema": { "type": "object", "properties": {
                "url": { "type": "string", "description": "URL|back|forward|reload" },
                "new_tab": { "type": "boolean" },
                "context": { "type": "string", "description": "Isolated cookies/storage" },
                "snapshot": { "type": "boolean", "description": "Return full semantic tree (alias: full)" },
                "full": { "type": "boolean" },
                "find": { "type": ["string", "array"], "items": { "type": "string" }, "description": "Any term; returns passages instead of overview" },
                "window": { "type": "integer", "minimum": 0, "maximum": 4096, "description": "Passage chars: default 240, cap 3x; 0: first 120" },
                "limit": { "type": "integer" },
                "summary": { "type": "boolean", "description": "Include overview with find" },
                "max_tokens": { "type": "integer", "description": "Full-snapshot budget (default 2500; implies snapshot=true)" },
                "bot": { "type": "boolean", "description": "RabbitHoleBot headers + robots.txt; default false (browser headers)" },
                "robots": { "type": "boolean", "description": "Respect robots.txt; defaults to bot mode" },
                "allowlist": { "type": "array", "items": { "type": "string" }, "description": "Only allow these domains from now on" },
                "javascript": { "type": "boolean" },
                "dialog": dialog
            }, "required": ["url"] } }),
        json!({ "name": "snapshot", "description": "Semantic tree with stable @eN refs; main content first; long lists truncated.",
            "inputSchema": { "type": "object", "properties": {
                "ref": { "type": "string", "description": "Expand only this element (e.g. a collapsed list or landmark)" },
                "max_tokens": { "type": "integer" },
                "hidden": { "type": "boolean", "description": "Also list hidden content, marked" },
                "urls": { "type": "boolean", "description": "Show link targets" }
            } } }),
        json!({ "name": "read", "description": "Main text as Markdown; default article beginning. find is cheaper for facts; links use refs.",
            "inputSchema": { "type": "object", "properties": {
                "ref": r.clone(), "max_tokens": { "type": "integer", "description": "Default 4000" }, "offset": { "type": "integer", "description": "Character offset to continue from" },
                "links": { "type": "boolean", "default": true }, "cites": { "type": "boolean", "default": false }
            } } }),
        json!({ "name": "find", "description": "Find any text terms with context; cheaper than read for facts. Also role/name or CSS selector; returns refs and absent terms.",
            "inputSchema": { "type": "object", "properties": {
                "text": { "type": ["string", "array"], "items": { "type": "string" } }, "role": { "type": "string" }, "name": { "type": "string" }, "selector": { "type": "string" }, "limit": { "type": "integer" },
                "window": { "type": "integer", "minimum": 0, "maximum": 4096, "description": "Passage chars: default 240, cap 3x; 0: first 120" },
                "context": { "type": "integer", "description": "Window alias" },
                "wait": { "type": "integer", "minimum": 0, "maximum": 30000 }, "gone": { "type": "boolean" }
            } } }),
        json!({ "name": "click", "description": "Click/hover ref or unique CSS selector; returns changes. ref=\"cookies\" rejects consent. until: text or {selector:CSS}; timeout ms≤30000.",
            "inputSchema": { "type": "object", "properties": {
                "ref": r.clone(), "selector": selector.clone(), "hover": { "type": "boolean" }, "double": { "type": "boolean" }, "force": { "type": "boolean", "description": "Bypass visibility" }, "dialog": dialog, "until": until.clone(), "timeout": timeout.clone()
            } } }),
        json!({ "name": "type", "description": "Type text/key at ref, unique CSS selector or focus; returns changes.",
            "inputSchema": { "type": "object", "properties": {
                "ref": r.clone(), "selector": selector.clone(), "text": { "type": "string" }, "key": { "type": "string" }, "clear": { "type": "boolean", "description": "Replace the current value (default false: append)" },
                "submit": { "type": "boolean", "description": "Press Enter" }, "dialog": dialog, "until": until.clone(), "timeout": timeout.clone()
            } } }),
        json!({ "name": "fill", "description": "List/fill forms or submit; stops remaining fields after field navigation.",
            "inputSchema": { "type": "object", "properties": {
                "ref": r.clone(), "selector": selector, "values": { "type": "object", "description": "Name/label/@eN values; no ref: @eN keys or one form" },
                "value": { "description": "Single field value" }, "submit": { "type": "boolean" }, "dialog": dialog, "until": until, "timeout": timeout
            } } }),
        json!({ "name": "scroll", "description": "Scroll the page or an element into view; until=@eN|text scrolls until visible; load_more=true loads infinite lists.",
            "inputSchema": { "type": "object", "properties": {
                "ref": r.clone(), "direction": { "type": "string", "enum": ["down", "up", "top", "bottom"] }, "pages": { "type": "number" },
                "until": { "type": "string" }, "load_more": { "type": "boolean" }, "max_steps": { "type": "integer" }
            } } }),
        json!({ "name": "screenshot", "description": "PNG viewport/full page/element; refs marked by default.",
            "inputSchema": { "type": "object", "properties": {
                "ref": r.clone(), "full": { "type": "boolean" }, "marks": { "type": "boolean", "description": "Draw refs (default true)" }, "scale": { "type": "number" }
            } } }),
        json!({ "name": "network", "description": "Requests of the tab (method, status, type, size, time) with API hints; id=N shows a response body (JSON).",
            "inputSchema": { "type": "object", "properties": {
                "id": { "type": "integer" }, "filter": { "type": "string" }, "type": { "type": "string", "description": "document | fetch | script | image | …" }, "all": { "type": "boolean" }, "limit": { "type": "integer" }
            } } }),
        json!({ "name": "tabs", "description": "Tabs, contexts, downloads and sessions: action=list|switch|new|close|download|save|load (id, url, path, context).",
            "inputSchema": { "type": "object", "properties": {
                "action": { "type": "string" }, "id": { "type": "integer" }, "url": { "type": "string" }, "path": { "type": "string" }, "context": { "type": "string" }
            } } }),
        json!({ "name": "debug", "description": "Evaluate JS/await; console, log=script|json, live view, stats. follow=true follows script navigation.",
            "inputSchema": { "type": "object", "properties": {
                "eval": { "type": "string" }, "timeout": { "type": "integer", "minimum": 1, "description": "Milliseconds; default auto-wait cap" }, "follow": { "type": "boolean", "default": false }, "console": { "type": "boolean" }, "clear": { "type": "boolean" }, "log": { "type": "string" }, "live": { "type": "boolean" }, "stats": { "type": "boolean" }
            } } }),
        json!({ "name": "batch", "description": "Run 1–20 tools in order; no nesting. Stops at first failure with counts. Earlier results <200 chars, last full; full=true keeps all text.",
            "inputSchema": { "type": "object", "properties": {
                "steps": { "type": "array", "minItems": 1, "maxItems": 20, "items": { "type": "object", "properties": {
                    "tool": { "type": "string" }, "args": { "type": "object" }
                }, "required": ["tool"], "additionalProperties": false } },
                "full": { "type": "boolean" }
            }, "required": ["steps"], "additionalProperties": false } }),
    ]
}

fn s<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn b(a: &Value, k: &str) -> Option<bool> {
    a.get(k).and_then(|v| v.as_bool().or_else(|| v.as_str().map(|s| matches!(s, "true" | "1" | "yes"))))
}

fn n(a: &Value, k: &str) -> Option<f64> {
    a.get(k).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
}

/// Accept literal terms, preserving their spelling for result labels.
fn search_terms(a: &Value, key: &str) -> Result<Option<Vec<String>>, String> {
    let Some(value) = a.get(key) else { return Ok(None) };
    let values: Vec<&str> = match value {
        Value::String(s) => vec![s],
        Value::Array(items) => items.iter().map(|v| v.as_str().ok_or_else(|| format!("{key} must contain only strings"))).collect::<Result<_, _>>()?,
        _ => return Err(format!("{key} needs a string or an array of strings")),
    };
    let mut terms: Vec<String> = Vec::new();
    for term in values.into_iter().map(str::trim).filter(|s| !s.is_empty()) {
        if !terms.iter().any(|t| t.to_lowercase() == term.to_lowercase()) {
            terms.push(term.to_string());
        }
    }
    if terms.is_empty() {
        return Err(format!("{key} needs at least one nonempty search term"));
    }
    Ok(Some(terms))
}

fn find_context(a: &Value) -> Result<usize, String> {
    let key = if a.get("window").is_some() { "window" } else { "context" };
    search_window(a.get(key), key)
}

fn search_window(value: Option<&Value>, key: &str) -> Result<usize, String> {
    let Some(value) = value else { return Ok(FIND_CONTEXT) };
    let error = || format!("{key} needs an integer between 0 and 4096");
    let context = value.as_u64().ok_or_else(error)?;
    if context > MAX_FIND_CONTEXT as u64 {
        return Err(error());
    }
    Ok(context as usize)
}

fn open_context(a: &Value) -> Result<Option<&str>, String> {
    let Some(value) = a.get("context") else { return Ok(None) };
    let error = "context is the isolated-session name; use window for the snippet size";
    let name = value.as_str().ok_or(error)?.trim();
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_digit()) { return Err(error.into()); }
    Ok((!name.is_empty()).then_some(name))
}

/// Dialog policy from a `dialog` argument.
fn dialog_policy(a: &Value) -> Option<DialogPolicy> {
    let d = s(a, "dialog")?;
    Some(match d.to_ascii_lowercase().as_str() {
        "accept" | "ok" | "yes" | "true" => DialogPolicy { confirm: true, prompt: Some(String::new()) },
        "dismiss" | "cancel" | "no" | "false" => DialogPolicy { confirm: false, prompt: None },
        _ => DialogPolicy { confirm: true, prompt: Some(d.to_string()) },
    })
}

/// Presence uses the same visible AX names/passages as find; CSS presence,
/// like ordinary find(selector), includes DOM matches marked hidden.
enum WaitQuery {
    Search { terms: Option<Vec<String>>, role: Option<String>, name: Option<String> },
    Css(Vec<rabbithole_engine::css::Selector>),
}

impl WaitQuery {
    fn css(selector: &str) -> Result<Self, String> {
        let tokens = Tokenizer::tokenize(selector);
        let list = SelectorParser::new(&tokens).parse_list().filter(|list| !list.is_empty())
            .ok_or_else(|| format!("invalid selector: {selector}"))?;
        Ok(Self::Css(list))
    }

    fn present(&self, agent: &mut Agent, ti: usize) -> bool {
        match self {
            Self::Css(list) => agent.tabs[ti].frames.iter().any(|frame| {
                let doc = &frame.page.doc;
                doc.descendants(Document::ROOT).any(|node| doc.element(node).is_some() && list.iter().any(|selector| matches(selector, doc, node)))
            }),
            Self::Search { terms, role, name } => {
                let (root, _, _) = agent.search_tree(ti);
                let mut elements: HashMap<(usize, NodeId), (String, Vec<String>)> = HashMap::new();
                root.visit(&mut |x, _| {
                    if !matches!(x.kind, Kind::Control | Kind::Heading | Kind::Text | Kind::Image | Kind::Landmark | Kind::Dialog | Kind::Form | Kind::Frame) { return; }
                    let full = if x.kind == Kind::Text { x.name.clone() } else {
                        let full = crate::ax::accessible_name(&agent.tabs[ti].frames[x.frame], x.node, &x.role);
                        if full.is_empty() { x.name.clone() } else { full }
                    };
                    if !full.is_empty() {
                        elements.entry((x.frame, x.node)).or_insert_with(|| (x.role.clone(), Vec::new())).1.push(full);
                    }
                });
                elements.values().any(|(candidate_role, passages)| {
                    role.as_ref().map(|role| candidate_role == role || (role == "textbox" && candidate_role == "searchbox")).unwrap_or(true)
                        && name.as_ref().map(|name| passages.iter().any(|p| p.to_lowercase().contains(name.as_str()))).unwrap_or(true)
                        && terms.as_ref().map(|terms| passages.iter().any(|p| text_matches(p, terms).iter().any(|hits| !hits.is_empty()))).unwrap_or(true)
                })
            }
        }
    }
}

fn wait_duration(a: &Value, key: &str, default: Duration) -> Result<Duration, String> {
    match a.get(key) {
        None => Ok(default.min(Duration::from_secs(30))),
        Some(value) => value.as_u64().filter(|ms| *ms <= 30_000).map(Duration::from_millis)
            .ok_or_else(|| format!("{key} needs an integer from 0 to 30000 milliseconds")),
    }
}

struct ActionWait { query: WaitQuery, timeout: Duration }

fn action_wait(name: &str, a: &Value, default: Duration) -> Result<Option<ActionWait>, String> {
    if !matches!(name, "click" | "type" | "fill") { return Ok(None); }
    let Some(until) = a.get("until") else {
        if a.get("timeout").is_some() { return Err("timeout needs until=text, until={text:...} or until={selector:...}".into()); }
        return Ok(None);
    };
    let query = if let Some(text) = until.as_str() {
        let text = text.trim();
        if text.is_empty() { return Err("until needs nonempty text".into()); }
        WaitQuery::Search { terms: Some(vec![text.to_string()]), role: None, name: None }
    } else {
        let object = until.as_object().filter(|object| object.len() == 1)
            .ok_or("until needs text or exactly one nonempty text/selector field")?;
        match object.iter().next() {
            Some((key, value)) if key == "text" => {
                let text = value.as_str().map(str::trim).filter(|s| !s.is_empty()).ok_or("until.text needs a nonempty string")?;
                WaitQuery::Search { terms: Some(vec![text.to_string()]), role: None, name: None }
            }
            Some((key, value)) if key == "selector" => {
                let selector = value.as_str().map(str::trim).filter(|s| !s.is_empty()).ok_or("until.selector needs a nonempty string")?;
                WaitQuery::css(selector)?
            }
            _ => return Err("until needs text or exactly one nonempty text/selector field".into()),
        }
    };
    let timeout = wait_duration(a, "timeout", default)?;
    if name == "fill" && a.get("value").is_none() && a.get("values").is_none() && !b(a, "submit").unwrap_or(false) {
        return Err("fill until needs value, values or submit=true".into());
    }
    Ok(Some(ActionWait { query, timeout }))
}

fn wait_status(observation: &crate::wait::Observation, absent: bool) -> String {
    let status = if observation.matched {
        format!("Wait satisfied{} after {} ms.", if absent { " (gone)" } else { "" }, observation.elapsed.as_millis())
    } else if let Some(reason) = &observation.interrupted {
        format!("Wait stopped after {} ms: {reason}.", observation.elapsed.as_millis())
    } else {
        format!("Wait timed out after {} ms; condition not met.", observation.elapsed.as_millis())
    };
    let mut lines = observation.reports.clone();
    lines.push(status);
    if !observation.matched { lines.push(format!("Pending: {}.", observation.pending)); }
    lines.join("\n")
}

fn dispatch_with_wait(agent: &mut Agent, name: &str, a: &Value, explicit: Option<ActionWait>) -> Result<Out, String> {
    let Some(explicit) = explicit else { return dispatch(agent, name, a); };
    let ti = agent.tab_index()?;
    agent.ensure_loaded(ti)?;
    let before = agent.items(ti);
    agent.without_auto_settle(|agent| {
        let mut out = dispatch(agent, name, a)?;
        if out.failed { return Ok(out); }
        let observation = crate::wait::until(agent, ti, explicit.timeout, |agent| explicit.query.present(agent, ti));
        out.failed = !observation.matched;
        out.text.push('\n');
        out.text.push_str(&wait_status(&observation, false));
        let after = agent.items(ti);
        let delta = crate::snapshot::diff(&before, &after, 40);
        if !delta.is_empty() {
            out.text.push_str("\nChanges from before the action through the wait:\n");
            out.text.push_str(&fence(&delta.lines.join("\n")));
        }
        Ok(out)
    })
}

/// Run a tool. Errors are user-facing messages.
pub fn call(agent: &mut Agent, name: &str, args: &Value) -> Result<Out, String> {
    let args = if args.is_null() { json!({}) } else { args.clone() };
    // Validate the condition before even the normal background task pump.
    let explicit = action_wait(name, &args, agent.cfg.settle_cap)?;
    // Each inner call applies its own policies/events/waits. Reject an invalid
    // batch before even advancing background page work on its behalf.
    if name == "batch" { return batch(agent, &args); }
    // Per-call dialog policy.
    let saved_policy = agent.active.map(|ti| agent.tabs[ti].dialog_policy.clone());
    if let (Some(p), Some(ti)) = (dialog_policy(&args), agent.active) {
        agent.tabs[ti].dialog_policy = p;
    }
    advance_background(agent);
    let mutating = matches!(name, "open" | "click" | "type" | "fill" | "scroll" | "tabs") || (name == "debug" && b(&args, "follow").unwrap_or(false) && args.get("eval").is_some());
    let r = dispatch_with_wait(agent, name, &args, explicit);
    if let (Some(p), Some(ti)) = (saved_policy, agent.active) {
        if ti < agent.tabs.len() {
            agent.tabs[ti].dialog_policy = p;
        }
    }
    if mutating && r.is_ok() {
        agent.log_action(name, &args);
    }
    let mut r = r;
    // Pending events of the active tab are appended to every result.
    if let Some(ti) = agent.active {
        if ti < agent.tabs.len() {
            let ev = agent.take_events(ti);
            if !ev.is_empty() {
                let block = format!("\nEvents:\n- {}", ev.join("\n- "));
                match &mut r {
                    Ok(o) => o.text.push_str(&block),
                    Err(e) => e.push_str(&block),
                }
            }
        }
    }
    if mutating {
        publish_live(agent);
    }
    r
}

fn publish_live(agent: &mut Agent) {
    if agent.live.is_none() {
        return;
    }
    let Some(ti) = agent.active else { return };
    let Some(page) = agent.tabs[ti].page() else { return };
    let (canvas, _) = crate::shot::render(&agent.engine, page, crate::shot::Area::Viewport, 1.0);
    let caption = format!("{} — {}", page.title.trim(), page.url);
    if let (Ok(png), Some(live)) = (crate::shot::png(&canvas), &agent.live) {
        live.publish(png, caption);
    }
}

fn dispatch(agent: &mut Agent, name: &str, a: &Value) -> Result<Out, String> {
    match name {
        "open" => open(agent, a),
        "snapshot" => {
            let ti = agent.tab_index()?;
            agent.ensure_loaded(ti)?;
            let max = n(a, "max_tokens").map(|x| x as usize).unwrap_or(SNAPSHOT_TOKENS).max(100);
            Ok(Out::text(agent.snapshot_text(ti, max, s(a, "ref"), b(a, "hidden").unwrap_or(false), b(a, "urls").unwrap_or(false))?))
        }
        "read" => read(agent, a),
        "find" => find(agent, a),
        "click" => {
            let ti = agent.tab_index()?;
            agent.ensure_loaded(ti)?;
            let r = action_ref(agent, ti, a)?.ok_or("click needs ref or selector")?;
            if truncated_list(agent, ti, &r).is_some() {
                return Err(format!("{r} is a truncated list, not a control; use snapshot ref={r} to see its remaining items"));
            }
            Ok(Out::text(agent.click_with_options(ti, &r, b(a, "hover").unwrap_or(false), b(a, "force").unwrap_or(false), b(a, "double").unwrap_or(false))?))
        }
        "type" => type_(agent, a),
        "fill" => fill(agent, a),
        "scroll" => scroll(agent, a),
        "screenshot" => screenshot(agent, a),
        "network" => {
            let ti = agent.tab_index()?;
            let tab = &agent.tabs[ti];
            if let Some(id) = n(a, "id") {
                return Ok(Out::text(crate::netinfo::body(tab, id as u64, n(a, "max_tokens").map(|x| x as usize * 4).unwrap_or(16_000))?));
            }
            Ok(Out::text(crate::netinfo::list(tab, s(a, "filter"), s(a, "type"), b(a, "all").unwrap_or(false), n(a, "limit").map(|x| x as usize).unwrap_or(50))))
        }
        "tabs" => tabs(agent, a),
        "debug" => debug(agent, a),
        other => Err(format!("unknown tool: {other} (tools: {})", TOOLS.join(", "))),
    }
}

fn batch(agent: &mut Agent, a: &Value) -> Result<Out, String> {
    let object = a.as_object().ok_or("batch needs an object; no steps executed")?;
    if object.keys().any(|key| !matches!(key.as_str(), "steps" | "full")) {
        return Err("batch accepts only steps and full; no steps executed".into());
    }
    let full = match a.get("full") {
        Some(value) => value.as_bool().ok_or("batch full needs a boolean; no steps executed")?,
        None => false,
    };
    let steps = a.get("steps").and_then(Value::as_array).ok_or("batch steps needs an array; no steps executed")?;
    if !(1..=20).contains(&steps.len()) {
        return Err("batch needs 1–20 steps; no steps executed".into());
    }
    let mut checked = Vec::with_capacity(steps.len());
    for (index, step) in steps.iter().enumerate() {
        let error = |reason: &str| format!("batch step {}: {reason}; no steps executed", index + 1);
        let step = step.as_object().ok_or_else(|| error("needs an object"))?;
        if step.keys().any(|key| !matches!(key.as_str(), "tool" | "args")) {
            return Err(error("accepts only tool and args"));
        }
        let tool = step.get("tool").and_then(Value::as_str).ok_or_else(|| error("tool needs a string"))?;
        if tool == "batch" { return Err(error("nested batch is not allowed")); }
        if !TOOLS.contains(&tool) { return Err(error("unknown tool")); }
        let args = match step.get("args") {
            Some(value) if value.is_object() => value.clone(),
            Some(_) => return Err(error("args needs an object")),
            None => json!({}),
        };
        checked.push((tool, args));
    }
    let mut results: Vec<(&str, String, Out)> = Vec::with_capacity(checked.len());
    let mut succeeded = 0;
    let mut failed = false;
    let mut omitted_png = false;
    for (tool, args) in checked {
        // Describe the target or operation, never typed/form values or JS.
        let principal = match tool {
            "debug" => ["eval", "console", "log", "stats", "live", "clear"].into_iter().find(|key| args.get(*key).is_some()).unwrap_or("debug").to_string(),
            "type" => s(&args, "selector").or_else(|| s(&args, "ref")).or_else(|| s(&args, "key")).unwrap_or("focused field").to_string(),
            "fill" => s(&args, "selector").or_else(|| s(&args, "ref")).unwrap_or(if args.get("values").is_some() { "values" } else if b(&args, "submit").unwrap_or(false) { "submit" } else { "forms" }).to_string(),
            _ => ["url", "selector", "ref", "key", "text", "action", "id", "direction", "filter", "role", "name"].into_iter().find_map(|key| args.get(key)).map(|value| value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string())).unwrap_or_else(|| "page".to_string()),
        };
        let principal = clip_text(&principal.split_whitespace().collect::<Vec<_>>().join(" "), 64);
        let output = match call(agent, tool, &args) {
            Ok(output) => output,
            Err(error) => Out { failed: true, ..Out::text(error) },
        };
        failed = output.failed;
        if !failed { succeeded += 1; }
        // Out carries one PNG. Release the preceding image as soon as a later
        // result exists, retaining its text and an explicit omission notice.
        if let Some((_, _, previous)) = results.last_mut() {
            if previous.png.take().is_some() {
                omitted_png = true;
                previous.text.push_str("\n[PNG only returned for last step]");
            }
        }
        results.push((tool, principal, output));
        if failed { break; }
    }
    let attempted = results.len();
    let last = attempted - 1;
    let mut text = format!("Batch: attempted {attempted}/{}, succeeded {succeeded}, failed {}, pending {}.", steps.len(), usize::from(failed), steps.len() - attempted);
    if omitted_png {
        text.push_str("\nOnly the last step's PNG is returned; earlier PNGs are omitted.");
    }
    if !full && last > 0 {
        let summaries: Vec<String> = results[..last].iter().enumerate().map(|(index, (tool, principal, output))| {
            let body = output.text.split_whitespace().collect::<Vec<_>>().join(" ");
            let line = format!("{}. {tool} {principal}: {body}{}", index + 1, if output.png.is_some() { " [PNG only returned for last step]" } else { "" });
            clip_text(&line, 199)
        }).collect();
        text.push_str(&format!("\nEarlier steps:\n{}", fence(&summaries.join("\n"))));
    }
    let start = if full { 0 } else { last };
    for (index, (tool, _, output)) in results.iter().enumerate().skip(start) {
        text.push_str(&format!("\nStep {} ({tool}{}):\n{}", index + 1, if output.failed { ", failed" } else { "" }, output.text));
        if index != last && output.png.is_some() { text.push_str("\n[PNG only returned for last step]"); }
    }
    if failed { return Err(text); }
    Ok(Out { text, png: results.pop().and_then(|(_, _, output)| output.png), failed: false })
}

/// The same CSS grammar and frame traversal as find, without choosing a match.
pub(crate) fn selector_nodes(agent: &Agent, ti: usize, selector: &str) -> Result<Vec<(usize, NodeId)>, String> {
    let tokens = Tokenizer::tokenize(selector);
    let list = SelectorParser::new(&tokens).parse_list().filter(|list| !list.is_empty()).ok_or_else(|| format!("invalid selector: {selector}"))?;
    let mut nodes = Vec::new();
    for (frame, data) in agent.tabs[ti].frames.iter().enumerate() {
        let doc = &data.page.doc;
        for node in doc.descendants(Document::ROOT) {
            if doc.element(node).is_some() && list.iter().any(|selector| matches(selector, doc, node)) { nodes.push((frame, node)); }
        }
    }
    Ok(nodes)
}

/// Items already shown of a list cut by `snapshot` (its "expand" ref).
pub(crate) fn truncated_list(agent: &Agent, ti: usize, r: &str) -> Option<usize> {
    let id: u32 = r.trim().strip_prefix("@e")?.parse().ok()?;
    let e = agent.tabs[ti].refs.get(id)?;
    if e.role != "list" { return None; }
    e.name.strip_prefix(crate::snapshot::TRUNCATED)?.parse().ok()
}

fn action_ref(agent: &mut Agent, ti: usize, a: &Value) -> Result<Option<String>, String> {
    if a.get("ref").is_some() && a.get("selector").is_some() {
        return Err("use ref or selector, not both".into());
    }
    if let Some(value) = a.get("ref") {
        return value.as_str().map(str::trim).filter(|r| !r.is_empty()).map(|r| Some(r.to_string())).ok_or_else(|| "ref needs a nonempty string".into());
    }
    let Some(value) = a.get("selector") else { return Ok(None) };
    let selector = value.as_str().map(str::trim).filter(|selector| !selector.is_empty()).ok_or("selector needs a nonempty string")?;
    let nodes = selector_nodes(agent, ti, selector)?;
    if nodes.len() != 1 { return Err(format!("selector matched {} elements; expected exactly one; use find to inspect refs", nodes.len())); }
    let (frame, node) = nodes[0];
    let page = &agent.tabs[ti].frames[frame];
    let is_form = page.doc.is_element(node, "form");
    let role = page.doc.tag(node).unwrap_or("").to_string();
    let name = clip_text(&crate::ax::content_text(page, node), 60);
    let refs = &mut agent.tabs[ti].refs;
    Ok(Some(if is_form { format!("@f{}", refs.assign_form(frame, node)) } else { format!("@e{}", refs.assign(frame, node, &role, &name)) }))
}

fn open(agent: &mut Agent, a: &Value) -> Result<Out, String> {
    let url = s(a, "url").ok_or("open needs url")?.to_string();
    // Validate search arguments before changing the current page.
    let context_name = open_context(a)?;
    let terms = search_terms(a, "find")?;
    let window = search_window(a.get("window"), "window")?;
    let limit = n(a, "limit").map(|x| x as usize).unwrap_or(20).max(1);
    if let Some(bot) = b(a, "bot") {
        agent.set_bot(bot);
    } else if let Some(bh) = b(a, "browser_headers") {
        // Legacy option (browser headers are now the default).
        agent.set_bot(!bh);
    }
    if let Some(r) = b(a, "robots") {
        agent.set_robots(r);
    }
    if let Some(list) = a.get("allowlist").and_then(Value::as_array) {
        agent.cfg.allowlist = list.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect();
    }
    if let Some(js) = b(a, "javascript") {
        agent.engine.scripting = js;
        agent.engine.loader.load_scripts = js;
    }
    let ctx = match context_name {
        Some(c) => agent.context_index(c),
        None => agent.active.map(|t| agent.tabs[t].ctx).unwrap_or(0),
    };
    let lower = url.to_ascii_lowercase();
    let report = match lower.as_str() {
        "back" | "forward" | "reload" => {
            let ti = agent.tab_index()?;
            let r = match lower.as_str() {
                "back" => agent.back_forward(ti, -1)?,
                "forward" => agent.back_forward(ti, 1)?,
                _ => agent.reload(ti)?,
            };
            r
        }
        _ => {
            let parsed = Url::parse(&url).ok_or_else(|| format!("invalid URL: {url}"))?;
            let need_new = b(a, "new_tab").unwrap_or(false) || agent.active.is_none() || agent.active.map(|t| agent.tabs[t].ctx != ctx).unwrap_or(true);
            let ti = if need_new {
                let ti = agent.new_tab(ctx);
                agent.active = Some(ti);
                ti
            } else {
                agent.active.unwrap()
            };
            if let Some(p) = dialog_policy(a) {
                agent.tabs[ti].dialog_policy = p;
            }
            agent.navigate(ti, Request::get(parsed), true)?
        }
    };
    let ti = agent.tab_index()?;
    let mut text = String::new();
    if !report.trim().is_empty() {
        text.push_str(report.trim());
        text.push('\n');
    }
    if agent.tabs[ti].frames.is_empty() {
        return Ok(Out::text(text));
    }
    let full = b(a, "snapshot").or(b(a, "full")).unwrap_or(agent.cfg.full_snapshot_on_open) || n(a, "max_tokens").is_some();
    if let Some(terms) = terms {
        let (root, _, overlays) = agent.search_tree(ti);
        let results = find_elements(agent, ti, &root, Some(&terms), None, None, limit, window);
        text.push_str(&agent.search_results_text(ti, &root, &overlays, b(a, "summary").unwrap_or(false), &results));
    } else if full {
        let max = n(a, "max_tokens").map(|x| x as usize).unwrap_or(SNAPSHOT_TOKENS).max(100);
        text.push_str(&agent.snapshot_text(ti, max, None, false, false)?);
    } else {
        text.push_str(&agent.summary_text(ti)?);
    }
    if a.get("find").is_none() {
        let hints = crate::netinfo::page_hints(&agent.tabs[ti]);
        for h in hints.into_iter().take(2) {
            text.push('\n');
            text.push_str(&h);
        }
    }
    Ok(Out::text(text))
}

fn read(agent: &mut Agent, a: &Value) -> Result<Out, String> {
    let ti = agent.tab_index()?;
    agent.ensure_loaded(ti)?;
    let max_chars = n(a, "max_tokens").map(|x| x as usize).unwrap_or(4000).max(100) * 4;
    let offset = n(a, "offset").map(|x| x as usize).unwrap_or(0);
    let tab = &mut agent.tabs[ti];
    let (frame, root) = match s(a, "ref") {
        Some(r) => {
            let (f, node) = agent_resolve(tab, r)?;
            (f, Some(node))
        }
        None => (0, None),
    };
    // Reading the frame element itself means reading its hosted document,
    // with refs and the URL attributed to that child context.
    let (frame, root) = if let Some(node) = root {
        if matches!(tab.frames[frame].doc.tag(node), Some("iframe" | "frame")) {
            tab.frames.iter().enumerate().find(|(_,f)| f.parent == Some((frame,node))).map(|(child,_)|(child,None)).unwrap_or((frame,root))
        } else { (frame,root) }
    } else { (frame,root) };
    let page = &tab.frames[frame];
    let md = match root {
        Some(node) => crate::read::read_subtree_with_options(page, &mut tab.refs, frame, node, b(a, "links").unwrap_or(true), b(a, "cites").unwrap_or(false)),
        None if page.doc.body().is_some_and(|body| page.doc.is_element(body,"frameset")) => {
            let views = Agent::frame_views(&tab.frames);
            crate::read::read_frameset_with_options(&views, &mut tab.refs, frame, b(a,"links").unwrap_or(true), b(a,"cites").unwrap_or(false))
        }
        None => crate::read::read_with_options(page, &mut tab.refs, frame, b(a, "links").unwrap_or(true), b(a, "cites").unwrap_or(false)),
    };
    let total = md.chars().count();
    let body: String = md.chars().skip(offset).take(max_chars).collect();
    let mut text = format!("{}  ({} chars)\n", page.url, total);
    text.push_str(&fence(&body));
    if offset + max_chars < total {
        text.push_str(&format!("\n(continues: read offset={})", offset + max_chars));
    }
    Ok(Out::text(text))
}

fn agent_resolve(tab: &crate::browser::Tab, r: &str) -> Result<(usize, NodeId), String> {
    let parsed = Ref::parse(r).ok_or_else(|| format!("\"{r}\" is not a ref"))?;
    let (f, node, generation) = match parsed {
        Ref::Element(n) => tab.refs.get(n).map(|e| (e.frame, e.node, e.generation)).ok_or_else(|| format!("{parsed} is unknown"))?,
        Ref::Form(n) => tab.refs.get_form(n).ok_or_else(|| format!("{parsed} is unknown"))?,
    };
    if generation != tab.refs.generation || f >= tab.frames.len() || !tab.frames[f].doc.is_connected(node) {
        return Err(format!("{parsed} no longer exists; take a new snapshot."));
    }
    Ok((f, node))
}

/// Lowercase the complete string (including context-sensitive sigma), while
/// mapping every folded byte to its original Unicode character. A match can
/// begin inside an expansion such as İ → i + combining dot.
fn text_matches(name: &str, terms: &[String]) -> Vec<Vec<(usize, usize)>> {
    let lower = name.to_lowercase();
    let origins: Vec<usize> = name.chars().enumerate().flat_map(|(i, c)| c.to_lowercase().map(move |_| i)).collect();
    let mut original = Vec::with_capacity(lower.len());
    for (c, index) in lower.chars().zip(origins) {
        original.extend(std::iter::repeat_n(index, c.len_utf8()));
    }
    terms.iter().map(|term| {
        let folded = term.to_lowercase();
        lower.match_indices(folded.as_str()).map(|(start, matched)| (original[start], original[start + matched.len() - 1] + 1)).collect()
    }).collect()
}

fn has_whole_word_match(text: &str, terms: &[String]) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let word = |c: char| c.is_alphanumeric() || c == '_';
    text_matches(text, terms).iter().flatten().any(|&(start, end)| {
        (start == 0 || !word(chars[start - 1])) && (end == chars.len() || !word(chars[end]))
    })
}

fn match_context(name: &str, first: Option<(usize, usize)>, context: usize) -> String {
    if context == 0 {
        return clip_text(name, 120);
    }
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= context {
        return name.to_string();
    }
    if context == 1 {
        return "…".into();
    }
    let (start, end) = window_bounds(chars.len(), first, context);
    let mut snippet = String::new();
    if start > 0 { snippet.push('…'); }
    snippet.extend(chars[start..end].iter());
    if end < chars.len() { snippet.push('…'); }
    snippet
}

fn window_bounds(len: usize, first: Option<(usize, usize)>, window: usize) -> (usize, usize) {
    if len <= window { return (0, len); }
    let center = first.map(|(start, end)| start + (end - start) / 2).unwrap_or(0);
    if window == 1 { return (center, center); }
    let width = window - 2;
    let mut start = center.saturating_sub(width / 2).min(len - width);
    let mut end = start + width;
    // Spend a missing edge ellipsis on one more original character.
    if start == 0 { end = window - 1; }
    else if end == len { start = len - (window - 1); }
    (start, end)
}

#[derive(Clone, Copy)]
struct SearchWindow { passage: usize, start: usize, end: usize }

fn merge_windows(mut windows: Vec<SearchWindow>) -> Vec<SearchWindow> {
    windows.sort_unstable_by_key(|w| (w.passage, w.start, w.end));
    let mut merged: Vec<SearchWindow> = Vec::new();
    for window in windows {
        if let Some(last) = merged.last_mut() {
            if last.passage == window.passage && window.start <= last.end {
                last.end = last.end.max(window.end);
                continue;
            }
        }
        merged.push(window);
    }
    merged
}

fn windows_len(windows: &[SearchWindow], passages: &[Vec<char>]) -> usize {
    let mut len = 0;
    let mut gap = false;
    for (i, w) in windows.iter().enumerate() {
        if (i > 0 || w.start > 0) && !gap { len += 1; gap = true; }
        if w.end > w.start { len += w.end - w.start; gap = false; }
    }
    if windows.last().map(|w| w.end < passages[w.passage].len()).unwrap_or(false) && !gap { len += 1; }
    len
}

/// The first occurrence of each requested term gets a window, in request
/// priority order. Admit its union only if the final text (ellipses included)
/// fits three windows; emit the selected regions in their original order.
fn search_snippet(passages: &[String], hits: &[Vec<Vec<(usize, usize)>>], window: usize) -> (String, usize) {
    let first = hits.iter().enumerate().find_map(|(i, terms)| {
        terms.iter().flatten().copied().min().map(|hit| (i, hit))
    });
    if window == 0 {
        let passage = first.map_or(0, |(i, _)| i);
        let text = match_context(&passages[passage], first.map(|(_, hit)| hit), window);
        let len = passages[passage].chars().count();
        let visible = if len <= 120 { len } else { 119 };
        let shown = hits[passage].iter().flatten().filter(|&&(start, end)| start < end && end <= visible).count();
        return (text, shown);
    }
    let chars: Vec<Vec<char>> = passages.iter().map(|p| p.chars().collect()).collect();
    let mut windows = Vec::new();
    for term in 0..hits.first().map_or(0, Vec::len) {
        let Some((passage, hit)) = hits.iter().enumerate().find_map(|(i, terms)| terms[term].first().map(|&hit| (i, hit))) else { continue };
        let (start, end) = window_bounds(chars[passage].len(), Some(hit), window);
        let mut proposed = windows.clone();
        proposed.push(SearchWindow { passage, start, end });
        let proposed = merge_windows(proposed);
        if windows_len(&proposed, &chars) <= window * 3 { windows = proposed; }
    }
    if windows.is_empty() {
        let (start, end) = window_bounds(chars[0].len(), None, window);
        windows.push(SearchWindow { passage: 0, start, end });
    }
    let mut text = String::new();
    let mut gap = false;
    for (i, w) in windows.iter().enumerate() {
        if (i > 0 || w.start > 0) && !gap { text.push('…'); gap = true; }
        if w.end > w.start {
            text.extend(chars[w.passage][w.start..w.end].iter());
            gap = false;
        }
    }
    let last = windows.last().unwrap();
    if last.end < chars[last.passage].len() && !gap { text.push('…'); }
    let shown = hits.iter().enumerate().map(|(passage, terms)| terms.iter().flatten().filter(|&&(start, end)| {
        windows.iter().any(|w| w.passage == passage && w.start <= start && end <= w.end)
    }).count()).sum();
    (text, shown)
}

fn normalized_name(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Flatten composed DOM order, inserting child documents at their iframe.
/// Snapshot rendering prioritizes main; search keeps document order instead.
fn search_order(tab: &crate::browser::Tab) -> HashMap<(usize, NodeId), usize> {
    let children: HashMap<_, _> = tab.frames.iter().enumerate().filter_map(|(i, f)| f.parent.map(|parent| (parent, i))).collect();
    let mut order = HashMap::new();
    let mut stack = vec![(0, Document::ROOT)];
    while let Some((frame, node)) = stack.pop() {
        if order.contains_key(&(frame, node)) { continue; }
        order.insert((frame, node), order.len());
        let doc = &tab.frames[frame].doc;
        stack.extend(doc.flat_children(node).into_iter().rev().map(|n| (frame, n)));
        if let Some(&child) = children.get(&(frame, node)) {
            stack.push((child, Document::ROOT));
        }
    }
    order
}

/// Visible text of an element as block-level segments: inline runs are
/// joined, blocks, cells, links and image alternatives start a new
/// segment. A segment repeated by another (a linked image's `alt` and the
/// link's clipped `title…` text) is kept once, in its longest form.
fn context_segments(page: &rabbithole_engine::Page, n: NodeId) -> Vec<String> {
    fn walk(page: &rabbithole_engine::Page, n: NodeId, cur: &mut String, out: &mut Vec<String>, depth: usize) {
        let doc = &page.doc;
        if depth > 40 { return; }
        for c in doc.flat_children(n) {
            if let Some(t) = doc.text(c) { cur.push_str(t); continue; }
            let Some(el) = doc.element(c) else { continue };
            if matches!(el.tag.as_str(), "script" | "style" | "template" | "noscript") { continue; }
            if !crate::ax::displayed(page, c) || el.attr("aria-hidden") == Some("true") { continue; }
            let inline = matches!(el.tag.as_str(), "span" | "b" | "i" | "em" | "strong" | "small" | "code" | "abbr" | "sub" | "sup" | "mark" | "u" | "s" | "label" | "time" | "data" | "q" | "cite" | "bdi");
            if el.tag == "img" {
                flush(cur, out);
                if let Some(alt) = el.attr("alt") { cur.push_str(alt); }
                flush(cur, out);
                continue;
            }
            if !inline { flush(cur, out); }
            walk(page, c, cur, out, depth + 1);
            if !inline { flush(cur, out); }
        }
    }
    fn flush(cur: &mut String, out: &mut Vec<String>) {
        let t = cur.split_whitespace().collect::<Vec<_>>().join(" ");
        if !t.is_empty() { out.push(t); }
        cur.clear();
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    walk(page, n, &mut cur, &mut out, 0);
    flush(&mut cur, &mut out);
    let stem = |s: &str| s.trim_end_matches(['…', '.']).trim_end().to_lowercase();
    let mut kept: Vec<String> = Vec::new();
    for seg in out {
        let st = stem(&seg);
        if st.is_empty() { continue; }
        if let Some(i) = kept.iter().position(|k| { let ks = stem(k); ks.starts_with(&st) || st.starts_with(&ks) }) {
            if seg.chars().count() > kept[i].chars().count() && !seg.ends_with(['…', '.']) { kept[i] = seg; }
            continue;
        }
        kept.push(seg);
    }
    kept
}

/// The smallest useful container of a short match: climb the ancestors of
/// `node` while their text fits `window` and they hold no other matching
/// element (stopping at the card or row, not the list). Returns its joined
/// text and the first link inside it, when it is larger than the match.
fn match_container(page: &rabbithole_engine::Page, node: NodeId, own_len: usize, window: usize, matched: &HashSet<NodeId>) -> Option<(String, Option<NodeId>)> {
    let doc = &page.doc;
    let mut ancestors = Vec::new();
    let mut up = doc.parent(node);
    while let Some(p) = up { ancestors.push(p); up = doc.parent(p); }
    let mut best = None;
    let mut cur = node;
    while let Some(parent) = doc.parent(cur) {
        if doc.element(parent).is_none() || doc.is_element(parent, "body") || doc.is_element(parent, "html") { break; }
        let others = doc.descendants(parent).any(|d| d != node && matched.contains(&d) && !ancestors.contains(&d));
        if others { break; }
        let segments = context_segments(page, parent);
        let text = segments.join(" · ");
        let len = text.chars().count();
        if len > window { break; }
        if len > own_len { best = Some((text, parent)); }
        cur = parent;
    }
    let (text, container) = best?;
    let link = std::iter::once(container).chain(doc.descendants(container)).find(|&d| doc.is_element(d, "a") && doc.element(d).and_then(|e| e.attr("href")).is_some())
        .or_else(|| ancestors.iter().copied().find(|&d| doc.is_element(d, "a") && doc.element(d).and_then(|e| e.attr("href")).is_some()));
    Some((text, link))
}

fn find_elements(agent: &mut Agent, ti: usize, root: &crate::ax::Ax, terms: Option<&[String]>, role: Option<&str>, name: Option<&str>, limit: usize, context: usize) -> String {
    let tab = &mut agent.tabs[ti];
    let order = search_order(tab);
    let role = role.map(str::to_lowercase);
    let name = name.map(str::to_lowercase);
    let mut candidates: Vec<(usize, NodeId, String, Vec<String>, Option<u32>)> = Vec::new();
    let mut elements: HashMap<(usize, NodeId), usize> = HashMap::new();
    root.visit(&mut |x, _| {
        if !matches!(x.kind, Kind::Control | Kind::Heading | Kind::Text | Kind::Image | Kind::Landmark | Kind::Dialog | Kind::Form | Kind::Frame) {
            return;
        }
        // AX labels are clipped for snapshots. Search full accessible names;
        // text runs already retain their complete, normalized content.
        let full_name = if x.kind == Kind::Text { x.name.clone() } else {
            let full = crate::ax::accessible_name(&tab.frames[x.frame], x.node, &x.role);
            if full.is_empty() { x.name.clone() } else { full }
        };
        if full_name.is_empty() { return; }
        if let Some(&i) = elements.get(&(x.frame, x.node)) {
            let candidate = &mut candidates[i];
            if x.kind == Kind::Text {
                // A control can separate two runs of the same element.
                // Group their results, but never invent a contiguous phrase.
                candidate.3.push(full_name);
            }
        } else {
            elements.insert((x.frame, x.node), candidates.len());
            candidates.push((x.frame, x.node, x.role.clone(), vec![full_name], x.reference));
        }
    });
    candidates.sort_by_key(|(f, nd, role, passages, _)| {
        let whole = terms.is_some_and(|terms| passages.iter().any(|p| has_whole_word_match(p, terms)));
        let actionable = terms.is_some() && matches!(role.as_str(), "button" | "link" | "clickable" | "checkbox" | "radio" | "combobox" | "textbox" | "searchbox");
        (!whole, !actionable, order.get(&(*f, *nd)).copied().unwrap_or(usize::MAX))
    });
    let mut seen = vec![false; terms.map_or(0, |terms| terms.len())];
    let mut prose = vec![false; seen.len()];
    if role.is_none() {
        if let Some(terms) = terms {
            for (_, _, candidate_role, passages, _) in &candidates {
                if candidate_role != "text" || !name.as_ref().map(|n| passages.iter().any(|p| p.to_lowercase().contains(n.as_str()))).unwrap_or(true) { continue; }
                for passage in passages {
                    for (present, hits) in prose.iter_mut().zip(text_matches(passage, terms)) { *present |= !hits.is_empty(); }
                }
            }
        }
    }
    let normalized_terms: Vec<_> = terms.unwrap_or_default().iter().map(|t| normalized_name(t)).collect();
    // Elements holding a match, per frame: a match's context container
    // must not swallow another one.
    let mut matched: HashMap<usize, HashSet<NodeId>> = HashMap::new();
    if let Some(terms) = terms {
        for (frame, node, _, passages, _) in &candidates {
            if passages.iter().any(|p| text_matches(p, terms).iter().any(|h| !h.is_empty())) {
                matched.entry(*frame).or_default().insert(*node);
            }
        }
    }
    let mut lines = Vec::new();
    let mut total = 0;
    let mut omitted_links = 0;
    for (frame, node, role_name, passages, reference) in candidates {
        let ok_role = role.as_ref().map(|r| role_name == *r || (r == "textbox" && role_name == "searchbox")).unwrap_or(true);
        let ok_name = name.as_ref().map(|n| passages.iter().any(|p| p.to_lowercase().contains(n.as_str()))).unwrap_or(true);
        if !ok_role || !ok_name { continue; }
        let passage_hits: Vec<_> = passages.iter().map(|p| terms.map(|terms| text_matches(p, terms)).unwrap_or_default()).collect();
        let mut counts = vec![0; seen.len()];
        for hits in &passage_hits {
            for (count, hits) in counts.iter_mut().zip(hits) { *count += hits.len(); }
        }
        for (seen, count) in seen.iter_mut().zip(&counts) { *seen |= *count > 0; }
        let count: usize = counts.iter().sum();
        if terms.is_some() && count == 0 { continue; }
        if role.is_none() && role_name == "link" {
            let label = normalized_name(&passages.join(" "));
            if normalized_terms.iter().zip(&prose).any(|(term, present)| *present && *term == label) {
                omitted_links += 1;
                continue;
            }
        }
        total += 1;
        if lines.len() >= limit { continue; }
        let reference = reference.unwrap_or_else(|| tab.refs.assign(frame, node, &role_name, &clip_text(&passages[0], 60)));
        let page = &tab.frames[frame];
        let position = if frame != 0 { " (in frame)" } else {
            page.rect_of(node).map(|rc| {
                let vy = rc.y - page.page_scroll.1;
                if vy < 0.0 { " (above viewport)" } else if vy > page.viewport.height { " (below viewport)" } else { "" }
            }).unwrap_or("")
        };
        let name_hits;
        let snippet_hits = if terms.is_none() {
            name_hits = passages.iter().map(|p| name.as_ref().map(|term| text_matches(p, std::slice::from_ref(term))).unwrap_or_default()).collect::<Vec<_>>();
            &name_hits
        } else { &passage_hits };
        let (snippet, shown) = search_snippet(&passages, snippet_hits, context);
        let mut line = format!("@e{reference} {role_name} \"{}\"{position}", esc(&snippet));
        if let Some(terms) = terms {
            let labels: Vec<_> = terms.iter().zip(&counts).filter(|(_, count)| **count > 0).map(|(term, _)| format!("\"{}\"", esc(term))).collect();
            line.push_str(&format!(" [text: {}]", labels.join(", ")));
        }
        if count > shown { line.push_str(&format!(" (+{} more in this element)", count - shown)); }
        // A short match (a price, a table label) says little alone: add the
        // card or row that holds it, and its link.
        let own_len = passages.iter().map(|p| p.chars().count()).sum::<usize>();
        if terms.is_some() && context > 0 && own_len < context {
            let empty = HashSet::new();
            let page = &tab.frames[frame];
            if let Some((text, link)) = match_container(page, node, own_len, context, matched.get(&frame).unwrap_or(&empty)) {
                line.push_str(&format!("  in: \"{}\"", esc(&text)));
                if let Some(l) = link.filter(|&l| l != node) {
                    let label = clip_text(&crate::ax::content_text(page, l), 60);
                    let r = tab.refs.assign(frame, l, "link", &label);
                    line.push_str(&format!("  link @e{r}"));
                }
            }
        }
        lines.push(line);
    }
    let mut body = if lines.is_empty() { "No matches.".to_string() } else { lines.join("\n") };
    if let Some(terms) = terms {
        let missing: Vec<_> = terms.iter().zip(seen).filter(|(_, seen)| !seen).map(|(term, _)| format!("\"{}\"", esc(term))).collect();
        if !missing.is_empty() {
            let absent = format!("No matches for: {}", missing.join(", "));
            if lines.is_empty() { body = absent; } else { body.push_str(&format!("\n{absent}")); }
        }
    }
    if total > lines.len() { body.push_str(&format!("\n({} more matches; raise limit)", total - lines.len())); }
    if omitted_links > 0 { body.push_str(&format!("\n({omitted_links} redundant links omitted; use role=link to include)")); }
    body
}

fn find(agent: &mut Agent, a: &Value) -> Result<Out, String> {
    let timeout = wait_duration(a, "wait", Duration::ZERO)?;
    let gone = match a.get("gone") {
        None => false,
        Some(value) => value.as_bool().ok_or("gone needs a boolean")?,
    };
    // Preserve the immediate path, output and allocation cost without options.
    if a.get("wait").is_none() && !gone { return find_now(agent, a); }
    let terms = search_terms(a, "text")?;
    find_context(a)?;
    let query = if let Some(selector) = s(a, "selector") { WaitQuery::css(selector)? } else {
        let role = s(a, "role").map(str::to_lowercase);
        let name = s(a, "name").map(str::to_lowercase);
        if terms.is_none() && role.is_none() && name.is_none() { return Err("find needs text, role/name or selector".into()); }
        WaitQuery::Search { terms, role, name }
    };
    let ti = agent.tab_index()?;
    agent.ensure_loaded(ti)?;
    let observation = crate::wait::until(agent, ti, timeout, |agent| query.present(agent, ti) != gone);
    let mut out = find_now(agent, a)?;
    out.text.push('\n');
    out.text.push_str(&wait_status(&observation, gone));
    out.failed = !observation.matched;
    Ok(out)
}

fn find_now(agent: &mut Agent, a: &Value) -> Result<Out, String> {
    let ti = agent.tab_index()?;
    agent.ensure_loaded(ti)?;
    let limit = n(a, "limit").map(|x| x as usize).unwrap_or(20).max(1);
    let context = find_context(a)?;
    let terms = search_terms(a, "text")?;
    if let Some(sel) = s(a, "selector") {
        // Element text is clipped to 100 chars unless a window is asked for.
        let clip = if a.get("window").is_some() || a.get("context").is_some() { context.max(1) } else { 100 };
        let toks = Tokenizer::tokenize(sel);
        let list = SelectorParser::new(&toks).parse_list().ok_or_else(|| format!("invalid selector: {sel}"))?;
        let tab = &mut agent.tabs[ti];
        let mut lines = Vec::new();
        let mut total = 0;
        for (fi, f) in tab.frames.iter().enumerate() {
            let doc = &f.page.doc;
            for nd in doc.descendants(Document::ROOT) {
                if doc.element(nd).is_none() || !list.iter().any(|s| matches(s, doc, nd)) { continue; }
                total += 1;
                if lines.len() >= limit { continue; }
                let role = doc.tag(nd).unwrap_or("").to_string();
                let text = context_segments(&f.page, nd).join(" · ");
                let r = tab.refs.assign(fi, nd, &role, &clip_text(&text, 60));
                let vis = if !crate::ax::displayed(&f.page, nd) { " (hidden)" } else { "" };
                lines.push(format!("@e{r} <{role}> \"{}\"{vis}", esc(&clip_text(&text, clip))));
            }
        }
        let mut text = if lines.is_empty() { "No matches.".to_string() } else { fence(&lines.join("\n")) };
        if total > lines.len() { text.push_str(&format!("\n({} more matches; raise limit)", total - lines.len())); }
        return Ok(Out::text(text));
    }
    let role = s(a, "role");
    let name = s(a, "name");
    if terms.is_none() && role.is_none() && name.is_none() {
        return Err("find needs text, role/name or selector".into());
    }
    let (root, _, _) = agent.search_tree(ti);
    // All search terms and page-derived snippets share one untrusted fence.
    Ok(Out::text(fence(&find_elements(agent, ti, &root, terms.as_deref(), role, name, limit, context))))
}

fn type_(agent: &mut Agent, a: &Value) -> Result<Out, String> {
    let ti = agent.tab_index()?;
    agent.ensure_loaded(ti)?;
    let text = a.get("text").and_then(Value::as_str).map(|s| s.to_string());
    let key = s(a, "key").map(|s| s.to_string());
    let submit = b(a, "submit").unwrap_or(false);
    let clear = b(a, "clear").unwrap_or(false);
    if text.is_none() && key.is_none() && !submit {
        return Err("type needs text and/or key".into());
    }
    let reference = action_ref(agent, ti, a)?;
    let target = match reference.as_deref() {
        Some(r) => Some(agent.resolve(ti, r)?),
        None => {
            let f = agent.tabs[ti].frames[0].state.focused;
            if text.is_some() && f.is_none() {
                return Err("type needs ref (no element has focus)".into());
            }
            f.map(|n| (0, n))
        }
    };
    let frame = target.map(|t| t.0).unwrap_or(0);
    let label = target.map(|(f, nd)| agent.describe_ref(ti, f, nd)).unwrap_or_else(|| "the page".into());
    let before = agent.before_state(ti);
    let engine=&agent.engine;
    let (outcome,request,what)=agent.tabs[ti].frames.with_page(engine,frame,|page| -> Result<_,String> {
        let mut outcome = ScriptOutcome::default();
        let mut request = None;
        let mut what = Vec::new();
        if let Some(t) = &text {
            let (_, node) = target.ok_or("no target")?;
            let o = crate::actions::type_text(engine, page, node, t, clear)?;
            crate::actions::merge(&mut outcome, o);
            let shown = if page.control_kind(node) == rabbithole_engine::ControlKind::Password { format!("{} chars", t.chars().count()) } else { format!("\"{}\"", esc(&clip_text(t, 60))) };
            what.push(format!("Typed {shown} into {label}"));
        }
        let keys: Vec<String> = key.into_iter().chain(if submit { Some("Enter".to_string()) } else { None }).collect();
        for k in keys {
            if request.is_some() || outcome.navigation.is_some() {
                break;
            }
            let r = crate::actions::press_key(engine, page, target.map(|t| t.1), &k);
            crate::actions::merge(&mut outcome, r.outcome);
            request = r.request;
            what.push(format!("pressed {k}{}", r.action.map(|x| format!(" ({x})")).unwrap_or_default()));
        }
        Ok((outcome,request,what))
    })??;
    let desc = what.join(", ");
    Ok(Out::text(agent.finish_action(ti, frame, before, outcome, request, &desc, target)?))
}

#[derive(Clone)]
enum FillTarget {
    Ref(String),
    InForm { form: String, key: String },
}

struct FillOperation {
    key: String,
    value: Value,
    target: FillTarget,
}

fn fill_forms(agent: &mut Agent, ti: usize) -> (Vec<(String, usize, NodeId)>, String) {
    let tab = &mut agent.tabs[ti];
    let mut forms = Vec::new();
    let mut lines = Vec::new();
    for frame in 0..tab.frames.len() {
        for node in crate::forms::forms(&tab.frames[frame]) {
            let reference = format!("@f{}", tab.refs.assign_form(frame, node));
            forms.push((reference, frame, node));
            lines.extend(crate::forms::describe(&tab.frames[frame], frame, node, &mut tab.refs));
        }
    }
    let listing = if lines.is_empty() {
        "No forms on this page (controls outside forms are in the snapshot).".to_string()
    } else {
        fence(&lines.join("\n"))
    };
    (forms, listing)
}

fn sole_fill_form(agent: &mut Agent, ti: usize) -> Result<String, String> {
    let (forms, listing) = fill_forms(agent, ti);
    if forms.len() == 1 {
        return Ok(forms[0].0.clone());
    }
    Err(format!("fill without ref needs exactly one form for name/label keys or submit (found {}). Pass ref=@fN or use only @eN keys.\n{listing}", forms.len()))
}

fn fill_fields(agent: &mut Agent, ti: usize, target: &FillTarget) -> Result<(usize, Vec<crate::forms::Field>, usize), String> {
    match target {
        FillTarget::Ref(reference) => {
            let (frame, node) = agent.resolve(ti, reference)?;
            let page = &agent.tabs[ti].frames[frame];
            if !matches!(page.doc.tag(node), Some("input" | "select" | "textarea")) {
                return Err(format!("{reference} is not a fillable field; use values for a form"));
            }
            let fields = single_field(page, node, page.form_of(node));
            let index = fields.iter().position(|field| field.node == node).ok_or_else(|| format!("{reference} is not a fillable field"))?;
            Ok((frame, fields, index))
        }
        FillTarget::InForm { form, key } => {
            // Resolve both refs against the current document, not an old fields
            // vector captured before a previous callback changed the page.
            let (frame, node) = agent.resolve(ti, form)?;
            let explicit = if key.trim().starts_with('@') && matches!(Ref::parse(key), Some(Ref::Element(_))) {
                Some(agent.resolve(ti, key)?)
            } else {
                None
            };
            let tab = &agent.tabs[ti];
            let page = &tab.frames[frame];
            let fields = crate::forms::fields(page, node);
            let index = if let Some((field_frame, field_node)) = explicit {
                fields.iter().position(|field| field_frame == frame && field.node == field_node)
            } else {
                crate::forms::match_field(page, &fields, key, &tab.refs, frame)
                    .or_else(|| fields.iter().position(|field| field.kind == rabbithole_engine::ControlKind::Radio && field.name.eq_ignore_ascii_case(key)))
            };
            match index {
                Some(index) => Ok((frame, fields, index)),
                None => {
                    let names: Vec<String> = fields.iter().filter(|field| field.role != "button").map(|field| if field.label.is_empty() { field.name.clone() } else { field.label.clone() }).collect();
                    Err(format!("no field matches \"{}\" (fields: {})", esc(key), names.join(", ")))
                }
            }
        }
    }
}

fn fill(agent: &mut Agent, a: &Value) -> Result<Out, String> {
    let ti = agent.tab_index()?;
    agent.ensure_loaded(ti)?;
    let values = match a.get("values") {
        Some(Value::Object(values)) => Some(values.clone()),
        Some(_) => return Err("fill values must be an object of field keys and values".into()),
        None => None,
    };
    let submit = b(a, "submit").unwrap_or(false);
    let single = a.get("value").cloned();
    let reference = action_ref(agent, ti, a)?;
    let mut form = None;
    if let Some(reference) = &reference {
        let (frame, node) = agent.resolve(ti, reference)?;
        let page = &agent.tabs[ti].frames[frame];
        let owner = if page.doc.is_element(node, "form") { Some(node) } else { page.form_of(node) };
        if let Some(node) = owner {
            form = Some(format!("@f{}", agent.tabs[ti].refs.assign_form(frame, node)));
        }
    }
    if values.is_none() && single.is_none() && !submit {
        if reference.is_none() {
            return Ok(Out::text(fill_forms(agent, ti).1));
        }
        let reference = reference.as_ref().unwrap();
        let form = form.ok_or_else(|| format!("{reference} is not in a form"))?;
        let (frame, node) = agent.resolve(ti, &form)?;
        let tab = &mut agent.tabs[ti];
        let lines = crate::forms::describe(&tab.frames[frame], frame, node, &mut tab.refs);
        return Ok(Out::text(fence(&lines.join("\n"))));
    }
    if single.is_some() && reference.is_none() {
        return Err("fill value needs a field ref or selector; use values for field-key/value pairs".into());
    }
    let reference_keys = reference.is_none() && values.as_ref().is_some_and(|values| {
        !values.is_empty() && values.keys().all(|key| key.trim().starts_with('@') && matches!(Ref::parse(key), Some(Ref::Element(_))))
    });
    if reference.is_none() && !reference_keys {
        form = Some(sole_fill_form(agent, ti)?);
    }
    if values.is_some() && !reference_keys && form.is_none() {
        return Err("the ref is not a form or inside one; use values with @eN keys and no ref for independent fields".into());
    }
    if reference_keys && submit {
        let mut owner = None;
        for key in values.as_ref().unwrap().keys() {
            let (frame, node) = agent.resolve(ti, key)?;
            let node = agent.tabs[ti].frames[frame].form_of(node).ok_or("submit needs fields belonging to one form, or an explicit form ref")?;
            if owner.is_some_and(|owner| owner != (frame, node)) {
                return Err("submit with field refs from multiple forms is ambiguous; pass one form ref and its values".into());
            }
            owner = Some((frame, node));
        }
        let (frame, node) = owner.ok_or("nothing to submit")?;
        form = Some(format!("@f{}", agent.tabs[ti].refs.assign_form(frame, node)));
    }
    if submit && form.is_none() {
        return Err("nothing to submit: the ref is not in a form".into());
    }
    let mut operations = Vec::new();
    if let Some(value) = single {
        let reference = reference.as_ref().unwrap().clone();
        let target = FillTarget::Ref(reference.clone());
        // Preserve the single-field preflight error instead of treating a form
        // or unrelated element as a field and then continuing with values.
        fill_fields(agent, ti, &target)?;
        operations.push(FillOperation { key: reference, value, target });
    }
    if let Some(values) = values {
        for (key, value) in values {
            let target = if reference_keys {
                FillTarget::Ref(key.clone())
            } else {
                FillTarget::InForm { form: form.as_ref().unwrap().clone(), key: key.clone() }
            };
            operations.push(FillOperation { key, value, target });
        }
    }
    let before = agent.before_state(ti);
    let mut done = Vec::new();
    let mut errors = Vec::new();
    let mut notes = Vec::new();
    let mut outcome = ScriptOutcome::default();
    let mut frame = 0;
    let mut field_navigation = false;
    let mut field_document_replaced = false;
    for (index, operation) in operations.iter().enumerate() {
        let (field_frame, fields, field_index) = match fill_fields(agent, ti, &operation.target) {
            Ok(fields) => fields,
            Err(error) => {
                errors.push(format!("{}: {error}", esc(&operation.key)));
                continue;
            }
        };
        frame = field_frame;
        let document = agent.tabs[ti].frames[frame].doc.weak_identity();
        let result = agent.tabs[ti].frames.with_page(&agent.engine,frame,|page| crate::forms::set_field_with_outcome(&agent.engine,page,&fields,field_index,&operation.value)).and_then(|result|result);
        match result {
            Ok((description, result)) => {
                if let Some(error) = result.error {
                    errors.push(format!("{}: {error}", esc(&operation.key)));
                } else if result.value_applied {
                    done.push(description);
                } else {
                    notes.push(format!("{} was not changed: navigation was requested while focusing the field.", esc(&operation.key)));
                }
                crate::actions::merge(&mut outcome, result.outcome);
            }
            Err(error) => {
                errors.push(format!("{}: {error}", esc(&operation.key)));
                continue;
            }
        }
        if outcome.navigation.is_none() {
            // The same settle routine as click, before touching another field:
            // timers/XHR from this callback may request navigation as well.
            let log = agent.tabs[ti].netlog.clone();
            let settled = crate::wait::settle(&agent.engine, &mut agent.tabs[ti].frames[0], Some(&log), agent.cfg.settle_cap);
            if let Some(pending) = &settled.pending {
                notes.push(format!("{} did not settle within the wait cap: {pending}", esc(&operation.key)));
            }
            crate::actions::merge(&mut outcome, ScriptOutcome { navigation: settled.navigation, new_windows: settled.new_windows, dialogs: settled.dialogs, relayout: settled.changed, ..ScriptOutcome::default() });
        }
        if agent.tabs[ti].frames.get(frame).is_none_or(|frame|!document.is_for(&frame.page.doc)) {
            // document.open may reattach the same control/form NodeIds. Their
            // old refs still belong to the replaced document incarnation.
            agent.tabs[ti].refs.drop_frame(frame);
            if outcome.navigation.is_none() {
                field_document_replaced = true;
                notes.push("The document was replaced while filling; take a new snapshot before continuing.".into());
                let pending: Vec<String> = operations[index + 1..].iter().map(|operation| format!("\"{}\"", esc(&operation.key))).collect();
                if !pending.is_empty() {
                    notes.push(format!("Not attempted after document replacement: {}.", pending.join(", ")));
                }
                if submit {
                    notes.push("submit=true was not attempted after document replacement.".into());
                }
                break;
            }
        }
        if outcome.navigation.is_some() {
            field_navigation = true;
            let pending: Vec<String> = operations[index + 1..].iter().map(|operation| format!("\"{}\"", esc(&operation.key))).collect();
            if !pending.is_empty() {
                notes.push(format!("Not attempted after navigation: {}. Take a new snapshot before filling remaining fields; old document refs are obsolete.", pending.join(", ")));
            }
            if submit {
                notes.push("submit=true was not attempted after the field requested navigation.".into());
            }
            break;
        }
    }
    let mut what = if done.is_empty() { String::new() } else { format!("Filled: {}", done.join("; ")) };
    if !errors.is_empty() {
        if what.is_empty() { what = "Nothing filled".into(); }
        what.push_str(&format!("\nNot filled: {}", errors.join("; ")));
    }
    let mut request = None;
    let mut submit_error = false;
    let mut submit_cancelled = false;
    if submit && !field_navigation && !field_document_replaced {
        let resolved = agent.resolve(ti, form.as_ref().unwrap());
        match resolved {
            Err(error) => {
                submit_error = true;
                notes.push(format!("Not submitted: {error}"));
            }
            Ok((form_frame, node)) => {
                frame = form_frame;
                let tab = &mut agent.tabs[ti];
                let invalid = tab.frames.with_page(&agent.engine,frame,|page| crate::forms::validate(&agent.engine,page,node))?;
                if !invalid.is_empty() {
                    submit_error = true;
                    tab.invalid = invalid.iter().map(|invalid| (frame, invalid.node)).collect();
                    let list: Vec<String> = invalid.iter().map(|invalid| {
                        let reference = tab.refs.assign(frame, invalid.node, "textbox", &invalid.label);
                        format!("@e{reference} \"{}\": {}", esc(&invalid.label), invalid.reason)
                    }).collect();
                    notes.push(format!("Not submitted: {} invalid field(s):\n- {}", list.len(), list.join("\n- ")));
                } else {
                    tab.invalid.clear();
                    let (submission_request,submitted)=tab.frames.with_page(&agent.engine,frame,|page| {
                        let mut submitted = ScriptOutcome::default();
                        let request = match page.default_submitter(node) {
                            Some(submitter) => {
                                let (action, event) = agent.engine.click_element(page, submitter, None, rabbithole_engine::Modifiers::default());
                                submitted = event;
                                crate::actions::click_request(&agent.engine, page, action, &mut submitted)
                            }
                            None => crate::actions::submit_request(&agent.engine, page, node, None, &mut submitted),
                        };
                        (request,submitted)
                    })?;
                    request=submission_request;
                    submit_cancelled = request.is_none() && submitted.default_prevented;
                    let description = if submit_cancelled { "Not submitted: the submit or click event was cancelled" } else { "Submitted the form" };
                    if !what.is_empty() { what.push('\n'); }
                    what.push_str(description);
                    crate::actions::merge(&mut outcome, submitted);
                }
            }
        }
    }
    if what.is_empty() { what = if field_navigation { "Fill" } else { "Nothing filled" }.into(); }
    let failed = !errors.is_empty() || field_document_replaced || submit_cancelled;
    // A callback may have removed/replaced its field. Do not dereference an
    // old target when finish_action follows a frame navigation or reports diff.
    let mut text = agent.finish_action(ti, frame, before, outcome, request, &what, None)?;
    if !notes.is_empty() {
        text.push('\n');
        text.push_str(&notes.join("\n"));
    }
    if submit_error || (!errors.is_empty() && done.is_empty() && (!submit || field_document_replaced) && !field_navigation) {
        return Err(text);
    }
    Ok(Out { failed, ..Out::text(text) })
}

fn single_field(page: &rabbithole_engine::Page, node: NodeId, form: Option<NodeId>) -> Vec<crate::forms::Field> {
    if let Some(f) = form {
        return crate::forms::fields(page, f);
    }
    let kind = page.control_kind(node);
    vec![crate::forms::Field {
        node,
        role: match kind {
            rabbithole_engine::ControlKind::Checkbox => "checkbox",
            rabbithole_engine::ControlKind::Radio => "radio",
            rabbithole_engine::ControlKind::Select => "combobox",
            _ => "textbox",
        },
        label: crate::ax::accessible_name(page, node, "textbox"),
        name: page.doc.attr(node, "name").unwrap_or("").to_string(),
        ty: page.doc.attr(node, "type").unwrap_or("text").to_string(),
        required: false,
        kind,
    }]
}

fn scroll(agent: &mut Agent, a: &Value) -> Result<Out, String> {
    let ti = agent.tab_index()?;
    agent.ensure_loaded(ti)?;
    let before = agent.before_state(ti);
    let max_steps = n(a, "max_steps").map(|x| x as usize).unwrap_or(10).clamp(1, 50);
    let cap = agent.cfg.settle_cap;
    let mut outcome = ScriptOutcome::default();
    let what;
    // Scroll an element into view.
    if let (Some(r), None) = (s(a, "ref"), s(a, "until")) {
        let (frame, node) = agent.resolve(ti, r)?;
        let label = agent.describe_ref(ti, frame, node);
        let (scrolled,y)=agent.tabs[ti].frames.with_page(&agent.engine,frame,|page| -> Result<_,String> {
            let rect = page.rect_of(node).ok_or_else(|| format!("{r} has no box"))?;
            let vh = page.viewport.height;
            let max = (page.content_height() - vh).max(0.0);
            let y = (rect.y + rect.h / 2.0 - vh / 2.0).clamp(0.0, max);
            agent.engine.set_page_scroll(page, page.page_scroll.0, y);
            let outcome = agent.engine.dispatch_window_event(page, "scroll");
            Ok((outcome,y))
        })??;
        outcome=scrolled;
        what = format!("Scrolled {label} into view (scroll={})", y.round());
        let t = Some((frame, node));
        let _ = t;
        return Ok(Out::text(agent.finish_action(ti, frame, before, outcome, None, &what, None)?));
    }
    let until = s(a, "until").map(|s| s.to_string());
    let load_more = b(a, "load_more").unwrap_or(false);
    let dir = s(a, "direction").unwrap_or("down").to_ascii_lowercase();
    let pages = n(a, "pages").unwrap_or(1.0) as f32;
    if until.is_none() && !load_more {
        let page = &mut agent.tabs[ti].frames[0];
        let vh = page.viewport.height;
        let max = (page.content_height() - vh).max(0.0);
        let cur = page.page_scroll.1;
        let y = match dir.as_str() {
            "top" => 0.0,
            "bottom" => max,
            "up" => (cur - vh * 0.85 * pages).max(0.0),
            _ => (cur + vh * 0.85 * pages).min(max),
        };
        agent.engine.set_page_scroll(page, page.page_scroll.0, y);
        crate::actions::merge(&mut outcome, agent.engine.dispatch_window_event(page, "scroll"));
        let at_end = (y - max).abs() < 1.0 && dir != "up" && dir != "top";
        what = format!("Scrolled {dir} to {}/{}{}", y.round(), max.round(), if at_end { " (end of page)" } else { "" });
        return Ok(Out::text(agent.finish_action(ti, 0, before, outcome, None, &what, None)?));
    }
    // Repeated scrolling: until a target is visible, or to load more.
    let mut steps = 0;
    let mut grew = 0;
    let mut found = false;
    let mut last_height = agent.tabs[ti].frames[0].content_height();
    while steps < max_steps {
        steps += 1;
        if let Some(u) = &until {
            if visible_target(agent, ti, u)? {
                found = true;
                break;
            }
        }
        let tab = &mut agent.tabs[ti];
        let log = tab.netlog.clone();
        let page = &mut tab.frames[0];
        let vh = page.viewport.height;
        let max = (page.content_height() - vh).max(0.0);
        let y = if load_more { max } else { (page.page_scroll.1 + vh * 0.85).min(max) };
        let at_bottom = (page.page_scroll.1 - max).abs() < 1.0;
        agent.engine.set_page_scroll(page, page.page_scroll.0, y);
        crate::actions::merge(&mut outcome, agent.engine.dispatch_window_event(page, "scroll"));
        let s = crate::wait::settle(&agent.engine, page, Some(&log), cap.min(Duration::from_secs(3)));
        if let Some(n) = s.navigation {
            outcome.navigation = Some(n);
            break;
        }
        outcome.dialogs.extend(s.dialogs);
        outcome.new_windows.extend(s.new_windows);
        let h = page.content_height();
        if h > last_height + 1.0 {
            grew += 1;
            last_height = h;
        } else if at_bottom {
            // Nothing more loads by scrolling: try a "load more" button.
            if load_more && !click_load_more(agent, ti, &mut outcome) {
                break;
            }
            if !load_more {
                break;
            }
        }
    }
    if let Some(u) = &until {
        if !found && visible_target(agent, ti, u)? {
            found = true;
        }
        what = if found { format!("Scrolled until {u} is visible ({steps} steps)") } else { format!("Scrolled {steps} steps; {u} is still not visible") };
    } else {
        what = format!("Scrolled to load more: page grew {grew} time(s) in {steps} steps");
    }
    Ok(Out::text(agent.finish_action(ti, 0, before, outcome, None, &what, None)?))
}

fn click_load_more(agent: &mut Agent, ti: usize, outcome: &mut ScriptOutcome) -> bool {
    let (root, _, _) = agent.tree(ti, false);
    let mut target = None;
    let words = ["load more", "show more", "more results", "ver más", "cargar más", "mostrar más", "mehr laden", "voir plus", "next page", "more"];
    root.visit(&mut |x, _| {
        if target.is_none() && x.kind == Kind::Control && x.frame == 0 && matches!(x.role.as_str(), "button" | "link" | "clickable") {
            let nm = x.name.to_lowercase();
            if words.iter().any(|w| nm == *w || nm.starts_with(w)) {
                target = Some(x.node);
            }
        }
    });
    let Some(node) = target else { return false };
    let page = &mut agent.tabs[ti].frames[0];
    let (action, o) = agent.engine.click_element(page, node, None, rabbithole_engine::Modifiers::default());
    let mut o = o;
    if let Some(req) = crate::actions::click_request(&agent.engine, page, action, &mut o) {
        o.navigation = Some(rabbithole_engine::PendingNavigation { request: req, replace: false });
    }
    crate::actions::merge(outcome, o);
    outcome.navigation.is_none()
}

/// Is the target (a ref, or text) inside the viewport?
fn visible_target(agent: &mut Agent, ti: usize, u: &str) -> Result<bool, String> {
    let page = &agent.tabs[ti].frames[0];
    let vp = (page.page_scroll.1, page.page_scroll.1 + page.viewport.height);
    if u.starts_with('@') {
        let (f, node) = agent.resolve(ti, u)?;
        if f != 0 {
            return Ok(true);
        }
        let page = &agent.tabs[ti].frames[0];
        return Ok(page.rect_of(node).map(|r| r.y < vp.1 && r.bottom() > vp.0).unwrap_or(false));
    }
    let needle = u.to_lowercase();
    let (root, _, _) = agent.tree(ti, false);
    let mut hit = false;
    root.visit(&mut |x, _| {
        if !hit && x.frame == 0 && x.name.to_lowercase().contains(&needle) {
            if let Some(r) = x.rect {
                let r = if x.fixed { r.translate(0.0, vp.0) } else { r };
                if r.y < vp.1 && r.bottom() > vp.0 {
                    hit = true;
                }
            }
        }
    });
    Ok(hit)
}

fn screenshot(agent: &mut Agent, a: &Value) -> Result<Out, String> {
    let ti = agent.tab_index()?;
    agent.ensure_loaded(ti)?;
    agent.ensure_fonts();
    // System faces can arrive after this page was built. Refresh its own
    // font view and geometry before computing the capture region or marks.
    agent.engine.restyle(&mut agent.tabs[ti].frames[0]);
    let scale = n(a, "scale").unwrap_or(1.0).clamp(0.25, 3.0) as f32;
    let marks = b(a, "marks").unwrap_or(true);
    // Images were skipped while loading: fetch them now (bounded wait).
    if !agent.engine.loader.load_images {
        agent.engine.loader.load_images = true;
        let tab = &mut agent.tabs[ti];
        let page = &mut tab.frames[0];
        agent.engine.restyle(page);
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while page.images_in_flight() > 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(30));
            let _ = agent.engine.tick(page);
        }
        agent.engine.loader.load_images = agent.cfg.images;
    }
    let area = match s(a, "ref") {
        Some(r) => {
            let (f, node) = agent.resolve(ti, r)?;
            if f != 0 {
                return Err(format!("{r} is inside a frame; screenshot the page instead"));
            }
            let rect = agent.tabs[ti].frames[0].rect_of(node).ok_or_else(|| format!("{r} has no box"))?;
            crate::shot::Area::Region(rect)
        }
        None if b(a, "full").unwrap_or(false) => crate::shot::Area::Full,
        None => crate::shot::Area::Viewport,
    };
    let root = if marks { Some(agent.tree(ti, false).0) } else { None };
    let page = &agent.tabs[ti].frames[0];
    let (mut canvas, region) = crate::shot::render(&agent.engine, page, area, scale);
    let legend = match &root {
        Some(r) => crate::shot::draw_marks(&agent.engine, &mut canvas, r, region, scale, page),
        None => Vec::new(),
    };
    let png = crate::shot::png(&canvas)?;
    let mut text = format!("{} ({}x{} px, region {}x{} at {},{})", esc(&page.url.to_string()), canvas.width, canvas.height, region.w.round(), region.h.round(), region.x.round(), region.y.round());
    if !legend.is_empty() {
        text.push('\n');
        text.push_str(&fence(&legend.join("\n")));
    }
    Ok(Out { text, png: Some(png), failed: false })
}

fn tabs(agent: &mut Agent, a: &Value) -> Result<Out, String> {
    let action = s(a, "action").unwrap_or("list").to_ascii_lowercase();
    let find_tab = |agent: &Agent, id: Option<f64>| -> Result<usize, String> {
        let id = id.ok_or("this action needs id")? as u32;
        agent.tabs.iter().position(|t| t.id == id).ok_or_else(|| format!("no tab t{id}"))
    };
    match action.as_str() {
        "list" => {
            let mut lines = Vec::new();
            for (i, t) in agent.tabs.iter().enumerate() {
                let (title, url) = match t.page() {
                    Some(p) => (p.title.trim().to_string(), p.url.to_string()),
                    None => (String::from("(not loaded)"), t.pending.as_ref().map(|u| u.to_string()).unwrap_or_default()),
                };
                lines.push(format!(
                    "{} t{} [{}] \"{}\" {}{}",
                    if Some(i) == agent.active { "*" } else { " " },
                    t.id,
                    agent.contexts[t.ctx].name,
                    esc(&clip_text(&title, 60)),
                    esc(&url),
                    t.opener.map(|o| format!(" (opened by t{o})")).unwrap_or_default()
                ));
            }
            if !agent.downloads.is_empty() {
                lines.push("Downloads (not saved):".into());
                for d in &agent.downloads {
                    lines.push(format!("  #{} \"{}\" {} {}", d.id, esc(&d.name), d.size.map(crate::browser::human_size).unwrap_or_else(|| "size unknown".into()), esc(&d.url)));
                }
            }
            let ctxs: Vec<String> = agent.contexts.iter().map(|c| c.name.clone()).collect();
            lines.push(format!("Contexts: {}", ctxs.join(", ")));
            if lines.len() == 1 {
                lines.insert(0, "(no tabs)".into());
            }
            Ok(Out::text(fence(&lines.join("\n"))))
        }
        "switch" => {
            let ti = find_tab(agent, n(a, "id"))?;
            agent.active = Some(ti);
            agent.ensure_loaded(ti)?;
            let id = agent.tabs[ti].id;
            Ok(Out::text(format!("Switched to t{id}\n{}", agent.snapshot_text(ti, SNAPSHOT_TOKENS, None, false, false)?)))
        }
        "new" => {
            let ctx = match s(a, "context") {
                Some(c) => agent.context_index(c),
                None => agent.active.map(|t| agent.tabs[t].ctx).unwrap_or(0),
            };
            let ti = agent.new_tab(ctx);
            agent.active = Some(ti);
            match s(a, "url") {
                Some(u) => {
                    let url = Url::parse(u).ok_or_else(|| format!("invalid URL: {u}"))?;
                    let report = agent.navigate(ti, Request::get(url), true)?;
                    let id = agent.tabs[ti].id;
                    Ok(Out::text(format!("New tab t{id}\n{}{}", report, agent.snapshot_text(ti, SNAPSHOT_TOKENS, None, false, false)?)))
                }
                None => Ok(Out::text(format!("New empty tab t{} (context {})", agent.tabs[ti].id, agent.contexts[ctx].name))),
            }
        }
        "close" => {
            let ti = match n(a, "id") {
                Some(_) => find_tab(agent, n(a, "id"))?,
                None => agent.active.ok_or("no tab")?,
            };
            let id = agent.tabs[ti].id;
            agent.tabs.remove(ti);
            agent.active = if agent.tabs.is_empty() { None } else { Some(ti.min(agent.tabs.len() - 1)) };
            Ok(Out::text(format!("Closed t{id}. {}", agent.active.map(|t| format!("Active: t{}", agent.tabs[t].id)).unwrap_or_else(|| "No tabs left.".into()))))
        }
        "download" => {
            let id = n(a, "id").ok_or("download needs id")? as u32;
            let path = s(a, "path").ok_or("download needs path")?;
            Ok(Out::text(agent.save_download(id, &PathBuf::from(path))?))
        }
        "save" => {
            let path = s(a, "path").ok_or("save needs path (a directory)")?;
            Ok(Out::text(agent.save_session(&PathBuf::from(path))?))
        }
        "load" => {
            let path = s(a, "path").ok_or("load needs path (a directory saved with action=save)")?;
            let name = s(a, "context").unwrap_or("restored").to_string();
            let (ci, urls) = agent.load_session(&PathBuf::from(path), &name)?;
            let mut opened = Vec::new();
            for u in urls.iter().take(10) {
                if let Some(url) = Url::parse(u) {
                    let ti = agent.new_tab(ci);
                    agent.tabs[ti].pending = Some(url);
                    opened.push(format!("t{}", agent.tabs[ti].id));
                    if agent.active.map(|t| agent.tabs[t].ctx != ci).unwrap_or(true) {
                        agent.active = Some(ti);
                    }
                }
            }
            Ok(Out::text(format!("Session restored into context \"{name}\" ({} tabs: {}; loaded on first use).", opened.len(), opened.join(", "))))
        }
        other => Err(format!("unknown tabs action: {other} (list, switch, new, close, download, save, load)")),
    }
}

fn merge_outcome(into: &mut ScriptOutcome, mut other: ScriptOutcome) {
    into.relayout |= other.relayout;
    into.default_prevented |= other.default_prevented;
    if into.navigation.is_none() { into.navigation = other.navigation.take(); }
    into.new_windows.append(&mut other.new_windows);
    into.dialogs.append(&mut other.dialogs);
    into.scroll_to = other.scroll_to.or(into.scroll_to);
}

// A tool call is an opportunity to deliver ready responses from an earlier
// evaluation. Keep effects queued for the next action; never silently navigate.
fn advance_background(agent: &mut Agent) {
    let Some(ti) = agent.active.filter(|ti| *ti < agent.tabs.len()) else { return };
    agent.prepare(ti);
    let deadline = Instant::now() + Duration::from_millis(10);
    let mut events = Vec::new();
    if let Some(page)=agent.tabs[ti].frames.root_mut() {
        if page.script.pending_navigation.is_none() && Instant::now()<deadline {
            let (_,mut out)=agent.engine.pump_console_tasks(page,deadline);
            if let Some(nav)=&out.navigation {
                events.push(format!("Background script requested navigation to {}; follow with debug follow=true.",nav.request.url));
            }
            page.script.pending_navigation=out.navigation.take();
            page.script.new_windows.append(&mut out.new_windows);
            page.script.dialogs.append(&mut out.dialogs);
        }
    }
    agent.load_frames(ti);
    agent.tabs[ti].events.extend(events);
}

fn debug_evaluate(agent: &mut Agent, args: &Value, source: &str) -> Result<(String, bool), String> {
    use rabbithole_engine::script::ConsolePoll;
    let ti = agent.tab_index()?;
    agent.ensure_loaded(ti)?;
    agent.prepare(ti);
    let timeout = match args.get("timeout") {
        Some(value) => Duration::from_millis(value.as_u64().filter(|v| *v > 0).ok_or("timeout needs a positive integer in milliseconds")?),
        None => agent.cfg.settle_cap.max(Duration::from_millis(1)),
    };
    let deadline = Instant::now().checked_add(timeout).ok_or("timeout is too large")?;
    let follow = b(args, "follow").unwrap_or(false);
    let before = follow.then(|| agent.before_state(ti));
    let (started, mut outcome) = agent.engine.start_console_evaluation(&mut agent.tabs[ti].frames[0], source, deadline);
    let mut failed = false;
    let result = match started {
        Err(error) => { failed = true; format!("Error: {error}") }
        Ok(evaluation) => {
            let mut pending = false;
            let result = loop {
                // Navigation terminates this document's observation before a
                // poll, and the handle drops before finish_action replaces it.
                if outcome.navigation.is_some() { break "Evaluation cancelled: script requested navigation".to_string(); }
                if Instant::now() >= deadline {
                    failed = true;
                    break if pending { format!("Promise still pending after {} ms", timeout.as_millis()) } else { format!("Evaluation timed out after {} ms", timeout.as_millis()) };
                }
                let (polled, out) = agent.engine.poll_console_evaluation(&mut agent.tabs[ti].frames[0], &evaluation, deadline);
                merge_outcome(&mut outcome, out);
                if outcome.navigation.is_some() { continue; }
                match polled {
                    Ok(ConsolePoll::Resolved(value)) => break value,
                    Ok(ConsolePoll::Rejected(error)) => { failed = true; break format!("Uncaught (in promise) {error}"); }
                    Err(_error) if Instant::now() >= deadline && pending => { failed = true; break format!("Promise still pending after {} ms", timeout.as_millis()); }
                    Err(error) => { failed = true; break format!("Error: {error}"); }
                    Ok(ConsolePoll::Pending) => pending = true,
                }
                let (pumped, out) = agent.engine.pump_console_tasks(&mut agent.tabs[ti].frames[0], deadline);
                merge_outcome(&mut outcome, out);
                if outcome.navigation.is_some() { continue; }
                if let Err(error) = pumped {
                    if Instant::now() >= deadline { continue; }
                    failed = true;
                    break format!("Error: {error}");
                }
                let now = Instant::now();
                if now < deadline {
                    let page = &agent.tabs[ti].frames[0];
                    let now_ms = agent.engine.start.elapsed().as_secs_f64() * 1000.0;
                    let delay = page.next_script_due().map(|due| Duration::from_secs_f64(((due - now_ms).max(0.0) / 1000.0).min(0.01))).unwrap_or(Duration::from_millis(10));
                    // Poll after a checkpoint before waiting, so a resolved
                    // microtask does not need a timer to become observable.
                    if !delay.is_zero() { std::thread::sleep(delay.min(deadline - now)); }
                }
            };
            drop(evaluation);
            result
        }
    };
    let mut text = format!("eval result:\n{}", fence(&clip_text(&result, 20_000)));
    if let Some(before) = before {
        text.push('\n');
        text.push_str(&agent.finish_action(ti, 0, before, outcome, None, "debug", None)?);
    } else if outcome.navigation.is_some() || !outcome.dialogs.is_empty() || !outcome.new_windows.is_empty() {
        text.push_str("\n(the script requested navigation, a dialog or a new window; it was not followed; use follow=true)");
    }
    agent.load_frames(ti);
    Ok((text, failed))
}

fn debug(agent: &mut Agent, a: &Value) -> Result<Out, String> {
    let mut parts = Vec::new();
    let mut failed = false;
    if b(a, "live").unwrap_or(false) {
        if agent.live.is_none() {
            agent.live = Some(crate::live::Live::start(0)?);
        }
        publish_live(agent);
        parts.push(format!("Live view: {} (updates after every action)", agent.live.as_ref().map(|l| l.url.clone()).unwrap_or_default()));
    }
    if let Some(js) = a.get("eval").and_then(Value::as_str) {
        let (text, eval_failed) = debug_evaluate(agent, a, js)?;
        parts.push(text);
        failed |= eval_failed;
    }
    if b(a, "console").unwrap_or(false) || b(a, "clear").unwrap_or(false) {
        let ti = agent.tab_index()?;
        let mut lines = Vec::new();
        agent.tabs[ti].frames.visit_mut(|page|lines.extend(page.take_console()));
        let tab = &mut agent.tabs[ti];
        tab.console.extend(lines);
        if b(a, "console").unwrap_or(false) {
            let shown: Vec<String> = tab.console.iter().rev().take(100).rev().map(|(l, m)| format!("[{l}] {}", clip_text(m, 400))).collect();
            parts.push(if shown.is_empty() { "Console is empty.".into() } else { format!("Console ({} lines):\n{}", tab.console.len(), fence(&shown.join("\n"))) });
        }
        if b(a, "clear").unwrap_or(false) {
            tab.console.clear();
        }
    }
    if let Some(fmt) = s(a, "log") {
        parts.push(export_log(agent, fmt));
    }
    if b(a, "stats").unwrap_or(false) {
        let nodes: usize = agent.tabs.iter().flat_map(|t| t.frames.iter()).map(|f| f.page.doc.node_count()).sum();
        parts.push(format!(
            "{} tabs, {} contexts, {} DOM nodes, RSS {}",
            agent.tabs.len(),
            agent.contexts.len(),
            nodes,
            Agent::rss_kb().map(|k| format!("{:.1} MB", k as f64 / 1024.0)).unwrap_or_else(|| "n/a".into())
        ));
    }
    if parts.is_empty() {
        return Err("debug needs eval, console, log, stats or live".into());
    }
    Ok(Out { failed, ..Out::text(parts.join("\n")) })
}

/// Export the action log as a replayable script (CLI syntax) or JSON test.
pub fn export_log(agent: &Agent, fmt: &str) -> String {
    match fmt {
        "json" | "test" => {
            let steps: Vec<Value> = agent.action_log.iter().map(|(t, a, url)| json!({ "tool": t, "args": a, "expect_url": url })).collect();
            serde_json::to_string_pretty(&json!({ "rabbithole_agent_test": 1, "steps": steps })).unwrap_or_default()
        }
        _ => {
            let mut out = String::from("# rabbithole agent script (replay: rabbithole agent --script FILE)\n");
            for (t, a, url) in &agent.action_log {
                out.push_str(&format!("{t} {}\n", a));
                if !url.is_empty() {
                    out.push_str(&format!("# expect url {url}\n"));
                }
            }
            out
        }
    }
}

/// Parse one CLI line (`click @e3`, `open https://x max_tokens=500`,
/// `fill @f1 values={"email":"a@b.c"} submit=true`, or `tool {json}`)
/// into (tool, args).
pub fn parse_line(line: &str) -> Result<(String, Value), String> {
    let line = line.trim();
    let (tool, rest) = match line.split_once(char::is_whitespace) {
        Some((t, r)) => (t.to_string(), r.trim()),
        None => (line.to_string(), ""),
    };
    if rest.starts_with('{') {
        let v: Value = serde_json::from_str(rest).map_err(|e| format!("bad JSON arguments: {e}"))?;
        return Ok((tool, v));
    }
    let toks = shell_split(rest)?;
    let positional: &[&str] = match tool.as_str() {
        "open" => &["url"],
        "click" | "snapshot" | "read" | "screenshot" => &["ref"],
        "type" => &["ref", "text"],
        "fill" => &["ref"],
        "find" => &["text"],
        "scroll" => &["direction"],
        "network" => &["id"],
        "tabs" => &["action", "id"],
        "debug" => &["eval"],
        _ => &[],
    };
    let mut args = serde_json::Map::new();
    let mut pi = 0;
    for t in toks {
        if let Some((k, v)) = t.split_once('=').filter(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
            let val = serde_json::from_str::<Value>(v).unwrap_or_else(|_| Value::String(v.to_string()));
            // Keep strings that merely look like numbers for text fields.
            let val = if matches!(k, "text" | "url" | "ref" | "value" | "path" | "eval") && !(k == "text" && val.is_array()) { Value::String(v.to_string()) } else { val };
            args.insert(k.to_string(), val);
        } else if tool == "scroll" && t.starts_with('@') {
            args.insert("ref".into(), Value::String(t));
        } else if let Some(k) = positional.get(pi) {
            let val = if *k == "id" { t.parse::<f64>().map(|x| json!(x)).unwrap_or(Value::String(t.clone())) } else { Value::String(t.clone()) };
            args.insert(k.to_string(), val);
            pi += 1;
        } else if tool == "debug" || tool == "type" || tool == "find" {
            // Extra words continue the last positional value.
            let k = positional.last().copied().unwrap_or("text");
            let prev = args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            args.insert(k.into(), Value::String(format!("{prev} {t}").trim().to_string()));
        } else {
            return Err(format!("unexpected argument `{t}` for {tool}"));
        }
    }
    Ok((tool, Value::Object(args)))
}

fn shell_split(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    let mut depth = 0;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q && depth == 0 => quote = None,
            (Some(_), '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            (Some(_), c) => cur.push(c),
            (None, '{') | (None, '[') => {
                depth += 1;
                cur.push(c);
                has = true;
            }
            (None, '}') | (None, ']') => {
                depth -= 1;
                cur.push(c);
            }
            (None, '"') | (None, '\'') if depth == 0 => {
                quote = Some(c);
                has = true;
            }
            (None, c) if c.is_whitespace() && depth == 0 => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            (None, c) => {
                cur.push(c);
                has = true;
            }
        }
    }
    if quote.is_some() {
        return Err("unterminated quote".into());
    }
    if has {
        out.push(cur);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_budget() {
        let d = definitions();
        assert!(d.len() <= 13, "{} tools", d.len());
        for t in &d {
            assert!(t["description"].as_str().unwrap().len() < 200, "{}", t["name"]);
        }
        let names: Vec<&str> = d.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, TOOLS);
        let click = d.iter().find(|tool| tool["name"] == "click").unwrap();
        assert_eq!(click["inputSchema"]["properties"]["double"]["type"], "boolean");
        let catalogue = serde_json::to_string(&json!({ "tools": d })).unwrap();
        assert!(catalogue.chars().count() <= 7500, "tools/list: {} chars", catalogue.chars().count());
        eprintln!("tools/list ({} chars): {catalogue}", catalogue.chars().count());
    }

    #[test]
    fn search_windows_map_expanding_lowercase_back_to_original_characters() {
        let terms = ["x", "ος", "i", "\u{307}"].map(str::to_string);
        assert_eq!(text_matches("İx ΟΣ!", &terms), [vec![(1, 2)], vec![(3, 5)], vec![(0, 1)], vec![(0, 1)]]);
        let name = format!("{}NEEDLE{}", "İ".repeat(400), "𐐀".repeat(400));
        let hits = text_matches(&name, &["needle".into()]);
        assert_eq!(hits, [vec![(400, 406)]]);
        let snippet = match_context(&name, Some(hits[0][0]), 240);
        assert_eq!(snippet.chars().count(), 240);
        assert!(snippet.starts_with('…') && snippet.ends_with('…') && snippet.contains("NEEDLE"), "{snippet}");
        assert!(!match_context(&name, Some(hits[0][0]), 0).contains("NEEDLE"));
    }

    #[test]
    fn search_windows_preserve_whole_names_edges_and_legacy_clipping() {
        assert_eq!(match_context("short", Some((0, 2)), 240), "short");
        assert_eq!(match_context("abcdefghij", Some((0, 1)), 5), "abcd…");
        assert_eq!(match_context("abcdefghij", Some((9, 10)), 5), "…ghij");
        assert_eq!(match_context("abcdefghij", Some((4, 5)), 5), "…def…");
        assert_eq!(match_context("abcdefghij", Some((4, 5)), 1), "…");
        assert_eq!(match_context(&"a".repeat(121), None, 0), format!("{}…", "a".repeat(119)));
        assert_eq!(search_terms(&json!({"text":["Guido","GUIDO","1989"]}), "text").unwrap().unwrap(), ["Guido", "1989"]);
        for args in [json!({"text":[]}), json!({"text":["ok",3]}), json!({"text":" "})] {
            assert!(search_terms(&args, "text").is_err());
        }
        for value in [json!(-1), json!(1.5), json!(4097), json!("240")] {
            assert!(find_context(&json!({"context":value})).is_err());
        }
    }

    #[test]
    fn search_windows_show_distant_terms_merge_touching_regions_and_count_only_hidden_hits() {
        fn snippet(len: usize, entries: &[(usize, char)], terms: &[&str], window: usize) -> (String, usize) {
            let mut chars = vec!['.'; len];
            for &(index, c) in entries { chars[index] = c; }
            let passage: String = chars.into_iter().collect();
            let terms: Vec<_> = terms.iter().map(|s| s.to_string()).collect();
            let hits = text_matches(&passage, &terms);
            search_snippet(&[passage], &[hits], window)
        }
        let (text, shown) = snippet(800, &[(30, 'A'), (600, 'B')], &["A", "B"], 240);
        assert!(text.contains('A') && text.contains('B'), "{text}");
        assert!(text.chars().count() <= 720);
        assert_eq!(shown, 2);
        let (text, shown) = snippet(2000, &[(100, 'A'), (600, 'B'), (1100, 'C'), (1600, 'D')], &["D", "C", "B", "A"], 40);
        assert!(!text.contains('A') && text.contains('B') && text.contains('C') && text.contains('D'), "{text}");
        assert!(text.find('B') < text.find('C') && text.find('C') < text.find('D'));
        assert!(text.chars().count() <= 120);
        assert_eq!(shown, 3);
        let (text, shown) = snippet(300, &[(100, 'A'), (138, 'B')], &["A", "B"], 40);
        assert!(text.contains('A') && text.contains('B'), "{text}");
        assert_eq!(text.matches('…').count(), 2, "touching windows share one region: {text}");
        assert_eq!(shown, 2);
        let (_, shown) = snippet(800, &[(30, 'A'), (40, 'A'), (600, 'A')], &["A"], 40);
        assert_eq!(shown, 2, "the later occurrence is the only omitted hit");
        let (_, shown) = snippet(800, &[(30, 'A'), (40, 'A'), (600, 'A')], &["A"], 0);
        assert_eq!(shown, 2, "legacy clipping still counts visible hits");
    }

    #[test]
    fn search_windows_keep_unicode_hit_coverage_and_tiny_budgets_honest() {
        let passage = format!("{}İx{}ΟΣ{}", "𐐀".repeat(100), "-".repeat(400), "尾".repeat(100));
        let terms = ["i", "ος", "\u{307}"].map(str::to_string);
        let hits = text_matches(&passage, &terms);
        let (text, shown) = search_snippet(std::slice::from_ref(&passage), std::slice::from_ref(&hits), 40);
        assert!(text.contains("İx") && text.contains("ΟΣ"), "{text}");
        assert!(text.chars().count() <= 120);
        assert_eq!(shown, 3, "overlapping folded i and combining-dot hits are both visible");
        for window in [1, 2, 3, 4] {
            let (text, shown) = search_snippet(std::slice::from_ref(&passage), std::slice::from_ref(&hits), window);
            assert!(text.chars().count() <= window * 3, "{window}: {text}");
            assert!(shown <= 3);
        }
        let hits = text_matches("--LONGTERM--", &["LONGTERM".into()]);
        let (text, shown) = search_snippet(&["--LONGTERM--".into()], &[hits], 4);
        assert_eq!(shown, 0, "a clipped occurrence is not reported as shown: {text}");
        assert_eq!(find_context(&json!({"window":40,"context":8})).unwrap(), 40);
        assert_eq!(find_context(&json!({"window":40,"context":"session"})).unwrap(), 40);
        assert_eq!(find_context(&json!({"context":40})).unwrap(), 40);
        for value in [json!(-1), json!(1.5), json!(4097), json!("240")] {
            assert!(find_context(&json!({"window":value})).is_err());
        }
        let passages = ["A", "B", "C", "D"].map(str::to_string);
        let terms = ["D", "B"].map(str::to_string);
        let hits: Vec<_> = passages.iter().map(|p| text_matches(p, &terms)).collect();
        let (text, shown) = search_snippet(&passages, &hits, 1);
        assert_eq!(text, "B…D", "single-character windows still follow request selection and text order");
        assert_eq!(shown, 2);
        let terms = ["D", "C", "B", "A"].map(str::to_string);
        let hits: Vec<_> = passages.iter().map(|p| text_matches(p, &terms)).collect();
        let (text, shown) = search_snippet(&passages, &hits, 1);
        assert_eq!(text, "C…D", "the first two requested terms exhaust the three-character budget");
        assert_eq!(shown, 2);
    }

    #[test]
    fn cli_lines() {
        let (t, a) = parse_line("click @e3").unwrap();
        assert_eq!((t.as_str(), a["ref"].as_str()), ("click", Some("@e3")));
        let (_, a) = parse_line("type @e5 \"hello world\" submit=true").unwrap();
        assert_eq!(a["text"], "hello world");
        assert_eq!(a["submit"], true);
        let (_, a) = parse_line("fill @f1 values={\"email\": \"a@b.c\", \"Remember me\": true} submit=true").unwrap();
        assert_eq!(a["values"]["email"], "a@b.c");
        assert_eq!(a["values"]["Remember me"], true);
        let (_, a) = parse_line("open https://example.com/ max_tokens=500").unwrap();
        assert_eq!(a["url"], "https://example.com/");
        assert_eq!(a["max_tokens"], 500);
        let (_, a) = parse_line("find text='[\"Guido\",\"1989\"]'").unwrap();
        assert_eq!(a["text"], json!(["Guido", "1989"]));
        let (_, a) = parse_line("find text=1989").unwrap();
        assert_eq!(a["text"], "1989");
        let (_, a) = parse_line("network 12").unwrap();
        assert_eq!(a["id"], 12.0);
        let (_, a) = parse_line("debug document.title + 1").unwrap();
        assert_eq!(a["eval"], "document.title + 1");
        let (_, a) = parse_line("scroll {\"until\": \"@e9\"}").unwrap();
        assert_eq!(a["until"], "@e9");
        let (_, a) = parse_line("scroll @e9").unwrap();
        assert_eq!(a["ref"], "@e9");
    }
}
