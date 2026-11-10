//! Rendering the semantic tree as compact text (see `docs/AI_INTERFACE.md`
//! for the grammar), fitting a token budget, and diffing two trees.

use std::collections::{HashMap, HashSet};

use rabbithole_engine::dom::NodeId;
use rabbithole_engine::gfx::Rect;

use crate::ax::{clip_text, esc, Ax, Kind};
use crate::refs::RefMap;

/// Rough token estimate (characters / 4).
pub fn tokens(s: &str) -> usize {
    s.chars().count().div_ceil(4)
}

/// How much detail to render.
#[derive(Debug, Clone)]
pub struct Detail {
    /// Items shown per list before "+N more".
    pub list_keep: usize,
    /// Characters per text item.
    pub text_max: usize,
    /// Landmark roles rendered as a one-line summary.
    pub collapse: HashSet<&'static str>,
    /// Show link targets.
    pub urls: bool,
}

impl Detail {
    pub fn level(l: usize, urls: bool) -> Detail {
        let (keep, text, collapse): (usize, usize, &[&'static str]) = match l {
            0 => (usize::MAX, 400, &[]),
            1 => (12, 240, &[]),
            2 => (8, 160, &["contentinfo"]),
            3 => (5, 120, &["contentinfo", "complementary"]),
            4 => (4, 90, &["contentinfo", "complementary", "navigation", "banner"]),
            5 => (3, 70, &["contentinfo", "complementary", "navigation", "banner", "search", "region", "form"]),
            _ => (2, 50, &["contentinfo", "complementary", "navigation", "banner", "search", "region", "form"]),
        };
        Detail { list_keep: keep, text_max: text, collapse: collapse.iter().copied().collect(), urls }
    }
}

/// Context needed to annotate the output.
pub struct RenderCtx<'a> {
    pub refs: &'a mut RefMap,
    /// Viewport rect of frame 0 (document coordinates).
    pub viewport: Rect,
    /// Overlays that cover content: node → note.
    pub overlay_notes: &'a HashMap<NodeId, String>,
}

/// A rendered line before joining.
#[derive(Debug, Clone)]
struct Piece {
    depth: usize,
    text: String,
    rect: Option<Rect>,
    joinable: bool,
    /// Ends a visual block (nothing joins across it).
    brk: bool,
}

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

/// One item rendered on its own (no indentation): used for lines and diffs.
pub fn leaf_text(a: &Ax, text_max: usize, urls: bool) -> String {
    let mut s = String::new();
    match a.kind {
        Kind::Text => {
            s.push_str("text \"");
            s.push_str(&esc(&clip_text(&a.name, text_max)));
            s.push('"');
        }
        Kind::Heading => {
            s.push_str(&format!("heading({}) \"{}\"", a.level, esc(&a.name)));
            // A heading whose content is one link: point at the link.
            if a.children.len() == 1 && a.children[0].role == "link" {
                if let Some(r) = a.children[0].reference {
                    s.push_str(&format!(" @e{r} → link"));
                    if urls {
                        if let Some(h) = &a.children[0].href {
                            s.push_str(&format!(" {}", esc(h)));
                        }
                    }
                }
            }
        }
        Kind::Image => {
            s.push_str(&format!("img \"{}\"", esc(&clip_text(&a.name, 80))));
        }
        _ => {
            s.push_str(&a.role);
            if a.level > 0 && a.role == "link" {
                s.push_str(&format!("(h{})", a.level));
            }
            if !a.name.is_empty() {
                s.push_str(&format!(" \"{}\"", esc(&a.name)));
            }
            if let Some(r) = a.reference {
                s.push_str(&format!(" @e{r}"));
            }
            for st in &a.states {
                s.push(' ');
                s.push_str(st);
            }
            if urls {
                if let Some(h) = &a.href {
                    s.push_str(&format!(" → {}", esc(&clip_text(h, 120))));
                }
            }
        }
    }
    if let Some(h) = a.hidden {
        s.push_str(&format!(" (hidden: {h})"));
    }
    s
}

/// Summary of a collapsed container.
fn summary(a: &Ax) -> String {
    let (mut links, mut controls, mut headings, mut texts) = (0, 0, 0, 0);
    a.visit(&mut |x, _| match x.kind {
        Kind::Control if x.role == "link" => links += 1,
        Kind::Control => controls += 1,
        Kind::Heading => headings += 1,
        Kind::Text => texts += 1,
        _ => {}
    });
    let mut parts = Vec::new();
    if links > 0 {
        parts.push(format!("{links} links"));
    }
    if controls > 0 {
        parts.push(format!("{controls} controls"));
    }
    if headings > 0 {
        parts.push(format!("{headings} headings"));
    }
    if texts > 0 {
        parts.push(format!("{texts} texts"));
    }
    if parts.is_empty() {
        "empty".into()
    } else {
        parts.join(", ")
    }
}

struct Renderer<'a, 'b> {
    ctx: &'a mut RenderCtx<'b>,
    detail: &'a Detail,
    out: Vec<Piece>,
    /// Depth of `main` landmarks/dialogs we are inside (never collapsed).
    in_main: usize,
}

impl Renderer<'_, '_> {
    fn push(&mut self, depth: usize, text: String, rect: Option<Rect>, joinable: bool) {
        self.out.push(Piece { depth, text, rect, joinable, brk: false });
    }

    fn brk(&mut self) {
        if let Some(l) = self.out.last_mut() {
            l.brk = true;
        }
    }

    fn container_header(&mut self, a: &Ax, depth: usize) {
        let mut s = format!("[{}", landmark_label(&a.role));
        if !a.name.is_empty() {
            s.push_str(&format!(" \"{}\"", esc(&a.name)));
        }
        s.push(']');
        if let Some(r) = a.reference {
            s.push_str(&format!(" @e{r}"));
        }
        for st in &a.states {
            s.push(' ');
            s.push_str(st);
        }
        if a.kind == Kind::Frame {
            if let Some(h) = &a.href {
                s.push_str(&format!(" src={}", esc(&clip_text(h, 100))));
            }
        }
        if let Some(note) = self.ctx.overlay_notes.get(&a.node).filter(|_| a.frame == 0) {
            s.push_str("  ⚠ ");
            s.push_str(note);
        }
        self.brk();
        self.push(depth, s, None, false);
        self.brk();
    }

    fn node(&mut self, a: &Ax, depth: usize) {
        match a.kind {
            Kind::Break => self.brk(),
            Kind::Text | Kind::Image => {
                let t = leaf_text(a, self.detail.text_max, self.detail.urls);
                self.push(depth, t, a.rect, true);
            }
            Kind::Heading => {
                self.brk();
                let t = leaf_text(a, self.detail.text_max, self.detail.urls);
                self.push(depth, t, a.rect, false);
                // Controls inside the heading other than the title link.
                if !(a.children.len() == 1 && a.children[0].role == "link") {
                    for c in &a.children {
                        let t = leaf_text(c, self.detail.text_max, self.detail.urls);
                        self.push(depth + 1, t, c.rect, true);
                    }
                }
                self.brk();
            }
            Kind::Control => {
                let t = leaf_text(a, self.detail.text_max, self.detail.urls);
                self.push(depth, t, a.rect, a.children.is_empty());
                if !a.children.is_empty() {
                    self.brk();
                    self.children(&a.children, depth + 1);
                    self.brk();
                }
            }
            Kind::Landmark | Kind::Form | Kind::Dialog | Kind::Frame | Kind::Group => {
                let collapse = (a.kind == Kind::Landmark || a.kind == Kind::Form) && self.detail.collapse.contains(a.role.as_str()) && (self.in_main == 0 || a.role != "region");
                if collapse {
                    let r = match a.reference {
                        Some(r) => r,
                        None => self.ctx.refs.assign(a.frame, a.node, &a.role, &a.name),
                    };
                    self.brk();
                    let mut s = format!("[{}", landmark_label(&a.role));
                    if !a.name.is_empty() {
                        s.push_str(&format!(" \"{}\"", esc(&a.name)));
                    }
                    s.push_str(&format!("] @e{r} collapsed ({})", summary(a)));
                    self.push(depth, s, None, false);
                    self.brk();
                    return;
                }
                if a.kind == Kind::Group && a.name.is_empty() && a.reference.is_none() {
                    self.brk();
                    self.children(&a.children, depth);
                    self.brk();
                    return;
                }
                self.container_header(a, depth);
                let main = a.role == "main" || a.kind == Kind::Dialog;
                if main {
                    self.in_main += 1;
                }
                self.children(&a.children, depth + 1);
                if main {
                    self.in_main -= 1;
                }
                self.brk();
            }
            Kind::List => {
                self.brk();
                let items: Vec<&Ax> = a.children.iter().filter(|c| c.kind != Kind::Break).collect();
                let keep = self.detail.list_keep;
                if items.len() > keep.saturating_add(1) {
                    for it in items.iter().take(keep) {
                        self.node(it, depth);
                        self.brk();
                    }
                    let rest = &items[keep..];
                    let below = rest
                        .iter()
                        .filter(|it| it.frame == 0 && it.rect.map(|r| r.y >= self.ctx.viewport.bottom()).unwrap_or(false))
                        .count();
                    let r = self.ctx.refs.assign(a.frame, a.node, "list", "");
                    let mut s = format!("… +{} more items", rest.len());
                    if below > 0 {
                        s.push_str(&format!(" ({below} below the viewport)"));
                    }
                    s.push_str(&format!(", expand @e{r}"));
                    self.push(depth, s, None, false);
                    self.brk();
                } else {
                    for it in items {
                        self.node(it, depth);
                        self.brk();
                    }
                }
            }
            Kind::Item => {
                self.brk();
                self.children(&a.children, depth);
                self.brk();
            }
        }
    }

    /// Children with heading-outline indentation.
    fn children(&mut self, kids: &[Ax], depth: usize) {
        let mut stack: Vec<u8> = Vec::new();
        for c in kids {
            if c.kind == Kind::Heading {
                while stack.last().is_some_and(|&l| l >= c.level) {
                    stack.pop();
                }
                self.node(c, depth + stack.len());
                stack.push(c.level);
            } else {
                // Landmarks and dialogs reset the outline.
                if matches!(c.kind, Kind::Landmark | Kind::Dialog) {
                    stack.clear();
                }
                self.node(c, depth + stack.len());
            }
        }
    }
}

/// Join pieces into lines: items on the same visual row share a line.
fn assemble(pieces: Vec<Piece>) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur: Option<(usize, String, Option<Rect>, bool)> = None;
    let mut prev_brk = true;
    for p in pieces {
        let can_join = match &cur {
            Some((d, text, rect, joinable)) => {
                *joinable
                    && p.joinable
                    && !prev_brk
                    && *d == p.depth
                    && text.chars().count() + p.text.chars().count() < 170
                    && match (rect, p.rect) {
                        (Some(a), Some(b)) => b.y < a.bottom() - 1.0 && b.bottom() > a.y + 1.0,
                        _ => false,
                    }
            }
            None => false,
        };
        if can_join {
            let (_, text, rect, _) = cur.as_mut().unwrap();
            text.push_str("  ");
            text.push_str(&p.text);
            if let (Some(a), Some(b)) = (rect.as_mut(), p.rect) {
                // Keep the row of the last item.
                *a = b;
            }
        } else {
            if let Some((d, text, _, _)) = cur.take() {
                lines.push(format!("{}{}", "  ".repeat(d), text));
            }
            cur = Some((p.depth, p.text, p.rect, p.joinable));
        }
        prev_brk = p.brk;
    }
    if let Some((d, text, _, _)) = cur.take() {
        lines.push(format!("{}{}", "  ".repeat(d), text));
    }
    lines
}

/// Order of top-level sections: dialogs/overlays, main, the rest.
fn top_sections(root: &Ax) -> Vec<&Ax> {
    let kids: Vec<&Ax> = root.children.iter().collect();
    let mut dialogs = Vec::new();
    let mut main = Vec::new();
    let mut rest = Vec::new();
    for c in kids {
        if c.kind == Kind::Dialog {
            dialogs.push(c);
        } else if c.kind == Kind::Landmark && c.role == "main" {
            main.push(c);
        } else {
            rest.push(c);
        }
    }
    dialogs.into_iter().chain(main).chain(rest).collect()
}

/// Render `root` (or the subtree) at a detail level.
pub fn render(root: &Ax, ctx: &mut RenderCtx, detail: &Detail, reorder: bool) -> Vec<String> {
    let mut r = Renderer { ctx, detail, out: Vec::new(), in_main: 0 };
    if reorder && root.kind == Kind::Group && root.role == "document" {
        let secs = top_sections(root);
        // Outline indentation only applies within a section; top level
        // flows like children.
        let owned: Vec<Ax> = secs.into_iter().cloned().collect();
        r.children(&owned, 0);
    } else if root.role == "document" {
        r.children(&root.children, 0);
    } else {
        r.node(root, 0);
    }
    assemble(r.out)
}

/// Render within a token budget, lowering detail as needed.
pub fn render_budget(root: &Ax, ctx: &mut RenderCtx, max_tokens: usize, urls: bool, full: bool) -> (Vec<String>, bool) {
    let max_chars = max_tokens.saturating_mul(4);
    let mut last = Vec::new();
    let start = if full { 0 } else { 1 };
    for level in start..=6 {
        let detail = Detail::level(level, urls);
        let lines = render(root, ctx, &detail, true);
        let len: usize = lines.iter().map(|l| l.chars().count() + 1).sum();
        if len <= max_chars || full && level == 0 && max_tokens >= 1_000_000 {
            return (lines, false);
        }
        last = lines;
    }
    // Hard cut.
    let mut out = Vec::new();
    let mut used = 0;
    for l in last {
        let n = l.chars().count() + 1;
        if used + n > max_chars {
            break;
        }
        used += n;
        out.push(l);
    }
    (out, true)
}

/// Flat (key, line) list of every item, for diffs.
pub fn flat_items(root: &Ax) -> Vec<((usize, NodeId, u16, bool), String)> {
    let mut out = Vec::new();
    root.visit(&mut |a, _| match a.kind {
        Kind::Text | Kind::Image | Kind::Control | Kind::Heading => out.push((a.key(), leaf_text(a, 160, false))),
        Kind::Landmark | Kind::Dialog | Kind::Form | Kind::Frame => {
            let mut s = format!("[{}", landmark_label(&a.role));
            if !a.name.is_empty() {
                s.push_str(&format!(" \"{}\"", esc(&a.name)));
            }
            s.push(']');
            if let Some(r) = a.reference {
                s.push_str(&format!(" @e{r}"));
            }
            out.push((a.key(), s));
        }
        _ => {}
    });
    out
}

/// A diff between two flat item lists.
#[derive(Debug, Default)]
pub struct Diff {
    pub lines: Vec<String>,
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
    pub total_after: usize,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.added + self.removed + self.changed == 0
    }
}

/// `~ changed`, `+ added`, `- removed` lines (at most `max_lines`).
pub fn diff(before: &[((usize, NodeId, u16, bool), String)], after: &[((usize, NodeId, u16, bool), String)], max_lines: usize) -> Diff {
    let b: HashMap<_, &String> = before.iter().map(|(k, v)| (*k, v)).collect();
    let a_keys: HashSet<_> = after.iter().map(|(k, _)| *k).collect();
    // Text identical before and after under another key is not a change
    // (re-rendered but same content).
    let before_texts: HashSet<&String> = before.iter().map(|(_, v)| v).collect();
    let after_texts: HashSet<&String> = after.iter().map(|(_, v)| v).collect();
    let mut d = Diff { total_after: after.len(), ..Default::default() };
    let mut lines = Vec::new();
    for (k, v) in after {
        match b.get(k) {
            Some(old) if *old == v => {}
            Some(old) => {
                d.changed += 1;
                lines.push(format!("~ {old} ⇒ {v}"));
            }
            None => {
                if before_texts.contains(v) {
                    continue;
                }
                d.added += 1;
                lines.push(format!("+ {v}"));
            }
        }
    }
    for (k, v) in before {
        if !a_keys.contains(k) && !after_texts.contains(v) {
            d.removed += 1;
            lines.push(format!("- {v}"));
        }
    }
    if lines.len() > max_lines {
        let extra = lines.len() - max_lines;
        lines.truncate(max_lines);
        lines.push(format!("… and {extra} more changes"));
    }
    d.lines = lines;
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_lines() {
        let k = |n| (0usize, n as NodeId, 0u16, false);
        let before = vec![(k(1), "button \"votar\" @e11".to_string()), (k(2), "text \"342 votos\"".to_string()), (k(3), "text \"old\"".to_string())];
        let after = vec![(k(1), "button \"votado\" @e11 disabled".to_string()), (k(2), "text \"343 votos\"".to_string()), (k(4), "text \"new\"".to_string())];
        let d = diff(&before, &after, 10);
        assert_eq!((d.added, d.removed, d.changed), (1, 1, 2));
        assert!(d.lines[0].starts_with("~ button \"votar\" @e11 ⇒ button \"votado\""));
        assert!(d.lines.iter().any(|l| l == "+ text \"new\""));
        assert!(d.lines.iter().any(|l| l == "- text \"old\""));
    }

    #[test]
    fn joins_same_row() {
        let p = |t: &str, y: f32, x: f32| Piece { depth: 1, text: t.into(), rect: Some(Rect::new(x, y, 40.0, 16.0)), joinable: true, brk: false };
        let lines = assemble(vec![p("link \"a\" @e1", 0.0, 0.0), p("link \"b\" @e2", 0.0, 50.0), p("text \"c\"", 40.0, 0.0)]);
        assert_eq!(lines, vec!["  link \"a\" @e1  link \"b\" @e2", "  text \"c\""]);
    }
}
