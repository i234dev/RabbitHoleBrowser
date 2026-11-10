//! Painting the window: page, overlays, chrome and popups.

use std::num::NonZeroU32;
use std::time::Duration;

use rabbithole_engine::gfx::{Color, Rect};

use super::chrome::{self, baseline_in, draw_button, draw_centered, draw_field, draw_progress, draw_scrollbar, ellipsize, FieldView, ToolButton, ACCENT, CHROME_BG, SEPARATOR, TAB_BAR_BG, TEXT, TEXT_MUTED, UI_FONT};
use super::{App, UiFocus};

impl App {
    /// Paint everything and present the frame. The page is the last frame
    /// from the tab's content thread, shifted to the current scroll.
    pub(crate) fn redraw(&mut self) {
        let size = self.window_size();
        let (w, h) = (size.width, size.height);
        let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) else { return };
        if self.surface.is_none() {
            return;
        }
        if self.surface.as_mut().map(|s| s.resize(nw, nh).is_err()).unwrap_or(true) {
            return;
        }
        if self.canvas.width != w || self.canvas.height != h {
            self.canvas.resize(w, h);
        }
        let idx = self.active;
        self.sync_view(idx);
        let chrome = self.chrome();
        let area = chrome.page_area();
        self.canvas.clear(CHROME_BG);
        let scale = self.page_scale();
        match self.tabs.get(self.active).and_then(|t| t.frame.as_ref().map(|f| (f, t.scroll))) {
            Some((frame, scroll)) => frame.present(&mut self.canvas, area, scroll, scale),
            None => self.canvas.fill_rect(area, Color::WHITE),
        }
        if let Some(tab) = self.tabs.get(self.active) {
            let vp = self.css_viewport();
            draw_scrollbar(&mut self.canvas, area, chrome.s, tab.content_size().1, vp.height, tab.scroll.1);
        }
        self.draw_hang_bar();
        self.draw_tab_bar();
        self.draw_toolbar();
        self.draw_find_bar();
        self.draw_console();
        self.draw_toasts(area);
        self.draw_status_bar();
        self.draw_stats(area);
        self.draw_popups();

        let Some(surface) = self.surface.as_mut() else { return };
        if let Ok(mut buffer) = surface.buffer_mut() {
            let n = buffer.len().min(self.canvas.data.len());
            buffer[..n].copy_from_slice(&self.canvas.data[..n]);
            let _ = buffer.present();
        }
    }

    /// "This page is not responding" with Wait / Stop buttons.
    fn draw_hang_bar(&mut self) {
        let Some((bar, wait, stop)) = self.hang_bar() else { return };
        let s = self.chrome().s;
        let fonts = self.engine.fonts.clone();
        let size = UI_FONT * s;
        self.canvas.fill_rect(bar, Color::rgb(0xfe, 0xf7, 0xe0));
        self.canvas.fill_rect(Rect::new(bar.x, bar.bottom() - 1.0, bar.w, 1.0), SEPARATOR);
        let secs = self.tab().and_then(|t| t.content.as_ref()).and_then(|c| c.busy_for()).map(|d| d.as_secs()).unwrap_or(0);
        let text = format!("This page is not responding ({secs} s). You can wait for it or stop it.");
        let baseline = baseline_in(&fonts, self.ui_font, size, bar);
        let label = ellipsize(&fonts, self.ui_font, size, &text, (wait.x - bar.x - 24.0 * s).max(10.0));
        self.canvas.draw_text(&fonts, self.ui_font, size, &label, bar.x + 12.0 * s, baseline, TEXT);
        for (r, label) in [(wait, "Wait"), (stop, "Stop")] {
            self.canvas.fill_rounded_rect(r, 4.0 * s, Color::WHITE);
            self.canvas.stroke_rect(r, [1.0; 4], [SEPARATOR; 4], [0; 4], 4.0 * s);
            draw_centered(&mut self.canvas, &fonts, self.ui_font, size, label, r, if label == "Stop" { ACCENT } else { TEXT });
        }
    }

    /// Frame-time overlay (Ctrl+Shift+U).
    fn draw_stats(&mut self, area: Rect) {
        if !self.show_stats {
            return;
        }
        let s = self.chrome().s;
        let fonts = self.engine.fonts.clone();
        let size = 12.0 * s;
        let mut text = self.stats.overlay_text();
        if let Some(d) = self.tab().and_then(|t| t.content.as_ref()).and_then(|c| c.busy_for()) {
            text.push_str(&format!(" · page busy {} ms", d.as_millis()));
        }
        let w = (fonts.measure(self.ui_font, size, &text) + 16.0 * s).min(area.w);
        let r = Rect::new(area.right() - w - 8.0 * s, area.bottom() - 30.0 * s, w, 22.0 * s);
        self.canvas.fill_rounded_rect(r, 4.0 * s, Color::rgba(0x20, 0x21, 0x24, 220));
        let baseline = baseline_in(&fonts, self.ui_font, size, r);
        let label = ellipsize(&fonts, self.ui_font, size, &text, r.w - 16.0 * s);
        self.canvas.draw_text(&fonts, self.ui_font, size, &label, r.x + 8.0 * s, baseline, Color::WHITE);
    }

    fn draw_tab_bar(&mut self) {
        let chrome = self.chrome();
        if chrome.fullscreen {
            return;
        }
        let bar = chrome.tab_bar();
        let s = chrome.s;
        self.canvas.fill_rect(bar, TAB_BAR_BG);
        let (rects, plus) = chrome.tab_rects(self.tabs.len());
        let size = UI_FONT * s;
        let fonts = self.engine.fonts.clone();
        for (i, r) in rects.iter().enumerate() {
            let active = i == self.active;
            let hovered = self.hover_tab == Some(i);
            let bg = if active { CHROME_BG } else if hovered { Color::rgb(0xe8, 0xeb, 0xee) } else { TAB_BAR_BG };
            let shape = Rect::new(r.x, r.y, r.w, r.h + 8.0 * s);
            self.canvas.push_clip(bar);
            self.canvas.fill_rounded_rect(shape, 8.0 * s, bg);
            self.canvas.pop_clip();
            if !active && i + 1 != self.active && i + 1 < rects.len() {
                self.canvas.fill_rect(Rect::new(r.right(), r.y + 8.0 * s, 1.0, r.h - 16.0 * s), SEPARATOR);
            }
            let tab = &self.tabs[i];
            let close = chrome.tab_close_rect(*r);
            let mut text_x = r.x + 10.0 * s;
            if let Some(icon) = tab.favicon.clone().filter(|_| !tab.is_loading()) {
                let d = 16.0 * s;
                self.canvas.draw_image(&icon, Rect::new(text_x, r.y + (r.h - d) / 2.0, d, d), 1.0);
                text_x += d + 6.0 * s;
            }
            let max_w = (close.x - text_x - 6.0 * s).max(10.0);
            let title = if tab.is_loading() { format!("⟳ {}", tab.display_title()) } else { tab.display_title().to_string() };
            let label = ellipsize(&fonts, self.ui_font, size, &title, max_w);
            let baseline = baseline_in(&fonts, self.ui_font, size, *r);
            let color = if active { TEXT } else { TEXT_MUTED };
            let font = if active { self.ui_font_bold } else { self.ui_font };
            let label = ellipsize(&fonts, font, size, &label, max_w);
            self.canvas.draw_text(&fonts, font, size, &label, text_x, baseline, color);
            if hovered && self.hover_tab_close {
                self.canvas.fill_rounded_rect(close, close.w / 2.0, chrome::HOVER_BG);
            }
            if active || hovered || rects.len() == 1 {
                draw_centered(&mut self.canvas, &fonts, self.ui_font, 13.0 * s, "×", close, TEXT_MUTED);
            }
        }
        if self.hover_plus {
            self.canvas.fill_rounded_rect(plus, 6.0 * s, chrome::HOVER_BG);
        }
        draw_centered(&mut self.canvas, &fonts, self.ui_font, 18.0 * s, "+", plus, TEXT_MUTED);
    }

    fn draw_toolbar(&mut self) {
        let chrome = self.chrome();
        if chrome.fullscreen {
            return;
        }
        let t = chrome.toolbar();
        let s = chrome.s;
        self.canvas.fill_rect(t, CHROME_BG);
        self.canvas.fill_rect(Rect::new(0.0, t.bottom() - 1.0, t.w, 1.0), SEPARATOR);
        let (back, fwd, loading) = self.tab().map(|t| (t.can_go_back(), t.can_go_forward(), t.is_loading())).unwrap_or((false, false, false));
        let bookmarked = self.is_bookmarked();
        let fonts = self.engine.fonts.clone();
        for b in [ToolButton::Back, ToolButton::Forward, ToolButton::Reload, ToolButton::Home, ToolButton::Star, ToolButton::Menu] {
            let enabled = match b {
                ToolButton::Back => back,
                ToolButton::Forward => fwd,
                _ => true,
            };
            let color = if b == ToolButton::Star && bookmarked { Some(chrome::STAR) } else { None };
            draw_button(&mut self.canvas, &fonts, self.ui_font, s, chrome.button_rect(b), b.glyph(loading, bookmarked), enabled, self.hover_button == Some(b), color);
        }
        let focused = self.ui_focus == UiFocus::Url;
        let view = FieldView {
            rect: chrome.url_field(),
            text: &self.url_field.text,
            caret: self.url_field.caret,
            selection: if focused { self.url_field.selection() } else { None },
            focused,
            placeholder: "Search or enter address",
        };
        draw_field(&mut self.canvas, &fonts, self.ui_font, s, &view);
        if loading {
            let t_anim = self.anim_start.elapsed().as_secs_f32() / 1.4;
            draw_progress(&mut self.canvas, chrome.progress_bar(), t_anim);
        }
    }

    fn draw_find_bar(&mut self) {
        let chrome = self.chrome();
        if !chrome.find_open {
            return;
        }
        let bar = chrome.find_bar();
        let s = chrome.s;
        self.canvas.fill_rect(bar, CHROME_BG);
        self.canvas.fill_rect(Rect::new(0.0, bar.bottom() - 1.0, bar.w, 1.0), SEPARATOR);
        let fonts = self.engine.fonts.clone();
        let (text, caret, sel, n, cur) = self
            .tab()
            .map(|t| (t.find.field.text.clone(), t.find.field.caret, t.find.field.selection(), t.find.count, t.find.current))
            .unwrap_or_default();
        let focused = self.ui_focus == UiFocus::Find;
        let view = FieldView { rect: chrome.find_field(), text: &text, caret, selection: if focused { sel } else { None }, focused, placeholder: "Find in page" };
        draw_field(&mut self.canvas, &fonts, self.ui_font, s, &view);
        draw_button(&mut self.canvas, &fonts, self.ui_font, s, chrome.find_button(0), "▲", n > 0, false, None);
        draw_button(&mut self.canvas, &fonts, self.ui_font, s, chrome.find_button(1), "▼", n > 0, false, None);
        let count = if text.is_empty() {
            String::new()
        } else if n == 0 {
            "No matches".to_string()
        } else {
            format!("{} of {}", cur + 1, n)
        };
        let size = UI_FONT * s;
        let bx = chrome.find_button(1).right() + 12.0 * s;
        let baseline = baseline_in(&fonts, self.ui_font, size, chrome.find_field());
        self.canvas.draw_text(&fonts, self.ui_font, size, &count, bx, baseline, TEXT_MUTED);
        draw_button(&mut self.canvas, &fonts, self.ui_font, s, chrome.find_close(), "×", true, false, None);
    }

    /// Script dialogs (`alert`/`confirm`/`prompt`) shown as banners at the
    /// top of the page area.
    fn draw_toasts(&mut self, area: Rect) {
        if self.toasts.is_empty() {
            return;
        }
        let s = self.chrome().s;
        let fonts = self.engine.fonts.clone();
        let size = 13.0 * s;
        let h = super::chrome::TOAST_H * s;
        let mut y = area.y + 8.0 * s;
        let texts: Vec<String> = self.toasts.iter().map(|(t, _)| t.clone()).collect();
        for text in texts {
            let w = (area.w - 32.0 * s).min(640.0 * s);
            let r = Rect::new(area.x + (area.w - w) / 2.0, y, w, h);
            self.canvas.fill_rounded_rect(r, 6.0 * s, rabbithole_engine::gfx::Color::rgb(0x30, 0x33, 0x36));
            let baseline = baseline_in(&fonts, self.ui_font, size, r);
            let label = ellipsize(&fonts, self.ui_font, size, &text, w - 24.0 * s);
            self.canvas.draw_text(&fonts, self.ui_font, size, &label, r.x + 12.0 * s, baseline, rabbithole_engine::gfx::Color::WHITE);
            y += h + 6.0 * s;
        }
    }

    fn draw_status_bar(&mut self) {
        let chrome = self.chrome();
        if chrome.fullscreen {
            return;
        }
        let bar = chrome.status_bar();
        let s = chrome.s;
        self.canvas.fill_rect(bar, CHROME_BG);
        self.canvas.fill_rect(Rect::new(0.0, bar.y, bar.w, 1.0), SEPARATOR);
        let transient = self.status.as_ref().filter(|(_, t)| t.elapsed() < Duration::from_secs(4)).map(|(m, _)| m.clone());
        let loading = self.tab().and_then(|t| match (&t.loading, t.load_phase) {
            (Some(u), _) => Some(format!("Loading {u}…")),
            (None, Some(phase)) => Some(format!("{phase}…")),
            _ => None,
        });
        let hover_link = self.tab().and_then(|t| t.hover_link.as_ref()).map(|u| u.to_string());
        let zoom = self.tab().map(|t| t.zoom).filter(|z| (z - 1.0).abs() > 0.01).map(|z| format!("Zoom {}%", (z * 100.0).round()));
        let text = transient
            .or(hover_link)
            .or_else(|| self.downloads.status_text())
            .or(loading)
            .or(zoom)
            .or_else(|| (!self.settings.javascript).then(|| "JavaScript disabled".to_string()))
            .unwrap_or_default();
        if text.is_empty() {
            return;
        }
        let size = 12.0 * s;
        let fonts = self.engine.fonts.clone();
        let baseline = baseline_in(&fonts, self.ui_font, size, bar);
        let label = ellipsize(&fonts, self.ui_font, size, &text, bar.w - 16.0 * s);
        self.canvas.draw_text(&fonts, self.ui_font, size, &label, 8.0 * s, baseline, TEXT);
    }

    fn draw_popups(&mut self) {
        let chrome = self.chrome();
        let s = chrome.s;
        let fonts = self.engine.fonts.clone();
        if self.ui_focus == UiFocus::Url && !self.suggestions.is_empty() {
            let rects = self.suggestion_rects();
            if let (Some(first), Some(last)) = (rects.first(), rects.last()) {
                let bg = Rect::new(first.x, first.y, first.w, last.bottom() - first.y);
                self.canvas.fill_rounded_rect(Rect::new(bg.x + 2.0 * s, bg.y + 3.0 * s, bg.w, bg.h), 6.0 * s, chrome::SHADOW);
                self.canvas.fill_rounded_rect(bg, 6.0 * s, Color::WHITE);
                self.canvas.stroke_rect(bg, [1.0; 4], [SEPARATOR; 4], [0; 4], 6.0 * s);
            }
            let size = UI_FONT * s;
            for (i, r) in rects.iter().enumerate() {
                if self.suggestion_sel == Some(i) {
                    self.canvas.fill_rect(Rect::new(r.x + 4.0 * s, r.y, r.w - 8.0 * s, r.h), chrome::HOVER_BG);
                }
                let (url, title) = &self.suggestions[i];
                let baseline = baseline_in(&fonts, self.ui_font, size, *r);
                let max_title = (r.w * 0.4).max(60.0);
                let t = ellipsize(&fonts, self.ui_font, size, title, max_title);
                let tw = fonts.measure(self.ui_font, size, &t);
                self.canvas.push_clip(*r);
                self.canvas.draw_text(&fonts, self.ui_font, size, &t, r.x + 12.0 * s, baseline, TEXT);
                let ux = r.x + 12.0 * s + tw + if t.is_empty() { 0.0 } else { 12.0 * s };
                let u = ellipsize(&fonts, self.ui_font, size, url, r.right() - ux - 8.0 * s);
                self.canvas.draw_text(&fonts, self.ui_font, size, &u, ux, baseline, TEXT_MUTED);
                self.canvas.pop_clip();
            }
        }
        if let Some(m) = &self.dropdown {
            m.draw(&mut self.canvas, &fonts, self.ui_font, s);
        }
        if let Some(m) = &self.menu {
            m.draw(&mut self.canvas, &fonts, self.ui_font, s);
        }
        if let Some(m) = &self.context_menu {
            m.draw(&mut self.canvas, &fonts, self.ui_font, s);
        }
    }
}
