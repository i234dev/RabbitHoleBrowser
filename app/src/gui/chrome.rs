//! Browser chrome geometry and painting: tab bar, toolbar, find bar,
//! status bar, menus and dropdowns. Everything is drawn on the canvas with
//! the engine's own rasterizer.

use rabbithole_engine::dom::NodeId;
use rabbithole_engine::gfx::{Canvas, Color, FontContext, FontId, Rect};
use rabbithole_engine::net::Url;

pub const TAB_H: f32 = 34.0;
pub const TOOLBAR_H: f32 = 44.0;
pub const FIND_H: f32 = 36.0;
pub const STATUS_H: f32 = 22.0;
pub const PROGRESS_H: f32 = 3.0;
pub const BUTTON: f32 = 32.0;
pub const GAP: f32 = 4.0;
pub const RADIUS: f32 = 6.0;
pub const SCROLLBAR_W: f32 = 8.0;
pub const UI_FONT: f32 = 13.0;
pub const MAX_TAB_W: f32 = 220.0;
pub const MIN_TAB_W: f32 = 80.0;

pub const CHROME_BG: Color = Color::rgb(0xf1, 0xf3, 0xf4);
pub const TAB_BAR_BG: Color = Color::rgb(0xde, 0xe1, 0xe6);
pub const SEPARATOR: Color = Color::rgb(0xda, 0xdc, 0xe0);
pub const TEXT: Color = Color::rgb(0x20, 0x21, 0x24);
pub const TEXT_MUTED: Color = Color::rgb(0x5f, 0x63, 0x68);
pub const TEXT_DISABLED: Color = Color::rgb(0xb0, 0xb4, 0xb9);
pub const HOVER_BG: Color = Color::rgba(0x20, 0x21, 0x24, 22);
pub const ACCENT: Color = Color::rgb(0x1a, 0x73, 0xe8);
pub const FIELD_BORDER: Color = Color::rgb(0xc4, 0xc7, 0xcc);
pub const SELECTION: Color = Color::rgba(0x1a, 0x73, 0xe8, 70);
pub const FIND_MATCH: Color = Color::rgba(0xff, 0xeb, 0x3b, 150);
pub const FIND_CURRENT: Color = Color::rgba(0xff, 0x98, 0x00, 190);
pub const SCROLL_THUMB: Color = Color::rgba(0, 0, 0, 80);
pub const STAR: Color = Color::rgb(0xf9, 0xab, 0x00);
pub const MENU_BG: Color = Color::WHITE;
pub const SHADOW: Color = Color::rgba(0, 0, 0, 40);

/// Toolbar buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolButton {
    Back,
    Forward,
    Reload,
    Home,
    Star,
    Menu,
}

impl ToolButton {
    pub const LEFT: [ToolButton; 4] = [ToolButton::Back, ToolButton::Forward, ToolButton::Reload, ToolButton::Home];

    pub fn glyph(self, loading: bool, bookmarked: bool) -> &'static str {
        match self {
            ToolButton::Back => "◀",
            ToolButton::Forward => "▶",
            ToolButton::Reload => {
                if loading {
                    "×"
                } else {
                    "↻"
                }
            }
            ToolButton::Home => "⌂",
            ToolButton::Star => {
                if bookmarked {
                    "★"
                } else {
                    "☆"
                }
            }
            ToolButton::Menu => "≡",
        }
    }
}

/// Actions triggered from menus.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    NewTab,
    CloseTab,
    Back,
    Forward,
    Reload,
    Home,
    Bookmarks,
    History,
    Downloads,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Find,
    ClearCookies,
    Quit,
    OpenInNewTab(Url),
    CopyLink(Url),
    CopySelection,
    ViewText,
    CopyPageUrl,
    SelectOption(NodeId, usize),
    ToggleBookmark,
    About,
}

#[derive(Debug, Clone)]
pub struct MenuItem {
    pub label: String,
    pub action: Action,
    pub enabled: bool,
    pub separator_before: bool,
}

impl MenuItem {
    pub fn new(label: &str, action: Action) -> MenuItem {
        MenuItem { label: label.into(), action, enabled: true, separator_before: false }
    }
    pub fn disabled(mut self, disabled: bool) -> MenuItem {
        self.enabled = !disabled;
        self
    }
    pub fn sep(mut self) -> MenuItem {
        self.separator_before = true;
        self
    }
}

/// A popup menu (also used for `<select>` dropdowns and the context menu).
#[derive(Debug, Clone)]
pub struct Menu {
    pub rect: Rect,
    pub items: Vec<MenuItem>,
    pub hover: Option<usize>,
    pub item_h: f32,
    pub sep_h: f32,
    /// Scroll offset in items (long select lists).
    pub first_visible: usize,
    pub max_visible: usize,
}

impl Menu {
    /// Lay out a menu anchored at (x, y) in physical px, kept inside the window.
    pub fn new(x: f32, y: f32, items: Vec<MenuItem>, s: f32, fonts: &FontContext, font: FontId, win_w: f32, win_h: f32) -> Menu {
        let item_h = 28.0 * s;
        let sep_h = 9.0 * s;
        let pad = 16.0 * s;
        let mut w = 120.0 * s;
        for it in &items {
            w = w.max(fonts.measure(font, UI_FONT * s, &it.label) + pad * 2.0);
        }
        w = w.min(win_w - 8.0 * s).max(40.0 * s);
        let max_visible = (((win_h - 16.0 * s) / item_h).floor() as usize).clamp(1, 24);
        let visible = items.len().min(max_visible);
        let seps = items.iter().take(visible).filter(|i| i.separator_before).count() as f32;
        let h = visible as f32 * item_h + seps * sep_h + 8.0 * s;
        let x = x.min(win_w - w - 4.0 * s).max(0.0);
        let y = if y + h > win_h { (y - h).max(0.0) } else { y };
        Menu { rect: Rect::new(x, y, w, h), items, hover: None, item_h, sep_h, first_visible: 0, max_visible }
    }

    fn rows(&self) -> Vec<(usize, Rect)> {
        let mut out = Vec::new();
        let mut y = self.rect.y + 4.0 * (self.item_h / 28.0);
        for (i, it) in self.items.iter().enumerate().skip(self.first_visible).take(self.max_visible) {
            if it.separator_before && i != self.first_visible {
                y += self.sep_h;
            }
            out.push((i, Rect::new(self.rect.x, y, self.rect.w, self.item_h)));
            y += self.item_h;
        }
        out
    }

    pub fn item_at(&self, x: f32, y: f32) -> Option<usize> {
        self.rows().into_iter().find(|(_, r)| r.contains(x, y)).map(|(i, _)| i)
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.rect.contains(x, y)
    }

    pub fn scroll(&mut self, delta: i32) {
        let max_first = self.items.len().saturating_sub(self.max_visible);
        self.first_visible = (self.first_visible as i64 + delta as i64).clamp(0, max_first as i64) as usize;
    }

    pub fn draw(&self, canvas: &mut Canvas, fonts: &FontContext, font: FontId, s: f32) {
        let r = self.rect;
        canvas.fill_rounded_rect(Rect::new(r.x + 2.0 * s, r.y + 3.0 * s, r.w, r.h), 6.0 * s, SHADOW);
        canvas.fill_rounded_rect(r, 6.0 * s, MENU_BG);
        canvas.stroke_rect(r, [1.0; 4], [SEPARATOR; 4], [0; 4], 6.0 * s);
        let size = UI_FONT * s;
        let m = fonts.metrics(font, size);
        for (i, row) in self.rows() {
            let it = &self.items[i];
            if it.separator_before && i != self.first_visible {
                canvas.fill_rect(Rect::new(r.x + 8.0 * s, row.y - self.sep_h / 2.0, r.w - 16.0 * s, 1.0), SEPARATOR);
            }
            if self.hover == Some(i) && it.enabled {
                canvas.fill_rect(Rect::new(r.x + 4.0 * s, row.y, r.w - 8.0 * s, row.h), HOVER_BG);
            }
            let baseline = row.y + (row.h + m.ascent - m.descent) / 2.0;
            let color = if it.enabled { TEXT } else { TEXT_DISABLED };
            canvas.draw_text(fonts, font, size, &it.label, r.x + 16.0 * s, baseline, color);
        }
        if self.items.len() > self.max_visible {
            let hint = if self.first_visible + self.max_visible < self.items.len() { "▼" } else { "▲" };
            canvas.draw_text(fonts, font, size * 0.8, hint, r.right() - 16.0 * s, r.bottom() - 6.0 * s, TEXT_MUTED);
        }
    }
}

/// Window geometry in physical pixels for a UI scale `s`.
#[derive(Debug, Clone, Copy)]
pub struct Chrome {
    pub s: f32,
    pub width: f32,
    pub height: f32,
    pub find_open: bool,
    pub fullscreen: bool,
}

impl Chrome {
    pub fn tab_bar(&self) -> Rect {
        if self.fullscreen {
            return Rect::new(0.0, 0.0, self.width, 0.0);
        }
        Rect::new(0.0, 0.0, self.width, TAB_H * self.s)
    }

    pub fn toolbar(&self) -> Rect {
        if self.fullscreen {
            return Rect::new(0.0, 0.0, self.width, 0.0);
        }
        Rect::new(0.0, self.tab_bar().bottom(), self.width, TOOLBAR_H * self.s)
    }

    pub fn progress_bar(&self) -> Rect {
        let t = self.toolbar();
        Rect::new(0.0, t.bottom() - PROGRESS_H * self.s, self.width, PROGRESS_H * self.s)
    }

    pub fn find_bar(&self) -> Rect {
        let h = if self.find_open { FIND_H * self.s } else { 0.0 };
        Rect::new(0.0, self.toolbar().bottom(), self.width, h)
    }

    pub fn status_bar(&self) -> Rect {
        if self.fullscreen {
            return Rect::new(0.0, self.height, self.width, 0.0);
        }
        Rect::new(0.0, self.height - STATUS_H * self.s, self.width, STATUS_H * self.s)
    }

    pub fn page_area(&self) -> Rect {
        let top = self.find_bar().bottom();
        let bottom = self.status_bar().y;
        Rect::new(0.0, top, self.width.max(1.0), (bottom - top).max(1.0))
    }

    pub fn button_rect(&self, b: ToolButton) -> Rect {
        let s = self.s;
        let t = self.toolbar();
        let y = t.y + (TOOLBAR_H - BUTTON) / 2.0 * s;
        match b {
            ToolButton::Menu => Rect::new(self.width - (BUTTON + 8.0) * s, y, BUTTON * s, BUTTON * s),
            ToolButton::Star => Rect::new(self.width - (BUTTON * 2.0 + 12.0) * s, y, BUTTON * s, BUTTON * s),
            _ => {
                let i = ToolButton::LEFT.iter().position(|x| *x == b).unwrap_or(0) as f32;
                Rect::new((8.0 + i * (BUTTON + GAP)) * s, y, BUTTON * s, BUTTON * s)
            }
        }
    }

    pub fn url_field(&self) -> Rect {
        let s = self.s;
        let x = self.button_rect(ToolButton::Home).right() + 12.0 * s;
        let right = self.button_rect(ToolButton::Star).x - 8.0 * s;
        let t = self.toolbar();
        Rect::new(x, t.y + (TOOLBAR_H - BUTTON) / 2.0 * s, (right - x).max(60.0 * s), BUTTON * s)
    }

    /// Tab rects for `n` tabs plus the "+" button rect.
    pub fn tab_rects(&self, n: usize) -> (Vec<Rect>, Rect) {
        let s = self.s;
        let bar = self.tab_bar();
        let plus_w = 30.0 * s;
        let avail = (self.width - plus_w - 12.0 * s).max(MIN_TAB_W * s);
        let w = (avail / n.max(1) as f32).clamp(MIN_TAB_W * s, MAX_TAB_W * s);
        let y = bar.y + 4.0 * s;
        let h = bar.h - 4.0 * s;
        let rects: Vec<Rect> = (0..n).map(|i| Rect::new(4.0 * s + i as f32 * w, y, w - 2.0 * s, h)).collect();
        let plus_x = rects.last().map(|r| r.right() + 4.0 * s).unwrap_or(4.0 * s);
        (rects, Rect::new(plus_x, y + 2.0 * s, plus_w - 4.0 * s, h - 4.0 * s))
    }

    pub fn tab_close_rect(&self, tab: Rect) -> Rect {
        let s = self.s;
        Rect::new(tab.right() - 24.0 * s, tab.y + (tab.h - 18.0 * s) / 2.0, 18.0 * s, 18.0 * s)
    }

    pub fn find_field(&self) -> Rect {
        let s = self.s;
        let f = self.find_bar();
        Rect::new(8.0 * s, f.y + 5.0 * s, 260.0 * s, f.h - 10.0 * s)
    }

    pub fn find_button(&self, i: usize) -> Rect {
        let s = self.s;
        let f = self.find_field();
        Rect::new(f.right() + 8.0 * s + i as f32 * 30.0 * s, f.y, 26.0 * s, f.h)
    }

    pub fn find_close(&self) -> Rect {
        let s = self.s;
        let f = self.find_bar();
        Rect::new(self.width - 34.0 * s, f.y + 5.0 * s, 26.0 * s, f.h - 10.0 * s)
    }
}

/// Baseline that vertically centres text of `size` in `rect`.
pub fn baseline_in(fonts: &FontContext, font: FontId, size: f32, rect: Rect) -> f32 {
    let m = fonts.metrics(font, size);
    rect.y + (rect.h + m.ascent - m.descent) / 2.0
}

/// Truncate `text` with an ellipsis so it fits in `max_w`.
pub fn ellipsize(fonts: &FontContext, font: FontId, size: f32, text: &str, max_w: f32) -> String {
    if fonts.measure(font, size, text) <= max_w {
        return text.to_string();
    }
    let ell_w = fonts.measure(font, size, "…");
    let mut out = String::new();
    for ch in text.chars() {
        let mut candidate = out.clone();
        candidate.push(ch);
        if fonts.measure(font, size, &candidate) + ell_w > max_w {
            break;
        }
        out = candidate;
    }
    out.push('…');
    out
}

/// Draw centred text in a rect.
pub fn draw_centered(canvas: &mut Canvas, fonts: &FontContext, font: FontId, size: f32, text: &str, rect: Rect, color: Color) {
    let w = fonts.measure(font, size, text);
    let x = rect.x + (rect.w - w) / 2.0;
    let baseline = baseline_in(fonts, font, size, rect);
    canvas.draw_text(fonts, font, size, text, x, baseline, color);
}

/// A text field (URL bar / find bar) rendering description.
pub struct FieldView<'a> {
    pub rect: Rect,
    pub text: &'a str,
    pub caret: usize,
    pub selection: Option<(usize, usize)>,
    pub focused: bool,
    pub placeholder: &'a str,
}

/// Draw an editable text field; returns the x where the text starts (for
/// hit testing) and the horizontal shift applied.
pub fn draw_field(canvas: &mut Canvas, fonts: &FontContext, font: FontId, s: f32, v: &FieldView) -> (f32, f32) {
    let r = v.rect;
    let radius = RADIUS * s;
    canvas.fill_rounded_rect(r, radius, Color::WHITE);
    let border = if v.focused { ACCENT } else { FIELD_BORDER };
    let bw = if v.focused { 2.0 } else { 1.0 };
    canvas.stroke_rect(r, [bw; 4], [border; 4], [0; 4], radius);
    let pad = 10.0 * s;
    let inner = Rect::new(r.x + pad, r.y, (r.w - 2.0 * pad).max(0.0), r.h);
    let size = UI_FONT * s;
    let baseline = baseline_in(fonts, font, size, r);
    let m = fonts.metrics(font, size);
    let text_w = fonts.measure(font, size, v.text);
    let caret_x = fonts.measure(font, size, &v.text[..v.caret.min(v.text.len())]);
    let shift = if v.focused && caret_x > inner.w { caret_x - inner.w } else { 0.0 };
    canvas.push_clip(inner);
    if let Some((a, b)) = v.selection {
        let x0 = fonts.measure(font, size, &v.text[..a.min(v.text.len())]);
        let x1 = fonts.measure(font, size, &v.text[..b.min(v.text.len())]);
        canvas.fill_rect(Rect::new(inner.x - shift + x0, baseline - m.ascent, x1 - x0, m.ascent + m.descent), SELECTION);
    }
    if v.text.is_empty() {
        canvas.draw_text(fonts, font, size, v.placeholder, inner.x, baseline, TEXT_MUTED);
    } else {
        let _ = text_w;
        canvas.draw_text(fonts, font, size, v.text, inner.x - shift, baseline, TEXT);
    }
    if v.focused {
        canvas.fill_rect(Rect::new(inner.x - shift + caret_x, baseline - m.ascent, 1.0_f32.max(s.round()), m.ascent + m.descent), TEXT);
    }
    canvas.pop_clip();
    (inner.x, shift)
}

/// Draw a toolbar button.
pub fn draw_button(canvas: &mut Canvas, fonts: &FontContext, font: FontId, s: f32, rect: Rect, glyph: &str, enabled: bool, hovered: bool, color: Option<Color>) {
    if hovered && enabled {
        canvas.fill_rounded_rect(rect, RADIUS * s, HOVER_BG);
    }
    let c = color.unwrap_or(if enabled { TEXT } else { TEXT_DISABLED });
    draw_centered(canvas, fonts, font, 16.0 * s, glyph, rect, c);
}

/// Indeterminate progress animation.
pub fn draw_progress(canvas: &mut Canvas, rect: Rect, t: f32) {
    canvas.fill_rect(rect, Color::rgb(0xd2, 0xe3, 0xfc));
    let seg = rect.w * 0.3;
    let x = rect.x + ((t % 1.0) * (rect.w + seg)) - seg;
    let r = Rect::new(x.max(rect.x), rect.y, (x + seg).min(rect.right()) - x.max(rect.x), rect.h);
    if r.w > 0.0 {
        canvas.fill_rect(r, ACCENT);
    }
}

/// Vertical scrollbar for the page area.
pub fn draw_scrollbar(canvas: &mut Canvas, area: Rect, s: f32, content_h: f32, viewport_h: f32, scroll_y: f32) {
    if content_h <= viewport_h || viewport_h <= 0.0 {
        return;
    }
    let track_h = area.h;
    let thumb_h = (track_h * viewport_h / content_h).max(20.0 * s).min(track_h);
    let max_scroll = (content_h - viewport_h).max(1.0);
    let thumb_y = area.y + (scroll_y / max_scroll).clamp(0.0, 1.0) * (track_h - thumb_h);
    let w = SCROLLBAR_W * s;
    let x = area.right() - w - 2.0 * s;
    canvas.fill_rounded_rect(Rect::new(x, thumb_y, w, thumb_h), w / 2.0, SCROLL_THUMB);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry() {
        let c = Chrome { s: 2.0, width: 1600.0, height: 1000.0, find_open: true, fullscreen: false };
        assert_eq!(c.tab_bar().h, 68.0);
        assert_eq!(c.toolbar().y, 68.0);
        assert_eq!(c.find_bar().h, 72.0);
        let area = c.page_area();
        assert_eq!(area.y, 68.0 + 88.0 + 72.0);
        assert_eq!(area.bottom(), 1000.0 - 44.0);
        let (tabs, plus) = c.tab_rects(3);
        assert_eq!(tabs.len(), 3);
        assert!(plus.x > tabs[2].x);
        assert!(c.url_field().w > 100.0);
        let fs = Chrome { fullscreen: true, ..c };
        assert_eq!(fs.page_area().y, 72.0);
        assert_eq!(fs.page_area().bottom(), 1000.0);
    }
}
