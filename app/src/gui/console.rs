//! Devtools-lite, drawn natively below the page: the JavaScript console
//! (log + an input line evaluated in the page), the Elements view (DOM
//! tree + computed style of the selected node) and the Network view
//! (requests with status, type, size and time).

use rabbithole_engine::gfx::Rect;
use winit::event::KeyEvent;
use winit::keyboard::{Key, NamedKey};

use super::chrome::{baseline_in, draw_centered, draw_field, ellipsize, pal, FieldView, UI_FONT};
use super::content::ToContent;
use super::pages::human_size;
use super::{App, Panel, UiFocus};

/// Row height of the devtools lists (logical px).
const ROW_H: f32 = 18.0;

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
    fn devtools_rows(&self) -> usize {
        let body = self.chrome().devtools_body();
        let reserve = if self.panel == Panel::Console { 34.0 } else { 0.0 };
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
        let body = chrome.devtools_body();
        let input = chrome.console_input();
        let log_area = Rect::new(body.x, body.y + 2.0 * s, body.w, (input.y - body.y - 6.0 * s).max(0.0));
        let rows = (log_area.h / line_h).floor().max(0.0) as usize;
        let lines: Vec<(String, String)> = self
            .tab()
            .map(|t| {
                let skip_from_end = t.console_scroll.min(t.console.len());
                t.console.lines().rev().skip(skip_from_end).take(rows).cloned().collect::<Vec<_>>().into_iter().rev().collect()
            })
            .unwrap_or_default();
        self.canvas.push_clip(log_area);
        let mut y = log_area.y;
        if lines.is_empty() {
            let baseline = baseline_in(&fonts, self.ui_font, size, Rect::new(log_area.x, y, log_area.w, line_h));
            self.canvas.draw_text(&fonts, self.ui_font, size, "No console messages. Type JavaScript below and press Enter (Ctrl+L clears).", log_area.x + 8.0 * s, baseline, p.text_muted);
        }
        for (level, text) in &lines {
            let row = Rect::new(log_area.x, y, log_area.w, line_h);
            let (bg, fg) = match level.as_str() {
                "error" => (Some(p.error_bg), p.error_fg),
                "warn" => (Some(p.warn_bg), p.warn_fg),
                "input" => (None, p.input_fg),
                "result" => (None, p.result_fg),
                _ => (None, p.text),
            };
            if let Some(bg) = bg {
                self.canvas.fill_rect(row, bg);
            }
            let one_line: String = text.lines().next().unwrap_or("").to_string();
            let more = text.lines().count().saturating_sub(1);
            let shown = if more > 0 { format!("{one_line}  (+{more} lines)") } else { one_line };
            let label = ellipsize(&fonts, self.mono_font, size, &shown, log_area.w - 16.0 * s);
            let baseline = baseline_in(&fonts, self.mono_font, size, row);
            self.canvas.draw_text(&fonts, self.mono_font, size, &label, log_area.x + 8.0 * s, baseline, fg);
            y += line_h;
        }
        self.canvas.pop_clip();
        // Input line.
        let focused = self.ui_focus == UiFocus::Console;
        let (text, caret, sel) = self.tab().map(|t| (t.console_input.text.clone(), t.console_input.caret, t.console_input.selection())).unwrap_or_default();
        let view = FieldView::new(input, &text, caret, if focused { sel } else { None }, focused, "Evaluate JavaScript in this page…");
        draw_field(&mut self.canvas, &fonts, self.ui_font, s, &view);
    }

    /// Split of the Elements body: (tree, styles).
    pub(crate) fn elements_split(&self) -> (Rect, Rect) {
        let body = self.chrome().devtools_body();
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
        let line_h = ROW_H * chrome.s;
        match self.panel {
            Panel::Console => {
                self.ui_focus = UiFocus::Console;
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
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        match panel {
            Panel::Console => {
                let max = tab.console.len().saturating_sub(1);
                if dy < 0.0 {
                    tab.console_scroll = (tab.console_scroll + 3).min(max);
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

    /// Keys while the console input has focus.
    pub(crate) fn handle_console_key(&mut self, event: &KeyEvent, ctrl: bool, shift: bool) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        match &event.logical_key {
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
            Key::Named(NamedKey::PageUp) => tab.console_scroll = (tab.console_scroll + 10).min(tab.console.len().saturating_sub(1)),
            Key::Named(NamedKey::PageDown) => tab.console_scroll = tab.console_scroll.saturating_sub(10),
            Key::Named(NamedKey::Space) => tab.console_input.insert(" ", None),
            Key::Character(c) if ctrl => match c.to_lowercase().as_str() {
                "a" => tab.console_input.select_all(),
                "l" => {
                    tab.console.clear();
                    tab.console_scroll = 0;
                }
                "c" => {
                    if let Some(t) = tab.console_input.selected_text().map(|s| s.to_string()) {
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
                if let Some(text) = event.text.as_ref() {
                    if !ctrl {
                        tab.console_input.insert(text, None);
                    }
                }
            }
        }
        self.request_redraw();
    }
}
