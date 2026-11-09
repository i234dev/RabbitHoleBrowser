//! Painting the window: page, overlays, chrome and popups.

use std::num::NonZeroU32;
use std::time::Duration;

use rabbithole_engine::gfx::{Color, Rect};

use super::chrome::{self, baseline_in, draw_button, draw_centered, draw_field, draw_progress, draw_scrollbar, ellipsize, FieldView, ToolButton, ACCENT, CHROME_BG, FIND_CURRENT, FIND_MATCH, SELECTION, SEPARATOR, TAB_BAR_BG, TEXT, TEXT_MUTED, UI_FONT};
use super::{App, UiFocus};

impl App {
    /// Paint everything and present the frame.
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
            self.page_dirty = true;
        }
        self.relayout_if_needed();
        let chrome = self.chrome();
        let area = chrome.page_area();
        let (pw, ph) = (area.w.max(1.0) as u32, area.h.max(1.0) as u32);
        if self.page_canvas.width != pw || self.page_canvas.height != ph {
            self.page_canvas.resize(pw, ph);
            self.page_dirty = true;
        }
        if self.page_dirty {
            self.page_canvas.clear(Color::WHITE);
            let scale = self.page_scale();
            if let Some(tab) = self.tabs.get(self.active) {
                if let Some(page) = &tab.page {
                    self.engine.render_scaled(page, &mut self.page_canvas, tab.scroll.0, tab.scroll.1, scale);
                }
            }
            self.page_dirty = false;
        }
        self.canvas.clear(CHROME_BG);
        self.canvas.blit(&self.page_canvas, area.x as i32, area.y as i32);
        self.canvas.push_clip(area);
        self.draw_page_overlays(area);
        self.canvas.pop_clip();
        if let Some(tab) = self.tabs.get(self.active) {
            let vp = self.css_viewport();
            let content_h = tab.page.as_ref().map(|p| p.content_height()).unwrap_or(0.0);
            draw_scrollbar(&mut self.canvas, area, chrome.s, content_h, vp.height, tab.scroll.1);
        }
        self.draw_tab_bar();
        self.draw_toolbar();
        self.draw_find_bar();
        self.draw_status_bar();
        self.draw_popups();

        let Some(surface) = self.surface.as_mut() else { return };
        if let Ok(mut buffer) = surface.buffer_mut() {
            let n = buffer.len().min(self.canvas.data.len());
            buffer[..n].copy_from_slice(&self.canvas.data[..n]);
            let _ = buffer.present();
        }
    }

    /// Selection, find highlights, focus ring and caret.
    fn draw_page_overlays(&mut self, _area: Rect) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let Some(page) = tab.page.as_ref() else { return };
        let s = self.page_scale();
        let mut rects_sel = Vec::new();
        if let Some(sel) = &tab.selection {
            for r in &sel.rects {
                rects_sel.push(self.doc_to_screen(*r));
            }
        }
        let mut finds = Vec::new();
        if tab.find.open {
            for (i, r) in tab.find.matches.iter().enumerate() {
                finds.push((self.doc_to_screen(*r), i == tab.find.current));
            }
        }
        let mut focus_ring = None;
        if let Some(n) = tab.focus {
            if tab.edit.as_ref().map(|e| e.node != n).unwrap_or(true) {
                if let Some(r) = page.rect_of(n) {
                    focus_ring = Some(self.doc_to_screen(r));
                }
            }
        }
        let mut caret = None;
        if let Some(edit) = &tab.edit {
            if let Some(r) = page.rect_of(edit.node) {
                let screen = self.doc_to_screen(r);
                let rel = page.caret_x(&self.engine.fonts, edit.node, edit.buf.caret).unwrap_or(4.0);
                let cx = screen.x + rel * s;
                let pad = (3.0 * s).min(screen.h / 4.0);
                caret = Some((Rect::new(cx, screen.y + pad, (1.0 * s).max(1.0), (screen.h - 2.0 * pad).max(2.0)), Rect::new(screen.x, screen.y, screen.w, screen.h)));
            }
        }
        for r in rects_sel {
            self.canvas.fill_rect(r, SELECTION);
        }
        for (r, current) in finds {
            self.canvas.fill_rect(r, if current { FIND_CURRENT } else { FIND_MATCH });
        }
        if let Some(r) = focus_ring {
            self.canvas.stroke_rect(Rect::new(r.x - 2.0, r.y - 2.0, r.w + 4.0, r.h + 4.0), [2.0; 4], [ACCENT; 4], [0; 4], 3.0);
        }
        if let Some((c, field)) = caret {
            self.canvas.stroke_rect(field, [1.5; 4], [ACCENT; 4], [0; 4], 2.0);
            self.canvas.fill_rect(c, TEXT);
        }
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
            let text_x = r.x + 10.0 * s;
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
            .map(|t| (t.find.field.text.clone(), t.find.field.caret, t.find.field.selection(), t.find.matches.len(), t.find.current))
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
        let loading = self.tab().and_then(|t| t.loading.as_ref()).map(|u| format!("Loading {u}…"));
        let zoom = self.tab().map(|t| t.zoom).filter(|z| (z - 1.0).abs() > 0.01).map(|z| format!("Zoom {}%", (z * 100.0).round()));
        let text = transient
            .or_else(|| self.hover_link.as_ref().map(|u| u.to_string()))
            .or_else(|| self.downloads.status_text())
            .or(loading)
            .or(zoom)
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
