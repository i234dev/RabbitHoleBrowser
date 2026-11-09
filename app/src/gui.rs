//! The graphical browser window.
//!
//! Everything on screen — toolbar, URL field, status bar, scrollbar and the
//! page itself — is painted by our own software rasterizer onto a `Canvas`
//! and handed to the OS through `softbuffer`. No toolkit widgets are used.

use std::num::NonZeroU32;
use std::rc::Rc;

use rabbithole_engine::gfx::{Canvas, Color, Family, FontId, Rect};
use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Page, Viewport};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, Window, WindowAttributes, WindowId};

const TOOLBAR_H: f32 = 44.0;
const STATUS_H: f32 = 22.0;
const BUTTON_SIZE: f32 = 32.0;
const BUTTON_GAP: f32 = 4.0;
const BUTTON_Y: f32 = 6.0;
const FIELD_RADIUS: f32 = 6.0;
const SCROLLBAR_W: f32 = 8.0;
const LINE_SCROLL: f32 = 40.0;
const UI_FONT_SIZE: f32 = 14.0;
const PLACEHOLDER: &str = "Search or enter address";

const TOOLBAR_BG: Color = Color::rgb(0xf3, 0xf3, 0xf3);
const TOOLBAR_BORDER: Color = Color::rgb(0xd0, 0xd0, 0xd0);
const BUTTON_BG: Color = Color::rgb(0xe4, 0xe4, 0xe4);
const BUTTON_FG: Color = Color::rgb(0x33, 0x33, 0x33);
const BUTTON_DISABLED: Color = Color::rgb(0xb0, 0xb0, 0xb0);
const FIELD_BORDER: Color = Color::rgb(0xc0, 0xc0, 0xc0);
const FIELD_FOCUS: Color = Color::rgb(0x3b, 0x82, 0xf6);
const TEXT_FG: Color = Color::rgb(0x20, 0x20, 0x20);
const TEXT_MUTED: Color = Color::rgb(0x90, 0x90, 0x90);
const SCROLL_THUMB: Color = Color::rgba(0, 0, 0, 80);

/// Toolbar buttons, left to right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Button {
    Back,
    Forward,
    Reload,
    Home,
}

impl Button {
    const ALL: [Button; 4] = [Button::Back, Button::Forward, Button::Reload, Button::Home];

    fn glyph(self) -> &'static str {
        match self {
            Button::Back => "◀",
            Button::Forward => "▶",
            Button::Reload => "↻",
            Button::Home => "⌂",
        }
    }

    fn rect(self) -> Rect {
        let index = Button::ALL.iter().position(|b| *b == self).unwrap_or(0) as f32;
        Rect::new(8.0 + index * (BUTTON_SIZE + BUTTON_GAP), BUTTON_Y, BUTTON_SIZE, BUTTON_SIZE)
    }
}

/// Run the GUI, opening `start_url` first.
pub fn run(start_url: String) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("could not create event loop: {e}"))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App::new(start_url);
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("event loop error: {e}"))
}

/// Editable text of the URL field.
#[derive(Debug, Default)]
struct UrlField {
    text: String,
    /// Caret position in chars.
    caret: usize,
    focused: bool,
    /// The whole text is selected: the next insertion replaces it.
    select_all: bool,
}

impl UrlField {
    fn byte_index(&self, char_index: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_index)
            .map(|(i, _)| i)
            .unwrap_or(self.text.len())
    }

    fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
        self.caret = self.char_len();
        self.select_all = false;
    }

    fn focus_all(&mut self) {
        self.focused = true;
        self.select_all = true;
        self.caret = self.char_len();
    }

    fn insert(&mut self, s: &str) {
        if self.select_all {
            self.text.clear();
            self.caret = 0;
            self.select_all = false;
        }
        let at = self.byte_index(self.caret);
        self.text.insert_str(at, s);
        self.caret += s.chars().count();
    }

    fn backspace(&mut self) {
        if self.select_all {
            self.set_text("");
            return;
        }
        if self.caret == 0 {
            return;
        }
        let start = self.byte_index(self.caret - 1);
        let end = self.byte_index(self.caret);
        self.text.replace_range(start..end, "");
        self.caret -= 1;
    }

    fn delete(&mut self) {
        if self.select_all {
            self.set_text("");
            return;
        }
        if self.caret >= self.char_len() {
            return;
        }
        let start = self.byte_index(self.caret);
        let end = self.byte_index(self.caret + 1);
        self.text.replace_range(start..end, "");
    }

    fn move_caret(&mut self, delta: isize) {
        self.select_all = false;
        let len = self.char_len() as isize;
        self.caret = (self.caret as isize + delta).clamp(0, len) as usize;
    }

    fn home(&mut self) {
        self.select_all = false;
        self.caret = 0;
    }

    fn end(&mut self) {
        self.select_all = false;
        self.caret = self.char_len();
    }
}

struct App {
    engine: Engine,
    page: Option<Page>,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    canvas: Canvas,
    page_canvas: Canvas,
    scroll_y: f32,
    history: Vec<Url>,
    history_index: usize,
    url_field: UrlField,
    status: String,
    hover_link: Option<Url>,
    modifiers: ModifiersState,
    cursor: PhysicalPosition<f64>,
    cursor_icon: CursorIcon,
    start_url: Option<String>,
    ui_font: FontId,
}

impl App {
    fn new(start_url: String) -> App {
        let engine = Engine::new();
        let ui_font = engine.fonts.select(Family::Sans, 400, false);
        App {
            engine,
            page: None,
            window: None,
            surface: None,
            canvas: Canvas::new(1, 1),
            page_canvas: Canvas::new(1, 1),
            scroll_y: 0.0,
            history: Vec::new(),
            history_index: 0,
            url_field: UrlField::default(),
            status: String::new(),
            hover_link: None,
            modifiers: ModifiersState::empty(),
            cursor: PhysicalPosition::new(0.0, 0.0),
            cursor_icon: CursorIcon::Default,
            start_url: Some(start_url),
            ui_font,
        }
    }

    // ------------------------------------------------------------ geometry

    fn window_size(&self) -> PhysicalSize<u32> {
        self.window
            .as_ref()
            .map(|w| w.inner_size())
            .unwrap_or(PhysicalSize::new(1200, 800))
    }

    fn page_area(&self) -> Rect {
        let size = self.window_size();
        let h = (size.height as f32 - TOOLBAR_H - STATUS_H).max(1.0);
        Rect::new(0.0, TOOLBAR_H, size.width.max(1) as f32, h)
    }

    fn viewport(&self) -> Viewport {
        let area = self.page_area();
        Viewport {
            width: area.w,
            height: area.h,
        }
    }

    fn url_field_rect(&self) -> Rect {
        let size = self.window_size();
        let x = Button::Home.rect().right() + 12.0;
        let label_w = self.engine.fonts.measure(self.ui_font, UI_FONT_SIZE, "RabbitHole") + 24.0;
        let w = (size.width as f32 - x - label_w - 8.0).max(60.0);
        Rect::new(x, BUTTON_Y, w, BUTTON_SIZE)
    }

    fn max_scroll(&self) -> f32 {
        let content = self.page.as_ref().map(|p| p.content_height()).unwrap_or(0.0);
        (content - self.page_area().h).max(0.0)
    }

    fn can_go_back(&self) -> bool {
        self.history_index > 0
    }

    fn can_go_forward(&self) -> bool {
        self.history_index + 1 < self.history.len()
    }

    // ---------------------------------------------------------- navigation

    /// Turn what the user typed into something loadable.
    fn resolve_input(input: &str) -> String {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return "about:home".to_string();
        }
        if looks_like_url(trimmed) {
            trimmed.to_string()
        } else {
            format!("https://duckduckgo.com/html/?q={}", percent_encode(trimmed))
        }
    }

    /// Load `url`, optionally pushing it onto the history.
    fn navigate(&mut self, url: &str, push_history: bool) {
        self.status = format!("Loading {url}…");
        self.hover_link = None;
        self.redraw();
        let viewport = self.viewport();
        let page = match self.engine.load(url, viewport) {
            Ok(page) => page,
            Err(err) => {
                let base = Url::parse(url).unwrap_or_else(about_blank);
                let html = error_page(url, &err.to_string());
                self.engine.load_html(&html, base, viewport)
            }
        };
        if push_history {
            self.history.truncate(self.history_index + 1);
            self.history.push(page.url.clone());
            self.history_index = self.history.len() - 1;
        } else if self.history.is_empty() {
            self.history.push(page.url.clone());
            self.history_index = 0;
        }
        self.set_page(page);
    }

    fn set_page(&mut self, page: Page) {
        self.scroll_y = 0.0;
        self.status.clear();
        self.url_field.focused = false;
        self.url_field.set_text(&page.url.to_string());
        if let Some(window) = &self.window {
            let title = if page.title.trim().is_empty() {
                "RabbitHole".to_string()
            } else {
                format!("{} — RabbitHole", page.title.trim())
            };
            window.set_title(&title);
        }
        let fragment = page.url.fragment.clone();
        self.page = Some(page);
        if let Some(fragment) = fragment {
            self.scroll_to_fragment(&fragment);
        }
        self.request_redraw();
    }

    fn scroll_to_fragment(&mut self, fragment: &str) {
        let target = fragment.trim_start_matches('#');
        if target.is_empty() || target == "top" {
            self.set_scroll(0.0);
            return;
        }
        let rect = self.page.as_ref().and_then(|p| p.anchor_rect(target));
        if let Some(rect) = rect {
            self.set_scroll(rect.y);
        }
    }

    fn go_history(&mut self, delta: isize) {
        let target = self.history_index as isize + delta;
        if target < 0 || target as usize >= self.history.len() {
            return;
        }
        self.history_index = target as usize;
        let url = self.history[self.history_index].to_string();
        self.navigate(&url, false);
    }

    fn reload(&mut self) {
        if let Some(url) = self.page.as_ref().map(|p| p.url.to_string()) {
            self.navigate(&url, false);
        }
    }

    fn follow_link(&mut self, link: Url) {
        let same_document = self
            .page
            .as_ref()
            .map(|p| p.url.without_fragment() == link.without_fragment())
            .unwrap_or(false);
        match (&link.fragment, same_document) {
            (Some(fragment), true) => {
                let fragment = fragment.clone();
                self.scroll_to_fragment(&fragment);
                self.request_redraw();
            }
            _ => self.navigate(&link.to_string(), true),
        }
    }

    fn submit_url_field(&mut self) {
        let target = App::resolve_input(&self.url_field.text);
        self.url_field.focused = false;
        self.navigate(&target, true);
    }

    fn cancel_url_edit(&mut self) {
        self.url_field.focused = false;
        self.url_field.select_all = false;
        if let Some(url) = self.page.as_ref().map(|p| p.url.to_string()) {
            self.url_field.set_text(&url);
        }
        self.request_redraw();
    }

    // ------------------------------------------------------------ scrolling

    fn set_scroll(&mut self, y: f32) {
        let clamped = y.clamp(0.0, self.max_scroll());
        if (clamped - self.scroll_y).abs() > f32::EPSILON {
            self.scroll_y = clamped;
            self.update_hover();
            self.request_redraw();
        }
    }

    fn scroll_by(&mut self, dy: f32) {
        self.set_scroll(self.scroll_y + dy);
    }

    fn relayout(&mut self) {
        let viewport = self.viewport();
        if let Some(page) = self.page.as_mut() {
            self.engine.relayout(page, viewport);
        }
        self.scroll_y = self.scroll_y.clamp(0.0, self.max_scroll());
    }

    // ---------------------------------------------------------------- input

    fn handle_key(&mut self, event: &KeyEvent, event_loop: &ActiveEventLoop) {
        if event.state != ElementState::Pressed {
            return;
        }
        let ctrl = self.modifiers.control_key() || self.modifiers.super_key();
        let alt = self.modifiers.alt_key();

        // Global shortcuts first.
        match &event.logical_key {
            Key::Character(c) if ctrl => match c.to_lowercase().as_str() {
                "q" | "w" => {
                    event_loop.exit();
                    return;
                }
                "l" => {
                    self.url_field.focus_all();
                    self.request_redraw();
                    return;
                }
                "r" => {
                    self.reload();
                    return;
                }
                _ => {}
            },
            Key::Named(NamedKey::F6) => {
                self.url_field.focus_all();
                self.request_redraw();
                return;
            }
            Key::Named(NamedKey::F5) => {
                self.reload();
                return;
            }
            Key::Named(NamedKey::ArrowLeft) if alt => {
                self.go_history(-1);
                return;
            }
            Key::Named(NamedKey::ArrowRight) if alt => {
                self.go_history(1);
                return;
            }
            Key::Named(NamedKey::BrowserBack) => {
                self.go_history(-1);
                return;
            }
            Key::Named(NamedKey::BrowserForward) => {
                self.go_history(1);
                return;
            }
            _ => {}
        }

        if self.url_field.focused {
            self.handle_url_field_key(event, ctrl);
        } else {
            self.handle_page_key(event);
        }
    }

    fn handle_url_field_key(&mut self, event: &KeyEvent, ctrl: bool) {
        match &event.logical_key {
            Key::Named(NamedKey::Enter) => {
                self.submit_url_field();
                return;
            }
            Key::Named(NamedKey::Escape) => {
                self.cancel_url_edit();
                return;
            }
            Key::Named(NamedKey::Backspace) => self.url_field.backspace(),
            Key::Named(NamedKey::Delete) => self.url_field.delete(),
            Key::Named(NamedKey::ArrowLeft) => self.url_field.move_caret(-1),
            Key::Named(NamedKey::ArrowRight) => self.url_field.move_caret(1),
            Key::Named(NamedKey::Home) => self.url_field.home(),
            Key::Named(NamedKey::End) => self.url_field.end(),
            Key::Named(NamedKey::Space) => self.url_field.insert(" "),
            Key::Character(c) if ctrl && c.to_lowercase() == "a" => self.url_field.focus_all(),
            Key::Character(_) if ctrl => {}
            _ => {
                if let Some(text) = event.text.as_ref() {
                    let printable: String = text.chars().filter(|c| !c.is_control()).collect();
                    if !printable.is_empty() {
                        self.url_field.insert(&printable);
                    }
                }
            }
        }
        self.request_redraw();
    }

    fn handle_page_key(&mut self, event: &KeyEvent) {
        let page_h = self.page_area().h;
        let page_step = (page_h - LINE_SCROLL).max(LINE_SCROLL);
        match &event.logical_key {
            Key::Named(NamedKey::ArrowDown) => self.scroll_by(LINE_SCROLL),
            Key::Named(NamedKey::ArrowUp) => self.scroll_by(-LINE_SCROLL),
            Key::Named(NamedKey::PageDown) | Key::Named(NamedKey::Space) => self.scroll_by(page_step),
            Key::Named(NamedKey::PageUp) => self.scroll_by(-page_step),
            Key::Named(NamedKey::Home) => self.set_scroll(0.0),
            Key::Named(NamedKey::End) => self.set_scroll(f32::MAX),
            Key::Named(NamedKey::Backspace) => self.go_history(-1),
            Key::Named(NamedKey::Escape) => {
                self.status.clear();
                self.request_redraw();
            }
            _ => {}
        }
    }

    fn handle_click(&mut self, button: MouseButton) {
        match button {
            MouseButton::Back => {
                self.go_history(-1);
                return;
            }
            MouseButton::Forward => {
                self.go_history(1);
                return;
            }
            MouseButton::Left => {}
            _ => return,
        }
        let (x, y) = (self.cursor.x as f32, self.cursor.y as f32);
        if y < TOOLBAR_H {
            self.handle_toolbar_click(x, y);
            return;
        }
        if self.url_field.focused {
            self.cancel_url_edit();
        }
        let area = self.page_area();
        if area.contains(x, y) {
            let link = self
                .page
                .as_ref()
                .and_then(|p| p.link_at(x, y - area.y + self.scroll_y));
            if let Some(link) = link {
                self.follow_link(link);
            }
        }
    }

    fn handle_toolbar_click(&mut self, x: f32, y: f32) {
        if self.url_field_rect().contains(x, y) {
            self.url_field.focus_all();
            self.request_redraw();
            return;
        }
        if self.url_field.focused {
            self.cancel_url_edit();
        }
        let hit = Button::ALL.iter().copied().find(|b| b.rect().contains(x, y));
        match hit {
            Some(Button::Back) => self.go_history(-1),
            Some(Button::Forward) => self.go_history(1),
            Some(Button::Reload) => self.reload(),
            Some(Button::Home) => self.navigate("about:home", true),
            None => {}
        }
    }

    fn handle_wheel(&mut self, delta: MouseScrollDelta) {
        match delta {
            MouseScrollDelta::LineDelta(_, lines) => self.scroll_by(-lines * LINE_SCROLL),
            MouseScrollDelta::PixelDelta(pos) => self.scroll_by(-pos.y as f32),
        }
    }

    /// Recompute the hovered link, status text and cursor icon.
    fn update_hover(&mut self) {
        let (x, y) = (self.cursor.x as f32, self.cursor.y as f32);
        let area = self.page_area();
        let mut icon = CursorIcon::Default;
        let mut link = None;
        if self.url_field_rect().contains(x, y) {
            icon = CursorIcon::Text;
        } else if area.contains(x, y) {
            link = self
                .page
                .as_ref()
                .and_then(|p| p.link_at(x, y - area.y + self.scroll_y));
            if link.is_some() {
                icon = CursorIcon::Pointer;
            }
        }
        if icon != self.cursor_icon {
            self.cursor_icon = icon;
            if let Some(window) = &self.window {
                window.set_cursor(icon);
            }
        }
        if link != self.hover_link {
            self.hover_link = link;
            self.status = self
                .hover_link
                .as_ref()
                .map(|u| u.to_string())
                .unwrap_or_default();
            self.request_redraw();
        }
    }

    // -------------------------------------------------------------- drawing

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Paint the whole window and present it.
    fn redraw(&mut self) {
        let size = self.window_size();
        let (w, h) = (size.width, size.height);
        let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) else {
            return;
        };
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        if surface.resize(nw, nh).is_err() {
            return;
        }
        if self.canvas.width != w || self.canvas.height != h {
            self.canvas.resize(w, h);
        }
        let area = self.page_area();
        let (pw, ph) = (area.w as u32, area.h as u32);
        if self.page_canvas.width != pw || self.page_canvas.height != ph {
            self.page_canvas.resize(pw, ph);
        }
        self.page_canvas.clear(Color::WHITE);
        if let Some(page) = &self.page {
            self.engine.render(page, &mut self.page_canvas, 0.0, self.scroll_y);
        }
        self.canvas.clear(Color::WHITE);
        self.canvas.blit(&self.page_canvas, 0, TOOLBAR_H as i32);
        self.draw_scrollbar();
        self.draw_toolbar();
        self.draw_status_bar();

        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        if let Ok(mut buffer) = surface.buffer_mut() {
            let n = buffer.len().min(self.canvas.data.len());
            buffer[..n].copy_from_slice(&self.canvas.data[..n]);
            let _ = buffer.present();
        }
    }

    fn text_baseline(&self, rect: Rect) -> f32 {
        let m = self.engine.fonts.metrics(self.ui_font, UI_FONT_SIZE);
        rect.y + (rect.h + m.ascent - m.descent) / 2.0
    }

    fn draw_toolbar(&mut self) {
        let width = self.canvas.width as f32;
        self.canvas.fill_rect(Rect::new(0.0, 0.0, width, TOOLBAR_H), TOOLBAR_BG);
        self.canvas.fill_rect(Rect::new(0.0, TOOLBAR_H - 1.0, width, 1.0), TOOLBAR_BORDER);

        for button in Button::ALL {
            let enabled = match button {
                Button::Back => self.can_go_back(),
                Button::Forward => self.can_go_forward(),
                _ => true,
            };
            self.draw_button(button, enabled);
        }
        self.draw_url_field();

        let label = "RabbitHole";
        let label_w = self.engine.fonts.measure(self.ui_font, UI_FONT_SIZE, label);
        let baseline = self.text_baseline(Rect::new(0.0, BUTTON_Y, 0.0, BUTTON_SIZE));
        self.canvas.draw_text(
            &self.engine.fonts,
            self.ui_font,
            UI_FONT_SIZE,
            label,
            width - label_w - 12.0,
            baseline,
            TEXT_MUTED,
        );
    }

    fn draw_button(&mut self, button: Button, enabled: bool) {
        let rect = button.rect();
        self.canvas.fill_rounded_rect(rect, FIELD_RADIUS, BUTTON_BG);
        let glyph = button.glyph();
        let size = 16.0;
        let w = self.engine.fonts.measure(self.ui_font, size, glyph);
        let m = self.engine.fonts.metrics(self.ui_font, size);
        let x = rect.x + (rect.w - w) / 2.0;
        let baseline = rect.y + (rect.h + m.ascent - m.descent) / 2.0;
        let color = if enabled { BUTTON_FG } else { BUTTON_DISABLED };
        self.canvas
            .draw_text(&self.engine.fonts, self.ui_font, size, glyph, x, baseline, color);
    }

    fn draw_url_field(&mut self) {
        let rect = self.url_field_rect();
        self.canvas.fill_rounded_rect(rect, FIELD_RADIUS, Color::WHITE);
        let border = if self.url_field.focused { FIELD_FOCUS } else { FIELD_BORDER };
        self.canvas
            .stroke_rect(rect, [1.0; 4], [border; 4], [0; 4], FIELD_RADIUS);

        let pad = 10.0;
        let inner = Rect::new(rect.x + pad, rect.y, (rect.w - 2.0 * pad).max(0.0), rect.h);
        let baseline = self.text_baseline(rect);
        let fonts = &self.engine.fonts;
        let text = self.url_field.text.clone();
        let text_w = fonts.measure(self.ui_font, UI_FONT_SIZE, &text);

        // Keep the caret visible by scrolling the text left when needed.
        let caret_byte = self.url_field.byte_index(self.url_field.caret);
        let caret_x = fonts.measure(self.ui_font, UI_FONT_SIZE, &text[..caret_byte]);
        let shift = if self.url_field.focused && caret_x > inner.w {
            caret_x - inner.w
        } else if !self.url_field.focused && text_w > inner.w {
            0.0
        } else {
            0.0
        };

        self.canvas.push_clip(inner);
        if self.url_field.select_all && self.url_field.focused && !text.is_empty() {
            let m = fonts.metrics(self.ui_font, UI_FONT_SIZE);
            let sel = Rect::new(inner.x - shift, baseline - m.ascent, text_w, m.ascent + m.descent);
            self.canvas.fill_rect(sel, Color::rgba(0x3b, 0x82, 0xf6, 70));
        }
        if text.is_empty() && !self.url_field.focused {
            self.canvas.draw_text(
                fonts,
                self.ui_font,
                UI_FONT_SIZE,
                PLACEHOLDER,
                inner.x,
                baseline,
                TEXT_MUTED,
            );
        } else {
            self.canvas.draw_text(
                fonts,
                self.ui_font,
                UI_FONT_SIZE,
                &text,
                inner.x - shift,
                baseline,
                TEXT_FG,
            );
        }
        if self.url_field.focused {
            let m = fonts.metrics(self.ui_font, UI_FONT_SIZE);
            let caret = Rect::new(inner.x - shift + caret_x, baseline - m.ascent, 1.0, m.ascent + m.descent);
            self.canvas.fill_rect(caret, TEXT_FG);
        }
        self.canvas.pop_clip();
    }

    fn draw_status_bar(&mut self) {
        let width = self.canvas.width as f32;
        let y = self.canvas.height as f32 - STATUS_H;
        let rect = Rect::new(0.0, y, width, STATUS_H);
        self.canvas.fill_rect(rect, TOOLBAR_BG);
        self.canvas.fill_rect(Rect::new(0.0, y, width, 1.0), TOOLBAR_BORDER);
        if self.status.is_empty() {
            return;
        }
        let size = 12.0;
        let m = self.engine.fonts.metrics(self.ui_font, size);
        let baseline = y + (STATUS_H + m.ascent - m.descent) / 2.0;
        let status = self.status.clone();
        self.canvas.push_clip(Rect::new(8.0, y, width - 16.0, STATUS_H));
        self.canvas
            .draw_text(&self.engine.fonts, self.ui_font, size, &status, 8.0, baseline, TEXT_FG);
        self.canvas.pop_clip();
    }

    fn draw_scrollbar(&mut self) {
        let area = self.page_area();
        let content = self.page.as_ref().map(|p| p.content_height()).unwrap_or(0.0);
        if content <= area.h {
            return;
        }
        let thumb_h = (area.h * area.h / content).max(20.0).min(area.h);
        let max_scroll = (content - area.h).max(1.0);
        let thumb_y = area.y + (self.scroll_y / max_scroll) * (area.h - thumb_h);
        let x = area.right() - SCROLLBAR_W - 2.0;
        self.canvas
            .fill_rounded_rect(Rect::new(x, thumb_y, SCROLLBAR_W, thumb_h), SCROLLBAR_W / 2.0, SCROLL_THUMB);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = WindowAttributes::default()
            .with_title("RabbitHole")
            .with_inner_size(PhysicalSize::new(1200u32, 800u32));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(e) => {
                eprintln!("rabbithole: could not create window: {e}");
                event_loop.exit();
                return;
            }
        };
        let context = match softbuffer::Context::new(window.clone()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("rabbithole: could not create graphics context: {e}");
                event_loop.exit();
                return;
            }
        };
        match softbuffer::Surface::new(&context, window.clone()) {
            Ok(s) => self.surface = Some(s),
            Err(e) => {
                eprintln!("rabbithole: could not create surface: {e}");
                event_loop.exit();
                return;
            }
        }
        window.set_ime_allowed(true);
        self.window = Some(window);
        if let Some(url) = self.start_url.take() {
            let target = App::resolve_input(&url);
            self.navigate(&target, true);
        }
        self.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) => {
                self.relayout();
                self.request_redraw();
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. } => self.handle_key(&event, event_loop),
            WindowEvent::Ime(Ime::Commit(text)) => {
                if self.url_field.focused {
                    self.url_field.insert(&text);
                    self.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = position;
                self.update_hover();
            }
            WindowEvent::CursorLeft { .. } => {
                if self.hover_link.take().is_some() {
                    self.status.clear();
                    self.request_redraw();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => self.handle_click(button),
            WindowEvent::MouseWheel { delta, .. } => self.handle_wheel(delta),
            _ => {}
        }
    }
}

// ------------------------------------------------------------------ helpers

fn about_blank() -> Url {
    Url {
        scheme: "about".to_string(),
        host: String::new(),
        port: None,
        path: "blank".to_string(),
        query: None,
        fragment: None,
    }
}

/// Heuristic: does the typed text look like a URL rather than a search?
fn looks_like_url(s: &str) -> bool {
    if s.contains(char::is_whitespace) {
        return false;
    }
    if s.contains("://") {
        return true;
    }
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("about:") || lower.starts_with("file:") || lower.starts_with("data:") || lower.starts_with("mailto:") {
        return true;
    }
    if lower == "localhost" || lower.starts_with("localhost:") || lower.starts_with("localhost/") {
        return true;
    }
    let host = lower.split(['/', '?', '#', ':']).next().unwrap_or("");
    if host.contains('.') && !host.starts_with('.') && !host.ends_with('.') {
        let tld = host.rsplit('.').next().unwrap_or("");
        return tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphanumeric());
    }
    false
}

/// Percent-encode a query string (RFC 3986 unreserved set, space as `+`).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn error_page(url: &str, error: &str) -> String {
    format!(
        "<!doctype html><html><head><title>Could not load page</title>\
         <style>body{{font-family:sans-serif;margin:48px;color:#222}}h1{{font-size:22px}}\
         code{{background:#f3f3f3;padding:2px 6px;border-radius:4px}}p{{line-height:1.5}}</style></head>\
         <body><h1>Could not load this page</h1>\
         <p>RabbitHole was unable to load <code>{}</code>.</p><p>{}</p>\
         <p><a href=\"about:home\">Go home</a></p></body></html>",
        escape_html(url),
        escape_html(error)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_detection() {
        assert!(looks_like_url("example.com"));
        assert!(looks_like_url("https://example.com/x?y"));
        assert!(looks_like_url("about:home"));
        assert!(looks_like_url("localhost:8080"));
        assert!(looks_like_url("sub.example.co.uk/path"));
        assert!(!looks_like_url("rust browser engine"));
        assert!(!looks_like_url("hello"));
        assert!(!looks_like_url("what is 2.5"));
    }

    #[test]
    fn search_fallback() {
        assert_eq!(App::resolve_input("  example.org "), "example.org");
        assert_eq!(
            App::resolve_input("rust & ünicode"),
            "https://duckduckgo.com/html/?q=rust+%26+%C3%BCnicode"
        );
        assert_eq!(App::resolve_input(""), "about:home");
    }

    #[test]
    fn url_field_editing() {
        let mut f = UrlField::default();
        f.set_text("héllo");
        assert_eq!(f.caret, 5);
        f.backspace();
        assert_eq!(f.text, "héll");
        f.home();
        f.delete();
        assert_eq!(f.text, "éll");
        f.move_caret(1);
        f.insert("X");
        assert_eq!(f.text, "éXll");
        f.focus_all();
        f.insert("new");
        assert_eq!(f.text, "new");
        assert_eq!(f.caret, 3);
        f.focus_all();
        f.backspace();
        assert_eq!(f.text, "");
    }
}
