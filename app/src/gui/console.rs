//! Devtools-lite, drawn natively below the page: the JavaScript console
//! (log + an input line evaluated in the page), the Elements view (DOM
//! tree + computed style of the selected node) and the Network view
//! (requests with status, type, size and time).

use rabbithole_engine::gfx::Rect;
use std::collections::BTreeSet;
use std::ops::Range;
use winit::event::KeyEvent;
use winit::keyboard::{Key, NamedKey};

use super::chrome::{baseline_in, draw_centered, draw_field, ellipsize, pal, Chrome, FieldView, UI_FONT};
use super::content::ToContent;
use super::pages::human_size;
use super::{App, Panel, UiFocus};

/// Row height of the devtools lists (logical px).
const ROW_H: f32 = 18.0;

fn console_log_area(chrome: &Chrome) -> Rect {
    let body = chrome.devtools_body();
    let top = chrome.console_toolbar().bottom() + 2.0 * chrome.s;
    Rect::new(body.x, top, body.w, (chrome.console_input().y - top - 4.0 * chrome.s).max(0.0))
}

/// Borrow the stored text; only the visible rows need owned display labels.
struct ConsoleRow<'a> {
    id: u64,
    level: &'a str,
    text: &'a str,
    header: bool,
    expandable: bool,
    expanded: bool,
    more: usize,
}

fn console_rows<'a>(log: &'a crate::scripting::ConsoleLog, expanded: &BTreeSet<u64>, width: f32, measure: impl Fn(&str) -> f32) -> Vec<ConsoleRow<'a>> {
    let mut rows = Vec::new();
    for (id, (level, text)) in log.entries() {
        // split preserves empty messages and trailing blank lines. CRLF is
        // displayed as a line break; clipboard text remains byte-for-byte raw.
        let more = text.split('\n').count().saturating_sub(1);
        let first = text.split('\n').next().unwrap_or("");
        let first = first.strip_suffix('\r').unwrap_or(first);
        let expandable = more > 0 || measure(first) > width;
        let is_expanded = expandable && expanded.contains(&id);
        if !is_expanded {
            rows.push(ConsoleRow { id, level, text: first, header: true, expandable, expanded: false, more });
            continue;
        }
        let mut header = true;
        for line in text.split('\n') {
            let line = line.strip_suffix('\r').unwrap_or(line);
            for part in wrap_console_line(line, width, &measure) {
                rows.push(ConsoleRow { id, level, text: part, header, expandable, expanded: true, more: 0 });
                header = false;
            }
        }
    }
    rows
}

/// Wrap on UTF-8 character boundaries using the same font measurements as
/// painting. The slices retain every character, including indentation.
fn wrap_console_line<'a>(line: &'a str, width: f32, measure: &impl Fn(&str) -> f32) -> Vec<&'a str> {
    if line.is_empty() || measure(line) <= width {
        return vec![line];
    }
    let boundaries: Vec<usize> = line.char_indices().map(|(i, _)| i).chain(std::iter::once(line.len())).collect();
    let mut parts = Vec::new();
    let mut start = 0;
    while start < boundaries.len() - 1 {
        // Bracket the next row locally. Searching the entire remaining text
        // for every row would repeatedly shape huge prefixes of long logs.
        let (mut lo, mut hi) = (start, start + 1);
        while hi < boundaries.len() - 1 && measure(&line[boundaries[start]..boundaries[hi]]) <= width {
            lo = hi;
            hi = (start + (hi - start).saturating_mul(2)).min(boundaries.len() - 1);
        }
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if measure(&line[boundaries[start]..boundaries[mid]]) <= width {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let end = lo.max(start + 1);
        parts.push(&line[boundaries[start]..boundaries[end]]);
        start = end;
    }
    parts
}

fn console_window(total: usize, visible: usize, scroll: usize) -> Range<usize> {
    let end = total - scroll.min(total.saturating_sub(visible));
    end.saturating_sub(visible)..end
}

/// Short resource type for the Network view.
pub fn resource_type(ev: &rabbithole_engine::net::NetEvent) -> &'static str {
    use rabbithole_engine::net::Destination as D;
    let ct = ev.content_type.to_ascii_lowercase();
    match ev.dest {
        D::Document => "document",
        D::Style => "stylesheet",
        D::Script => "script",
        D::Font => "font",
        D::Image => "image",
        D::Empty => {
            if ct.contains("json") {
                "fetch/json"
            } else {
                "fetch"
            }
        }
    }
}

impl App {
    fn console_copy_all_enabled(&self) -> bool {
        self.tab().is_some_and(|tab| !tab.console.is_empty())
    }

    fn elements_copy_all_enabled(&self) -> bool {
        self.tab().is_some_and(|tab| tab.content.is_some())
    }

    /// This reply belongs to the explicit Elements action, never to a pending
    /// "search selection" request. Keep clipboard errors on the central path.
    pub(crate) fn copy_elements_result(&mut self, html: Option<String>) {
        match html {
            Some(html) => self.clipboard_set(&html),
            None => self.set_status("No document to copy"),
        }
    }

    fn console_visible_rows(&self) -> usize {
        (console_log_area(&self.chrome()).h / (ROW_H * self.ui_scale())).floor().max(0.0) as usize
    }

    fn console_rows(&self) -> Vec<ConsoleRow<'_>> {
        let chrome = self.chrome();
        let width = (console_log_area(&chrome).w - 30.0 * chrome.s).max(0.0);
        self.tab().map(|tab| console_rows(&tab.console, &tab.console_expanded, width, |text| self.engine.fonts.measure(self.mono_font, 12.0 * chrome.s, text))).unwrap_or_default()
    }

    fn set_console_expanded(&mut self, id: u64, expanded: bool) {
        let visible = self.console_visible_rows();
        let total = self.console_rows().len();
        let start = console_window(total, visible, self.tab().map(|t| t.console_scroll).unwrap_or(0)).start;
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        if expanded {
            tab.console_expanded.insert(id);
        } else {
            tab.console_expanded.remove(&id);
        }
        // Keep the clicked header in place rather than jumping to the bottom
        // of a newly expanded stack trace.
        let total = self.console_rows().len();
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.console_scroll = total.saturating_sub(visible).saturating_sub(start);
        }
    }

    fn devtools_rows(&self) -> usize {
        let body = self.chrome().devtools_body();
        let reserve = match self.panel { Panel::Console => 34.0, Panel::Elements => 28.0, Panel::Network => 0.0 };
        (((body.h - reserve * self.ui_scale()) / (ROW_H * self.ui_scale())).floor().max(1.0)) as usize
    }

    /// Paint the devtools pane (when open).
    pub(crate) fn draw_devtools(&mut self) {
        let chrome = self.chrome();
        if !chrome.devtools_open {
            return;
        }
        let p = *pal();
        let pane = chrome.devtools_pane();
        let s = chrome.s;
        self.canvas.fill_rect(pane, p.panel_bg);
        self.canvas.fill_rect(Rect::new(0.0, pane.y, pane.w, 1.0), p.separator);
        let fonts = self.engine.fonts.clone();
        let header = chrome.devtools_header();
        self.canvas.fill_rect(header, p.chrome_bg);
        self.canvas.fill_rect(Rect::new(header.x, header.bottom() - 1.0, header.w, 1.0), p.separator);
        for (i, panel) in Panel::ALL.iter().enumerate() {
            let r = chrome.devtools_tab(i);
            let active = *panel == self.panel;
            if active {
                self.canvas.fill_rect(Rect::new(r.x, r.bottom() - 2.0 * s, r.w, 2.0 * s), p.accent);
            }
            let label = match panel {
                Panel::Console => {
                    let errors = self.tab().map(|t| t.console.error_count()).unwrap_or(0);
                    if errors > 0 {
                        format!("Console ({errors})")
                    } else {
                        "Console".to_string()
                    }
                }
                Panel::Network => format!("Network ({})", self.tab().map(|t| t.inspector.network.len()).unwrap_or(0)),
                Panel::Elements => panel.label().to_string(),
            };
            draw_centered(&mut self.canvas, &fonts, if active { self.ui_font_bold } else { self.ui_font }, 12.0 * s, &label, r, if active { p.text } else { p.text_muted });
        }
        draw_centered(&mut self.canvas, &fonts, self.ui_font, 14.0 * s, "×", chrome.devtools_close(), p.text_muted);
        match self.panel {
            Panel::Console => self.draw_console_panel(),
            Panel::Elements => self.draw_elements_panel(),
            Panel::Network => self.draw_network_panel(),
        }
    }

    fn draw_console_panel(&mut self) {
        let chrome = self.chrome();
        let p = *pal();
        let s = chrome.s;
        let fonts = self.engine.fonts.clone();
        let size = 12.0 * s;
        let line_h = ROW_H * s;
        let input = chrome.console_input();
        let toolbar = chrome.console_toolbar();
        let copy = chrome.console_copy_all();
        let enabled = self.console_copy_all_enabled();
        let hovered = copy.contains(self.cursor.x as f32, self.cursor.y as f32);
        self.canvas.fill_rect(toolbar, p.chrome_bg);
        self.canvas.fill_rounded_rect(copy, 4.0 * s, if enabled && hovered { p.hover_bg } else { p.field_bg });
        self.canvas.stroke_rect(copy, [1.0; 4], [p.field_border; 4], [0; 4], 4.0 * s);
        draw_centered(&mut self.canvas, &fonts, self.ui_font, size, "Copy all", copy, if enabled { p.text } else { p.text_disabled });
        let count = self.tab().map(|tab| tab.console.len()).unwrap_or(0);
        let count = if count == 1 { "1 message".to_string() } else { format!("{count} messages") };
        let baseline = baseline_in(&fonts, self.ui_font, size, toolbar);
        self.canvas.draw_text(&fonts, self.ui_font, size, &count, toolbar.x + 8.0 * s, baseline, p.text_muted);
        let log_area = console_log_area(&chrome);
        let rows = (log_area.h / line_h).floor().max(0.0) as usize;
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.prune_console_state();
        }
        let (total, scroll, lines) = {
            let layout = self.console_rows();
            let scroll = self.tab().map(|t| t.console_scroll).unwrap_or(0).min(layout.len().saturating_sub(rows));
            let lines: Vec<_> = layout[console_window(layout.len(), rows, scroll)].iter().map(|row| {
                let summary = if row.more > 0 && !row.expanded { format!("{}  (+{} lines)", row.text, row.more) } else { row.text.to_string() };
                let label = if row.expanded { summary } else { ellipsize(&fonts, self.mono_font, size, &summary, log_area.w - 30.0 * s) };
                (row.id, row.level.to_string(), label, row.header, row.expandable, row.expanded)
            }).collect();
            (layout.len(), scroll, lines)
        };
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.console_scroll = scroll;
        }
        let selected = self.tab().and_then(|t| t.console_selected);
        self.canvas.push_clip(log_area);
        let mut y = log_area.y;
        if total == 0 {
            let baseline = baseline_in(&fonts, self.ui_font, size, Rect::new(log_area.x, y, log_area.w, line_h));
            self.canvas.draw_text(&fonts, self.ui_font, size, "No console messages. Type JavaScript below and press Enter (Ctrl+L clears).", log_area.x + 8.0 * s, baseline, p.text_muted);
        }
        for (id, level, label, header, expandable, expanded) in &lines {
            let row = Rect::new(log_area.x, y, log_area.w, line_h);
            let (bg, fg) = match level.as_str() {
                "error" => (Some(p.error_bg), p.error_fg),
                "warn" => (Some(p.warn_bg), p.warn_fg),
                "input" => (None, p.input_fg),
                "result" => (None, p.result_fg),
                _ => (None, p.text),
            };
            if selected == Some(*id) {
                self.canvas.fill_rect(row, p.selection);
            } else if let Some(bg) = bg {
                self.canvas.fill_rect(row, bg);
            }
            let baseline = baseline_in(&fonts, self.mono_font, size, row);
            if *header && *expandable {
                self.canvas.draw_text(&fonts, self.mono_font, size, if *expanded { "▾" } else { "▸" }, log_area.x + 6.0 * s, baseline, fg);
            }
            self.canvas.draw_text(&fonts, self.mono_font, size, label, log_area.x + 22.0 * s, baseline, fg);
            y += line_h;
        }
        self.canvas.pop_clip();
        // Input line.
        let focused = self.ui_focus == UiFocus::Console && selected.is_none();
        let (text, caret, sel) = self.tab().map(|t| (t.console_input.text.clone(), t.console_input.caret, t.console_input.selection())).unwrap_or_default();
        let view = FieldView::new(input, &text, caret, if focused { sel } else { None }, focused, "Evaluate JavaScript in this page…");
        draw_field(&mut self.canvas, &fonts, self.ui_font, s, &view);
    }

    /// Split of the Elements body: (tree, styles).
    pub(crate) fn elements_split(&self) -> (Rect, Rect) {
        let chrome = self.chrome();
        let body = chrome.devtools_body();
        let toolbar = chrome.elements_toolbar();
        let body = Rect::new(body.x, toolbar.bottom(), body.w, (body.bottom() - toolbar.bottom()).max(0.0));
        let tree_w = (body.w * 0.58).round();
        (Rect::new(body.x, body.y, tree_w, body.h), Rect::new(body.x + tree_w, body.y, body.w - tree_w, body.h))
    }

    fn draw_elements_panel(&mut self) {
        let chrome = self.chrome();
        let p = *pal();
        let s = chrome.s;
        let fonts = self.engine.fonts.clone();
        let size = 12.0 * s;
        let line_h = ROW_H * s;
        let toolbar = chrome.elements_toolbar();
        let copy = chrome.elements_copy_all();
        let enabled = self.elements_copy_all_enabled();
        let hovered = copy.contains(self.cursor.x as f32, self.cursor.y as f32);
        self.canvas.fill_rect(toolbar, p.chrome_bg);
        self.canvas.fill_rounded_rect(copy, 4.0 * s, if enabled && hovered { p.hover_bg } else { p.field_bg });
        self.canvas.stroke_rect(copy, [1.0; 4], [p.field_border; 4], [0; 4], 4.0 * s);
        draw_centered(&mut self.canvas, &fonts, self.ui_font, size, "Copy all", copy, if enabled { p.text } else { p.text_disabled });
        let baseline = baseline_in(&fonts, self.ui_font, size, toolbar);
        self.canvas.draw_text(&fonts, self.ui_font, size, "DOM as HTML", toolbar.x + 8.0 * s, baseline, p.text_muted);
        let (tree, styles) = self.elements_split();
        self.canvas.fill_rect(Rect::new(styles.x, styles.y, 1.0, styles.h), p.separator);
        let Some(tab) = self.tabs.get(self.active) else { return };
        let insp = &tab.inspector;
        let rows = (tree.h / line_h).floor().max(1.0) as usize;
        self.canvas.push_clip(tree);
        if insp.dom.is_empty() {
            let baseline = baseline_in(&fonts, self.ui_font, size, Rect::new(tree.x, tree.y, tree.w, line_h));
            self.canvas.draw_text(&fonts, self.ui_font, size, "Loading the DOM… (right-click → Inspect selects an element)", tree.x + 8.0 * s, baseline, p.text_muted);
        }
        let mut draws = Vec::new();
        for (i, row) in insp.dom.iter().enumerate().skip(insp.scroll).take(rows) {
            let y = tree.y + (i - insp.scroll) as f32 * line_h;
            let r = Rect::new(tree.x, y, tree.w, line_h);
            draws.push((i, r, row.depth, row.label.clone(), row.is_text, insp.selected == Some(i)));
        }
        for (_, r, depth, label, is_text, selected) in draws {
            if selected {
                self.canvas.fill_rect(r, p.selection);
            }
            let x = r.x + 8.0 * s + depth as f32 * 12.0 * s;
            let baseline = baseline_in(&fonts, self.mono_font, size, r);
            let color = if is_text { p.text } else { p.input_fg };
            let label = ellipsize(&fonts, self.mono_font, size, &label, (r.right() - x - 4.0 * s).max(10.0));
            self.canvas.draw_text(&fonts, self.mono_font, size, &label, x, baseline, color);
        }
        self.canvas.pop_clip();
        // Styles of the selected node.
        let Some(tab) = self.tabs.get(self.active) else { return };
        let d = &tab.inspector.details;
        let mut lines: Vec<(String, String, bool)> = Vec::new();
        if d.node.is_none() {
            lines.push(("Select an element to see its computed style.".into(), String::new(), true));
        } else {
            for (k, v) in d.attrs.iter().take(8) {
                lines.push((format!("@{k}"), v.clone(), false));
            }
            for (k, v) in &d.styles {
                lines.push((k.clone(), v.clone(), false));
            }
        }
        self.canvas.push_clip(styles);
        let key_w = 128.0 * s;
        for (i, (k, v, muted)) in lines.iter().enumerate() {
            let y = styles.y + 2.0 * s + i as f32 * line_h;
            if y > styles.bottom() {
                break;
            }
            let r = Rect::new(styles.x + 8.0 * s, y, styles.w - 12.0 * s, line_h);
            let baseline = baseline_in(&fonts, self.mono_font, size, r);
            if *muted {
                self.canvas.draw_text(&fonts, self.ui_font, size, k, r.x, baseline, p.text_muted);
                continue;
            }
            let kl = ellipsize(&fonts, self.mono_font, size, k, key_w - 6.0 * s);
            self.canvas.draw_text(&fonts, self.mono_font, size, &kl, r.x, baseline, p.error_fg);
            let vl = ellipsize(&fonts, self.mono_font, size, v, (r.w - key_w).max(10.0));
            self.canvas.draw_text(&fonts, self.mono_font, size, &vl, r.x + key_w, baseline, p.text);
        }
        self.canvas.pop_clip();
    }

    /// Network view columns: x offsets (relative) and titles.
    fn network_columns(&self, w: f32) -> [(f32, &'static str); 5] {
        let s = self.ui_scale();
        let fixed = (60.0 + 90.0 + 80.0 + 70.0) * s;
        let name_w = (w - fixed).max(120.0 * s);
        [(0.0, "Name"), (name_w, "Status"), (name_w + 60.0 * s, "Type"), (name_w + 150.0 * s, "Size"), (name_w + 230.0 * s, "Time")]
    }

    fn draw_network_panel(&mut self) {
        let chrome = self.chrome();
        let p = *pal();
        let s = chrome.s;
        let fonts = self.engine.fonts.clone();
        let size = 12.0 * s;
        let line_h = ROW_H * s;
        let body = chrome.devtools_body();
        let cols = self.network_columns(body.w - 16.0 * s);
        let head = Rect::new(body.x, body.y, body.w, line_h);
        self.canvas.fill_rect(head, p.chrome_bg);
        let baseline = baseline_in(&fonts, self.ui_font_bold, size, head);
        for (x, title) in cols {
            self.canvas.draw_text(&fonts, self.ui_font_bold, size, title, body.x + 8.0 * s + x, baseline, p.text_muted);
        }
        let Some(tab) = self.tabs.get(self.active) else { return };
        let insp = &tab.inspector;
        let list = Rect::new(body.x, head.bottom(), body.w, body.bottom() - head.bottom());
        let rows = (list.h / line_h).floor().max(1.0) as usize;
        let total_bytes: u64 = insp.network.iter().filter_map(|e| e.size).sum();
        let mut out = Vec::new();
        for (i, ev) in insp.network.iter().enumerate().skip(insp.network_scroll).take(rows.saturating_sub(1)) {
            let y = list.y + (i - insp.network_scroll) as f32 * line_h;
            let name = {
                let path = ev.url.path.rsplit('/').find(|p| !p.is_empty()).unwrap_or("");
                let base = if path.is_empty() { ev.url.host.clone() } else { path.to_string() };
                match &ev.url.query {
                    Some(q) if !q.is_empty() => format!("{base}?{q}"),
                    _ => base,
                }
            };
            let status = match (ev.status, &ev.error) {
                (Some(s), _) => s.to_string(),
                (None, Some(e)) if e.starts_with("blocked") => "blocked".into(),
                _ => "failed".into(),
            };
            let failed = ev.status.map(|s| s >= 400).unwrap_or(true);
            let sz = ev.size.map(human_size).unwrap_or_else(|| "—".into());
            let time = format!("{} ms", ev.duration.as_millis());
            out.push((i, Rect::new(list.x, y, list.w, line_h), name, status, resource_type(ev), sz, time, failed, insp.network_selected == Some(i)));
        }
        let summary = format!("{} requests · {} transferred (announced sizes)", insp.network.len(), human_size(total_bytes));
        self.canvas.push_clip(list);
        for (i, r, name, status, ty, sz, time, failed, selected) in out {
            if selected {
                self.canvas.fill_rect(r, p.selection);
            } else if i % 2 == 1 {
                self.canvas.fill_rect(r, p.row_alt);
            }
            let b = baseline_in(&fonts, self.ui_font, size, r);
            let fg = if failed { p.error_fg } else { p.text };
            let x0 = r.x + 8.0 * s;
            let name = ellipsize(&fonts, self.ui_font, size, &name, cols[1].0 - 8.0 * s);
            self.canvas.draw_text(&fonts, self.ui_font, size, &name, x0, b, fg);
            self.canvas.draw_text(&fonts, self.ui_font, size, &status, x0 + cols[1].0, b, fg);
            self.canvas.draw_text(&fonts, self.ui_font, size, ty, x0 + cols[2].0, b, p.text_muted);
            self.canvas.draw_text(&fonts, self.ui_font, size, &sz, x0 + cols[3].0, b, p.text_muted);
            self.canvas.draw_text(&fonts, self.ui_font, size, &time, x0 + cols[4].0, b, p.text_muted);
        }
        let sr = Rect::new(list.x, list.bottom() - line_h, list.w, line_h);
        self.canvas.fill_rect(sr, p.chrome_bg);
        let b = baseline_in(&fonts, self.ui_font, size, sr);
        // The selected request's full URL (or the totals).
        let foot = insp.network_selected.and_then(|i| insp.network.get(i)).map(|e| format!("{} {}{}", e.method, e.url, e.error.as_ref().map(|x| format!(" — {x}")).unwrap_or_default())).unwrap_or(summary);
        let foot = ellipsize(&fonts, self.ui_font, size, &foot, sr.w - 16.0 * s);
        self.canvas.draw_text(&fonts, self.ui_font, size, &foot, sr.x + 8.0 * s, b, p.text_muted);
        self.canvas.pop_clip();
        let _ = UI_FONT;
    }

    /// A click in the devtools pane.
    pub(crate) fn devtools_click(&mut self, x: f32, y: f32) {
        let chrome = self.chrome();
        if chrome.devtools_close().contains(x, y) {
            self.toggle_devtools();
            return;
        }
        for (i, panel) in Panel::ALL.iter().enumerate() {
            if chrome.devtools_tab(i).contains(x, y) {
                self.show_panel(*panel);
                return;
            }
        }
        if self.panel == Panel::Console && chrome.console_copy_all().contains(x, y) {
            if self.console_copy_all_enabled() {
                if let Some(text) = self.tab().and_then(|tab| tab.console_copy_all_text()) {
                    self.clipboard_set(&text);
                }
            }
            // Copying the log does not move focus or change either selection.
            self.request_redraw();
            return;
        }
        if self.panel == Panel::Elements && chrome.elements_copy_all().contains(x, y) {
            if self.elements_copy_all_enabled() {
                self.send_active(ToContent::CopyElements);
            }
            // Do not change the inspector selection or an editor's focus.
            self.request_redraw();
            return;
        }
        let line_h = ROW_H * chrome.s;
        match self.panel {
            Panel::Console => {
                self.ui_focus = UiFocus::Console;
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.prune_console_state();
                }
                if chrome.console_input().contains(x, y) {
                    if let Some(tab) = self.tabs.get_mut(self.active) {
                        // Restore the input's existing caret and text selection.
                        tab.console_selected = None;
                    }
                } else {
                    let area = console_log_area(&chrome);
                    let hit = if area.contains(x, y) {
                        let layout = self.console_rows();
                        let visible = self.console_visible_rows();
                        let scroll = self.tab().map(|t| t.console_scroll).unwrap_or(0);
                        let window = console_window(layout.len(), visible, scroll);
                        let i = ((y - area.y) / line_h).floor() as usize;
                        layout[window].get(i).map(|row| (row.id, row.header && row.expandable, row.expanded))
                    } else {
                        None
                    };
                    if let Some((id, toggle, expanded)) = hit {
                        if let Some(tab) = self.tabs.get_mut(self.active) {
                            tab.console_selected = Some(id);
                        }
                        if toggle {
                            self.set_console_expanded(id, !expanded);
                        }
                    }
                }
            }
            Panel::Elements => {
                let (tree, _) = self.elements_split();
                if tree.contains(x, y) {
                    let Some(tab) = self.tabs.get_mut(self.active) else { return };
                    let i = tab.inspector.scroll + ((y - tree.y) / line_h).floor() as usize;
                    if let Some(row) = tab.inspector.dom.get(i) {
                        tab.inspector.selected = Some(i);
                        let node = row.node;
                        tab.send(ToContent::InspectNode(Some(node)));
                    }
                }
                self.ui_focus = UiFocus::Page;
            }
            Panel::Network => {
                let body = chrome.devtools_body();
                let list_y = body.y + line_h;
                if y >= list_y {
                    let Some(tab) = self.tabs.get_mut(self.active) else { return };
                    let i = tab.inspector.network_scroll + ((y - list_y) / line_h).floor() as usize;
                    if i < tab.inspector.network.len() {
                        tab.inspector.network_selected = Some(i);
                    }
                }
                self.ui_focus = UiFocus::Page;
            }
        }
        self.request_redraw();
    }

    /// Keep the selected Elements row on screen.
    pub(crate) fn ensure_inspector_selection_visible(&mut self) {
        let rows = self.devtools_rows();
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        if let Some(sel) = tab.inspector.selected {
            if sel < tab.inspector.scroll {
                tab.inspector.scroll = sel;
            } else if sel >= tab.inspector.scroll + rows {
                tab.inspector.scroll = sel + 1 - rows.min(sel + 1);
                tab.inspector.scroll = tab.inspector.scroll.saturating_add(rows / 2).min(sel);
            }
        }
    }

    /// Wheel over the devtools pane.
    pub(crate) fn scroll_devtools(&mut self, dy: f32) {
        let rows = self.devtools_rows();
        let step = if dy < 0.0 { -3i64 } else { 3 };
        let panel = self.panel;
        let console_max = if panel == Panel::Console { self.console_rows().len().saturating_sub(self.console_visible_rows()) } else { 0 };
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        match panel {
            Panel::Console => {
                tab.prune_console_state();
                if dy < 0.0 {
                    tab.console_scroll = tab.console_scroll.saturating_add(3).min(console_max);
                } else {
                    tab.console_scroll = tab.console_scroll.saturating_sub(3);
                }
            }
            Panel::Elements => {
                let max = tab.inspector.dom.len().saturating_sub(rows);
                tab.inspector.scroll = (tab.inspector.scroll as i64 + step).clamp(0, max as i64) as usize;
            }
            Panel::Network => {
                let max = tab.inspector.network.len().saturating_sub(rows.saturating_sub(2));
                tab.inspector.network_scroll = (tab.inspector.network_scroll as i64 + step).clamp(0, max as i64) as usize;
            }
        }
        self.request_redraw();
    }

    /// Arrow keys in the Elements view move the selection.
    pub(crate) fn elements_key(&mut self, down: bool) -> bool {
        if !(self.devtools_open && self.panel == Panel::Elements) {
            return false;
        }
        let Some(tab) = self.tabs.get_mut(self.active) else { return false };
        let n = tab.inspector.dom.len();
        if n == 0 {
            return false;
        }
        let cur = tab.inspector.selected.unwrap_or(0);
        let next = if down { (cur + 1).min(n - 1) } else { cur.saturating_sub(1) };
        tab.inspector.selected = Some(next);
        let node = tab.inspector.dom[next].node;
        tab.send(ToContent::InspectNode(Some(node)));
        self.ensure_inspector_selection_visible();
        self.request_redraw();
        true
    }

    /// Keys while either the console log or its input has focus.
    pub(crate) fn handle_console_key(&mut self, event: &KeyEvent, ctrl: bool, shift: bool) {
        self.console_key(&event.logical_key, event.text.as_deref(), ctrl, shift);
    }

    pub(crate) fn console_key(&mut self, key: &Key, text: Option<&str>, ctrl: bool, shift: bool) {
        let console_max = self.console_rows().len().saturating_sub(self.console_visible_rows());
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        tab.prune_console_state();
        if let Some(id) = tab.console_selected {
            let expanded = tab.console_expanded.contains(&id);
            let expansion = match key {
                Key::Named(NamedKey::Enter | NamedKey::Space) if !ctrl => Some(!expanded),
                Key::Named(NamedKey::ArrowRight) => Some(true),
                Key::Named(NamedKey::ArrowLeft) => Some(false),
                _ => None,
            };
            if let Some(expand) = expansion {
                self.set_console_expanded(id, expand);
                self.request_redraw();
                return;
            }
            // Only actual input editing returns to the field. In particular,
            // Control/Shift/Alt/Meta events arrive before shortcut characters
            // and must preserve the selected message for Ctrl+C.
            let edits_input = matches!(key, Key::Named(NamedKey::Backspace | NamedKey::Delete | NamedKey::Home | NamedKey::End | NamedKey::ArrowUp | NamedKey::Enter | NamedKey::Space))
                || (ctrl && matches!(key, Key::Character(c) if matches!(c.to_lowercase().as_str(), "a" | "v")))
                || (!ctrl && text.is_some_and(|text| !text.is_empty()));
            if edits_input {
                tab.console_selected = None;
            }
        }
        match key {
            Key::Named(NamedKey::Enter) => {
                self.console_eval();
                return;
            }
            Key::Named(NamedKey::Escape) => {
                self.toggle_devtools();
                return;
            }
            Key::Named(NamedKey::Backspace) => tab.console_input.backspace(ctrl),
            Key::Named(NamedKey::Delete) => tab.console_input.delete(ctrl),
            Key::Named(NamedKey::ArrowLeft) => tab.console_input.move_left(shift, ctrl),
            Key::Named(NamedKey::ArrowRight) => tab.console_input.move_right(shift, ctrl),
            Key::Named(NamedKey::Home) => tab.console_input.home(shift),
            Key::Named(NamedKey::End) => tab.console_input.end(shift),
            Key::Named(NamedKey::ArrowUp) => {
                // Recall the previous input line.
                let prev = tab.console.lines().rev().find(|(l, _)| l == "input").map(|(_, t)| t.trim_start_matches("> ").to_string());
                if let Some(p) = prev {
                    tab.console_input.set_text(&p);
                }
            }
            Key::Named(NamedKey::PageUp) => tab.console_scroll = tab.console_scroll.saturating_add(10).min(console_max),
            Key::Named(NamedKey::PageDown) => tab.console_scroll = tab.console_scroll.saturating_sub(10),
            Key::Named(NamedKey::Space) => tab.console_input.insert(" ", None),
            Key::Character(c) if ctrl => match c.to_lowercase().as_str() {
                "a" => tab.console_input.select_all(),
                "l" => tab.clear_console(),
                "c" => {
                    if let Some(t) = tab.console_copy_text().map(str::to_string) {
                        self.clipboard_set(&t);
                    }
                }
                "v" => {
                    if let Some(t) = self.clipboard_get() {
                        if let Some(tab) = self.tabs.get_mut(self.active) {
                            tab.console_input.insert(&t, None);
                        }
                    }
                }
                _ => {}
            },
            _ => {
                if let Some(text) = text {
                    if !ctrl {
                        tab.console_input.insert(text, None);
                    }
                }
            }
        }
        self.request_redraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::snapshots::{headless_app, lock, save_png};
    use super::super::input::TextBuffer;
    use super::super::tab::Tab;
    use crate::scripting::ConsoleLog;

    fn console_app(tag: &str) -> App {
        let mut app = headless_app(tag);
        app.tabs.push(Tab::new(1));
        app.devtools_open = true;
        app.panel = Panel::Console;
        app.settings.show_bookmarks_bar = false;
        app.clipboard_test = Some(Ok(Vec::new()));
        app
    }

    fn click_log_row(app: &mut App, row: usize) {
        let area = console_log_area(&app.chrome());
        app.devtools_click(area.x + 40.0, area.y + (row as f32 + 0.5) * ROW_H);
    }

    fn click_copy_all(app: &mut App) {
        let r = app.chrome().console_copy_all();
        app.devtools_click(r.x + r.w / 2.0, r.y + r.h / 2.0);
    }

    fn copied(app: &App) -> &[String] {
        app.clipboard_test.as_ref().unwrap().as_ref().unwrap()
    }

    fn console_state(app: &App) -> (Option<u64>, BTreeSet<u64>, usize, TextBuffer) {
        let tab = &app.tabs[app.active];
        (tab.console_selected, tab.console_expanded.clone(), tab.console_scroll, tab.console_input.clone())
    }

    #[test]
    fn console_control_press_then_c_copies_selected_message_without_losing_focus() {
        let _guard = lock();
        let mut app = console_app("console-control-copy");
        let original = "TypeError: café 🐇\r\n    at página.js:12:3\n\n";
        app.tabs[0].console.push("error", original);
        app.tabs[0].console_input.set_text("input e\u{301} 🐇");
        app.tabs[0].console_input.select_all();
        click_log_row(&mut app, 0);
        let before = console_state(&app);
        let focus = app.ui_focus;

        // handle_console_key forwards every pressed logical key to this App
        // handler. Control itself arrives before the shortcut's C event.
        for ctrl in [false, true] {
            app.console_key(&Key::Named(NamedKey::Control), None, ctrl, false);
            assert_eq!(console_state(&app), before, "Control must not focus the input");
            assert!(app.ui_focus == focus);
        }
        assert!(copied(&app).is_empty());
        app.console_key(&Key::Character("c".into()), None, true, false);
        assert_eq!(copied(&app), &[original.to_string()]);
        assert_eq!(console_state(&app), before);
        assert!(app.ui_focus == focus);

        for key in [NamedKey::Shift, NamedKey::Alt, NamedKey::Meta, NamedKey::Super, NamedKey::F1] {
            app.console_key(&Key::Named(key), None, false, false);
            assert_eq!(console_state(&app), before, "non-editing keys must preserve log selection");
            assert!(app.ui_focus == focus);
        }
        let input = app.chrome().console_input();
        app.devtools_click(input.x + 20.0, input.y + input.h / 2.0);
        let before_input = app.tabs[0].console_input.clone();
        app.console_key(&Key::Named(NamedKey::Control), None, true, false);
        app.console_key(&Key::Character("c".into()), None, true, false);
        assert_eq!(copied(&app).last().unwrap(), "input e\u{301} 🐇");
        assert_eq!(app.tabs[0].console_input, before_input);
    }

    #[test]
    fn console_enter_and_space_expand_without_ctrl_and_edit_the_input_with_ctrl() {
        let _guard = lock();
        let mut app = console_app("console-control-enter-space");
        app.tabs[0].console.push("error", "TypeError\n  trace 🐇");
        app.tabs[0].console_input.set_text("1 + 1");
        click_log_row(&mut app, 0);
        let id = app.tabs[0].console_selected.unwrap();
        let input = app.tabs[0].console_input.clone();
        assert!(app.tabs[0].console_expanded.contains(&id));
        app.console_key(&Key::Named(NamedKey::Enter), None, false, false);
        assert!(!app.tabs[0].console_expanded.contains(&id));
        assert_eq!(app.tabs[0].console_selected, Some(id));
        assert_eq!(app.tabs[0].console_input, input);
        assert_eq!(app.tabs[0].console.len(), 1);
        app.console_key(&Key::Named(NamedKey::Space), None, false, false);
        assert!(app.tabs[0].console_expanded.contains(&id));
        assert_eq!(app.tabs[0].console_selected, Some(id));
        assert_eq!(app.tabs[0].console_input, input);
        assert_eq!(app.tabs[0].console.len(), 1);

        app.console_key(&Key::Named(NamedKey::Enter), None, true, false);
        assert_eq!(app.tabs[0].console_selected, None);
        assert!(app.tabs[0].console_input.text.is_empty());
        assert_eq!(app.tabs[0].console.len(), 2);
        assert_eq!(app.tabs[0].console.lines().next_back().unwrap(), &("input".to_string(), "> 1 + 1".to_string()));

        app.tabs[0].clear_console();
        app.tabs[0].console.push("error", "TypeError\n  trace 🐇");
        app.tabs[0].console_input.set_text("leftRIGHT");
        app.tabs[0].console_input.set_caret(4, false);
        app.tabs[0].console_input.end(true);
        click_log_row(&mut app, 0);
        app.console_key(&Key::Named(NamedKey::Space), None, true, false);
        assert_eq!(app.tabs[0].console_selected, None);
        assert_eq!(app.tabs[0].console_input.text, "left ");
        assert_eq!(app.tabs[0].console_input.caret, 5);
        assert_eq!(app.tabs[0].console_input.anchor, None);
        assert_eq!(app.tabs[0].console.len(), 1);
        assert!(app.ui_focus == UiFocus::Console);
    }

    #[test]
    fn console_copy_all_click_includes_folded_offscreen_messages_and_only_current_tab() {
        let _guard = lock();
        let mut app = console_app("console-copy-all");
        let entries = [("log", "first e\u{301} 🐇\n"), ("error", "TypeError: 東京\r\n  at página.js:12:3\n\n"), ("warn", "warning\tmañana"), ("input", "> 1 + 1"), ("result", "2")];
        let mut payloads = Vec::new();
        for (level, text) in entries {
            app.tabs[0].console.push(level, text);
            payloads.push(text.to_string());
        }
        let error_id = app.tabs[0].console.entries().nth(1).unwrap().0;
        let visible = app.console_visible_rows();
        for i in 0..visible + 12 {
            let text = format!("later message {i} 🐇");
            app.tabs[0].console.push("log", text.clone());
            payloads.push(text);
        }
        let expected = payloads.join("\n");
        app.tabs[0].console_input.set_text("selected input 🐇");
        app.tabs[0].console_input.select_all();
        for _ in 0..20 {
            app.scroll_devtools(-1.0);
        }
        click_log_row(&mut app, 1);
        click_log_row(&mut app, 1);
        assert_eq!(app.tabs[0].console_selected, Some(error_id));
        assert!(!app.tabs[0].console_expanded.contains(&error_id));
        app.paint(1200, 800);
        save_png(&app.canvas, "devtools-console-copy-all.png");
        let button = app.chrome().console_copy_all();
        assert!(button.bottom() <= console_log_area(&app.chrome()).y);
        for _ in 0..20 {
            app.scroll_devtools(1.0);
        }
        let layout = app.console_rows();
        let window = console_window(layout.len(), visible, app.tabs[0].console_scroll);
        assert!(layout[window].iter().all(|row| row.id != error_id));
        drop(layout);
        assert_eq!(app.chrome().console_copy_all(), button, "copy stays above the scrolling log");
        let before = console_state(&app);
        let focus = app.ui_focus;
        click_copy_all(&mut app);
        assert_eq!(copied(&app)[0].as_bytes(), expected.as_bytes());
        assert_eq!(console_state(&app), before);
        assert!(app.ui_focus == focus);
        app.paint(1200, 800);
        save_png(&app.canvas, "devtools-console-copy-all-scrolled.png");

        // No selected entry is required, and clicking the button must retain
        // the input field's caret and selection instead of focusing the log.
        let input = app.chrome().console_input();
        app.devtools_click(input.x + 20.0, input.y + input.h / 2.0);
        assert_eq!(app.tabs[0].console_selected, None);
        let before = console_state(&app);
        click_copy_all(&mut app);
        assert_eq!(copied(&app)[1].as_bytes(), expected.as_bytes());
        assert_eq!(console_state(&app), before);
        assert!(app.ui_focus == UiFocus::Console);
        app.tabs.push(Tab::new(2));
        app.tabs[1].console.push("error", "OTHER TAB\n終わり\r\n");
        app.active = 1;
        click_copy_all(&mut app);
        assert_eq!(copied(&app)[2], "OTHER TAB\n終わり\r\n");
        app.active = 0;
        assert_eq!(console_state(&app), before);
    }

    #[test]
    fn console_copy_all_uses_retained_messages_and_is_disabled_when_empty() {
        let _guard = lock();
        let mut app = console_app("console-copy-all-empty");
        app.ui_focus = UiFocus::Console;
        app.tabs[0].console_input.set_text("keep input selection");
        app.tabs[0].console_input.select_all();
        let before = console_state(&app);
        assert!(!app.console_copy_all_enabled());
        app.paint(1200, 800);
        save_png(&app.canvas, "devtools-console-copy-all-empty.png");
        click_copy_all(&mut app);
        assert!(copied(&app).is_empty());
        assert!(app.status.is_none());
        assert_eq!(console_state(&app), before);
        assert!(app.ui_focus == UiFocus::Console);

        app.tabs[0].console.push("log", "");
        assert!(app.console_copy_all_enabled(), "an empty payload is still a retained message");
        click_copy_all(&mut app);
        assert_eq!(copied(&app), &[String::new()]);
        app.tabs[0].clear_console();
        let capacity = crate::scripting::CONSOLE_CAPACITY;
        let mut expected = Vec::new();
        for i in 0..capacity + 3 {
            let text = format!("message {i}: café 🐇\r\n\t");
            app.tabs[0].console.push(if i % 2 == 0 { "error" } else { "warn" }, text.clone());
            if i >= 3 {
                expected.push(text);
            }
        }
        let button = app.chrome().console_copy_all();
        app.devtools_click(button.x - 1.0, button.y + button.h / 2.0);
        assert_eq!(copied(&app).len(), 1, "outside the shared hit rectangle must not copy");
        click_copy_all(&mut app);
        assert_eq!(copied(&app)[1].as_bytes(), expected.join("\n").as_bytes());
        assert_eq!(app.tabs[0].console.len(), capacity);
        assert_eq!(console_state(&app), before);
    }

    #[test]
    fn console_copy_all_clipboard_failure_is_honest_and_preserves_selection() {
        let _guard = lock();
        let mut app = console_app("console-copy-all-failure");
        app.tabs[0].console.push("error", "TypeError\n  trace 🐇");
        app.tabs[0].console_input.set_text("preserve input");
        app.tabs[0].console_input.select_all();
        click_log_row(&mut app, 0);
        let before = console_state(&app);
        let focus = app.ui_focus;
        app.clipboard_test = Some(Err(()));
        click_copy_all(&mut app);
        assert_eq!(app.status.as_ref().unwrap().0, "Clipboard unavailable");
        assert_eq!(console_state(&app), before);
        assert!(app.ui_focus == focus);
        assert!(matches!(app.clipboard_test.as_ref(), Some(Err(()))));
    }

    #[test]
    fn console_multiline_clicks_render_copy_and_restore_input() {
        let _guard = lock();
        let mut app = console_app("console-multiline");
        let original = "TypeError: café 🐇\n    at cargar (página.js:12:3)\r\n\n終わり\n";
        app.tabs[0].console.push("error", original);
        app.tabs[0].console_input.set_text("expresión 🐇");
        app.tabs[0].console_input.select_all();
        app.paint(1200, 800);
        save_png(&app.canvas, "devtools-console-collapsed.png");
        assert_eq!(app.console_rows().len(), 1);
        let id = app.tabs[0].console.entries().next().unwrap().0;
        click_log_row(&mut app, 0);
        assert_eq!(app.tabs[0].console_selected, Some(id));
        assert!(app.tabs[0].console_expanded.contains(&id));
        assert_eq!(app.tabs[0].console_copy_text(), Some(original));
        let layout = app.console_rows();
        assert_eq!(layout.iter().map(|r| r.text).collect::<Vec<_>>(), vec!["TypeError: café 🐇", "    at cargar (página.js:12:3)", "", "終わり", ""]);
        drop(layout);
        app.paint(1200, 800);
        save_png(&app.canvas, "devtools-console-expanded.png");
        // Verify that the stack's second row contains painted glyphs, beyond
        // the selection background that all rows of the message share.
        let area = console_log_area(&app.chrome());
        let mut glyphs = 0;
        for y in (area.y + ROW_H) as usize..(area.y + ROW_H * 2.0) as usize {
            for x in 22..500 {
                if app.canvas.data[y * app.canvas.width as usize + x] != pal().selection.to_u32() {
                    glyphs += 1;
                }
            }
        }
        assert!(glyphs > 0, "expanded stack row was not painted");
        click_log_row(&mut app, 1);
        assert_eq!(app.tabs[0].console_selected, Some(id));
        assert!(app.tabs[0].console_expanded.contains(&id), "selecting a continuation must not collapse it");
        click_log_row(&mut app, 0);
        assert_eq!(app.console_rows().len(), 1);
        assert_eq!(app.tabs[0].console_copy_text(), Some(original), "collapsed copy must still include the full stack");

        let input = app.chrome().console_input();
        app.devtools_click(input.x + 20.0, input.y + input.h / 2.0);
        assert_eq!(app.tabs[0].console_selected, None);
        assert_eq!(app.tabs[0].console_copy_text(), Some("expresión 🐇"));
        click_log_row(&mut app, 0);
        app.console_key(&Key::Character("ñ".into()), Some("ñ"), false, false);
        assert_eq!(app.tabs[0].console_selected, None);
        assert_eq!(app.tabs[0].console_input.text, "ñ");
        app.tabs[0].console_input.select_all();
        click_log_row(&mut app, 0);
        app.handle_text_input("東京");
        assert_eq!(app.tabs[0].console_selected, None);
        assert_eq!(app.tabs[0].console_input.text, "東京");
    }

    #[test]
    fn console_scroll_and_clicks_follow_expanded_visual_rows() {
        let _guard = lock();
        let mut app = console_app("console-scroll");
        let visible = app.console_visible_rows();
        let stack = (0..visible + 12).map(|i| format!("    at frame_{i} (script.js:{i}:1)")).collect::<Vec<_>>().join("\n");
        app.tabs[0].console.push("error", stack.as_str());
        let stack_id = app.tabs[0].console.entries().next().unwrap().0;
        click_log_row(&mut app, 0);
        assert_eq!(app.console_rows().len(), visible + 12);
        assert_eq!(app.tabs[0].console_scroll, 12, "expanding keeps the header visible");
        app.scroll_devtools(1.0);
        assert_eq!(app.tabs[0].console_scroll, 9);
        click_log_row(&mut app, 0);
        assert_eq!(app.tabs[0].console_selected, Some(stack_id));
        assert!(app.tabs[0].console_expanded.contains(&stack_id));
        app.tabs[0].console.push("log", "newest 🐇");
        let newest_id = app.tabs[0].console.entries().next_back().unwrap().0;
        app.console_key(&Key::Named(NamedKey::PageDown), None, false, false);
        assert_eq!(app.tabs[0].console_scroll, 0);
        click_log_row(&mut app, visible - 1);
        assert_eq!(app.tabs[0].console_selected, Some(newest_id));
        assert_eq!(app.tabs[0].console_copy_text(), Some("newest 🐇"));
        app.console_key(&Key::Named(NamedKey::PageUp), None, false, false);
        assert_eq!(app.tabs[0].console_scroll, 10);
        click_log_row(&mut app, 0);
        assert_eq!(app.tabs[0].console_selected, Some(stack_id));
        for _ in 0..20 {
            app.scroll_devtools(-1.0);
        }
        assert_eq!(app.tabs[0].console_scroll, 13);
        click_log_row(&mut app, 0);
        assert!(!app.tabs[0].console_expanded.contains(&stack_id));
        assert_eq!(app.tabs[0].console_scroll, 0);
        assert_eq!(app.console_rows().len(), 2);
        click_log_row(&mut app, 1);
        assert_eq!(app.tabs[0].console_selected, Some(newest_id));
        app.console_key(&Key::Character("l".into()), None, true, false);
        assert!(app.tabs[0].console.is_empty());
        assert_eq!(app.tabs[0].console_selected, None);
        assert!(app.tabs[0].console_expanded.is_empty());
        assert_eq!(app.tabs[0].console_scroll, 0);
    }

    #[test]
    fn console_expansion_is_per_tab_and_wraps_unicode_without_loss() {
        let _guard = lock();
        let mut app = console_app("console-tabs");
        app.tabs.push(Tab::new(2));
        for tab in &mut app.tabs {
            tab.console.push("error", "same\nstack");
        }
        click_log_row(&mut app, 0);
        assert_eq!(app.console_rows().len(), 2);
        app.active = 1;
        assert_eq!(app.console_rows().len(), 1);
        assert!(app.tabs[1].console_expanded.is_empty());
        assert_eq!(app.tabs[1].console_selected, None);
        app.active = 0;
        assert_eq!(app.console_rows().len(), 2);

        let mut log = ConsoleLog::default();
        let text = "é🐇東京  αβγ";
        log.push("log", text);
        let id = log.entries().next().unwrap().0;
        let expanded = BTreeSet::from([id]);
        let rows = console_rows(&log, &expanded, 3.0, |t| t.chars().count() as f32);
        assert!(rows.len() > 1);
        assert_eq!(rows.iter().map(|r| r.text).collect::<String>(), text);
        assert!(rows.iter().all(|r| r.text.chars().count() <= 3));
        assert_eq!(log.get(id).unwrap().1, text);
        assert_eq!(console_window(20, 6, usize::MAX), 0..6);
        assert_eq!(console_window(2, 6, 0), 0..2);
        assert_eq!(console_window(2, 0, 0), 2..2);
    }

    #[test]
    fn elements_copy_all_click_snapshots_live_complete_dom_and_uses_central_clipboard() {
        use std::sync::{mpsc, Arc, Mutex};
        use std::time::{Duration, Instant};
        use super::super::content::FromContent;

        let _guard = lock();
        let mut app = console_app("elements-copy-all");
        app.panel = Panel::Elements;
        app.canvas.resize(1000, 750);
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        app.sink = Arc::new(move |id, _, msg| { let _ = tx.lock().unwrap().send((id, msg)); });
        let wait = |id, pred: fn(&FromContent) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let (tab, msg) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("content reply");
                if tab == id && pred(&msg) { break msg; }
            }
        };
        let click = |app: &mut App| {
            let r = app.chrome().elements_copy_all();
            assert!(app.chrome().elements_toolbar().contains(r.x + r.w / 2.0, r.y + r.h / 2.0));
            assert!(!app.elements_split().0.contains(r.x + r.w / 2.0, r.y + r.h / 2.0));
            app.devtools_click(r.x + r.w / 2.0, r.y + r.h / 2.0);
        };
        let reply = |app: &mut App, id| {
            let FromContent::ElementsHtml(html) = wait(id, |m| matches!(m, FromContent::ElementsHtml(_))) else { unreachable!() };
            app.copy_elements_result(html);
        };
        // Without a content thread the always-visible button is disabled.
        assert!(!app.elements_copy_all_enabled());
        click(&mut app);
        assert!(copied(&app).is_empty());
        app.tabs[0].content = Some(app.spawn_content(1, app.view_for(0)));
        assert!(app.elements_copy_all_enabled());
        // A content thread without a document reports that honestly.
        click(&mut app);
        reply(&mut app, 1);
        assert_eq!(app.status.as_ref().unwrap().0, "No document to copy");
        assert!(copied(&app).is_empty());

        let text = "café 🐇 e\u{301}\n  line two & < > ".repeat(12);
        let long_attr = "á🐇".repeat(90);
        let html = format!("<!doctype html><html><head><style>.hidden {{display:none}}</style></head><body><!-- comment 🐇 --><div id='live' data-long='{long_attr}'>{}</div><template><b>template-content</b></template><script type='text/plain'>raw < & script</script>{}<p hidden>last-hidden-node</p></body></html>", text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;"), "<i>offscreen</i>".repeat(5005));
        app.tabs[0].send(ToContent::SetActive(true));
        app.tabs[0].send(ToContent::LoadHtml { html, url: rabbithole_engine::net::Url::parse("https://example.test/").unwrap(), push_history: true });
        wait(1, |m| matches!(m, FromContent::Frame(_)));
        app.tabs[0].send(ToContent::Eval("document.getElementById('live').setAttribute('data-now', 'mutated 🐇')".into()));
        wait(1, |m| matches!(m, FromContent::EvalResult(_)));
        // Deliberately stale/short inspector rows and an offscreen selection
        // cannot limit the exported document, including >5000 actual nodes.
        app.tabs[0].inspector.dom.clear();
        app.tabs[0].inspector.scroll = 4000;
        app.tabs[0].inspector.selected = Some(17);
        app.tabs[0].console_input.set_text("editor selection");
        app.tabs[0].console_input.select_all();
        app.ui_focus = UiFocus::Console;
        app.pending_search = true;
        let input = app.tabs[0].console_input.clone();
        click(&mut app);
        reply(&mut app, 1);
        let exported = &copied(&app)[0];
        assert!(exported.contains(&format!("data-long=\"{long_attr}\"")), "full Unicode attribute");
        assert!(exported.contains("data-now=\"mutated 🐇\""), "live DOM, not original source");
        assert!(exported.contains(&text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")), "full text and newlines");
        assert!(exported.contains("<!-- comment 🐇 -->"));
        assert!(exported.contains("<style>.hidden {display:none}</style>"));
        assert!(exported.contains("<script type=\"text/plain\">raw < & script</script>"));
        assert!(exported.contains("<template><b>template-content</b></template>"));
        assert!(exported.contains("last-hidden-node"));
        assert_eq!(exported.matches("<i>offscreen</i>").count(), 5005);
        assert_eq!(app.status.as_ref().unwrap().0, "Copied to clipboard");
        assert!(app.pending_search, "copy DOM must not consume a selection-search request");
        assert_eq!(app.ui_focus, UiFocus::Console);
        assert_eq!(app.tabs[0].console_input, input);
        assert_eq!(app.tabs[0].inspector.selected, Some(17));
        assert_eq!(app.tabs[0].inspector.scroll, 4000);

        app.tabs.push(Tab::new(2));
        app.tabs[1].content = Some(app.spawn_content(2, app.view_for(1)));
        app.tabs[1].send(ToContent::SetActive(true));
        app.tabs[1].send(ToContent::LoadHtml { html: "<p>other-tab-only</p>".into(), url: rabbithole_engine::net::Url::parse("https://other.test/").unwrap(), push_history: true });
        wait(2, |m| matches!(m, FromContent::Frame(_)));
        app.activate_tab(1);
        click(&mut app);
        reply(&mut app, 2);
        assert_eq!(copied(&app).len(), 2);
        assert!(copied(&app)[1].contains("other-tab-only"));
        assert!(!copied(&app)[1].contains("last-hidden-node"));

        app.clipboard_test = Some(Err(()));
        click(&mut app);
        reply(&mut app, 2);
        assert_eq!(app.status.as_ref().unwrap().0, "Clipboard unavailable");
    }

}
