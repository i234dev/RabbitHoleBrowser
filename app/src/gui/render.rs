//! Painting the window: page, overlays, chrome and popups.

use std::num::NonZeroU32;
use std::time::Duration;

use rabbithole_engine::gfx::{Color, Rect};

use super::chrome::{self, baseline_in, draw_button, draw_centered, draw_field, draw_progress, draw_ring, draw_scrollbar, draw_spinner, ellipsize, pal, FieldView, ToolButton, UI_FONT};
use super::omnibox::{security_of, Security, SuggestionKind};
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
        self.paint(w, h);
        let Some(surface) = self.surface.as_mut() else { return };
        if let Ok(mut buffer) = surface.buffer_mut() {
            let n = buffer.len().min(self.canvas.data.len());
            buffer[..n].copy_from_slice(&self.canvas.data[..n]);
            let _ = buffer.present();
        }
    }

    /// Paint the whole window into `self.canvas` (`w × h` device px).
    pub(crate) fn paint(&mut self, w: u32, h: u32) {
        if self.canvas.width != w || self.canvas.height != h {
            self.canvas.resize(w, h);
        }
        let idx = self.active;
        self.sync_view(idx);
        let chrome = self.chrome();
        let area = chrome.page_area();
        self.canvas.clear(pal().chrome_bg);
        let scale = self.page_scale();
        match self.tabs.get(self.active).and_then(|t| t.frame.as_ref().map(|f| (f, t.scroll))) {
            Some((frame, scroll)) => frame.present(&mut self.canvas, area, scroll, scale),
            None => self.canvas.fill_rect(area, pal().page_bg),
        }
        if let Some(tab) = self.tabs.get(self.active) {
            let vp = self.css_viewport();
            draw_scrollbar(&mut self.canvas, area, chrome.s, tab.content_size().1, vp.height, tab.scroll.1);
        }
        self.draw_hang_bar();
        self.draw_tab_bar();
        self.draw_toolbar();
        self.draw_bookmarks_bar();
        self.draw_find_bar();
        self.draw_prompt_bar();
        self.draw_devtools();
        self.draw_toasts(area);
        self.draw_status_bar();
        self.draw_stats(area);
        self.draw_popups();
    }

    /// "This page is not responding" with Wait / Stop buttons.
    fn draw_hang_bar(&mut self) {
        let Some((bar, wait, stop)) = self.hang_bar() else { return };
        let p = pal();
        let s = self.chrome().s;
        let fonts = self.engine.fonts.clone();
        let size = UI_FONT * s;
        self.canvas.fill_rect(bar, p.notice_bg);
        self.canvas.fill_rect(Rect::new(bar.x, bar.bottom() - 1.0, bar.w, 1.0), p.separator);
        let secs = self.tab().and_then(|t| t.content.as_ref()).and_then(|c| c.busy_for()).map(|d| d.as_secs()).unwrap_or(0);
        let text = format!("This page is not responding ({secs} s). You can wait for it or stop it.");
        let baseline = baseline_in(&fonts, self.ui_font, size, bar);
        let label = ellipsize(&fonts, self.ui_font, size, &text, (wait.x - bar.x - 24.0 * s).max(10.0));
        self.canvas.draw_text(&fonts, self.ui_font, size, &label, bar.x + 12.0 * s, baseline, p.text);
        for (r, label) in [(wait, "Wait"), (stop, "Stop")] {
            self.canvas.fill_rounded_rect(r, 4.0 * s, p.field_bg);
            self.canvas.stroke_rect(r, [1.0; 4], [p.separator; 4], [0; 4], 4.0 * s);
            draw_centered(&mut self.canvas, &fonts, self.ui_font, size, label, r, if label == "Stop" { p.accent } else { p.text });
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
        let p = *pal();
        let bar = chrome.tab_bar();
        let s = chrome.s;
        self.canvas.fill_rect(bar, if self.options.private { p.private_bg } else { p.tab_bar_bg });
        let (rects, plus) = chrome.tab_rects(&self.pinned_flags());
        let fonts = self.engine.fonts.clone();
        let drag = self.tab_drag.filter(|d| d.dragging);
        // The dragged tab is drawn last, on top, following the pointer.
        let mut order: Vec<usize> = (0..rects.len()).collect();
        if let Some(d) = drag {
            order.retain(|i| *i != d.index);
            order.push(d.index);
        }
        let private_text = if self.options.private { Some(Color::rgb(0xe8, 0xe0, 0xf8)) } else { None };
        for i in order {
            let mut r = rects[i];
            if let Some(d) = drag.filter(|d| d.index == i) {
                r = d.ghost(r, bar);
            }
            let active = i == self.active;
            let hovered = self.hover_tab == Some(i) && drag.is_none();
            let bg = if active { p.chrome_bg } else if hovered { p.tab_hover_bg } else if self.options.private { p.private_bg } else { p.tab_bar_bg };
            let shape = Rect::new(r.x, r.y, r.w, r.h + 8.0 * s);
            self.canvas.push_clip(bar);
            self.canvas.fill_rounded_rect(shape, 8.0 * s, bg);
            self.canvas.pop_clip();
            if !active && i + 1 != self.active && i + 1 < rects.len() && drag.is_none() {
                self.canvas.fill_rect(Rect::new(r.right(), r.y + 8.0 * s, 1.0, r.h - 16.0 * s), p.separator);
            }
            let tab = &self.tabs[i];
            let close = chrome.tab_close_rect(r);
            let icon_d = 16.0 * s;
            let icon_x = if tab.pinned { r.x + (r.w - icon_d) / 2.0 } else { r.x + 10.0 * s };
            let icon = Rect::new(icon_x, r.y + (r.h - icon_d) / 2.0, icon_d, icon_d);
            let mut text_x = r.x + 10.0 * s;
            if tab.is_loading() {
                let t = tab.load_started.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
                draw_spinner(&mut self.canvas, (icon.x + icon.w / 2.0, icon.y + icon.h / 2.0), icon.w / 2.0, t * 1.2, p.accent);
                text_x = icon.right() + 6.0 * s;
            } else if let Some(img) = tab.favicon.clone() {
                self.canvas.draw_image(&img, icon, 1.0);
                text_x = icon.right() + 6.0 * s;
            } else if tab.pinned || tab.current_url.is_some() {
                // A neutral globe-ish placeholder.
                self.canvas.fill_rounded_rect(icon, icon.w / 2.0, p.text_muted.with_alpha(70));
                let letter = tab.current_url.as_ref().and_then(|u| u.host.trim_start_matches("www.").chars().next()).unwrap_or('•').to_ascii_uppercase().to_string();
                draw_centered(&mut self.canvas, &fonts, self.ui_font_bold, 10.0 * s, &letter, icon, p.text);
                text_x = icon.right() + 6.0 * s;
            }
            if tab.pinned {
                continue;
            }
            let show_close = active || hovered || rects.len() == 1;
            let max_w = (if show_close { close.x } else { r.right() - 4.0 * s } - text_x - 6.0 * s).max(10.0);
            let size = UI_FONT * s;
            let baseline = baseline_in(&fonts, self.ui_font, size, r);
            let color = private_text.filter(|_| !active).unwrap_or(if active { p.text } else { p.text_muted });
            let font = if active { self.ui_font_bold } else { self.ui_font };
            let label = ellipsize(&fonts, font, size, tab.display_title(), max_w);
            self.canvas.draw_text(&fonts, font, size, &label, text_x, baseline, color);
            if show_close {
                if hovered && self.hover_tab_close {
                    self.canvas.fill_rounded_rect(close, close.w / 2.0, p.hover_bg);
                }
                draw_centered(&mut self.canvas, &fonts, self.ui_font, 13.0 * s, "×", close, p.text_muted);
            }
        }
        if self.hover_plus {
            self.canvas.fill_rounded_rect(plus, 6.0 * s, p.hover_bg);
        }
        draw_centered(&mut self.canvas, &fonts, self.ui_font, 18.0 * s, "+", plus, private_text.unwrap_or(p.text_muted));
        if self.options.private {
            let label = "Private";
            let size = 12.0 * s;
            let w = fonts.measure(self.ui_font_bold, size, label);
            let x = bar.right() - w - 12.0 * s;
            if x > plus.right() + 8.0 * s {
                let baseline = baseline_in(&fonts, self.ui_font_bold, size, bar);
                self.canvas.draw_text(&fonts, self.ui_font_bold, size, label, x, baseline, Color::rgb(0xe8, 0xe0, 0xf8));
            }
        }
    }

    fn draw_toolbar(&mut self) {
        let chrome = self.chrome();
        if chrome.fullscreen {
            return;
        }
        let p = *pal();
        let t = chrome.toolbar();
        let s = chrome.s;
        self.canvas.fill_rect(t, p.chrome_bg);
        self.canvas.fill_rect(Rect::new(0.0, t.bottom() - 1.0, t.w, 1.0), p.separator);
        let (back, fwd, loading) = self.tab().map(|t| (t.can_go_back(), t.can_go_forward(), t.is_loading())).unwrap_or((false, false, false));
        let bookmarked = self.is_bookmarked();
        let fonts = self.engine.fonts.clone();
        for b in ToolButton::ALL {
            let enabled = match b {
                ToolButton::Back => back,
                ToolButton::Forward => fwd,
                _ => true,
            };
            let rect = chrome.button_rect(b);
            if b == ToolButton::Downloads {
                if let Some(f) = self.downloads.overall_progress() {
                    if self.hover_button == Some(b) {
                        self.canvas.fill_rounded_rect(rect, chrome::RADIUS * s, p.hover_bg);
                    }
                    let c = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
                    draw_ring(&mut self.canvas, c, 10.0 * s, f, 2.0 * s, p.accent, p.separator);
                    draw_centered(&mut self.canvas, &fonts, self.ui_font, 11.0 * s, "↓", rect, p.accent);
                    continue;
                }
            }
            let color = if b == ToolButton::Star && bookmarked {
                Some(chrome::STAR)
            } else if b == ToolButton::Downloads && self.downloads.active().next().is_some() {
                Some(p.accent)
            } else {
                None
            };
            draw_button(&mut self.canvas, &fonts, self.ui_font, s, rect, b.glyph(loading, bookmarked), enabled, self.hover_button == Some(b), color);
        }
        let focused = self.ui_focus == UiFocus::Url;
        let host = if focused { None } else { chrome::host_range(&self.url_field.text) };
        let view = FieldView {
            rect: chrome.url_field(),
            text_rect: Some(chrome.url_text()),
            text: &self.url_field.text,
            caret: self.url_field.caret,
            selection: if focused { self.url_field.selection() } else { None },
            focused,
            placeholder: "Search or enter address",
            emphasis: host,
        };
        draw_field(&mut self.canvas, &fonts, self.ui_font, s, &view);
        self.draw_security_icon();
        self.draw_consent_icon();
        if loading {
            let t_anim = self.anim_start.elapsed().as_secs_f32() / 1.4;
            draw_progress(&mut self.canvas, chrome.progress_bar(), t_anim);
        }
    }

    /// Lock (https), warning (http) or info (internal) at the left of the
    /// address bar.
    fn draw_security_icon(&mut self) {
        let chrome = self.chrome();
        let s = chrome.s;
        let p = *pal();
        let r = chrome.security_icon();
        let editing = self.ui_focus == UiFocus::Url && self.url_edited;
        let sec = if editing { Security::None } else { security_of(self.tab().and_then(|t| t.current_url.as_ref())) };
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        match sec {
            Security::Secure => {
                // Shackle + body.
                let body = Rect::new(cx - 5.0 * s, cy - 1.0 * s, 10.0 * s, 8.0 * s);
                let shackle = Rect::new(cx - 3.5 * s, cy - 7.0 * s, 7.0 * s, 9.0 * s);
                self.canvas.stroke_rect(shackle, [1.6 * s; 4], [p.secure; 4], [0; 4], 3.5 * s);
                self.canvas.fill_rounded_rect(body, 1.5 * s, p.secure);
            }
            Security::Insecure => {
                let fonts = self.engine.fonts.clone();
                draw_centered(&mut self.canvas, &fonts, self.ui_font_bold, 13.0 * s, "⚠", r, p.insecure);
            }
            Security::Local => {
                let fonts = self.engine.fonts.clone();
                let c = Rect::new(cx - 6.0 * s, cy - 6.0 * s, 12.0 * s, 12.0 * s);
                self.canvas.stroke_rect(c, [1.4 * s; 4], [p.text_muted; 4], [0; 4], 6.0 * s);
                draw_centered(&mut self.canvas, &fonts, self.ui_font_bold, 9.0 * s, "i", c, p.text_muted);
            }
            Security::None => {
                let fonts = self.engine.fonts.clone();
                draw_centered(&mut self.canvas, &fonts, self.ui_font, 13.0 * s, "⌕", r, p.text_muted);
            }
        }
    }

    /// The cookie indicator at the right of the address bar: a cookie
    /// with a check (accepted), a cross (rejected) or a question mark
    /// (dialog left for the user).
    fn draw_consent_icon(&mut self) {
        let chrome = self.chrome();
        if !chrome.consent_icon {
            return;
        }
        let Some(outcome) = self.tab().and_then(|t| t.consent.as_ref()).map(|(e, _)| e.outcome) else { return };
        let s = chrome.s;
        let p = *pal();
        let r = chrome.consent_icon_rect();
        let (x, y) = (self.cursor.x as f32, self.cursor.y as f32);
        if r.contains(x, y) {
            self.canvas.fill_rounded_rect(Rect::new(r.x, r.y + 3.0 * s, r.w, r.h - 6.0 * s), chrome::RADIUS * s, p.hover_bg);
        }
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let rad = 7.5 * s;
        let dough = Color::rgb(0xd4, 0x9a, 0x4f);
        let crumb = Color::rgb(0x6b, 0x3e, 0x1a);
        self.canvas.fill_rounded_rect(Rect::new(cx - rad, cy - rad, rad * 2.0, rad * 2.0), rad, dough);
        for (dx, dy) in [(-3.0, -3.0), (2.5, -2.0), (-1.0, 2.5), (3.5, 3.0), (-4.0, 1.0)] {
            let d = 1.6 * s;
            self.canvas.fill_rounded_rect(Rect::new(cx + dx * s - d / 2.0, cy + dy * s - d / 2.0, d, d), d / 2.0, crumb);
        }
        // Badge at the bottom right.
        let (badge, glyph) = match outcome {
            rabbithole_engine::consent::ConsentOutcome::Rejected => (p.secure, "✕"),
            rabbithole_engine::consent::ConsentOutcome::Accepted => (p.accent, "✓"),
            _ => (p.insecure, "?"),
        };
        let b = Rect::new(cx + 2.0 * s, cy + 1.0 * s, 9.0 * s, 9.0 * s);
        self.canvas.fill_rounded_rect(Rect::new(b.x - 1.0 * s, b.y - 1.0 * s, b.w + 2.0 * s, b.h + 2.0 * s), 5.5 * s, p.field_bg);
        self.canvas.fill_rounded_rect(b, 4.5 * s, badge);
        let fonts = self.engine.fonts.clone();
        draw_centered(&mut self.canvas, &fonts, self.ui_font_bold, 7.0 * s, glyph, b, Color::WHITE);
    }

    /// Tooltip of the cookie indicator (status bar) when hovered.
    pub(crate) fn consent_tooltip(&self) -> Option<String> {
        let chrome = self.chrome();
        if !chrome.consent_icon || !chrome.consent_icon_rect().contains(self.cursor.x as f32, self.cursor.y as f32) {
            return None;
        }
        let (e, _) = self.tab()?.consent.as_ref()?;
        Some(format!("{} — click to see about:autocookies", e.summary()))
    }

    /// Bookmark buttons for the bookmarks bar: (index, rect) that fit,
    /// and the overflow button rect when some do not.
    pub(crate) fn bookmark_bar_items(&self) -> (Vec<(usize, Rect)>, Option<Rect>) {
        let chrome = self.chrome();
        let bar = chrome.bookmarks_bar();
        if bar.h <= 0.0 {
            return (Vec::new(), None);
        }
        let s = chrome.s;
        let size = 12.0 * s;
        let fonts = &self.engine.fonts;
        let more_w = 28.0 * s;
        let mut x = 8.0 * s;
        let mut out = Vec::new();
        for (i, b) in self.bookmarks.items.iter().enumerate() {
            let label: String = b.label().chars().take(28).collect();
            let w = (fonts.measure(self.ui_font, size, &label) + 34.0 * s).min(200.0 * s);
            if x + w > bar.right() - more_w - 8.0 * s {
                return (out, Some(Rect::new(bar.right() - more_w - 6.0 * s, bar.y + 3.0 * s, more_w, bar.h - 6.0 * s)));
            }
            out.push((i, Rect::new(x, bar.y + 3.0 * s, w, bar.h - 6.0 * s)));
            x += w + 4.0 * s;
        }
        (out, None)
    }

    fn draw_bookmarks_bar(&mut self) {
        let chrome = self.chrome();
        let bar = chrome.bookmarks_bar();
        if bar.h <= 0.0 {
            return;
        }
        let p = *pal();
        let s = chrome.s;
        self.canvas.fill_rect(bar, p.chrome_bg);
        self.canvas.fill_rect(Rect::new(0.0, bar.bottom() - 1.0, bar.w, 1.0), p.separator);
        let fonts = self.engine.fonts.clone();
        let size = 12.0 * s;
        let (items, more) = self.bookmark_bar_items();
        if items.is_empty() && more.is_none() {
            let baseline = baseline_in(&fonts, self.ui_font, size, bar);
            self.canvas.draw_text(&fonts, self.ui_font, size, "Bookmarks appear here — press Ctrl+D on a page to add one.", 12.0 * s, baseline, p.text_muted);
            return;
        }
        for (i, r) in items {
            let b = &self.bookmarks.items[i];
            if self.hover_bookmark == Some(i) {
                self.canvas.fill_rounded_rect(r, 4.0 * s, p.hover_bg);
            }
            let icon = Rect::new(r.x + 6.0 * s, r.y + (r.h - 14.0 * s) / 2.0, 14.0 * s, 14.0 * s);
            self.canvas.fill_rounded_rect(icon, 7.0 * s, p.text_muted.with_alpha(60));
            let host = rabbithole_engine::net::Url::parse(&b.url).map(|u| u.host).unwrap_or_default();
            let letter = host.trim_start_matches("www.").chars().next().unwrap_or('★').to_ascii_uppercase().to_string();
            draw_centered(&mut self.canvas, &fonts, self.ui_font_bold, 8.5 * s, &letter, icon, p.text);
            let baseline = baseline_in(&fonts, self.ui_font, size, r);
            let label = ellipsize(&fonts, self.ui_font, size, b.label(), r.w - 30.0 * s);
            self.canvas.draw_text(&fonts, self.ui_font, size, &label, icon.right() + 6.0 * s, baseline, p.text);
        }
        if let Some(m) = more {
            draw_centered(&mut self.canvas, &fonts, self.ui_font, 14.0 * s, "»", m, p.text_muted);
        }
    }

    fn draw_find_bar(&mut self) {
        let chrome = self.chrome();
        if !chrome.find_open {
            return;
        }
        let p = *pal();
        let bar = chrome.find_bar();
        let s = chrome.s;
        self.canvas.fill_rect(bar, p.chrome_bg);
        self.canvas.fill_rect(Rect::new(0.0, bar.bottom() - 1.0, bar.w, 1.0), p.separator);
        let fonts = self.engine.fonts.clone();
        let (text, caret, sel, n, cur) = self
            .tab()
            .map(|t| (t.find.field.text.clone(), t.find.field.caret, t.find.field.selection(), t.find.count, t.find.current))
            .unwrap_or_default();
        let focused = self.ui_focus == UiFocus::Find;
        let view = FieldView::new(chrome.find_field(), &text, caret, if focused { sel } else { None }, focused, "Find in page");
        draw_field(&mut self.canvas, &fonts, self.ui_font, s, &view);
        if !text.is_empty() && n == 0 {
            // No match: a red border makes it obvious.
            self.canvas.stroke_rect(chrome.find_field(), [2.0; 4], [p.insecure; 4], [0; 4], chrome::RADIUS * s);
        }
        draw_button(&mut self.canvas, &fonts, self.ui_font, s, chrome.find_button(0), "▲", n > 0, false, None);
        draw_button(&mut self.canvas, &fonts, self.ui_font, s, chrome.find_button(1), "▼", n > 0, false, None);
        let (count, color) = if text.is_empty() {
            ("Enter: next   Shift+Enter: previous   Esc: close".to_string(), p.text_muted)
        } else if n == 0 {
            ("No matches".to_string(), p.insecure)
        } else {
            (format!("{} of {}", cur + 1, n), p.text_muted)
        };
        let size = UI_FONT * s;
        let bx = chrome.find_button(1).right() + 12.0 * s;
        let baseline = baseline_in(&fonts, self.ui_font, size, chrome.find_field());
        self.canvas.draw_text(&fonts, self.ui_font, size, &count, bx, baseline, color);
        draw_button(&mut self.canvas, &fonts, self.ui_font, s, chrome.find_close(), "×", true, false, None);
    }

    /// "Save as" prompt for a download.
    fn draw_prompt_bar(&mut self) {
        let chrome = self.chrome();
        if !chrome.prompt_open {
            return;
        }
        let p = *pal();
        let bar = chrome.prompt_bar();
        let s = chrome.s;
        self.canvas.fill_rect(bar, p.notice_bg);
        self.canvas.fill_rect(Rect::new(0.0, bar.bottom() - 1.0, bar.w, 1.0), p.separator);
        let fonts = self.engine.fonts.clone();
        let (field, ok, cancel) = chrome.prompt_controls();
        let size = UI_FONT * s;
        let baseline = baseline_in(&fonts, self.ui_font_bold, size, bar);
        let from = self.save_prompt.as_ref().and_then(|pr| rabbithole_engine::net::Url::parse(&pr.url)).map(|u| u.host).unwrap_or_default();
        let title = ellipsize(&fonts, self.ui_font_bold, size, &format!("Save from {from} as:"), field.x - 20.0 * s);
        self.canvas.draw_text(&fonts, self.ui_font_bold, size, &title, 12.0 * s, baseline, p.text);
        if let Some(pr) = &self.save_prompt {
            let focused = self.ui_focus == UiFocus::Prompt;
            let text = pr.field.text.clone();
            let view = FieldView::new(field, &text, pr.field.caret, if focused { pr.field.selection() } else { None }, focused, "");
            draw_field(&mut self.canvas, &fonts, self.ui_font, s, &view);
        }
        for (r, label, primary) in [(ok, "Save", true), (cancel, "Cancel", false)] {
            self.canvas.fill_rounded_rect(r, 4.0 * s, if primary { p.accent } else { p.field_bg });
            self.canvas.stroke_rect(r, [1.0; 4], [p.separator; 4], [0; 4], 4.0 * s);
            draw_centered(&mut self.canvas, &fonts, self.ui_font, size, label, r, if primary { Color::WHITE } else { p.text });
        }
    }

    /// Script dialogs (`alert`/`confirm`/`prompt`) and notices shown as
    /// banners at the top of the page area.
    fn draw_toasts(&mut self, area: Rect) {
        if self.toasts.is_empty() {
            return;
        }
        let s = self.chrome().s;
        let fonts = self.engine.fonts.clone();
        let size = 13.0 * s;
        let h = chrome::TOAST_H * s;
        let mut y = area.y + 8.0 * s;
        let texts: Vec<String> = self.toasts.iter().map(|(t, _)| t.clone()).collect();
        for text in texts {
            let w = (area.w - 32.0 * s).min(640.0 * s);
            let r = Rect::new(area.x + (area.w - w) / 2.0, y, w, h);
            self.canvas.fill_rounded_rect(Rect::new(r.x + 1.0 * s, r.y + 2.0 * s, r.w, r.h), 6.0 * s, pal().shadow);
            self.canvas.fill_rounded_rect(r, 6.0 * s, Color::rgb(0x30, 0x33, 0x36));
            let baseline = baseline_in(&fonts, self.ui_font, size, r);
            let label = ellipsize(&fonts, self.ui_font, size, &text, w - 24.0 * s);
            self.canvas.draw_text(&fonts, self.ui_font, size, &label, r.x + 12.0 * s, baseline, Color::WHITE);
            y += h + 6.0 * s;
        }
    }

    fn draw_status_bar(&mut self) {
        let chrome = self.chrome();
        if chrome.fullscreen {
            return;
        }
        let p = *pal();
        let bar = chrome.status_bar();
        let s = chrome.s;
        self.canvas.fill_rect(bar, p.chrome_bg);
        self.canvas.fill_rect(Rect::new(0.0, bar.y, bar.w, 1.0), p.separator);
        let transient = self.status.as_ref().filter(|(_, t)| t.elapsed() < Duration::from_secs(4)).map(|(m, _)| m.clone());
        let loading = self.tab().and_then(|t| match (&t.loading, t.load_phase) {
            (Some(u), _) => Some(format!("Loading {u}…")),
            (None, Some(phase)) => Some(format!("{phase}…")),
            _ => None,
        });
        let hover_link = self.tab().and_then(|t| t.hover_link.as_ref()).map(|u| u.to_string());
        let tooltip = self.consent_tooltip().or_else(|| self.hover_button.map(|b| b.tooltip().to_string()));
        let hover_tab = self.hover_tab.and_then(|i| self.tabs.get(i)).filter(|_| self.tab_drag.is_none()).map(|t| match &t.current_url {
            Some(u) => format!("{} — {}", t.display_title(), u),
            None => t.display_title().to_string(),
        });
        let text = transient
            .or(hover_link)
            .or(tooltip)
            .or(hover_tab)
            .or_else(|| self.downloads.status_text())
            .or(loading)
            .or_else(|| (!self.settings.javascript).then(|| "JavaScript disabled".to_string()))
            .unwrap_or_default();
        let size = 12.0 * s;
        let fonts = self.engine.fonts.clone();
        let baseline = baseline_in(&fonts, self.ui_font, size, bar);
        // Right side: zoom level.
        let mut right = bar.right() - 8.0 * s;
        if let Some(z) = self.tab().map(|t| t.zoom).filter(|z| (z - 1.0).abs() > 0.01) {
            let label = format!("{}%", (z * 100.0).round());
            let w = fonts.measure(self.ui_font, size, &label);
            right -= w;
            self.canvas.draw_text(&fonts, self.ui_font, size, &label, right, baseline, p.text_muted);
            right -= 12.0 * s;
        }
        if text.is_empty() {
            return;
        }
        let label = ellipsize(&fonts, self.ui_font, size, &text, right - 16.0 * s);
        self.canvas.draw_text(&fonts, self.ui_font, size, &label, 8.0 * s, baseline, p.text);
    }

    fn draw_popups(&mut self) {
        let chrome = self.chrome();
        let s = chrome.s;
        let p = *pal();
        let fonts = self.engine.fonts.clone();
        if self.ui_focus == UiFocus::Url && !self.suggestions.is_empty() {
            let rects = self.suggestion_rects();
            if let (Some(first), Some(last)) = (rects.first(), rects.last()) {
                let bg = Rect::new(first.x, first.y - 4.0 * s, first.w, last.bottom() - first.y + 8.0 * s);
                self.canvas.fill_rounded_rect(Rect::new(bg.x + 2.0 * s, bg.y + 3.0 * s, bg.w, bg.h), 6.0 * s, p.shadow);
                self.canvas.fill_rounded_rect(bg, 6.0 * s, p.menu_bg);
                self.canvas.stroke_rect(bg, [1.0; 4], [p.separator; 4], [0; 4], 6.0 * s);
            }
            let size = UI_FONT * s;
            for (i, r) in rects.iter().enumerate() {
                if self.suggestion_sel == Some(i) {
                    self.canvas.fill_rounded_rect(Rect::new(r.x + 4.0 * s, r.y, r.w - 8.0 * s, r.h), 4.0 * s, p.hover_bg);
                }
                let sug = &self.suggestions[i];
                let baseline = baseline_in(&fonts, self.ui_font, size, *r);
                let icon = match sug.kind {
                    SuggestionKind::Search => "⌕",
                    SuggestionKind::Url => "→",
                    SuggestionKind::Bookmark => "★",
                    SuggestionKind::History => "◷",
                };
                let icon_color = if sug.kind == SuggestionKind::Bookmark { chrome::STAR } else { p.text_muted };
                let icon_rect = Rect::new(r.x + 8.0 * s, r.y, 22.0 * s, r.h);
                draw_centered(&mut self.canvas, &fonts, self.ui_font, 14.0 * s, icon, icon_rect, icon_color);
                let x0 = icon_rect.right() + 6.0 * s;
                self.canvas.push_clip(*r);
                match sug.kind {
                    SuggestionKind::Search | SuggestionKind::Url => {
                        let text = if sug.kind == SuggestionKind::Url { sug.url.clone() } else { sug.title.clone() };
                        let t = ellipsize(&fonts, self.ui_font, size, &text, r.right() - x0 - 8.0 * s);
                        self.canvas.draw_text(&fonts, self.ui_font, size, &t, x0, baseline, p.text);
                    }
                    _ => {
                        let title = if sug.title.trim().is_empty() { sug.url.clone() } else { sug.title.clone() };
                        let max_title = (r.w * 0.45).max(60.0);
                        let t = ellipsize(&fonts, self.ui_font, size, &title, max_title);
                        let tw = fonts.measure(self.ui_font, size, &t);
                        self.canvas.draw_text(&fonts, self.ui_font, size, &t, x0, baseline, p.text);
                        let ux = x0 + tw + 10.0 * s;
                        let u = ellipsize(&fonts, self.ui_font, size, &format!("— {}", sug.url), r.right() - ux - 8.0 * s);
                        self.canvas.draw_text(&fonts, self.ui_font, size, &u, ux, baseline, p.accent);
                    }
                }
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
