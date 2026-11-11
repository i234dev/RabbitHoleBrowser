//! The agent tools (shared by the MCP server and the CLI): definitions,
//! argument handling and dispatch.

use std::path::PathBuf;
use std::time::Duration;

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

/// Tool names, in the order they are listed.
pub const TOOLS: &[&str] = &["open", "snapshot", "read", "find", "click", "type", "fill", "scroll", "screenshot", "network", "tabs", "debug"];

/// JSON schemas of the tools (MCP `tools/list`).
pub fn definitions() -> Vec<Value> {
    let r = json!({ "type": "string", "description": "Element ref from a snapshot, e.g. @e12 (forms: @f1)" });
    let dialog = json!({ "type": "string", "description": "Answer to alert/confirm/prompt raised during this call: accept, dismiss (default) or the prompt text" });
    vec![
        json!({ "name": "open", "description": "Load a URL (or back/forward/reload) in the current tab and return a short page summary (title, landmarks, heading outline, forms with field refs, overlays, counts); snapshot=true returns the full semantic snapshot instead (or call snapshot). Uses the standard browser identity by default; bot=true switches to the RabbitHoleBot identity with robots.txt.",
            "inputSchema": { "type": "object", "properties": {
                "url": { "type": "string", "description": "Absolute URL, or back | forward | reload" },
                "new_tab": { "type": "boolean" },
                "context": { "type": "string", "description": "Isolated context (cookies, storage) to use; created on demand" },
                "snapshot": { "type": "boolean", "description": "Return the full semantic snapshot instead of the short summary (alias: full)" },
                "full": { "type": "boolean", "description": "Alias of snapshot" },
                "max_tokens": { "type": "integer", "description": "Full-snapshot budget (default 2500; implies snapshot=true)" },
                "bot": { "type": "boolean", "description": "Session option: identify as RabbitHoleBot (bot User-Agent, no client hints) and respect robots.txt (default false: standard browser headers)" },
                "robots": { "type": "boolean", "description": "Respect robots.txt (default: on in bot mode, off otherwise)" },
                "allowlist": { "type": "array", "items": { "type": "string" }, "description": "Only allow these domains from now on" },
                "javascript": { "type": "boolean" },
                "dialog": dialog
            }, "required": ["url"] } }),
        json!({ "name": "snapshot", "description": "Compact semantic tree of the page with stable refs (@eN). Main content first; long lists truncated.",
            "inputSchema": { "type": "object", "properties": {
                "ref": { "type": "string", "description": "Expand only this element (e.g. a collapsed list or landmark)" },
                "max_tokens": { "type": "integer" },
                "hidden": { "type": "boolean", "description": "Also list hidden content, marked" },
                "urls": { "type": "boolean", "description": "Show link targets" }
            } } }),
        json!({ "name": "read", "description": "Main content as clean Markdown (article text, links as refs), without nav/footer/related.",
            "inputSchema": { "type": "object", "properties": {
                "ref": r.clone(), "max_tokens": { "type": "integer", "description": "Default 4000" }, "offset": { "type": "integer", "description": "Character offset to continue from" }
            } } }),
        json!({ "name": "find", "description": "Find elements by text, role/name or CSS selector; returns refs.",
            "inputSchema": { "type": "object", "properties": {
                "text": { "type": "string" }, "role": { "type": "string" }, "name": { "type": "string" }, "selector": { "type": "string" }, "limit": { "type": "integer" }
            } } }),
        json!({ "name": "click", "description": "Click an element (or hover=true). Waits until the page settles and returns what changed. ref=\"cookies\" rejects a cookie banner.",
            "inputSchema": { "type": "object", "properties": {
                "ref": r.clone(), "hover": { "type": "boolean" }, "force": { "type": "boolean", "description": "Dispatch even if covered or invisible" }, "dialog": dialog
            }, "required": ["ref"] } }),
        json!({ "name": "type", "description": "Type text into a field with key events and/or press a key (Enter, Tab, Escape, ArrowDown…). Returns what changed.",
            "inputSchema": { "type": "object", "properties": {
                "ref": r.clone(), "text": { "type": "string" }, "key": { "type": "string" }, "clear": { "type": "boolean", "description": "Replace the current value (default false: append)" },
                "submit": { "type": "boolean", "description": "Press Enter afterwards" }, "dialog": dialog
            } } }),
        json!({ "name": "fill", "description": "Forms as a unit. No args: list forms. ref=@fN: list its fields. values={label|name|@eN: value} fills; submit=true validates and submits.",
            "inputSchema": { "type": "object", "properties": {
                "ref": r.clone(), "values": { "type": "object", "description": "Field → value (text, true/false for checkboxes, option label for selects/radios)" },
                "value": { "description": "Value for a single field ref" }, "submit": { "type": "boolean" }, "dialog": dialog
            } } }),
        json!({ "name": "scroll", "description": "Scroll the page or an element into view; until=@eN|text scrolls until visible; load_more=true loads infinite lists.",
            "inputSchema": { "type": "object", "properties": {
                "ref": r.clone(), "direction": { "type": "string", "enum": ["down", "up", "top", "bottom"] }, "pages": { "type": "number" },
                "until": { "type": "string" }, "load_more": { "type": "boolean" }, "max_steps": { "type": "integer" }
            } } }),
        json!({ "name": "screenshot", "description": "PNG of the viewport (or full page / one element) with refs drawn over interactive elements.",
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
        json!({ "name": "debug", "description": "eval JavaScript, console/JS errors, action log export (log=script|json), live=true to watch the tab in a browser.",
            "inputSchema": { "type": "object", "properties": {
                "eval": { "type": "string" }, "console": { "type": "boolean" }, "clear": { "type": "boolean" }, "log": { "type": "string" }, "live": { "type": "boolean" }, "stats": { "type": "boolean" }
            } } }),
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

/// Dialog policy from a `dialog` argument.
fn dialog_policy(a: &Value) -> Option<DialogPolicy> {
    let d = s(a, "dialog")?;
    Some(match d.to_ascii_lowercase().as_str() {
        "accept" | "ok" | "yes" | "true" => DialogPolicy { confirm: true, prompt: Some(String::new()) },
        "dismiss" | "cancel" | "no" | "false" => DialogPolicy { confirm: false, prompt: None },
        _ => DialogPolicy { confirm: true, prompt: Some(d.to_string()) },
    })
}

/// Run a tool. Errors are user-facing messages.
pub fn call(agent: &mut Agent, name: &str, args: &Value) -> Result<Out, String> {
    let args = if args.is_null() { json!({}) } else { args.clone() };
    // Per-call dialog policy.
    let saved_policy = agent.active.map(|ti| agent.tabs[ti].dialog_policy.clone());
    if let (Some(p), Some(ti)) = (dialog_policy(&args), agent.active) {
        agent.tabs[ti].dialog_policy = p;
    }
    let mutating = matches!(name, "open" | "click" | "type" | "fill" | "scroll" | "tabs");
    let r = dispatch(agent, name, &args);
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
            let r = s(a, "ref").ok_or("click needs ref (e.g. @e12, or \"cookies\")")?.to_string();
            Ok(Out::text(agent.click(ti, &r, b(a, "hover").unwrap_or(false), b(a, "force").unwrap_or(false))?))
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

fn open(agent: &mut Agent, a: &Value) -> Result<Out, String> {
    let url = s(a, "url").ok_or("open needs url")?.to_string();
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
    let ctx = match s(a, "context") {
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
    if full {
        let max = n(a, "max_tokens").map(|x| x as usize).unwrap_or(SNAPSHOT_TOKENS).max(100);
        text.push_str(&agent.snapshot_text(ti, max, None, false, false)?);
    } else {
        text.push_str(&agent.summary_text(ti)?);
    }
    let hints = crate::netinfo::page_hints(&agent.tabs[ti]);
    for h in hints.into_iter().take(2) {
        text.push('\n');
        text.push_str(&h);
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
    let page = &tab.frames[frame].page;
    let md = match root {
        Some(node) => crate::read::read_subtree(page, &mut tab.refs, frame, node),
        None => crate::read::read(page, &mut tab.refs, frame),
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
    if generation != tab.refs.generation || f >= tab.frames.len() || !tab.frames[f].page.doc.is_connected(node) {
        return Err(format!("{parsed} no longer exists; take a new snapshot."));
    }
    Ok((f, node))
}

fn find(agent: &mut Agent, a: &Value) -> Result<Out, String> {
    let ti = agent.tab_index()?;
    agent.ensure_loaded(ti)?;
    let limit = n(a, "limit").map(|x| x as usize).unwrap_or(20).max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut total = 0;
    if let Some(sel) = s(a, "selector") {
        let toks = Tokenizer::tokenize(sel);
        let list = SelectorParser::new(&toks).parse_list().ok_or_else(|| format!("invalid selector: {sel}"))?;
        let tab = &mut agent.tabs[ti];
        for (fi, f) in tab.frames.iter().enumerate() {
            let doc = &f.page.doc;
            for nd in doc.descendants(Document::ROOT) {
                if doc.element(nd).is_none() || !list.iter().any(|s| matches(s, doc, nd)) {
                    continue;
                }
                total += 1;
                if lines.len() >= limit {
                    continue;
                }
                let role = doc.tag(nd).unwrap_or("").to_string();
                let text = crate::ax::content_text(&f.page, nd);
                let r = tab.refs.assign(fi, nd, &role, &clip_text(&text, 60));
                let vis = if !crate::ax::displayed(&f.page, nd) { " (hidden)" } else { "" };
                lines.push(format!("@e{r} <{role}> \"{}\"{vis}", esc(&clip_text(&text, 100))));
            }
        }
    } else {
        let text = s(a, "text").map(|t| t.to_lowercase());
        let role = s(a, "role").map(|t| t.to_lowercase());
        let name = s(a, "name").map(|t| t.to_lowercase());
        if text.is_none() && role.is_none() && name.is_none() {
            return Err("find needs text, role/name or selector".into());
        }
        let (root, _, _) = agent.tree(ti, false);
        let tab = &mut agent.tabs[ti];
        let mut found: Vec<(usize, NodeId, String, String, Option<u32>)> = Vec::new();
        // Track the nearest heading for context.
        root.visit(&mut |x, _| {
            if x.kind == Kind::Break {
                return;
            }
            let nm = x.name.to_lowercase();
            let ok_text = text.as_ref().map(|t| nm.contains(t.as_str())).unwrap_or(true);
            let ok_role = role.as_ref().map(|r| x.role == *r || (r == "textbox" && x.role == "searchbox")).unwrap_or(true);
            let ok_name = name.as_ref().map(|t| nm.contains(t.as_str())).unwrap_or(true);
            let leafish = matches!(x.kind, Kind::Control | Kind::Heading | Kind::Text | Kind::Image | Kind::Landmark | Kind::Dialog | Kind::Form | Kind::Frame);
            if ok_text && ok_role && ok_name && leafish && !x.name.is_empty() {
                found.push((x.frame, x.node, x.role.clone(), x.name.clone(), x.reference));
            }
        });
        total = found.len();
        for (f, nd, role, nm, r) in found.into_iter().take(limit) {
            let r = r.unwrap_or_else(|| tab.refs.assign(f, nd, &role, &clip_text(&nm, 60)));
            let page = &tab.frames[f].page;
            let pos = page.rect_of(nd).map(|rc| {
                let vy = rc.y - page.page_scroll.1;
                if f != 0 {
                    " (in frame)".to_string()
                } else if vy < 0.0 {
                    " (above viewport)".to_string()
                } else if vy > page.viewport.height {
                    " (below viewport)".to_string()
                } else {
                    String::new()
                }
            });
            lines.push(format!("@e{r} {role} \"{}\"{}", esc(&clip_text(&nm, 120)), pos.unwrap_or_default()));
        }
    }
    let mut text = if lines.is_empty() { "No matches.".to_string() } else { fence(&lines.join("\n")) };
    if total > lines.len() {
        text.push_str(&format!("\n({} more matches; raise limit)", total - lines.len()));
    }
    Ok(Out::text(text))
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
    let target = match s(a, "ref") {
        Some(r) => Some(agent.resolve(ti, r)?),
        None => {
            let f = agent.tabs[ti].frames[0].page.state.focused;
            if text.is_some() && f.is_none() {
                return Err("type needs ref (no element has focus)".into());
            }
            f.map(|n| (0, n))
        }
    };
    let frame = target.map(|t| t.0).unwrap_or(0);
    let label = target.map(|(f, nd)| agent.describe_ref(ti, f, nd)).unwrap_or_else(|| "the page".into());
    let before = agent.before_state(ti);
    let tab = &mut agent.tabs[ti];
    let page = &mut tab.frames[frame].page;
    let mut outcome = ScriptOutcome::default();
    let mut request = None;
    let mut what = Vec::new();
    if let Some(t) = &text {
        let (_, node) = target.ok_or("no target")?;
        let o = crate::actions::type_text(&agent.engine, page, node, t, clear)?;
        crate::actions::merge(&mut outcome, o);
        let shown = if page.control_kind(node) == rabbithole_engine::ControlKind::Password { format!("{} chars", t.chars().count()) } else { format!("\"{}\"", esc(&clip_text(t, 60))) };
        what.push(format!("Typed {shown} into {label}"));
    }
    let keys: Vec<String> = key.into_iter().chain(if submit { Some("Enter".to_string()) } else { None }).collect();
    for k in keys {
        if request.is_some() || outcome.navigation.is_some() {
            break;
        }
        let r = crate::actions::press_key(&agent.engine, page, target.map(|t| t.1), &k);
        crate::actions::merge(&mut outcome, r.outcome);
        request = r.request;
        what.push(format!("pressed {k}{}", r.action.map(|x| format!(" ({x})")).unwrap_or_default()));
    }
    let desc = what.join(", ");
    Ok(Out::text(agent.finish_action(ti, frame, before, outcome, request, &desc, target)?))
}

fn fill(agent: &mut Agent, a: &Value) -> Result<Out, String> {
    let ti = agent.tab_index()?;
    agent.ensure_loaded(ti)?;
    let values = a.get("values").and_then(Value::as_object).cloned();
    let submit = b(a, "submit").unwrap_or(false);
    let single = a.get("value").cloned();
    let Some(r) = s(a, "ref").map(|s| s.to_string()) else {
        if values.is_some() {
            return Err("fill with values needs ref (the form @fN or a field @eN); call fill without arguments to list forms".into());
        }
        // List every form.
        let tab = &mut agent.tabs[ti];
        let mut lines = Vec::new();
        for fi in 0..tab.frames.len() {
            let forms = crate::forms::forms(&tab.frames[fi].page);
            for f in forms {
                let page = &tab.frames[fi].page;
                lines.extend(crate::forms::describe(page, fi, f, &mut tab.refs));
            }
        }
        if lines.is_empty() {
            return Ok(Out::text("No forms on this page (controls outside forms are in the snapshot)."));
        }
        return Ok(Out::text(fence(&lines.join("\n"))));
    };
    let (frame, node) = agent.resolve(ti, &r)?;
    let is_form = agent.tabs[ti].frames[frame].page.doc.is_element(node, "form");
    let form = if is_form { Some(node) } else { agent.tabs[ti].frames[frame].page.form_of(node) };
    // Describe only.
    if values.is_none() && single.is_none() && !submit {
        let f = form.ok_or_else(|| format!("{r} is not in a form"))?;
        let tab = &mut agent.tabs[ti];
        let lines = crate::forms::describe(&tab.frames[frame].page, frame, f, &mut tab.refs);
        return Ok(Out::text(fence(&lines.join("\n"))));
    }
    let before = agent.before_state(ti);
    let mut done = Vec::new();
    let mut errors = Vec::new();
    {
        let tab = &mut agent.tabs[ti];
        if let Some(v) = &single {
            // A single field.
            let page = &tab.frames[frame].page;
            let scope = form.unwrap_or(node);
            let fields = if is_form { crate::forms::fields(page, scope) } else { single_field(page, node, form) };
            let idx = fields.iter().position(|f| f.node == node).ok_or_else(|| format!("{r} is not a fillable field"))?;
            match crate::forms::set_field(&agent.engine, &mut tab.frames[frame].page, &fields, idx, v) {
                Ok(d) => done.push(d),
                Err(e) => errors.push(e),
            }
        }
        if let Some(vals) = &values {
            let scope = form.ok_or_else(|| format!("{r} is not a form or inside one; fill single fields with ref=@eN value=…"))?;
            let fields = crate::forms::fields(&tab.frames[frame].page, scope);
            for (k, v) in vals {
                let idx = {
                    let page = &tab.frames[frame].page;
                    // Radio groups by name.
                    crate::forms::match_field(page, &fields, k, &tab.refs, frame).or_else(|| fields.iter().position(|f| f.kind == rabbithole_engine::ControlKind::Radio && f.name.eq_ignore_ascii_case(k)))
                };
                match idx {
                    Some(i) => match crate::forms::set_field(&agent.engine, &mut tab.frames[frame].page, &fields, i, v) {
                        Ok(d) => done.push(d),
                        Err(e) => errors.push(format!("{k}: {e}")),
                    },
                    None => {
                        let names: Vec<String> = fields.iter().filter(|f| f.role != "button").map(|f| if f.label.is_empty() { f.name.clone() } else { f.label.clone() }).collect();
                        errors.push(format!("no field matches \"{k}\" (fields: {})", names.join(", ")));
                    }
                }
            }
        }
    }
    let mut outcome = ScriptOutcome::default();
    let mut request = None;
    let mut what = if done.is_empty() { String::new() } else { format!("Filled: {}", done.join("; ")) };
    if !errors.is_empty() {
        if what.is_empty() {
            what = "Nothing filled".into();
        }
        what.push_str(&format!("\nNot filled: {}", errors.join("; ")));
    }
    if submit {
        let f = form.ok_or("nothing to submit: the ref is not in a form")?;
        let tab = &mut agent.tabs[ti];
        let page = &mut tab.frames[frame].page;
        let invalid = crate::forms::validate(&agent.engine, page, f);
        if !invalid.is_empty() {
            tab.invalid = invalid.iter().map(|i| (frame, i.node)).collect();
            let list: Vec<String> = invalid
                .iter()
                .map(|i| {
                    let rr = tab.refs.assign(frame, i.node, "textbox", &i.label);
                    format!("@e{rr} \"{}\": {}", esc(&i.label), i.reason)
                })
                .collect();
            let mut msg = String::new();
            if !what.is_empty() {
                msg.push_str(&what);
                msg.push('\n');
            }
            msg.push_str(&format!("Not submitted: {} invalid field(s):\n- {}", list.len(), list.join("\n- ")));
            return Err(msg);
        }
        tab.invalid.clear();
        // Submit by clicking the default submit button (so handlers run),
        // else fire submit on the form.
        let page = &mut tab.frames[frame].page;
        match page.default_submitter(f) {
            Some(sub) => {
                let (action, o) = agent.engine.click_element(page, sub, None, rabbithole_engine::Modifiers::default());
                let mut o = o;
                request = crate::actions::click_request(&agent.engine, page, action, &mut o);
                outcome = o;
            }
            None => {
                let mut o = ScriptOutcome::default();
                request = crate::actions::submit_request(&agent.engine, page, f, None, &mut o);
                outcome = o;
            }
        }
        what = if what.is_empty() { "Submitted the form".into() } else { format!("{what}\nSubmitted the form") };
    }
    let target = Some((frame, node));
    let text = agent.finish_action(ti, frame, before, outcome, request, &what, if submit { None } else { target })?;
    if !errors.is_empty() && done.is_empty() && !submit {
        return Err(text);
    }
    Ok(Out::text(text))
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
        let page = &mut agent.tabs[ti].frames[frame].page;
        let rect = page.rect_of(node).ok_or_else(|| format!("{r} has no box"))?;
        let vh = page.viewport.height;
        let max = (page.content_height() - vh).max(0.0);
        let y = (rect.y + rect.h / 2.0 - vh / 2.0).clamp(0.0, max);
        agent.engine.set_page_scroll(page, page.page_scroll.0, y);
        outcome = agent.engine.dispatch_window_event(page, "scroll");
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
        let page = &mut agent.tabs[ti].frames[0].page;
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
    let mut last_height = agent.tabs[ti].frames[0].page.content_height();
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
        let page = &mut tab.frames[0].page;
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
    let page = &mut agent.tabs[ti].frames[0].page;
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
    let page = &agent.tabs[ti].frames[0].page;
    let vp = (page.page_scroll.1, page.page_scroll.1 + page.viewport.height);
    if u.starts_with('@') {
        let (f, node) = agent.resolve(ti, u)?;
        if f != 0 {
            return Ok(true);
        }
        let page = &agent.tabs[ti].frames[0].page;
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
    let scale = n(a, "scale").unwrap_or(1.0).clamp(0.25, 3.0) as f32;
    let marks = b(a, "marks").unwrap_or(true);
    // Images were skipped while loading: fetch them now (bounded wait).
    if !agent.engine.loader.load_images {
        agent.engine.loader.load_images = true;
        let tab = &mut agent.tabs[ti];
        let page = &mut tab.frames[0].page;
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
            let rect = agent.tabs[ti].frames[0].page.rect_of(node).ok_or_else(|| format!("{r} has no box"))?;
            crate::shot::Area::Region(rect)
        }
        None if b(a, "full").unwrap_or(false) => crate::shot::Area::Full,
        None => crate::shot::Area::Viewport,
    };
    let root = if marks { Some(agent.tree(ti, false).0) } else { None };
    let page = &agent.tabs[ti].frames[0].page;
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
    Ok(Out { text, png: Some(png) })
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

fn debug(agent: &mut Agent, a: &Value) -> Result<Out, String> {
    let mut parts = Vec::new();
    if b(a, "live").unwrap_or(false) {
        if agent.live.is_none() {
            agent.live = Some(crate::live::Live::start(0)?);
        }
        publish_live(agent);
        parts.push(format!("Live view: {} (updates after every action)", agent.live.as_ref().map(|l| l.url.clone()).unwrap_or_default()));
    }
    if let Some(js) = a.get("eval").and_then(Value::as_str) {
        let ti = agent.tab_index()?;
        let tab = &mut agent.tabs[ti];
        let page = &mut tab.frames[0].page;
        let (result, out) = agent.engine.eval_with_outcome(page, js);
        let mut text = format!("eval result:\n{}", fence(&clip_text(&result, 20_000)));
        if out.navigation.is_some() || !out.dialogs.is_empty() {
            text.push_str("\n(the script requested a navigation or dialog; it was not followed)");
        }
        parts.push(text);
    }
    if b(a, "console").unwrap_or(false) || b(a, "clear").unwrap_or(false) {
        let ti = agent.tab_index()?;
        let mut lines = Vec::new();
        for f in &mut agent.tabs[ti].frames {
            lines.extend(f.page.take_console());
        }
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
    Ok(Out::text(parts.join("\n")))
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
            let val = if matches!(k, "text" | "url" | "ref" | "value" | "path" | "eval") { Value::String(v.to_string()) } else { val };
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
        assert!(d.len() <= 12, "{} tools", d.len());
        for t in &d {
            assert!(t["description"].as_str().unwrap().len() < 200, "{}", t["name"]);
        }
        let names: Vec<&str> = d.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, TOOLS);
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
