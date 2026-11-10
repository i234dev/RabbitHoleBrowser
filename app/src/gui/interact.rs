//! Mouse and keyboard handling on the UI thread. Chrome input is handled
//! here; page input is forwarded to the tab's content thread.

use std::time::{Duration, Instant};

use rabbithole_engine::dom::NodeId;
use rabbithole_engine::gfx::Rect;
use rabbithole_engine::net::Url;
use rabbithole_engine::Modifiers;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::CursorIcon;

use super::chrome::{Action, Menu, MenuItem, ToolButton};
use super::content::{KeyInput, ToContent};
use super::{App, MouseDown, UiFocus};

const LINE_SCROLL: f32 = 40.0;

fn button_code(button: MouseButton) -> u8 {
    match button {
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        _ => 0,
    }
}

impl App {
    // ---------------------------------------------------------------- mouse

    pub(crate) fn handle_cursor_moved(&mut self) {
        let (x, y) = (self.cursor.x as f32, self.cursor.y as f32);
        // Popup hover.
        let mut popup_hover_changed = false;
        for menu in [&mut self.menu, &mut self.context_menu, &mut self.dropdown].into_iter().flatten() {
            let h = menu.item_at(x, y);
            if h != menu.hover {
                menu.hover = h;
                popup_hover_changed = true;
            }
        }
        if popup_hover_changed {
            self.request_redraw();
        }
        // A press on the page: the page follows the pointer (drag selection).
        if self.mouse_down.as_ref().map(|m| m.on_page).unwrap_or(false) {
            let css = self.to_css_clamped(self.cursor);
            let mods = self.pointer_modifiers();
            self.send_active(ToContent::PointerMove { x: css.0, y: css.1, mods });
            return;
        }
        self.update_hover();
    }

    /// The cursor shape for the chrome under the pointer.
    pub(crate) fn chrome_cursor(&self) -> CursorIcon {
        let (x, y) = (self.cursor.x as f32, self.cursor.y as f32);
        let chrome = self.chrome();
        let over_button = [ToolButton::Back, ToolButton::Forward, ToolButton::Reload, ToolButton::Home, ToolButton::Star, ToolButton::Menu].into_iter().any(|b| chrome.button_rect(b).contains(x, y));
        let (tab_rects, plus) = chrome.tab_rects(self.tabs.len());
        let over_tab = tab_rects.iter().any(|r| r.contains(x, y)) || plus.contains(x, y);
        let over_hang_button = self.hang_bar().map(|(_, w, s)| w.contains(x, y) || s.contains(x, y)).unwrap_or(false);
        if chrome.url_field().contains(x, y) || (chrome.find_open && chrome.find_field().contains(x, y)) {
            CursorIcon::Text
        } else if over_button || over_tab || over_hang_button {
            CursorIcon::Pointer
        } else {
            CursorIcon::Default
        }
    }

    /// Recompute hover state: toolbar buttons, tabs; the page hover is
    /// computed on the content thread (its answer sets the cursor).
    pub(crate) fn update_hover(&mut self) {
        let (x, y) = (self.cursor.x as f32, self.cursor.y as f32);
        let chrome = self.chrome();
        let mut redraw = false;
        let button = [ToolButton::Back, ToolButton::Forward, ToolButton::Reload, ToolButton::Home, ToolButton::Star, ToolButton::Menu]
            .into_iter()
            .find(|b| chrome.button_rect(*b).contains(x, y));
        if button != self.hover_button {
            self.hover_button = button;
            redraw = true;
        }
        let (tab_rects, plus) = chrome.tab_rects(self.tabs.len());
        let hover_tab = tab_rects.iter().position(|r| r.contains(x, y));
        let hover_close = hover_tab.map(|i| chrome.tab_close_rect(tab_rects[i]).contains(x, y)).unwrap_or(false);
        let hover_plus = plus.contains(x, y);
        if hover_tab != self.hover_tab || hover_close != self.hover_tab_close || hover_plus != self.hover_plus {
            self.hover_tab = hover_tab;
            self.hover_tab_close = hover_close;
            self.hover_plus = hover_plus;
            redraw = true;
        }
        let popup_open = self.menu.is_some() || self.context_menu.is_some() || self.dropdown.is_some();
        match self.to_css(self.cursor) {
            Some(css) if !popup_open => {
                let mods = self.pointer_modifiers();
                self.send_active(ToContent::PointerMove { x: css.0, y: css.1, mods });
            }
            _ => {
                if let Some(tab) = self.tab_mut() {
                    if tab.hover_link.take().is_some() {
                        redraw = true;
                    }
                }
            }
        }
        self.apply_cursor();
        if redraw {
            self.request_redraw();
        }
    }

    pub(crate) fn handle_mouse_down(&mut self, button: MouseButton, event_loop: &ActiveEventLoop) {
        let (x, y) = (self.cursor.x as f32, self.cursor.y as f32);
        let chrome = self.chrome();

        // Popups take precedence.
        if let Some(menu) = self.dropdown.take() {
            if let Some(i) = menu.item_at(x, y) {
                let action = menu.items[i].action.clone();
                if menu.items[i].enabled {
                    self.perform(action, event_loop);
                }
            }
            self.request_redraw();
            return;
        }
        for which in 0..2 {
            let menu = if which == 0 { self.menu.take() } else { self.context_menu.take() };
            if let Some(menu) = menu {
                if let Some(i) = menu.item_at(x, y) {
                    if menu.items[i].enabled {
                        let action = menu.items[i].action.clone();
                        self.perform(action, event_loop);
                    }
                }
                self.request_redraw();
                return;
            }
        }
        if !self.suggestions.is_empty() {
            if let Some(i) = self.suggestion_at(x, y) {
                let url = self.suggestions[i].0.clone();
                self.close_popups();
                self.ui_focus = UiFocus::Page;
                let idx = self.active;
                self.navigate_tab(idx, &url, true);
                return;
            }
        }

        match button {
            MouseButton::Back => {
                self.go_history(-1);
                return;
            }
            MouseButton::Forward => {
                self.go_history(1);
                return;
            }
            _ => {}
        }

        // Tab bar.
        if chrome.tab_bar().contains(x, y) {
            let (rects, plus) = chrome.tab_rects(self.tabs.len());
            if plus.contains(x, y) && button == MouseButton::Left {
                self.new_tab(None, false);
                return;
            }
            if let Some(i) = rects.iter().position(|r| r.contains(x, y)) {
                if button == MouseButton::Middle || (button == MouseButton::Left && chrome.tab_close_rect(rects[i]).contains(x, y)) {
                    self.close_tab(i, event_loop);
                } else if button == MouseButton::Left {
                    self.activate_tab(i);
                }
            }
            return;
        }

        // Toolbar.
        if chrome.toolbar().contains(x, y) {
            if button != MouseButton::Left {
                return;
            }
            if chrome.url_field().contains(x, y) {
                if self.ui_focus != UiFocus::Url {
                    self.focus_url_field();
                } else {
                    let (inner_x, shift) = self.url_text_origin();
                    let fonts = &self.engine.fonts;
                    let size = super::chrome::UI_FONT * chrome.s;
                    let font = self.ui_font;
                    let idx = super::input::index_for_x(&self.url_field.text, x - inner_x + shift, |t| fonts.measure(font, size, t));
                    self.url_field.set_caret(idx, self.modifiers.shift_key());
                }
                self.request_redraw();
                return;
            }
            self.blur_url_field();
            let hit = [ToolButton::Back, ToolButton::Forward, ToolButton::Reload, ToolButton::Home, ToolButton::Star, ToolButton::Menu]
                .into_iter()
                .find(|b| chrome.button_rect(*b).contains(x, y));
            match hit {
                Some(ToolButton::Back) => self.go_history(-1),
                Some(ToolButton::Forward) => self.go_history(1),
                Some(ToolButton::Reload) => {
                    if self.tab().map(|t| t.is_loading()).unwrap_or(false) {
                        self.stop_loading();
                    } else {
                        self.reload(self.modifiers.shift_key());
                    }
                }
                Some(ToolButton::Home) => {
                    let idx = self.active;
                    self.navigate_tab(idx, "about:home", true);
                }
                Some(ToolButton::Star) => self.toggle_bookmark(),
                Some(ToolButton::Menu) => self.open_main_menu(),
                None => {}
            }
            return;
        }

        // Find bar.
        if chrome.find_open && chrome.find_bar().contains(x, y) {
            if button != MouseButton::Left {
                return;
            }
            if chrome.find_field().contains(x, y) {
                self.ui_focus = UiFocus::Find;
            } else if chrome.find_button(0).contains(x, y) {
                self.find_step(-1);
            } else if chrome.find_button(1).contains(x, y) {
                self.find_step(1);
            } else if chrome.find_close().contains(x, y) {
                self.close_find();
            }
            self.request_redraw();
            return;
        }

        // Console pane.
        if chrome.console_open && chrome.console_pane().contains(x, y) {
            self.blur_url_field();
            self.ui_focus = UiFocus::Console;
            self.request_redraw();
            return;
        }

        // "Not responding" bar.
        if let Some((bar, wait, stop)) = self.hang_bar() {
            if bar.contains(x, y) {
                if button == MouseButton::Left {
                    if wait.contains(x, y) {
                        self.hang_wait();
                    } else if stop.contains(x, y) {
                        self.hang_stop();
                    }
                }
                return;
            }
        }

        // Page.
        self.blur_url_field();
        if matches!(self.ui_focus, UiFocus::Console | UiFocus::Find) {
            self.ui_focus = UiFocus::Page;
        }
        let Some(css) = self.to_css(self.cursor) else { return };
        if !matches!(button, MouseButton::Left | MouseButton::Middle | MouseButton::Right) {
            return;
        }
        // Double click → word selection (click + dblclick on release).
        let mut click_count = 1;
        if button == MouseButton::Left {
            let now = Instant::now();
            let double = self
                .last_click
                .map(|(t, p)| now.duration_since(t) < Duration::from_millis(400) && (p.x - self.cursor.x).abs() < 5.0 && (p.y - self.cursor.y).abs() < 5.0)
                .unwrap_or(false);
            self.last_click = Some((now, self.cursor));
            if double {
                click_count = 2;
            }
        }
        if button == MouseButton::Right {
            self.context_pos = (x, y);
        }
        let mods = self.pointer_modifiers();
        self.send_active(ToContent::PointerDown { x: css.0, y: css.1, button: button_code(button), mods, click_count });
        if button != MouseButton::Right {
            self.mouse_down = Some(MouseDown { button, pos: self.cursor, on_page: true });
        }
    }

    /// Keyboard modifiers in the engine's form.
    pub(crate) fn pointer_modifiers(&self) -> Modifiers {
        let m = self.modifiers;
        Modifiers { ctrl: m.control_key(), shift: m.shift_key(), alt: m.alt_key(), meta: m.super_key() }
    }

    pub(crate) fn handle_mouse_up(&mut self, button: MouseButton) {
        let Some(md) = self.mouse_down.take() else { return };
        if md.button != button || !md.on_page {
            return;
        }
        let inside = self.to_css(self.cursor).is_some();
        let css = self.to_css_clamped(self.cursor);
        let mods = self.pointer_modifiers();
        let _ = md.pos;
        self.send_active(ToContent::PointerUp { x: css.0, y: css.1, button: button_code(button), mods, inside });
    }

    pub(crate) fn handle_wheel(&mut self, delta: MouseScrollDelta) {
        let (x, y) = (self.cursor.x as f32, self.cursor.y as f32);
        let (dx, dy) = match delta {
            MouseScrollDelta::LineDelta(h, v) => (-h * LINE_SCROLL, -v * LINE_SCROLL),
            MouseScrollDelta::PixelDelta(p) => (-(p.x as f32) / self.page_scale(), -(p.y as f32) / self.page_scale()),
        };
        if let Some(menu) = self.dropdown.as_mut().or(self.menu.as_mut()).or(self.context_menu.as_mut()) {
            if menu.contains(x, y) {
                menu.scroll(if dy > 0.0 { 3 } else { -3 });
                self.request_redraw();
                return;
            }
        }
        let chrome = self.chrome();
        if chrome.console_open && chrome.console_pane().contains(x, y) {
            self.scroll_console(dy);
            return;
        }
        if self.modifiers.control_key() {
            let z = self.tab().map(|t| t.zoom).unwrap_or(1.0);
            self.set_zoom(z + if dy < 0.0 { 0.1 } else { -0.1 });
            return;
        }
        if let Some(css) = self.to_css(self.cursor) {
            // Inner scroll containers scroll on the content thread; the
            // page itself scrolls right here.
            let over_container = self.tab().and_then(|t| t.frame.as_ref().map(|f| f.scroll_container_at(css, t.scroll))).unwrap_or(false);
            if over_container {
                self.send_active(ToContent::Wheel { x: css.0, y: css.1, dx, dy });
                return;
            }
        }
        self.scroll_by(dx, dy);
    }

    /// Origin x and horizontal shift of the URL field text (for hit testing).
    fn url_text_origin(&self) -> (f32, f32) {
        let chrome = self.chrome();
        let r = chrome.url_field();
        let inner_x = r.x + 10.0 * chrome.s;
        let inner_w = r.w - 20.0 * chrome.s;
        let size = super::chrome::UI_FONT * chrome.s;
        let caret_x = self.engine.fonts.measure(self.ui_font, size, &self.url_field.text[..self.url_field.caret.min(self.url_field.text.len())]);
        let shift = if caret_x > inner_w { caret_x - inner_w } else { 0.0 };
        (inner_x, shift)
    }

    fn blur_url_field(&mut self) {
        if self.ui_focus == UiFocus::Url {
            self.ui_focus = UiFocus::Page;
            self.url_edited = false;
            self.sync_url_field();
            self.suggestions.clear();
            self.suggestion_sel = None;
            self.request_redraw();
        }
    }

    pub(crate) fn suggestion_rects(&self) -> Vec<Rect> {
        let chrome = self.chrome();
        let field = chrome.url_field();
        let h = 30.0 * chrome.s;
        (0..self.suggestions.len()).map(|i| Rect::new(field.x, field.bottom() + 4.0 * chrome.s + i as f32 * h, field.w, h)).collect()
    }

    fn suggestion_at(&self, x: f32, y: f32) -> Option<usize> {
        self.suggestion_rects().iter().position(|r| r.contains(x, y))
    }

    fn open_main_menu(&mut self) {
        let chrome = self.chrome();
        let r = chrome.button_rect(ToolButton::Menu);
        let zoom = self.tab().map(|t| t.zoom).unwrap_or(1.0);
        let items = vec![
            MenuItem::new("New tab            Ctrl+T", Action::NewTab),
            MenuItem::new("Close tab          Ctrl+W", Action::CloseTab),
            MenuItem::new("Home page", Action::Home),
            MenuItem::new("Bookmarks    Ctrl+Shift+O", Action::Bookmarks).sep(),
            MenuItem::new("History            Ctrl+H", Action::History),
            MenuItem::new("Downloads          Ctrl+J", Action::Downloads),
            MenuItem::new(&format!("Zoom in   ({}%)     Ctrl++", (zoom * 100.0).round()), Action::ZoomIn).sep(),
            MenuItem::new("Zoom out               Ctrl+-", Action::ZoomOut),
            MenuItem::new("Reset zoom             Ctrl+0", Action::ZoomReset),
            MenuItem::new("Find in page       Ctrl+F", Action::Find).sep(),
            MenuItem::new("Page text", Action::ViewText),
            MenuItem::new("JavaScript console  Ctrl+Shift+J", Action::ToggleConsole).sep(),
            MenuItem::new(if self.settings.javascript { "Disable JavaScript" } else { "Enable JavaScript" }, Action::ToggleJavaScript),
            MenuItem::new(if self.settings.block_trackers { "Allow ads & trackers" } else { "Block ads & trackers" }, Action::ToggleTrackerBlocking),
            MenuItem::new("Clear cookies", Action::ClearCookies).sep(),
            MenuItem::new("About RabbitHole", Action::About),
            MenuItem::new("Quit               Ctrl+Q", Action::Quit).sep(),
        ];
        let win = self.window_size();
        let menu = Menu::new(r.right() - 240.0 * chrome.s, r.bottom() + 4.0 * chrome.s, items, chrome.s, &self.engine.fonts, self.ui_font, win.width as f32, win.height as f32);
        self.menu = Some(menu);
        self.request_redraw();
    }

    pub(crate) fn open_context_menu(&mut self, link: Option<Url>, has_sel: bool, x: f32, y: f32) {
        let chrome = self.chrome();
        let (back, fwd) = self.tab().map(|t| (t.can_go_back(), t.can_go_forward())).unwrap_or((false, false));
        let mut items = Vec::new();
        if let Some(l) = &link {
            items.push(MenuItem::new("Open link in new tab", Action::OpenInNewTab(l.clone())));
            items.push(MenuItem::new("Copy link address", Action::CopyLink(l.clone())));
        }
        if has_sel {
            let mut it = MenuItem::new("Copy", Action::CopySelection);
            if link.is_some() {
                it = it.sep();
            }
            items.push(it);
        }
        let mut back_item = MenuItem::new("Back", Action::Back).disabled(!back);
        if !items.is_empty() {
            back_item = back_item.sep();
        }
        items.push(back_item);
        items.push(MenuItem::new("Forward", Action::Forward).disabled(!fwd));
        items.push(MenuItem::new("Reload", Action::Reload));
        items.push(MenuItem::new("Copy page URL", Action::CopyPageUrl).sep());
        items.push(MenuItem::new("View page text", Action::ViewText));
        items.push(MenuItem::new("Bookmark this page", Action::ToggleBookmark));
        let win = self.window_size();
        self.context_menu = Some(Menu::new(x, y, items, chrome.s, &self.engine.fonts, self.ui_font, win.width as f32, win.height as f32));
        self.apply_cursor();
        self.request_redraw();
    }

    /// Show a `<select>` popup for the page (`rect` in document px).
    pub(crate) fn open_select_dropdown(&mut self, select: NodeId, options: Vec<(String, bool, bool)>, rect: Rect) {
        let chrome = self.chrome();
        let rect = if rect.w > 0.0 || rect.h > 0.0 { self.doc_to_screen(rect) } else { Rect::new(self.cursor.x as f32, self.cursor.y as f32, 0.0, 0.0) };
        let items: Vec<MenuItem> = options
            .iter()
            .enumerate()
            .map(|(i, (label, selected, disabled))| {
                let text = if *selected { format!("✓ {label}") } else { format!("   {label}") };
                MenuItem::new(&text, Action::SelectOption(select, i)).disabled(*disabled)
            })
            .collect();
        if items.is_empty() {
            return;
        }
        let win = self.window_size();
        let mut menu = Menu::new(rect.x, rect.bottom(), items, chrome.s, &self.engine.fonts, self.ui_font, win.width as f32, win.height as f32);
        menu.rect.w = menu.rect.w.max(rect.w);
        self.dropdown = Some(menu);
        self.apply_cursor();
        self.request_redraw();
    }

    /// Execute a menu action.
    pub(crate) fn perform(&mut self, action: Action, event_loop: &ActiveEventLoop) {
        let idx = self.active;
        match action {
            Action::NewTab => {
                self.new_tab(None, false);
            }
            Action::CloseTab => self.close_tab(idx, event_loop),
            Action::Back => self.go_history(-1),
            Action::Forward => self.go_history(1),
            Action::Reload => self.reload(false),
            Action::Home => self.navigate_tab(idx, "about:home", true),
            Action::Bookmarks => self.navigate_tab(idx, "about:bookmarks", true),
            Action::History => self.navigate_tab(idx, "about:history", true),
            Action::Downloads => self.navigate_tab(idx, "about:downloads", true),
            Action::About => self.navigate_tab(idx, "about:version", true),
            Action::ZoomIn => self.set_zoom(self.tab().map(|t| t.zoom).unwrap_or(1.0) + 0.1),
            Action::ZoomOut => self.set_zoom(self.tab().map(|t| t.zoom).unwrap_or(1.0) - 0.1),
            Action::ZoomReset => self.set_zoom(1.0),
            Action::Find => self.open_find(),
            Action::ClearCookies => {
                if let Ok(mut jar) = self.cookies.lock() {
                    jar.clear();
                }
                self.profile.save_cookies(&self.cookies);
                self.set_status("Cookies cleared");
            }
            Action::Quit => self.shutdown(event_loop),
            Action::OpenInNewTab(u) => self.follow_link(u, true),
            Action::CopyLink(u) => self.clipboard_set(&u.to_string()),
            Action::CopySelection => self.send_active(ToContent::CopySelection),
            Action::ViewText => self.navigate_tab(idx, "about:text", true),
            Action::CopyPageUrl => {
                if let Some(u) = self.tab().and_then(|t| t.current_url.as_ref()).map(|u| u.to_string()) {
                    self.clipboard_set(&u);
                }
            }
            Action::SelectOption(node, index) => self.send_active(ToContent::SelectOption { node, index }),
            Action::ToggleBookmark => self.toggle_bookmark(),
            Action::ToggleJavaScript => self.toggle_javascript(),
            Action::ToggleTrackerBlocking => self.toggle_tracker_blocking(),
            Action::ToggleConsole => self.toggle_console(),
            Action::ClearConsole => {
                if let Some(tab) = self.tabs.get_mut(idx) {
                    tab.console.clear();
                    tab.console_scroll = 0;
                }
                self.request_redraw();
            }
        }
    }

    // ------------------------------------------------------------- keyboard

    /// The key event in the content thread's form.
    fn key_input(&mut self, event: &KeyEvent) -> KeyInput {
        let m = self.modifiers;
        let ctrl = m.control_key() || m.super_key();
        let key = key_name(&event.logical_key);
        // A paste reads the clipboard here (it belongs to the UI thread).
        let clipboard = if ctrl && event.state == ElementState::Pressed && key.eq_ignore_ascii_case("v") { self.clipboard_get() } else { None };
        KeyInput {
            key,
            code: format!("{:?}", event.physical_key).trim_start_matches("Code(").trim_end_matches(')').to_string(),
            text: event.text.as_ref().map(|t| t.to_string()),
            mods: self.pointer_modifiers(),
            repeat: event.repeat,
            pressed: event.state == ElementState::Pressed,
            clipboard,
        }
    }

    /// Scroll keys handled on the UI thread when the page cannot react
    /// to them (no key listeners, nothing focused): instant even while
    /// the page is busy.
    fn ui_scroll_key(&mut self, event: &KeyEvent) -> bool {
        let Some(tab) = self.tab() else { return false };
        let st = tab.state;
        let m = self.modifiers;
        if st.editing || st.key_listeners || st.focused || m.control_key() || m.alt_key() || m.super_key() {
            return false;
        }
        let vp = self.css_viewport();
        let page_step = (vp.height - LINE_SCROLL).max(LINE_SCROLL);
        let shift = m.shift_key();
        match &event.logical_key {
            Key::Named(NamedKey::ArrowDown) => self.scroll_by(0.0, LINE_SCROLL),
            Key::Named(NamedKey::ArrowUp) => self.scroll_by(0.0, -LINE_SCROLL),
            Key::Named(NamedKey::ArrowLeft) => self.scroll_by(-LINE_SCROLL, 0.0),
            Key::Named(NamedKey::ArrowRight) => self.scroll_by(LINE_SCROLL, 0.0),
            Key::Named(NamedKey::PageDown) => self.scroll_by(0.0, page_step),
            Key::Named(NamedKey::PageUp) => self.scroll_by(0.0, -page_step),
            Key::Named(NamedKey::Space) => self.scroll_by(0.0, if shift { -page_step } else { page_step }),
            Key::Named(NamedKey::Home) => self.set_scroll(0.0, 0.0),
            Key::Named(NamedKey::End) => self.set_scroll(0.0, f32::MAX),
            _ => return false,
        }
        true
    }

    pub(crate) fn handle_key(&mut self, event: &KeyEvent, event_loop: &ActiveEventLoop) {
        let page_focus = self.ui_focus == UiFocus::Page && self.menu.is_none() && self.context_menu.is_none() && self.dropdown.is_none();
        if event.state != ElementState::Pressed {
            if page_focus {
                let k = self.key_input(event);
                self.send_active(ToContent::Key(k));
            }
            return;
        }
        let ctrl = self.modifiers.control_key() || self.modifiers.super_key();
        let alt = self.modifiers.alt_key();
        let shift = self.modifiers.shift_key();
        let idx = self.active;

        // Popups: Esc closes; arrows navigate menus.
        if self.menu.is_some() || self.context_menu.is_some() || self.dropdown.is_some() {
            match &event.logical_key {
                Key::Named(NamedKey::Escape) => {
                    self.close_popups();
                    self.request_redraw();
                }
                Key::Named(NamedKey::ArrowDown) | Key::Named(NamedKey::ArrowUp) => {
                    let down = matches!(event.logical_key, Key::Named(NamedKey::ArrowDown));
                    if let Some(menu) = self.dropdown.as_mut().or(self.menu.as_mut()).or(self.context_menu.as_mut()) {
                        let n = menu.items.len();
                        if n > 0 {
                            let cur = menu.hover.unwrap_or(if down { n - 1 } else { 0 });
                            menu.hover = Some(if down { (cur + 1) % n } else { (cur + n - 1) % n });
                        }
                    }
                    self.request_redraw();
                }
                Key::Named(NamedKey::Enter) => {
                    let action = self
                        .dropdown
                        .as_ref()
                        .or(self.menu.as_ref())
                        .or(self.context_menu.as_ref())
                        .and_then(|m| m.hover.and_then(|h| m.items.get(h)).filter(|i| i.enabled).map(|i| i.action.clone()));
                    self.close_popups();
                    if let Some(a) = action {
                        self.perform(a, event_loop);
                    }
                    self.request_redraw();
                }
                _ => {}
            }
            return;
        }

        // Browser shortcuts (handled here, never by the page).
        if ctrl {
            if let Key::Character(c) = &event.logical_key {
                match c.to_lowercase().as_str() {
                    "q" => return self.shutdown(event_loop),
                    "w" => return self.close_tab(idx, event_loop),
                    "t" => {
                        self.new_tab(None, false);
                        return;
                    }
                    "l" if self.ui_focus != UiFocus::Console => return self.focus_url_field(),
                    "r" => return self.reload(shift),
                    "f" => return self.open_find(),
                    "d" => return self.toggle_bookmark(),
                    "h" => return self.navigate_tab(idx, "about:history", true),
                    "j" if shift => return self.toggle_console(),
                    "j" => return self.navigate_tab(idx, "about:downloads", true),
                    "o" if shift => return self.navigate_tab(idx, "about:bookmarks", true),
                    "u" if shift => {
                        self.show_stats = !self.show_stats;
                        self.request_redraw();
                        return;
                    }
                    "+" | "=" => return self.set_zoom(self.tab().map(|t| t.zoom).unwrap_or(1.0) + 0.1),
                    "-" | "_" => return self.set_zoom(self.tab().map(|t| t.zoom).unwrap_or(1.0) - 0.1),
                    "0" => return self.set_zoom(1.0),
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" if !shift => {
                        let n: usize = c.parse().unwrap_or(1);
                        let target = if n == 9 { self.tabs.len().saturating_sub(1) } else { n - 1 };
                        if target < self.tabs.len() {
                            self.activate_tab(target);
                        }
                        return;
                    }
                    _ => {}
                }
            }
            if let Key::Named(NamedKey::Tab) = &event.logical_key {
                if !self.tabs.is_empty() {
                    let n = self.tabs.len();
                    let next = if shift { (idx + n - 1) % n } else { (idx + 1) % n };
                    self.activate_tab(next);
                }
                return;
            }
        }
        match &event.logical_key {
            Key::Named(NamedKey::F5) => return self.reload(ctrl || shift),
            Key::Named(NamedKey::F6) => return self.focus_url_field(),
            Key::Named(NamedKey::F11) => return self.toggle_fullscreen(),
            Key::Named(NamedKey::F3) => {
                if self.tab().map(|t| t.find.open).unwrap_or(false) {
                    self.find_step(if shift { -1 } else { 1 });
                } else {
                    self.open_find();
                }
                return;
            }
            Key::Named(NamedKey::ArrowLeft) if alt => return self.go_history(-1),
            Key::Named(NamedKey::ArrowRight) if alt => return self.go_history(1),
            Key::Named(NamedKey::BrowserBack) => return self.go_history(-1),
            Key::Named(NamedKey::BrowserForward) => return self.go_history(1),
            Key::Character(c) if alt && c.to_lowercase() == "d" => return self.focus_url_field(),
            _ => {}
        }

        match self.ui_focus {
            UiFocus::Url => self.handle_url_key(event, ctrl, shift),
            UiFocus::Find => self.handle_find_key(event, ctrl, shift),
            UiFocus::Console => self.handle_console_key(event, ctrl, shift),
            UiFocus::Page => {
                if matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
                    self.status = None;
                    if self.tab().map(|t| t.is_loading()).unwrap_or(false) {
                        self.stop_loading();
                        return;
                    }
                }
                if self.ui_scroll_key(event) {
                    return;
                }
                let k = self.key_input(event);
                self.send_active(ToContent::Key(k));
            }
        }
    }

    /// IME / text commit events.
    pub(crate) fn handle_text_input(&mut self, text: &str) {
        match self.ui_focus {
            UiFocus::Url => {
                self.url_field.insert(text, None);
                self.on_url_text_changed();
            }
            UiFocus::Find => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.find.field.insert(text, None);
                }
                self.refresh_find();
            }
            UiFocus::Console => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.console_input.insert(text, None);
                }
                self.request_redraw();
            }
            UiFocus::Page => self.send_active(ToContent::Text(text.to_string())),
        }
        self.request_redraw();
    }

    fn on_url_text_changed(&mut self) {
        self.url_edited = true;
        let q = self.url_field.text.trim().to_string();
        self.suggestions.clear();
        self.suggestion_sel = None;
        if q.len() >= 2 {
            let mut seen = std::collections::HashSet::new();
            for b in self.bookmarks.search(&q, 4) {
                if seen.insert(b.url.clone()) {
                    self.suggestions.push((b.url.clone(), b.title.clone()));
                }
            }
            for h in self.history.search(&q, 12) {
                if self.suggestions.len() >= 8 {
                    break;
                }
                if seen.insert(h.url.clone()) {
                    self.suggestions.push((h.url.clone(), h.title.clone()));
                }
            }
        }
        self.request_redraw();
    }

    fn handle_url_key(&mut self, event: &KeyEvent, ctrl: bool, shift: bool) {
        match &event.logical_key {
            Key::Named(NamedKey::Enter) => return self.submit_url_field(),
            Key::Named(NamedKey::Escape) => {
                self.ui_focus = UiFocus::Page;
                self.url_edited = false;
                self.close_popups();
                self.sync_url_field();
            }
            Key::Named(NamedKey::ArrowDown) if !self.suggestions.is_empty() => {
                let n = self.suggestions.len();
                self.suggestion_sel = Some(self.suggestion_sel.map(|i| (i + 1) % n).unwrap_or(0));
            }
            Key::Named(NamedKey::ArrowUp) if !self.suggestions.is_empty() => {
                let n = self.suggestions.len();
                self.suggestion_sel = Some(self.suggestion_sel.map(|i| (i + n - 1) % n).unwrap_or(n - 1));
            }
            Key::Named(NamedKey::Backspace) => {
                self.url_field.backspace(ctrl);
                self.on_url_text_changed();
            }
            Key::Named(NamedKey::Delete) => {
                self.url_field.delete(ctrl);
                self.on_url_text_changed();
            }
            Key::Named(NamedKey::ArrowLeft) => self.url_field.move_left(shift, ctrl),
            Key::Named(NamedKey::ArrowRight) => self.url_field.move_right(shift, ctrl),
            Key::Named(NamedKey::Home) => self.url_field.home(shift),
            Key::Named(NamedKey::End) => self.url_field.end(shift),
            Key::Named(NamedKey::Space) => {
                self.url_field.insert(" ", None);
                self.on_url_text_changed();
            }
            Key::Character(c) if ctrl => match c.to_lowercase().as_str() {
                "a" => self.url_field.select_all(),
                "c" => {
                    if let Some(t) = self.url_field.selected_text().map(|s| s.to_string()).or_else(|| Some(self.url_field.text.clone())) {
                        self.clipboard_set(&t);
                    }
                }
                "x" => {
                    if let Some(t) = self.url_field.selected_text().map(|s| s.to_string()) {
                        self.clipboard_set(&t);
                        self.url_field.delete_selection();
                        self.on_url_text_changed();
                    }
                }
                "v" => {
                    if let Some(t) = self.clipboard_get() {
                        self.url_field.insert(t.trim(), None);
                        self.on_url_text_changed();
                    }
                }
                _ => {}
            },
            _ => {
                if let Some(text) = event.text.as_ref() {
                    if !ctrl {
                        self.url_field.insert(text, None);
                        self.on_url_text_changed();
                    }
                }
            }
        }
        self.request_redraw();
    }

    fn handle_find_key(&mut self, event: &KeyEvent, ctrl: bool, shift: bool) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let mut changed = false;
        match &event.logical_key {
            Key::Named(NamedKey::Enter) => {
                self.find_step(if shift { -1 } else { 1 });
                return;
            }
            Key::Named(NamedKey::Escape) => return self.close_find(),
            Key::Named(NamedKey::Backspace) => {
                tab.find.field.backspace(ctrl);
                changed = true;
            }
            Key::Named(NamedKey::Delete) => {
                tab.find.field.delete(ctrl);
                changed = true;
            }
            Key::Named(NamedKey::ArrowLeft) => tab.find.field.move_left(shift, ctrl),
            Key::Named(NamedKey::ArrowRight) => tab.find.field.move_right(shift, ctrl),
            Key::Named(NamedKey::Home) => tab.find.field.home(shift),
            Key::Named(NamedKey::End) => tab.find.field.end(shift),
            Key::Named(NamedKey::Space) => {
                tab.find.field.insert(" ", None);
                changed = true;
            }
            Key::Character(c) if ctrl => match c.to_lowercase().as_str() {
                "a" => tab.find.field.select_all(),
                "v" => {
                    if let Some(t) = self.clipboard_get() {
                        if let Some(tab) = self.tabs.get_mut(self.active) {
                            tab.find.field.insert(t.trim(), None);
                        }
                        changed = true;
                    }
                }
                _ => {}
            },
            _ => {
                if let Some(text) = event.text.as_ref() {
                    if !ctrl {
                        tab.find.field.insert(text, None);
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.refresh_find();
        }
        self.request_redraw();
    }

    // ----------------------------------------------------------------- find

    pub(crate) fn open_find(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        tab.find.open = true;
        tab.find.field.select_all();
        self.ui_focus = UiFocus::Find;
        self.request_redraw();
    }

    pub(crate) fn close_find(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.find.open = false;
            tab.find.count = 0;
            tab.send(ToContent::FindClose);
        }
        self.ui_focus = UiFocus::Page;
        self.request_redraw();
    }

    /// Search the page for the find field's text (on the content thread).
    pub(crate) fn refresh_find(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let query = tab.find.field.text.clone();
        if query.is_empty() {
            tab.find.count = 0;
            tab.send(ToContent::FindClose);
        } else {
            tab.send(ToContent::Find(query));
        }
        self.request_redraw();
    }

    pub(crate) fn find_step(&mut self, delta: isize) {
        self.send_active(ToContent::FindStep(delta));
    }
}

/// DOM `KeyboardEvent.key` value for a winit key.
fn key_name(key: &Key) -> String {
    match key {
        Key::Character(c) => c.to_string(),
        Key::Named(n) => match n {
            NamedKey::Space => " ".into(),
            NamedKey::ArrowDown => "ArrowDown".into(),
            NamedKey::ArrowUp => "ArrowUp".into(),
            NamedKey::ArrowLeft => "ArrowLeft".into(),
            NamedKey::ArrowRight => "ArrowRight".into(),
            other => format!("{other:?}"),
        },
        _ => "Unidentified".into(),
    }
}
