//! Browser chrome geometry, colours and widgets: tab bar, toolbar,
//! bookmarks bar, find bar, status bar, devtools pane, menus and
//! dropdowns. Everything is drawn on the canvas with the engine's own
//! rasterizer.

use std::sync::atomic::{AtomicBool, Ordering};

use rabbithole_engine::dom::NodeId;
use rabbithole_engine::gfx::{Canvas, Color, FontContext, FontId, Rect};
use rabbithole_engine::net::Url;

use super::tabstrip;

pub const TAB_H: f32 = 34.0;
pub const TOOLBAR_H: f32 = 44.0;
pub const BOOKMARKS_H: f32 = 28.0;
pub const FIND_H: f32 = 36.0;
pub const PROMPT_H: f32 = 40.0;
pub const STATUS_H: f32 = 22.0;
pub const DEVTOOLS_H: f32 = 260.0;
pub const TOAST_H: f32 = 30.0;
pub const PROGRESS_H: f32 = 3.0;
pub const BUTTON: f32 = 32.0;
pub const GAP: f32 = 4.0;
pub const RADIUS: f32 = 6.0;
pub const SCROLLBAR_W: f32 = 8.0;
pub const UI_FONT: f32 = 13.0;

// Colours used on page overlays (painted by the content threads).
pub const TEXT: Color = Color::rgb(0x20, 0x21, 0x24);
pub const ACCENT: Color = Color::rgb(0x1a, 0x73, 0xe8);
pub const SELECTION: Color = Color::rgba(0x1a, 0x73, 0xe8, 70);
pub const FIND_MATCH: Color = Color::rgba(0xff, 0xeb, 0x3b, 150);
pub const FIND_CURRENT: Color = Color::rgba(0xff, 0x98, 0x00, 190);
pub const STAR: Color = Color::rgb(0xf9, 0xab, 0x00);

/// Chrome colours of a theme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub chrome_bg: Color,
    pub tab_bar_bg: Color,
    pub tab_hover_bg: Color,
    pub separator: Color,
    pub text: Color,
    pub text_muted: Color,
    pub text_disabled: Color,
    pub hover_bg: Color,
    pub accent: Color,
    pub field_bg: Color,
    pub field_border: Color,
    pub selection: Color,
    pub scroll_thumb: Color,
    pub menu_bg: Color,
    pub shadow: Color,
    pub page_bg: Color,
    pub secure: Color,
    pub insecure: Color,
    pub private_bg: Color,
    pub panel_bg: Color,
    pub row_alt: Color,
    pub error_bg: Color,
    pub error_fg: Color,
    pub warn_bg: Color,
    pub warn_fg: Color,
    pub input_fg: Color,
    pub result_fg: Color,
    pub notice_bg: Color,
}

pub const LIGHT: Palette = Palette {
    chrome_bg: Color::rgb(0xf1, 0xf3, 0xf4),
    tab_bar_bg: Color::rgb(0xde, 0xe1, 0xe6),
    tab_hover_bg: Color::rgb(0xe8, 0xeb, 0xee),
    separator: Color::rgb(0xda, 0xdc, 0xe0),
    text: Color::rgb(0x20, 0x21, 0x24),
    text_muted: Color::rgb(0x5f, 0x63, 0x68),
    text_disabled: Color::rgb(0xb0, 0xb4, 0xb9),
    hover_bg: Color::rgba(0x20, 0x21, 0x24, 22),
    accent: Color::rgb(0x1a, 0x73, 0xe8),
    field_bg: Color::WHITE,
    field_border: Color::rgb(0xc4, 0xc7, 0xcc),
    selection: Color::rgba(0x1a, 0x73, 0xe8, 70),
    scroll_thumb: Color::rgba(0, 0, 0, 80),
    menu_bg: Color::WHITE,
    shadow: Color::rgba(0, 0, 0, 40),
    page_bg: Color::WHITE,
    secure: Color::rgb(0x18, 0x80, 0x38),
    insecure: Color::rgb(0xc5, 0x22, 0x1f),
    private_bg: Color::rgb(0x3c, 0x2a, 0x5c),
    panel_bg: Color::rgb(0xfa, 0xfa, 0xfa),
    row_alt: Color::rgb(0xf3, 0xf4, 0xf6),
    error_bg: Color::rgb(0xfd, 0xec, 0xea),
    error_fg: Color::rgb(0xc5, 0x22, 0x1f),
    warn_bg: Color::rgb(0xff, 0xf8, 0xe1),
    warn_fg: Color::rgb(0x7a, 0x4f, 0x01),
    input_fg: Color::rgb(0x1a, 0x73, 0xe8),
    result_fg: Color::rgb(0x18, 0x80, 0x38),
    notice_bg: Color::rgb(0xfe, 0xf7, 0xe0),
};

pub const DARK: Palette = Palette {
    chrome_bg: Color::rgb(0x35, 0x36, 0x3a),
    tab_bar_bg: Color::rgb(0x20, 0x21, 0x24),
    tab_hover_bg: Color::rgb(0x2b, 0x2c, 0x30),
    separator: Color::rgb(0x4a, 0x4c, 0x50),
    text: Color::rgb(0xe8, 0xea, 0xed),
    text_muted: Color::rgb(0x9a, 0xa0, 0xa6),
    text_disabled: Color::rgb(0x5f, 0x63, 0x68),
    hover_bg: Color::rgba(0xff, 0xff, 0xff, 26),
    accent: Color::rgb(0x8a, 0xb4, 0xf8),
    field_bg: Color::rgb(0x20, 0x21, 0x24),
    field_border: Color::rgb(0x5f, 0x63, 0x68),
    selection: Color::rgba(0x8a, 0xb4, 0xf8, 90),
    scroll_thumb: Color::rgba(0x80, 0x80, 0x80, 140),
    menu_bg: Color::rgb(0x29, 0x2a, 0x2d),
    shadow: Color::rgba(0, 0, 0, 90),
    page_bg: Color::rgb(0x20, 0x21, 0x24),
    secure: Color::rgb(0x81, 0xc9, 0x95),
    insecure: Color::rgb(0xf2, 0x8b, 0x82),
    private_bg: Color::rgb(0x2a, 0x1f, 0x40),
    panel_bg: Color::rgb(0x24, 0x25, 0x28),
    row_alt: Color::rgb(0x2b, 0x2c, 0x30),
    error_bg: Color::rgb(0x41, 0x24, 0x22),
    error_fg: Color::rgb(0xf2, 0x8b, 0x82),
    warn_bg: Color::rgb(0x41, 0x37, 0x1c),
    warn_fg: Color::rgb(0xfd, 0xd6, 0x63),
    input_fg: Color::rgb(0x8a, 0xb4, 0xf8),
    result_fg: Color::rgb(0x81, 0xc9, 0x95),
    notice_bg: Color::rgb(0x41, 0x37, 0x1c),
};

static DARK_THEME: AtomicBool = AtomicBool::new(false);

/// Use the dark chrome palette.
pub fn set_dark(dark: bool) {
    DARK_THEME.store(dark, Ordering::Relaxed);
}

/// The current chrome palette.
pub fn pal() -> &'static Palette {
    if DARK_THEME.load(Ordering::Relaxed) {
        &DARK
    } else {
        &LIGHT
    }
}

/// Toolbar buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolButton {
    Back,
    Forward,
    Reload,
    Home,
    Star,
    Downloads,
    Menu,
}

impl ToolButton {
    pub const LEFT: [ToolButton; 4] = [ToolButton::Back, ToolButton::Forward, ToolButton::Reload, ToolButton::Home];
    pub const ALL: [ToolButton; 7] = [ToolButton::Back, ToolButton::Forward, ToolButton::Reload, ToolButton::Home, ToolButton::Star, ToolButton::Downloads, ToolButton::Menu];

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
            ToolButton::Downloads => "↓",
            ToolButton::Menu => "≡",
        }
    }

    pub fn tooltip(self) -> &'static str {
        match self {
            ToolButton::Back => "Back (Alt+Left)",
            ToolButton::Forward => "Forward (Alt+Right)",
            ToolButton::Reload => "Reload (F5) — Shift: bypass cache",
            ToolButton::Home => "Home page",
            ToolButton::Star => "Bookmark this page (Ctrl+D)",
            ToolButton::Downloads => "Downloads (Ctrl+J)",
            ToolButton::Menu => "Menu",
        }
    }
}

/// Actions triggered from menus.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    NewTab,
    NewWindow,
    NewPrivateWindow,
    ReopenClosedTab,
    DuplicateTab,
    PinTab(usize),
    CloseOtherTabs(usize),
    CloseTabsToRight(usize),
    ReloadTab(usize),
    CloseTabAt(usize),
    Back,
    Forward,
    Reload,
    Bookmarks,
    History,
    Downloads,
    Settings,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Find,
    PrintPdf,
    Quit,
    OpenInNewTab(Url),
    OpenInNewWindow(Url),
    OpenInPrivateWindow(Url),
    CopyLink(Url),
    SaveUrl(Url),
    CopySelection,
    SearchSelection,
    ViewText,
    CopyPageUrl,
    SelectOption(NodeId, usize),
    ToggleBookmark,
    ToggleBookmarksBar,
    About,
    ToggleJavaScript,
    ToggleTrackerBlocking,
    ToggleDevtools,
    Inspect(f32, f32),
    // Address bar menu.
    UrlCut,
    UrlCopy,
    UrlPaste,
    UrlPasteAndGo,
    UrlSelectAll,
    // Bookmarks bar.
    OpenBookmark(String, bool),
    RemoveBookmark(String),
}

#[derive(Debug, Clone)]
pub struct MenuItem {
    pub label: String,
    /// Keyboard shortcut shown right-aligned.
    pub shortcut: String,
    pub action: Action,
    pub enabled: bool,
    pub separator_before: bool,
}

impl MenuItem {
    pub fn new(label: &str, action: Action) -> MenuItem {
        MenuItem { label: label.into(), shortcut: String::new(), action, enabled: true, separator_before: false }
    }
    pub fn key(mut self, shortcut: &str) -> MenuItem {
        self.shortcut = shortcut.into();
        self
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(x: f32, y: f32, items: Vec<MenuItem>, s: f32, fonts: &FontContext, font: FontId, win_w: f32, win_h: f32) -> Menu {
        let item_h = 28.0 * s;
        let sep_h = 9.0 * s;
        let pad = 16.0 * s;
        let mut w = 140.0 * s;
        for it in &items {
            let sc = if it.shortcut.is_empty() { 0.0 } else { fonts.measure(font, UI_FONT * s, &it.shortcut) + 28.0 * s };
            w = w.max(fonts.measure(font, UI_FONT * s, &it.label) + sc + pad * 2.0);
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

    /// Keyboard navigation: move the highlight, skipping disabled items.
    pub fn step(&mut self, down: bool) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        let mut cur = self.hover.unwrap_or(if down { n - 1 } else { 0 });
        for _ in 0..n {
            cur = if down { (cur + 1) % n } else { (cur + n - 1) % n };
            if self.items[cur].enabled {
                break;
            }
        }
        self.hover = Some(cur);
        if cur < self.first_visible {
            self.first_visible = cur;
        } else if cur >= self.first_visible + self.max_visible {
            self.first_visible = cur + 1 - self.max_visible;
        }
    }

    pub fn draw(&self, canvas: &mut Canvas, fonts: &FontContext, font: FontId, s: f32) {
        let p = pal();
        let r = self.rect;
        canvas.fill_rounded_rect(Rect::new(r.x + 2.0 * s, r.y + 3.0 * s, r.w, r.h), 6.0 * s, p.shadow);
        canvas.fill_rounded_rect(r, 6.0 * s, p.menu_bg);
        canvas.stroke_rect(r, [1.0; 4], [p.separator; 4], [0; 4], 6.0 * s);
        let size = UI_FONT * s;
        let m = fonts.metrics(font, size);
        for (i, row) in self.rows() {
            let it = &self.items[i];
            if it.separator_before && i != self.first_visible {
                canvas.fill_rect(Rect::new(r.x + 8.0 * s, row.y - self.sep_h / 2.0, r.w - 16.0 * s, 1.0), p.separator);
            }
            if self.hover == Some(i) && it.enabled {
                canvas.fill_rounded_rect(Rect::new(r.x + 4.0 * s, row.y, r.w - 8.0 * s, row.h), 4.0 * s, p.hover_bg);
            }
            let baseline = row.y + (row.h + m.ascent - m.descent) / 2.0;
            let color = if it.enabled { p.text } else { p.text_disabled };
            canvas.draw_text(fonts, font, size, &it.label, r.x + 16.0 * s, baseline, color);
            if !it.shortcut.is_empty() {
                let w = fonts.measure(font, size, &it.shortcut);
                canvas.draw_text(fonts, font, size, &it.shortcut, r.right() - 16.0 * s - w, baseline, p.text_muted);
            }
        }
        if self.items.len() > self.max_visible {
            let hint = if self.first_visible + self.max_visible < self.items.len() { "▼" } else { "▲" };
            canvas.draw_text(fonts, font, size * 0.8, hint, r.right() - 16.0 * s, r.bottom() - 6.0 * s, p.text_muted);
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
    /// The devtools pane is shown below the page.
    pub devtools_open: bool,
    pub bookmarks_bar: bool,
    /// A prompt bar (save download as…) is shown above the page.
    pub prompt_open: bool,
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

    pub fn bookmarks_bar(&self) -> Rect {
        let h = if self.bookmarks_bar && !self.fullscreen { BOOKMARKS_H * self.s } else { 0.0 };
        Rect::new(0.0, self.toolbar().bottom(), self.width, h)
    }

    pub fn progress_bar(&self) -> Rect {
        let t = self.toolbar();
        Rect::new(0.0, t.bottom() - PROGRESS_H * self.s, self.width, PROGRESS_H * self.s)
    }

    pub fn find_bar(&self) -> Rect {
        let h = if self.find_open { FIND_H * self.s } else { 0.0 };
        Rect::new(0.0, self.bookmarks_bar().bottom(), self.width, h)
    }

    pub fn prompt_bar(&self) -> Rect {
        let h = if self.prompt_open { PROMPT_H * self.s } else { 0.0 };
        Rect::new(0.0, self.find_bar().bottom(), self.width, h)
    }

    pub fn status_bar(&self) -> Rect {
        if self.fullscreen {
            return Rect::new(0.0, self.height, self.width, 0.0);
        }
        Rect::new(0.0, self.height - STATUS_H * self.s, self.width, STATUS_H * self.s)
    }

    /// The devtools pane (zero height when closed), above the status bar.
    pub fn devtools_pane(&self) -> Rect {
        let h = if self.devtools_open { (DEVTOOLS_H * self.s).min(self.height * 0.6) } else { 0.0 };
        let bottom = self.status_bar().y;
        Rect::new(0.0, bottom - h, self.width, h)
    }

    /// Header row of the devtools pane (panel tabs).
    pub fn devtools_header(&self) -> Rect {
        let p = self.devtools_pane();
        Rect::new(p.x, p.y, p.w, (26.0 * self.s).min(p.h))
    }

    /// Panel tab buttons in the devtools header (Console, Elements,
    /// Network) and the close button.
    pub fn devtools_tab(&self, i: usize) -> Rect {
        let h = self.devtools_header();
        let w = 86.0 * self.s;
        Rect::new(h.x + 6.0 * self.s + i as f32 * (w + 4.0 * self.s), h.y + 2.0 * self.s, w, h.h - 4.0 * self.s)
    }

    pub fn devtools_close(&self) -> Rect {
        let h = self.devtools_header();
        Rect::new(h.right() - 28.0 * self.s, h.y + 2.0 * self.s, 24.0 * self.s, h.h - 4.0 * self.s)
    }

    /// The devtools content below the header.
    pub fn devtools_body(&self) -> Rect {
        let p = self.devtools_pane();
        let h = self.devtools_header();
        Rect::new(p.x, h.bottom(), p.w, (p.bottom() - h.bottom()).max(0.0))
    }

    /// The console input line at the bottom of the pane.
    pub fn console_input(&self) -> Rect {
        let p = self.devtools_pane();
        let h = 26.0 * self.s;
        Rect::new(p.x + 6.0 * self.s, p.bottom() - h - 4.0 * self.s, p.w - 12.0 * self.s, h)
    }

    pub fn page_area(&self) -> Rect {
        let top = self.prompt_bar().bottom();
        let bottom = self.devtools_pane().y;
        Rect::new(0.0, top, self.width.max(1.0), (bottom - top).max(1.0))
    }

    pub fn button_rect(&self, b: ToolButton) -> Rect {
        let s = self.s;
        let t = self.toolbar();
        let y = t.y + (TOOLBAR_H - BUTTON) / 2.0 * s;
        let right = |i: f32| Rect::new(self.width - (BUTTON * (i + 1.0) + 8.0 + i * GAP) * s, y, BUTTON * s, BUTTON * s);
        match b {
            ToolButton::Menu => right(0.0),
            ToolButton::Downloads => right(1.0),
            ToolButton::Star => right(2.0),
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

    /// The security indicator inside the URL field.
    pub fn security_icon(&self) -> Rect {
        let f = self.url_field();
        Rect::new(f.x + 4.0 * self.s, f.y, 24.0 * self.s, f.h)
    }

    /// The editable text area of the URL field (after the indicator).
    pub fn url_text(&self) -> Rect {
        let f = self.url_field();
        let x = self.security_icon().right() + 2.0 * self.s;
        Rect::new(x, f.y, (f.right() - 10.0 * self.s - x).max(10.0), f.h)
    }

    /// Tab rects for tabs with the given pinned flags, plus the "+" rect.
    pub fn tab_rects(&self, pinned: &[bool]) -> (Vec<Rect>, Rect) {
        tabstrip::layout(self.tab_bar(), self.s, pinned)
    }

    pub fn tab_close_rect(&self, tab: Rect) -> Rect {
        tabstrip::close_rect(tab, self.s)
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

    /// Text field and buttons of the prompt bar: (field, ok, cancel).
    pub fn prompt_controls(&self) -> (Rect, Rect, Rect) {
        let s = self.s;
        let b = self.prompt_bar();
        let bw = 80.0 * s;
        let cancel = Rect::new(b.right() - bw - 10.0 * s, b.y + 6.0 * s, bw, b.h - 12.0 * s);
        let ok = Rect::new(cancel.x - bw - 8.0 * s, cancel.y, bw, cancel.h);
        let fx = 150.0 * s;
        let field = Rect::new(fx, cancel.y, (ok.x - 10.0 * s - fx).max(40.0 * s), cancel.h);
        (field, ok, cancel)
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
    // Binary search on the char count (long URLs).
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let end = chars.get(mid).map(|(i, _)| *i).unwrap_or(text.len());
        if fonts.measure(font, size, &text[..end]) + ell_w <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let end = chars.get(lo).map(|(i, _)| *i).unwrap_or(text.len());
    format!("{}…", &text[..end])
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
    /// Where the text goes (inside `rect`).
    pub text_rect: Option<Rect>,
    pub text: &'a str,
    pub caret: usize,
    pub selection: Option<(usize, usize)>,
    pub focused: bool,
    pub placeholder: &'a str,
    /// Unfocused URL display: dim everything but this byte range (host).
    pub emphasis: Option<(usize, usize)>,
}

impl<'a> FieldView<'a> {
    pub fn new(rect: Rect, text: &'a str, caret: usize, selection: Option<(usize, usize)>, focused: bool, placeholder: &'a str) -> FieldView<'a> {
        FieldView { rect, text_rect: None, text, caret, selection, focused, placeholder, emphasis: None }
    }
}

/// Inner text area of a field.
pub fn field_inner(v: &FieldView, s: f32) -> Rect {
    match v.text_rect {
        Some(r) => r,
        None => {
            let pad = 10.0 * s;
            Rect::new(v.rect.x + pad, v.rect.y, (v.rect.w - 2.0 * pad).max(0.0), v.rect.h)
        }
    }
}

/// Horizontal scroll of a field's text so the caret stays visible.
pub fn field_shift(fonts: &FontContext, font: FontId, s: f32, v: &FieldView) -> f32 {
    let inner = field_inner(v, s);
    let size = UI_FONT * s;
    let caret_x = fonts.measure(font, size, &v.text[..v.caret.min(v.text.len())]);
    if v.focused && caret_x > inner.w - 2.0 * s {
        caret_x - inner.w + 2.0 * s
    } else {
        0.0
    }
}

/// Draw an editable text field; returns the x where the text starts (for
/// hit testing) and the horizontal shift applied.
pub fn draw_field(canvas: &mut Canvas, fonts: &FontContext, font: FontId, s: f32, v: &FieldView) -> (f32, f32) {
    let p = pal();
    let r = v.rect;
    let radius = RADIUS * s;
    canvas.fill_rounded_rect(r, radius, p.field_bg);
    let border = if v.focused { p.accent } else { p.field_border };
    let bw = if v.focused { 2.0 } else { 1.0 };
    canvas.stroke_rect(r, [bw; 4], [border; 4], [0; 4], radius);
    let inner = field_inner(v, s);
    let size = UI_FONT * s;
    let baseline = baseline_in(fonts, font, size, r);
    let m = fonts.metrics(font, size);
    let caret_x = fonts.measure(font, size, &v.text[..v.caret.min(v.text.len())]);
    let shift = field_shift(fonts, font, s, v);
    canvas.push_clip(inner);
    if let Some((a, b)) = v.selection {
        let x0 = fonts.measure(font, size, &v.text[..a.min(v.text.len())]);
        let x1 = fonts.measure(font, size, &v.text[..b.min(v.text.len())]);
        canvas.fill_rect(Rect::new(inner.x - shift + x0, baseline - m.ascent, x1 - x0, m.ascent + m.descent), p.selection);
    }
    if v.text.is_empty() {
        canvas.draw_text(fonts, font, size, v.placeholder, inner.x, baseline, p.text_muted);
    } else if let (Some((a, b)), false) = (v.emphasis, v.focused) {
        // scheme://  host  /path — the host stands out.
        let (a, b) = (a.min(v.text.len()), b.min(v.text.len()));
        let mut x = inner.x - shift;
        x += canvas.draw_text(fonts, font, size, &v.text[..a], x, baseline, p.text_muted);
        x += canvas.draw_text(fonts, font, size, &v.text[a..b], x, baseline, p.text);
        canvas.draw_text(fonts, font, size, &v.text[b..], x, baseline, p.text_muted);
    } else {
        canvas.draw_text(fonts, font, size, v.text, inner.x - shift, baseline, p.text);
    }
    if v.focused && v.selection.is_none() {
        canvas.fill_rect(Rect::new(inner.x - shift + caret_x, baseline - m.ascent, 1.0_f32.max(s.round()), m.ascent + m.descent), p.text);
    }
    canvas.pop_clip();
    (inner.x, shift)
}

/// Draw a toolbar button.
#[allow(clippy::too_many_arguments)]
pub fn draw_button(canvas: &mut Canvas, fonts: &FontContext, font: FontId, s: f32, rect: Rect, glyph: &str, enabled: bool, hovered: bool, color: Option<Color>) {
    let p = pal();
    if hovered && enabled {
        canvas.fill_rounded_rect(rect, RADIUS * s, p.hover_bg);
    }
    let c = color.unwrap_or(if enabled { p.text } else { p.text_disabled });
    draw_centered(canvas, fonts, font, 16.0 * s, glyph, rect, c);
}

/// Indeterminate progress animation.
pub fn draw_progress(canvas: &mut Canvas, rect: Rect, t: f32) {
    let p = pal();
    canvas.fill_rect(rect, p.accent.with_alpha(60));
    let seg = rect.w * 0.3;
    let x = rect.x + ((t % 1.0) * (rect.w + seg)) - seg;
    let r = Rect::new(x.max(rect.x), rect.y, (x + seg).min(rect.right()) - x.max(rect.x), rect.h);
    if r.w > 0.0 {
        canvas.fill_rect(r, p.accent);
    }
}

/// A loading spinner: a ring of dots fading around the circle.
pub fn draw_spinner(canvas: &mut Canvas, center: (f32, f32), radius: f32, t: f32, color: Color) {
    const DOTS: usize = 8;
    let head = (t * DOTS as f32).floor() as usize;
    let dot = (radius * 0.32).max(1.0);
    for i in 0..DOTS {
        let a = i as f32 / DOTS as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let age = (head + DOTS - i) % DOTS;
        let alpha = (255.0 * (1.0 - age as f32 / DOTS as f32)).max(40.0) as u8;
        let (x, y) = (center.0 + a.cos() * (radius - dot), center.1 + a.sin() * (radius - dot));
        canvas.fill_rounded_rect(Rect::new(x - dot, y - dot, dot * 2.0, dot * 2.0), dot, color.with_alpha(alpha));
    }
}

/// A circular progress ring (downloads button).
pub fn draw_ring(canvas: &mut Canvas, center: (f32, f32), radius: f32, fraction: f32, width: f32, color: Color, track: Color) {
    let steps = 48;
    for i in 0..steps {
        let f0 = i as f32 / steps as f32;
        let f1 = (i + 1) as f32 / steps as f32;
        let a0 = f0 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let a1 = f1 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let c = if f0 < fraction { color } else { track };
        canvas.draw_line(center.0 + a0.cos() * radius, center.1 + a0.sin() * radius, center.0 + a1.cos() * radius, center.1 + a1.sin() * radius, width, c);
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
    canvas.fill_rounded_rect(Rect::new(x, thumb_y, w, thumb_h), w / 2.0, pal().scroll_thumb);
}

/// Byte range of the host in a URL string (for emphasis).
pub fn host_range(url: &str) -> Option<(usize, usize)> {
    let start = url.find("://")? + 3;
    let rest = &url[start..];
    let end = rest.find(['/', '?', '#']).map(|i| start + i).unwrap_or(url.len());
    // Skip credentials.
    let start = url[start..end].rfind('@').map(|i| start + i + 1).unwrap_or(start);
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chrome() -> Chrome {
        Chrome { s: 2.0, width: 1600.0, height: 1000.0, find_open: true, fullscreen: false, devtools_open: false, bookmarks_bar: false, prompt_open: false }
    }

    #[test]
    fn geometry() {
        let c = chrome();
        assert_eq!(c.tab_bar().h, 68.0);
        assert_eq!(c.toolbar().y, 68.0);
        assert_eq!(c.find_bar().h, 72.0);
        let area = c.page_area();
        assert_eq!(area.y, 68.0 + 88.0 + 72.0);
        assert_eq!(area.bottom(), 1000.0 - 44.0);
        let (tabs, plus) = c.tab_rects(&[false, false, false]);
        assert_eq!(tabs.len(), 3);
        assert!(plus.x > tabs[2].x);
        assert!(c.url_field().w > 100.0);
        assert!(c.url_text().x > c.security_icon().right() - 1.0);
        assert!(c.button_rect(ToolButton::Star).right() < c.button_rect(ToolButton::Downloads).x);
        assert!(c.url_field().right() < c.button_rect(ToolButton::Star).x);
        let fs = Chrome { fullscreen: true, ..c };
        assert_eq!(fs.page_area().y, 72.0);
        assert_eq!(fs.page_area().bottom(), 1000.0);
        let co = Chrome { devtools_open: true, ..c };
        assert_eq!(co.devtools_pane().h, 520.0);
        assert_eq!(co.page_area().bottom(), 1000.0 - 44.0 - 520.0);
        assert!(co.console_input().y > co.devtools_pane().y);
        assert!(co.devtools_body().y >= co.devtools_header().bottom());
        let bb = Chrome { bookmarks_bar: true, prompt_open: true, find_open: false, ..c };
        assert_eq!(bb.bookmarks_bar().h, 56.0);
        assert_eq!(bb.page_area().y, 68.0 + 88.0 + 56.0 + 80.0);
        let (f, ok, cancel) = bb.prompt_controls();
        assert!(f.right() < ok.x && ok.right() < cancel.x);
    }

    #[test]
    fn hosts() {
        assert_eq!(host_range("https://example.com/a"), Some((8, 19)));
        assert_eq!(host_range("https://u:p@example.com"), Some((12, 23)));
        assert_eq!(host_range("about:home"), None);
    }

    #[test]
    fn menu_keyboard_skips_disabled() {
        let fonts = FontContext::new();
        let font = fonts.select(rabbithole_engine::gfx::Family::Sans, 400, false);
        let items = vec![MenuItem::new("a", Action::Back), MenuItem::new("b", Action::Forward).disabled(true), MenuItem::new("c", Action::Reload)];
        let mut m = Menu::new(0.0, 0.0, items, 1.0, &fonts, font, 800.0, 600.0);
        m.step(true);
        assert_eq!(m.hover, Some(0));
        m.step(true);
        assert_eq!(m.hover, Some(2));
        m.step(false);
        assert_eq!(m.hover, Some(0));
    }
}
