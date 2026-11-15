//! The short page summary returned by `open` (and by actions that navigate)
//! instead of the full snapshot: title/URL/status, landmarks, the heading
//! outline, forms with their fields, overlays and element counts, within a
//! few hundred tokens. Refs are the same ones later `snapshot` calls show
//! (they come from the same [`crate::refs::RefMap`]).

use std::collections::HashMap;

use rabbithole_engine::dom::NodeId;
use rabbithole_engine::Page;

use crate::ax::{clip_text, esc, Ax, Kind};
use crate::overlay::Overlay;
use crate::refs::RefMap;

/// Headings listed in the outline before "+N more".
const MAX_HEADINGS: usize = 12;
/// Forms described before "+N more".
const MAX_FORMS: usize = 3;
/// Fields listed per form.
const MAX_FIELDS: usize = 6;
/// Landmarks listed.
const MAX_LANDMARKS: usize = 10;

fn landmark_label(role: &str) -> &str {
    match role {
        "banner" => "header",
        "navigation" => "nav",
        "contentinfo" => "footer",
        "complementary" => "aside",
        "alertdialog" => "dialog",
        r => r,
    }
}

/// Counts of interactive elements in a subtree.
#[derive(Default)]
struct Counts {
    links: usize,
    buttons: usize,
    inputs: usize,
    other: usize,
}

impl Counts {
    fn of(a: &Ax) -> Counts {
        let mut c = Counts::default();
        a.visit(&mut |x, _| {
            if x.kind == Kind::Control {
                match x.role.as_str() {
                    "link" => c.links += 1,
                    "button" | "menuitem" | "tab" => c.buttons += 1,
                    "textbox" | "searchbox" | "combobox" | "spinbutton" | "slider" | "checkbox" | "radio" | "switch" | "listbox" => c.inputs += 1,
                    _ => c.other += 1,
                }
            }
        });
        c
    }

    fn render(&self) -> String {
        let mut parts = Vec::new();
        for (n, what) in [(self.links, "links"), (self.buttons, "buttons"), (self.inputs, "inputs"), (self.other, "other controls")] {
            if n > 0 {
                parts.push(format!("{n} {what}"));
            }
        }
        if parts.is_empty() {
            "no controls".into()
        } else {
            parts.join(", ")
        }
    }
}

/// Render the summary lines (without the `[page]` header).
pub fn render(page: &Page, root: &Ax, refs: &mut RefMap, overlays: &[Overlay]) -> Vec<String> {
    let mut out = Vec::new();
    let notes: HashMap<NodeId, String> = overlays.iter().map(|o| (o.node, o.note())).collect();

    // Overlays and dialogs first: they block everything else.
    let mut dialogs = Vec::new();
    root.visit(&mut |a, _| {
        if a.kind == Kind::Dialog && a.frame == 0 {
            dialogs.push(a);
        }
    });
    for d in dialogs.iter().take(3) {
        let mut s = format!("⚠ [{}", landmark_label(&d.role));
        if !d.name.is_empty() {
            s.push_str(&format!(" \"{}\"", esc(&clip_text(&d.name, 50))));
        }
        s.push(']');
        if let Some(r) = d.reference {
            s.push_str(&format!(" @e{r}"));
        }
        if let Some(n) = notes.get(&d.node) {
            s.push(' ');
            s.push_str(n);
        }
        // Scripted close controls are often ordinary paragraphs or spans.
        let mut buttons = Vec::new();
        d.visit(&mut |x, _| {
            if x.kind == Kind::Control && matches!(x.role.as_str(), "button" | "clickable") && !x.name.is_empty() {
                if let Some(r) = x.reference {
                    let role = if x.role == "clickable" { "clickable " } else { "" };
                    buttons.push(format!("{role}\"{}\" @e{r}", esc(&clip_text(&x.name, 30))));
                }
            }
        });
        if !buttons.is_empty() {
            let more = buttons.len().saturating_sub(4);
            buttons.truncate(4);
            s.push_str(&format!(": {}", buttons.join(", ")));
            if more > 0 {
                s.push_str(&format!(" +{more}"));
            }
        }
        out.push(s);
    }

    // Main/search remain discoverable even when deeply wrapped. Navigation
    // landmarks share one slot while retaining the first actionable ref.
    let mut marks: Vec<&Ax> = Vec::new();
    root.visit(&mut |a, depth| {
        if a.kind == Kind::Landmark && a.role != "form" && (depth <= 3 || matches!(a.role.as_str(), "main" | "search" | "navigation")) {
            marks.push(a);
        }
    });
    let nav_count = marks.iter().filter(|a| a.role == "navigation").count();
    let mut nav_seen = false;
    marks.retain(|a| {
        if a.role != "navigation" { return true; }
        let keep = !nav_seen;
        nav_seen = true;
        keep
    });
    if !marks.is_empty() {
        let mut chosen: Vec<usize> = marks.iter().enumerate().filter(|(_, m)| matches!(m.role.as_str(), "main" | "search")).map(|(i, _)| i).take(MAX_LANDMARKS).collect();
        for i in 0..marks.len() {
            if chosen.len() >= MAX_LANDMARKS { break; }
            if !chosen.contains(&i) { chosen.push(i); }
        }
        chosen.sort_unstable();
        let mut items: Vec<String> = Vec::new();
        for &i in &chosen {
            let m = marks[i];
            let mut label = landmark_label(&m.role).to_string();
            if m.role == "navigation" && nav_count > 1 { label.push_str(&format!(" ×{nav_count}")); }
            let mut item = format!("[{label}");
            if !m.name.is_empty() && !(m.role == "navigation" && nav_count > 1) {
                item.push_str(&format!(" \"{}\"", esc(&clip_text(&m.name, 30))));
            }
            item.push(']');
            if let Some(r) = m.reference { item.push_str(&format!(" @e{r}")); }
            items.push(item);
        }
        let mut line = format!("landmarks: {}", items.join("  "));
        if marks.len() > chosen.len() { line.push_str(&format!("  +{} more", marks.len() - chosen.len())); }
        out.push(line);
    }

    // Heading outline.
    let mut heads: Vec<&Ax> = Vec::new();
    root.visit(&mut |a, _| {
        if a.kind == Kind::Heading && !a.name.trim().is_empty() {
            heads.push(a);
        }
    });
    if !heads.is_empty() {
        out.push("outline:".into());
        let min = heads.iter().map(|h| h.level).min().unwrap_or(1);
        let second = heads.iter().map(|h| h.level).filter(|&level| level > min).min().unwrap_or(min);
        let mut chosen: Vec<usize> = heads.iter().enumerate().filter(|(_, h)| h.level <= second).map(|(i, _)| i).take(MAX_HEADINGS).collect();
        for i in 0..heads.len() {
            if chosen.len() >= MAX_HEADINGS { break; }
            if !chosen.contains(&i) { chosen.push(i); }
        }
        chosen.sort_unstable();
        for i in chosen {
            let h = heads[i];
            // A heading that is one link points at the link.
            let r = match h.children.as_slice() {
                [c] if c.role == "link" && c.reference.is_some() => c.reference.unwrap(),
                _ => refs.assign(h.frame, h.node, "heading", &h.name),
            };
            let indent = "  ".repeat(1 + h.level.saturating_sub(min) as usize);
            out.push(format!("{indent}h{} \"{}\" @e{r}", h.level, esc(&clip_text(&h.name, 70))));
        }
        if heads.len() > MAX_HEADINGS {
            out.push(format!("  … +{} more headings", heads.len() - MAX_HEADINGS));
        }
    }

    // A sparse outline can otherwise look like an empty page even when script
    // produced substantial prose. Keep articles with a useful outline compact.
    if heads.len() <= 2 {
        let chars = crate::read::main_text_chars(page);
        if chars >= 200 {
            out.push(format!("text: {chars} chars in main (read shows it)"));
        }
    }

    // Forms, with their fields.
    let forms = crate::forms::forms(page);
    let mut shown = 0;
    let mut eligible = 0;
    let mut preferences = 0;
    let mut form_lines = Vec::new();
    for f in &forms {
        let fields = crate::forms::fields(page, *f);
        if !fields.iter().any(|field| field.kind == rabbithole_engine::ControlKind::Submit || (field.role == "textbox" && !matches!(field.ty.as_str(), "color" | "range"))) {
            preferences += 1;
            continue;
        }
        eligible += 1;
        if shown >= MAX_FORMS { continue; }
        shown += 1;
        let desc = crate::forms::describe(page, 0, *f, refs);
        let head = desc.first().cloned().unwrap_or_default();
        let mut parts: Vec<String> = Vec::new();
        for l in desc.iter().skip(1).take(MAX_FIELDS) {
            parts.push(compact_field(l));
        }
        let mut line = format!("  {head}: {}", parts.join(", "));
        if fields.len() > MAX_FIELDS {
            line.push_str(&format!(", +{} more fields", fields.len() - MAX_FIELDS));
        }
        form_lines.push(line);
    }
    // Text inputs outside any form (script-driven search boxes).
    let mut loose = Vec::new();
    root.visit(&mut |a, _| {
        if a.kind == Kind::Control && a.frame == 0 && matches!(a.role.as_str(), "textbox" | "searchbox" | "combobox") && page.form_of(a.node).is_none() {
            if let Some(r) = a.reference {
                loose.push(format!("{} \"{}\" @e{r}", a.role, esc(&clip_text(&a.name, 40))));
            }
        }
    });
    if !form_lines.is_empty() || !loose.is_empty() || preferences > 0 {
        out.push("forms:".into());
        out.extend(form_lines);
        if eligible > shown {
            out.push(format!("  … +{} more forms (fill lists them all)", eligible - shown));
        }
        if preferences > 0 { out.push(format!("  … +{preferences} formularios sin campos de texto")); }
        if !loose.is_empty() {
            let more = loose.len().saturating_sub(3);
            loose.truncate(3);
            let mut l = format!("  inputs outside forms: {}", loose.join(", "));
            if more > 0 {
                l.push_str(&format!(" +{more}"));
            }
            out.push(l);
        }
    }

    // Counts.
    let mut main: Option<&Ax> = None;
    root.visit(&mut |a, _| {
        if main.is_none() && a.kind == Kind::Landmark && a.role == "main" {
            main = Some(a);
        }
    });
    let all = Counts::of(root);
    match main {
        Some(m) => {
            let r = m.reference.map(|r| format!(" @e{r}")).unwrap_or_default();
            out.push(format!("main{r}: {}; whole page: {}", Counts::of(m).render(), all.render()));
        }
        None => out.push(format!("page: {}", all.render())),
    }
    out
}

/// A form field line from [`crate::forms::describe`] made shorter:
/// `textbox "Search" @e4 name=q` → `textbox "Search" @e4`.
fn compact_field(l: &str) -> String {
    let l = l.trim();
    // Keep role, label and ref, plus the submit marker / type / required.
    let mut out = String::new();
    let rest = match l.find(" @e") {
        Some(i) => {
            let end = l[i + 3..].find(' ').map(|j| i + 3 + j).unwrap_or(l.len());
            out.push_str(&l[..end]);
            &l[end..]
        }
        None => {
            out.push_str(l);
            ""
        }
    };
    for tag in [" (submit)", " required"] {
        if rest.contains(tag) {
            out.push_str(tag);
        }
    }
    if let Some(i) = rest.find(" type=") {
        let t: String = rest[i..].split_whitespace().next().unwrap_or("").to_string();
        out.push(' ');
        out.push_str(&t);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::compact_field;

    #[test]
    fn compact_fields() {
        assert_eq!(compact_field("  textbox \"Search\" @e4 name=q required"), "textbox \"Search\" @e4 required");
        assert_eq!(compact_field("  button \"Go\" @e5 (submit)"), "button \"Go\" @e5 (submit)");
        assert_eq!(compact_field("  textbox \"Mail\" @e6 name=m type=email"), "textbox \"Mail\" @e6 type=email");
    }
}
