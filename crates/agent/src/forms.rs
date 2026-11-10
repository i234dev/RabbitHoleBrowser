//! Forms as a unit: listing fields (label, type, ref, required), filling
//! by label/name, and HTML constraint validation before submitting.

use rabbithole_engine::dom::{Document, NodeId};
use rabbithole_engine::{ControlKind, Engine, Page};

use crate::ax::{accessible_name, clip_text, esc};
use crate::refs::RefMap;

/// One field of a form.
#[derive(Debug, Clone)]
pub struct Field {
    pub node: NodeId,
    pub role: &'static str,
    pub label: String,
    pub name: String,
    pub ty: String,
    pub required: bool,
    pub kind: ControlKind,
}

/// Controls of a form (including those associated with `form=` outside it).
pub fn fields(page: &Page, form: NodeId) -> Vec<Field> {
    let doc = &page.doc;
    let id = doc.attr(form, "id").map(|s| s.to_string());
    let mut out = Vec::new();
    let mut seen_radio: Vec<String> = Vec::new();
    for n in doc.descendants(Document::ROOT) {
        if !matches!(doc.tag(n), Some("input" | "select" | "textarea" | "button")) {
            continue;
        }
        let owner = page.form_of(n);
        let inside = owner == Some(form) || (id.is_some() && doc.attr(n, "form") == id.as_deref());
        if !inside || !crate::ax::displayed(page, n) {
            continue;
        }
        let kind = page.control_kind(n);
        if kind == ControlKind::Hidden {
            continue;
        }
        let ty = doc.attr(n, "type").unwrap_or(match doc.tag(n) {
            Some("select") => "select",
            Some("textarea") => "textarea",
            Some("button") => "submit",
            _ => "text",
        });
        let role = match kind {
            ControlKind::Checkbox => "checkbox",
            ControlKind::Radio => "radio",
            ControlKind::Select => "combobox",
            ControlKind::Submit | ControlKind::Reset | ControlKind::Button => "button",
            ControlKind::File => "file",
            _ => "textbox",
        };
        let name = doc.attr(n, "name").unwrap_or("").to_string();
        if kind == ControlKind::Radio && !name.is_empty() {
            if seen_radio.contains(&name) {
                // Radios are listed individually but keep one group label.
            } else {
                seen_radio.push(name.clone());
            }
        }
        out.push(Field {
            node: n,
            role,
            label: clip_text(&accessible_name(page, n, role), 80),
            name,
            ty: ty.to_ascii_lowercase(),
            required: doc.attr(n, "required").is_some() || doc.attr(n, "aria-required") == Some("true"),
            kind,
        });
    }
    out
}

/// All forms of a page.
pub fn forms(page: &Page) -> Vec<NodeId> {
    let doc = &page.doc;
    doc.descendants(Document::ROOT).filter(|&n| doc.is_element(n, "form") && crate::ax::displayed(page, n)).collect()
}

/// Describe a form and its fields.
pub fn describe(page: &Page, frame: usize, form: NodeId, refs: &mut RefMap) -> Vec<String> {
    let doc = &page.doc;
    let fr = refs.assign_form(frame, form);
    let mut name = accessible_name(page, form, "form");
    if name.is_empty() {
        // First heading/legend inside or just before the form.
        name = doc
            .descendants(form)
            .find(|&d| matches!(doc.tag(d), Some("h1" | "h2" | "h3" | "h4" | "legend")))
            .map(|h| doc.text_content(h).split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
    }
    let method = doc.attr(form, "method").unwrap_or("get").to_ascii_uppercase();
    let action = doc.attr(form, "action").and_then(|a| page.resolve(a)).map(|u| {
        let mut s = u.path.clone();
        if let Some(q) = &u.query {
            s.push('?');
            s.push_str(q);
        }
        s
    });
    let mut lines = vec![format!(
        "@f{fr} form{} ({method} {})",
        if name.is_empty() { String::new() } else { format!(" \"{}\"", esc(&clip_text(&name, 60))) },
        esc(&action.unwrap_or_else(|| page.url.path.clone()))
    )];
    for f in fields(page, form) {
        let r = refs.assign(frame, f.node, f.role, &f.label);
        let mut l = format!("  {} \"{}\" @e{r}", f.role, esc(&f.label));
        if !f.name.is_empty() {
            l.push_str(&format!(" name={}", esc(&f.name)));
        }
        if f.role == "textbox" && f.ty != "text" && f.ty != "textarea" {
            l.push_str(&format!(" type={}", esc(&f.ty)));
        }
        if f.role == "button" {
            l.push_str(&format!(" ({})", if f.kind == ControlKind::Submit { "submit" } else { "button" }));
        }
        if f.required {
            l.push_str(" required");
        }
        match f.kind {
            ControlKind::Checkbox | ControlKind::Radio => {
                if doc.attr(f.node, "checked").is_some() {
                    l.push_str(" checked");
                }
                if f.kind == ControlKind::Radio {
                    if let Some(v) = doc.attr(f.node, "value") {
                        l.push_str(&format!(" value=\"{}\"", esc(v)));
                    }
                }
            }
            ControlKind::Select => {
                let opts = page.select_options(f.node);
                let sel = opts.iter().find(|o| o.2).or(opts.first()).map(|o| o.0.clone()).unwrap_or_default();
                l.push_str(&format!(" value=\"{}\"", esc(&clip_text(&sel, 40))));
                let list: Vec<String> = opts.iter().take(12).map(|o| esc(&clip_text(&o.0, 30))).collect();
                l.push_str(&format!(" options=[{}{}]", list.join("|"), if opts.len() > 12 { format!("|… {} more", opts.len() - 12) } else { String::new() }));
            }
            ControlKind::Password => {
                let v = page.control_text(f.node);
                if !v.is_empty() {
                    l.push_str(&format!(" value=({} chars hidden)", v.chars().count()));
                }
            }
            ControlKind::Submit | ControlKind::Reset | ControlKind::Button | ControlKind::File => {}
            _ => {
                let v = page.control_text(f.node);
                if !v.is_empty() {
                    l.push_str(&format!(" value=\"{}\"", esc(&clip_text(&v, 60))));
                }
            }
        }
        lines.push(l);
    }
    lines
}

fn norm(s: &str) -> String {
    s.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ").trim_end_matches([':', '*', ' ']).to_string()
}

/// Find the field a key refers to: ref, name, id, label, placeholder.
pub fn match_field(page: &Page, fields: &[Field], key: &str, refs: &RefMap, frame: usize) -> Option<usize> {
    if let Some(crate::refs::Ref::Element(r)) = crate::refs::Ref::parse(key).filter(|_| key.starts_with('@')) {
        let e = refs.get(r)?;
        return fields.iter().position(|f| f.node == e.node && e.frame == frame);
    }
    let k = norm(key);
    let doc = &page.doc;
    let exact = |f: &Field| norm(&f.name) == k || doc.attr(f.node, "id").map(norm).as_deref() == Some(k.as_str()) || norm(&f.label) == k;
    if let Some(i) = fields.iter().position(|f| f.role != "button" && exact(f)) {
        return Some(i);
    }
    let ph = |f: &Field| doc.attr(f.node, "placeholder").map(norm).unwrap_or_default();
    if let Some(i) = fields.iter().position(|f| f.role != "button" && ph(f) == k) {
        return Some(i);
    }
    // Radio group by name handled by the caller; finally a substring match.
    fields.iter().position(|f| f.role != "button" && !k.is_empty() && (norm(&f.label).contains(&k) || ph(f).contains(&k)))
}

/// Set one field from a JSON-ish value. Returns a description of what was
/// done, or an error.
pub fn set_field(engine: &Engine, page: &mut Page, fields: &[Field], idx: usize, value: &serde_json::Value) -> Result<String, String> {
    let f = &fields[idx];
    let as_text = match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        v => v.to_string(),
    };
    match f.kind {
        ControlKind::Checkbox => {
            let want = match value {
                serde_json::Value::Bool(b) => *b,
                _ => matches!(as_text.trim().to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes" | "checked" | "sí" | "si"),
            };
            crate::actions::fill_control(engine, page, f.node, if want { "true" } else { "false" })?;
            Ok(format!("{} \"{}\" {}", f.role, esc(&f.label), if want { "checked" } else { "unchecked" }))
        }
        ControlKind::Radio => {
            // Pick the radio of the group whose label/value matches.
            let doc = &page.doc;
            let group: Vec<&Field> = fields.iter().filter(|g| g.kind == ControlKind::Radio && g.name == f.name).collect();
            let want = norm(&as_text);
            let pick = group
                .iter()
                .find(|g| norm(&g.label) == want || doc.attr(g.node, "value").map(norm).as_deref() == Some(want.as_str()))
                .or_else(|| group.iter().find(|g| norm(&g.label).contains(&want)))
                .copied()
                .or(if matches!(value, serde_json::Value::Bool(true)) { Some(f) } else { None })
                .ok_or_else(|| {
                    let opts: Vec<String> = group.iter().map(|g| g.label.clone()).collect();
                    format!("no radio option \"{as_text}\" in group {} (options: {})", f.name, opts.join(", "))
                })?;
            let node = pick.node;
            let label = pick.label.clone();
            crate::actions::fill_control(engine, page, node, "true")?;
            Ok(format!("radio \"{}\" checked", esc(&label)))
        }
        ControlKind::Select => {
            crate::actions::fill_control(engine, page, f.node, &as_text)?;
            Ok(format!("combobox \"{}\" = \"{}\"", esc(&f.label), esc(&as_text)))
        }
        ControlKind::File => Err(format!("file input \"{}\": file uploads are not supported by the agent interface", f.label)),
        ControlKind::Submit | ControlKind::Reset | ControlKind::Button => Err(format!("\"{}\" is a button; use submit or click", f.label)),
        _ => {
            crate::actions::fill_control(engine, page, f.node, &as_text)?;
            let shown = if f.kind == ControlKind::Password { format!("({} chars)", as_text.chars().count()) } else { format!("\"{}\"", esc(&clip_text(&as_text, 60))) };
            Ok(format!("textbox \"{}\" = {shown}", esc(&f.label)))
        }
    }
}

/// One failed constraint.
#[derive(Debug, Clone)]
pub struct Invalid {
    pub node: NodeId,
    pub label: String,
    pub reason: String,
}

fn valid_email(v: &str) -> bool {
    let Some((local, domain)) = v.split_once('@') else { return false };
    !local.is_empty() && !domain.is_empty() && !domain.starts_with('.') && !domain.ends_with('.') && !v.contains(char::is_whitespace) && !domain.contains('@')
}

/// HTML constraint validation of a form's fields (required, type,
/// pattern, length, range). `pattern` is checked with the page's own
/// JavaScript RegExp when scripting is available.
pub fn validate(engine: &Engine, page: &mut Page, form: NodeId) -> Vec<Invalid> {
    if page.doc.attr(form, "novalidate").is_some() {
        return Vec::new();
    }
    let fs = fields(page, form);
    let mut out = Vec::new();
    let mut radio_groups_ok: Vec<(String, bool)> = Vec::new();
    for f in &fs {
        let doc = &page.doc;
        if matches!(f.kind, ControlKind::Submit | ControlKind::Reset | ControlKind::Button) {
            continue;
        }
        if doc.attr(f.node, "disabled").is_some() || doc.attr(f.node, "readonly").is_some() {
            continue;
        }
        let required = doc.attr(f.node, "required").is_some();
        let mut reason: Option<String> = None;
        match f.kind {
            ControlKind::Checkbox => {
                if required && doc.attr(f.node, "checked").is_none() {
                    reason = Some("must be checked (required)".into());
                }
            }
            ControlKind::Radio => {
                let checked = doc.attr(f.node, "checked").is_some();
                match radio_groups_ok.iter_mut().find(|(n, _)| *n == f.name) {
                    Some(g) => g.1 |= checked,
                    None => radio_groups_ok.push((f.name.clone(), checked)),
                }
            }
            ControlKind::Select => {
                if required {
                    let opts = page.select_options(f.node);
                    let sel = opts.iter().find(|o| o.2).or(opts.first());
                    if sel.map(|o| o.1.is_empty()).unwrap_or(true) {
                        reason = Some("an option must be selected (required)".into());
                    }
                }
            }
            _ => {
                let v = page.control_text(f.node);
                let len = v.chars().count();
                if v.is_empty() {
                    if required {
                        reason = Some("required (empty)".into());
                    }
                } else {
                    let ty = f.ty.as_str();
                    let email_ok = if doc.attr(f.node, "multiple").is_some() { v.split(',').all(|e| valid_email(e.trim())) } else { valid_email(&v) };
                    if ty == "email" && !email_ok {
                        reason = Some(format!("\"{}\" is not a valid email address", esc(&clip_text(&v, 40))));
                    } else if ty == "url" && rabbithole_engine::net::Url::parse_absolute(&v).is_none() {
                        reason = Some("not a valid URL".into());
                    } else if ty == "number" || ty == "range" {
                        match v.trim().parse::<f64>() {
                            Err(_) => reason = Some("not a number".into()),
                            Ok(x) => {
                                if let Some(min) = doc.attr(f.node, "min").and_then(|m| m.parse::<f64>().ok()) {
                                    if x < min {
                                        reason = Some(format!("must be ≥ {min}"));
                                    }
                                }
                                if let Some(max) = doc.attr(f.node, "max").and_then(|m| m.parse::<f64>().ok()) {
                                    if x > max {
                                        reason = Some(format!("must be ≤ {max}"));
                                    }
                                }
                            }
                        }
                    }
                    if reason.is_none() {
                        if let Some(min) = doc.attr(f.node, "minlength").and_then(|m| m.parse::<usize>().ok()) {
                            if len < min {
                                reason = Some(format!("too short ({len} < minlength {min})"));
                            }
                        }
                        if let Some(max) = doc.attr(f.node, "maxlength").and_then(|m| m.parse::<usize>().ok()) {
                            if len > max {
                                reason = Some(format!("too long ({len} > maxlength {max})"));
                            }
                        }
                    }
                    if reason.is_none() {
                        if let Some(p) = doc.attr(f.node, "pattern").map(|s| s.to_string()) {
                            let js = format!(
                                "new RegExp({}).test({})",
                                serde_json::Value::String(format!("^(?:{p})$")),
                                serde_json::Value::String(v.clone())
                            );
                            let r = engine.eval_in_page(page, &js);
                            if r.trim() == "false" {
                                let title = page.doc.attr(f.node, "title").map(|t| format!(" ({})", esc(&clip_text(t, 60)))).unwrap_or_default();
                                reason = Some(format!("does not match the required pattern{title}"));
                            }
                        }
                    }
                }
            }
        }
        if let Some(r) = reason {
            out.push(Invalid { node: f.node, label: f.label.clone(), reason: r });
        }
    }
    let doc = &page.doc;
    for (name, ok) in radio_groups_ok {
        if ok {
            continue;
        }
        if let Some(f) = fs.iter().find(|f| f.kind == ControlKind::Radio && f.name == name && doc.attr(f.node, "required").is_some()) {
            out.push(Invalid { node: f.node, label: if f.name.is_empty() { f.label.clone() } else { f.name.clone() }, reason: "choose one option (required)".into() });
        }
    }
    out
}
