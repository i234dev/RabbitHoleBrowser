//! The JavaScript console pane (devtools-lite): log lines per tab and an
//! input line evaluated in the page.

use rabbithole_engine::gfx::{Color, Rect};
use winit::event::KeyEvent;
use winit::keyboard::{Key, NamedKey};

use super::chrome::{baseline_in, draw_field, ellipsize, FieldView, CHROME_BG, SEPARATOR, TEXT, TEXT_MUTED, UI_FONT};
use super::{App, UiFocus};

const CONSOLE_BG: Color = Color::rgb(0xfa, 0xfa, 0xfa);
const ERROR_BG: Color = Color::rgb(0xfd, 0xec, 0xea);
const ERROR_FG: Color = Color::rgb(0xc5, 0x22, 0x1f);
const WARN_BG: Color = Color::rgb(0xff, 0xf8, 0xe1);
const WARN_FG: Color = Color::rgb(0x7a, 0x4f, 0x01);
const INPUT_FG: Color = Color::rgb(0x1a, 0x73, 0xe8);
const RESULT_FG: Color = Color::rgb(0x18, 0x80, 0x38);

impl App {
    /// Paint the console pane (when open).
    pub(crate) fn draw_console(&mut self) {
        let chrome = self.chrome();
        if !chrome.console_open {
            return;
        }
        let pane = chrome.console_pane();
        let s = chrome.s;
        self.canvas.fill_rect(pane, CONSOLE_BG);
        self.canvas.fill_rect(Rect::new(0.0, pane.y, pane.w, 1.0), SEPARATOR);
        let fonts = self.engine.fonts.clone();
        let size = 12.0 * s;
        let line_h = 17.0 * s;
        let input = chrome.console_input();
        let log_area = Rect::new(pane.x, pane.y + 1.0, pane.w, input.y - pane.y - 4.0 * s);
        // Title + hint.
        let header_h = 18.0 * s;
        let baseline = baseline_in(&fonts, self.ui_font_bold, size, Rect::new(pane.x, pane.y, pane.w, header_h));
        let (count, errors) = self.tab().map(|t| (t.console.len(), t.console.error_count())).unwrap_or((0, 0));
        let title = format!("Console — {count} messages, {errors} errors   (Enter evaluates, Esc closes, Ctrl+L clears)");
        self.canvas.draw_text(&fonts, self.ui_font_bold, size, &ellipsize(&fonts, self.ui_font_bold, size, &title, pane.w - 16.0 * s), pane.x + 8.0 * s, baseline, TEXT_MUTED);
        // Log lines, newest at the bottom.
        let rows = ((log_area.h - header_h) / line_h).floor().max(0.0) as usize;
        let lines: Vec<(String, String)> = self
            .tab()
            .map(|t| {
                let skip_from_end = t.console_scroll.min(t.console.len());
                t.console.lines().rev().skip(skip_from_end).take(rows).cloned().collect::<Vec<_>>().into_iter().rev().collect()
            })
            .unwrap_or_default();
        self.canvas.push_clip(log_area);
        let mut y = log_area.y + header_h;
        for (level, text) in &lines {
            let row = Rect::new(log_area.x, y, log_area.w, line_h);
            let (bg, fg) = match level.as_str() {
                "error" => (Some(ERROR_BG), ERROR_FG),
                "warn" => (Some(WARN_BG), WARN_FG),
                "input" => (None, INPUT_FG),
                "result" => (None, RESULT_FG),
                _ => (None, TEXT),
            };
            if let Some(bg) = bg {
                self.canvas.fill_rect(row, bg);
            }
            let one_line: String = text.lines().next().unwrap_or("").to_string();
            let more = text.lines().count().saturating_sub(1);
            let shown = if more > 0 { format!("{one_line}  (+{more} lines)") } else { one_line };
            let label = ellipsize(&fonts, self.ui_font, size, &shown, log_area.w - 16.0 * s);
            let baseline = baseline_in(&fonts, self.ui_font, size, row);
            self.canvas.draw_text(&fonts, self.ui_font, size, &label, log_area.x + 8.0 * s, baseline, fg);
            y += line_h;
        }
        self.canvas.pop_clip();
        // Input line.
        let focused = self.ui_focus == UiFocus::Console;
        let (text, caret, sel) = self.tab().map(|t| (t.console_input.text.clone(), t.console_input.caret, t.console_input.selection())).unwrap_or_default();
        let view = FieldView { rect: input, text: &text, caret, selection: if focused { sel } else { None }, focused, placeholder: "Evaluate JavaScript in this page…" };
        draw_field(&mut self.canvas, &fonts, self.ui_font, s, &view);
        self.canvas.fill_rect(Rect::new(pane.x, input.y - 3.0 * s, pane.w, 1.0), SEPARATOR);
        let _ = (CHROME_BG, UI_FONT);
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
                self.toggle_console();
                return;
            }
            Key::Named(NamedKey::Backspace) => tab.console_input.backspace(ctrl),
            Key::Named(NamedKey::Delete) => tab.console_input.delete(ctrl),
            Key::Named(NamedKey::ArrowLeft) => tab.console_input.move_left(shift, ctrl),
            Key::Named(NamedKey::ArrowRight) => tab.console_input.move_right(shift, ctrl),
            Key::Named(NamedKey::Home) => tab.console_input.home(shift),
            Key::Named(NamedKey::End) => tab.console_input.end(shift),
            Key::Named(NamedKey::ArrowUp) => tab.console_scroll = (tab.console_scroll + 1).min(tab.console.len().saturating_sub(1)),
            Key::Named(NamedKey::ArrowDown) => tab.console_scroll = tab.console_scroll.saturating_sub(1),
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

    /// Scroll the console log with the wheel.
    pub(crate) fn scroll_console(&mut self, dy: f32) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let max = tab.console.len().saturating_sub(1);
        if dy < 0.0 {
            tab.console_scroll = (tab.console_scroll + 3).min(max);
        } else {
            tab.console_scroll = tab.console_scroll.saturating_sub(3);
        }
        self.request_redraw();
    }
}
