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

use super::chrome::{field_shift, Action, FieldView, Menu, MenuItem, ToolButton};
use super::content::{KeyInput, ToContent};
use super::omnibox;
use super::tabstrip::{self, TabDrag};
use super::{App, MouseDown, Panel, UiFocus};

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
            if h != menu.hover && (h.is_some() || menu.contains(x, y)) {
                menu.hover = h;
                popup_hover_changed = true;
            }
        }
        if popup_hover_changed {
            self.request_redraw();
        }
        // Dragging a tab.
        if let Some(mut d) = self.tab_drag {
            let s = self.ui_scale();
            if d.moved(x, s) {
                let chrome = self.chrome();
                let pinned = self.pinned_flags();
                let (rects, _) = chrome.tab_rects(&pinned);
                if let Some(r) = rects.get(d.index) {
                    let ghost = d.ghost(*r, chrome.tab_bar());
                    let to = tabstrip::drop_index(&rects, &pinned, d.index, ghost.x + ghost.w / 2.0);
                    if to != d.index {
                        self.move_tab(d.index, to);
                        // Keep the grab point under the pointer in the new slot.
                        let (rects, _) = self.chrome().tab_rects(&self.pinned_flags());
                        d.index = to;
                        let _ = rects;
                    }
                }
                self.request_redraw();
            }
            self.tab_drag = Some(d);
            self.apply_cursor();
            return;
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
        let over_button = ToolButton::ALL.into_iter().any(|b| chrome.button_rect(b).contains(x, y));
        let (tab_rects, plus) = chrome.tab_rects(&self.pinned_flags());
        let over_tab = tab_rects.iter().any(|r| r.contains(x, y)) || plus.contains(x, y);
        let over_hang_button = self.hang_bar().map(|(_, w, s)| w.contains(x, y) || s.contains(x, y)).unwrap_or(false);
        let over_bookmark = self.bookmark_at(x, y).is_some();
        let (pf, _, _) = chrome.prompt_controls();
        if chrome.url_text().contains(x, y) || (chrome.find_open && chrome.find_field().contains(x, y)) || (chrome.prompt_open && pf.contains(x, y)) {
            CursorIcon::Text
        } else if over_button || over_tab || over_hang_button || over_bookmark {
            CursorIcon::Pointer
        } else {
            CursorIcon::Default
        }
    }

    /// Bookmark bar item under a point: Some(Some(i)) = a bookmark,
    /// Some(None) = the overflow button.
    pub(crate) fn bookmark_at(&self, x: f32, y: f32) -> Option<Option<usize>> {
        let (items, more) = self.bookmark_bar_items();
        if let Some((i, _)) = items.iter().find(|(_, r)| r.contains(x, y)) {
            return Some(Some(*i));
        }
        if more.map(|m| m.contains(x, y)).unwrap_or(false) {
            return Some(None);
        }
        None
    }

    /// Recompute hover state: toolbar buttons, tabs, bookmarks; the page
    /// hover is computed on the content thread (its answer sets the cursor).
    pub(crate) fn update_hover(&mut self) {
        let (x, y) = (self.cursor.x as f32, self.cursor.y as f32);
        let chrome = self.chrome();
        let mut redraw = false;
        let button = ToolButton::ALL.into_iter().find(|b| chrome.button_rect(*b).contains(x, y));
        if button != self.hover_button {
            self.hover_button = button;
            redraw = true;
        }
        let (tab_rects, plus) = chrome.tab_rects(&self.pinned_flags());
        let hover_tab = tab_rects.iter().position(|r| r.contains(x, y));
        let hover_close = hover_tab.filter(|i| !self.tabs[*i].pinned).map(|i| chrome.tab_close_rect(tab_rects[i]).contains(x, y)).unwrap_or(false);
        let hover_plus = plus.contains(x, y);
        if hover_tab != self.hover_tab || hover_close != self.hover_tab_close || hover_plus != self.hover_plus {
            self.hover_tab = hover_tab;
            self.hover_tab_close = hover_close;
            self.hover_plus = hover_plus;
            redraw = true;
        }
        let hb = self.bookmark_at(x, y).flatten();
        if hb != self.hover_bookmark {
            self.hover_bookmark = hb;
            redraw = true;
        }
        let popup_open = self.menu.is_some() || self.context_menu.is_some() || self.dropdown.is_some();
        let over_hang = self.hang_bar().map(|(b, _, _)| b.contains(x, y)).unwrap_or(false);
        match self.to_css(self.cursor) {
            Some(css) if !popup_open && !over_hang => {
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
                } else if menu.contains(x, y) {
                    // Separator / padding: keep the menu open.
                    if which == 0 {
                        self.menu = Some(menu);
                    } else {
                        self.context_menu = Some(menu);
                    }
                }
                self.request_redraw();
                return;
            }
        }
        if !self.suggestions.is_empty() {
            if let Some(i) = self.suggestion_at(x, y) {
                let url = self.suggestions[i].url.clone();
                self.close_popups();
                self.ui_focus = UiFocus::Page;
                let idx = self.active;
                if button == MouseButton::Middle {
                    self.new_tab(Some(&url), true);
                    self.sync_url_field();
                } else {
                    self.navigate_tab(idx, &url, true);
                }
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
            let pinned = self.pinned_flags();
            let (rects, plus) = chrome.tab_rects(&pinned);
            if plus.contains(x, y) {
                if button == MouseButton::Left {
                    self.new_tab(None, false);
                } else if button == MouseButton::Right {
                    self.open_tab_menu(None, x, y);
                }
                return;
            }
            if let Some(i) = rects.iter().position(|r| r.contains(x, y)) {
                let on_close = !self.tabs[i].pinned && chrome.tab_close_rect(rects[i]).contains(x, y);
                match button {
                    MouseButton::Middle => self.close_tab(i, event_loop),
                    MouseButton::Left if on_close => self.close_tab(i, event_loop),
                    MouseButton::Left => {
                        self.activate_tab(i);
                        self.tab_drag = Some(TabDrag::new(i, x, rects[i]));
                    }
                    MouseButton::Right => self.open_tab_menu(Some(i), x, y),
                    _ => {}
                }
            } else if button == MouseButton::Left {
                // Double-click on the empty strip: new tab.
                let now = Instant::now();
                let double = self.last_click.map(|(t, p)| now.duration_since(t) < Duration::from_millis(400) && (p.x - self.cursor.x).abs() < 5.0).unwrap_or(false);
                self.last_click = Some((now, self.cursor));
                if double {
                    self.new_tab(None, false);
                }
            }
            return;
        }

        // Toolbar.
        if chrome.toolbar().contains(x, y) {
            if chrome.url_field().contains(x, y) {
                if button == MouseButton::Right {
                    self.open_url_menu(x, y);
                    return;
                }
                if button != MouseButton::Left {
                    return;
                }
                if self.ui_focus != UiFocus::Url {
                    self.focus_url_field();
                } else {
                    let idx = self.url_index_at(x);
                    self.url_field.set_caret(idx, self.modifiers.shift_key());
                    self.mouse_down = Some(MouseDown { button, pos: self.cursor, on_page: false });
                }
                self.request_redraw();
                return;
            }
            self.blur_url_field();
            let hit = ToolButton::ALL.into_iter().find(|b| chrome.button_rect(*b).contains(x, y));
            let middle = button == MouseButton::Middle;
            if button != MouseButton::Left && !middle {
                return;
            }
            match hit {
                Some(ToolButton::Back) => self.history_button(-1, middle),
                Some(ToolButton::Forward) => self.history_button(1, middle),
                Some(ToolButton::Reload) => {
                    if self.tab().map(|t| t.is_loading()).unwrap_or(false) {
                        self.stop_loading();
                    } else if middle {
                        let i = self.active;
                        self.duplicate_tab(i);
                    } else {
                        self.reload(self.modifiers.shift_key());
                    }
                }
                Some(ToolButton::Home) => {
                    let home = self.settings.homepage.clone();
                    if middle {
                        self.new_tab(Some(&home), true);
                    } else {
                        let idx = self.active;
                        self.navigate_tab(idx, &home, true);
                    }
                }
                Some(ToolButton::Star) => self.toggle_bookmark(),
                Some(ToolButton::Downloads) => {
                    let idx = self.active;
                    self.navigate_tab(idx, "about:downloads", true);
                }
                Some(ToolButton::Menu) => self.open_main_menu(),
                None => {}
            }
            return;
        }

        // Bookmarks bar.
        if chrome.bookmarks_bar().contains(x, y) {
            match self.bookmark_at(x, y) {
                Some(Some(i)) => {
                    let url = self.bookmarks.items[i].url.clone();
                    match button {
                        MouseButton::Left if !self.modifiers.control_key() => {
                            let idx = self.active;
                            self.navigate_tab(idx, &url, true);
                        }
                        MouseButton::Left | MouseButton::Middle => {
                            self.new_tab(Some(&url), true);
                        }
                        MouseButton::Right => {
                            let items = vec![
                                MenuItem::new("Open", Action::OpenBookmark(url.clone(), false)),
                                MenuItem::new("Open in new tab", Action::OpenBookmark(url.clone(), true)),
                                MenuItem::new("Open in new window", Action::OpenInNewWindow(Url::parse(&url).unwrap_or_else(super::about_url))),
                                MenuItem::new("Remove", Action::RemoveBookmark(url.clone())).sep(),
                                MenuItem::new("Manage bookmarks…", Action::Bookmarks).key("Ctrl+Shift+O").sep(),
                                MenuItem::new("Hide bookmarks bar", Action::ToggleBookmarksBar).key("Ctrl+Shift+B"),
                            ];
                            self.show_context_menu(items, x, y);
                        }
                        _ => {}
                    }
                }
                Some(None) => {
                    let (shown, more) = self.bookmark_bar_items();
                    let first_hidden = shown.len();
                    let items: Vec<MenuItem> = self.bookmarks.items.iter().skip(first_hidden).take(40).map(|b| MenuItem::new(&b.label().chars().take(50).collect::<String>(), Action::OpenBookmark(b.url.clone(), false))).collect();
                    if let (Some(m), false) = (more, items.is_empty()) {
                        self.show_context_menu(items, m.x, m.bottom());
                    }
                }
                None => {
                    if button == MouseButton::Right {
                        let items = vec![MenuItem::new("Manage bookmarks…", Action::Bookmarks), MenuItem::new("Hide bookmarks bar", Action::ToggleBookmarksBar).key("Ctrl+Shift+B")];
                        self.show_context_menu(items, x, y);
                    }
                }
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

        // "Save as" prompt.
        if chrome.prompt_open && chrome.prompt_bar().contains(x, y) {
            let (field, ok, cancel) = chrome.prompt_controls();
            if ok.contains(x, y) {
                self.answer_save_prompt(true);
            } else if cancel.contains(x, y) {
                self.answer_save_prompt(false);
            } else if field.contains(x, y) {
                self.ui_focus = UiFocus::Prompt;
            }
            self.request_redraw();
            return;
        }

        // Devtools pane.
        if chrome.devtools_open && chrome.devtools_pane().contains(x, y) {
            self.blur_url_field();
            if button == MouseButton::Left {
                self.devtools_click(x, y);
            }
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
        if matches!(self.ui_focus, UiFocus::Console | UiFocus::Find | UiFocus::Prompt) {
            self.ui_focus = UiFocus::Page;
        }
        let Some(css) = self.to_css(self.cursor) else { return };
        if !matches!(button, MouseButton::Left | MouseButton::Middle | MouseButton::Right) {
            return;
        }
        self.scroll_anim = None;
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

    /// Back / forward buttons: middle click opens the entry in a new tab.
    fn history_button(&mut self, delta: isize, new_tab: bool) {
        if !new_tab {
            return self.go_history(delta);
        }
        let Some(t) = self.tab() else { return };
        let target = t.index as isize + delta;
        if let Some(u) = (target >= 0).then(|| t.history.get(target as usize)).flatten().map(|u| u.to_string()) {
            self.new_tab(Some(&u), true);
        }
    }

    /// Byte index in the URL text under window x.
    fn url_index_at(&self, x: f32) -> usize {
        let chrome = self.chrome();
        let s = chrome.s;
        let focused = self.ui_focus == UiFocus::Url;
        let view = FieldView { text_rect: Some(chrome.url_text()), ..FieldView::new(chrome.url_field(), &self.url_field.text, self.url_field.caret, None, focused, "") };
        let shift = field_shift(&self.engine.fonts, self.ui_font, s, &view);
        let fonts = &self.engine.fonts;
        let size = super::chrome::UI_FONT * s;
        let font = self.ui_font;
        super::input::index_for_x(&self.url_field.text, x - chrome.url_text().x + shift, |t| fonts.measure(font, size, t))
    }

    /// Keyboard modifiers in the engine's form.
    pub(crate) fn pointer_modifiers(&self) -> Modifiers {
        let m = self.modifiers;
        Modifiers { ctrl: m.control_key(), shift: m.shift_key(), alt: m.alt_key(), meta: m.super_key() }
    }

    pub(crate) fn handle_mouse_up(&mut self, button: MouseButton, _event_loop: &ActiveEventLoop) {
        if button == MouseButton::Left {
            if let Some(d) = self.tab_drag.take() {
                if d.dragging {
                    self.request_redraw();
                }
                self.apply_cursor();
                return;
            }
        }
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
        let (dx, dy, lines) = match delta {
            MouseScrollDelta::LineDelta(h, v) => (-h * LINE_SCROLL, -v * LINE_SCROLL, true),
            MouseScrollDelta::PixelDelta(p) => (-(p.x as f32) / self.page_scale(), -(p.y as f32) / self.page_scale(), false),
        };
        // Shift+wheel scrolls horizontally.
        let (dx, dy) = if self.modifiers.shift_key() && dx == 0.0 { (dy, 0.0) } else { (dx, dy) };
        if let Some(menu) = self.dropdown.as_mut().or(self.menu.as_mut()).or(self.context_menu.as_mut()) {
            if menu.contains(x, y) {
                menu.scroll(if dy > 0.0 { 3 } else { -3 });
                self.request_redraw();
                return;
            }
        }
        let chrome = self.chrome();
        if chrome.devtools_open && chrome.devtools_pane().contains(x, y) {
            self.scroll_devtools(dy);
            return;
        }
        if chrome.tab_bar().contains(x, y) && !self.tabs.is_empty() {
            // Wheel over the strip switches tabs.
            let n = self.tabs.len();
            let next = if dy > 0.0 { (self.active + 1).min(n - 1) } else { self.active.saturating_sub(1) };
            self.activate_tab(next);
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
                self.scroll_anim = None;
                self.send_active(ToContent::Wheel { x: css.0, y: css.1, dx, dy });
                return;
            }
        }
        if lines {
            self.scroll_by_smooth(dx, dy);
        } else {
            self.scroll_by(dx, dy);
        }
    }

    pub(crate) fn blur_url_field(&mut self) {
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
        (0..self.suggestions.len()).map(|i| Rect::new(field.x, field.bottom() + 8.0 * chrome.s + i as f32 * h, field.w, h)).collect()
    }

    fn suggestion_at(&self, x: f32, y: f32) -> Option<usize> {
        self.suggestion_rects().iter().position(|r| r.contains(x, y))
    }

    fn show_menu_at(&mut self, items: Vec<MenuItem>, x: f32, y: f32) -> Menu {
        let chrome = self.chrome();
        let win = self.window_size();
        Menu::new(x, y, items, chrome.s, &self.engine.fonts, self.ui_font, win.width as f32, win.height as f32)
    }

    pub(crate) fn show_context_menu(&mut self, items: Vec<MenuItem>, x: f32, y: f32) {
        let m = self.show_menu_at(items, x, y);
        self.context_menu = Some(m);
        self.apply_cursor();
        self.request_redraw();
    }

    fn open_main_menu(&mut self) {
        let chrome = self.chrome();
        let r = chrome.button_rect(ToolButton::Menu);
        let zoom = self.tab().map(|t| t.zoom).unwrap_or(1.0);
        let items = vec![
            MenuItem::new("New tab", Action::NewTab).key("Ctrl+T"),
            MenuItem::new("New window", Action::NewWindow).key("Ctrl+N"),
            MenuItem::new("New private window", Action::NewPrivateWindow).key("Ctrl+Shift+N"),
            MenuItem::new("Reopen closed tab", Action::ReopenClosedTab).key("Ctrl+Shift+T").disabled(self.closed.len() == 0),
            MenuItem::new("History", Action::History).key("Ctrl+H").sep(),
            MenuItem::new("Downloads", Action::Downloads).key("Ctrl+J"),
            MenuItem::new("Bookmarks", Action::Bookmarks).key("Ctrl+Shift+O"),
            MenuItem::new(if self.settings.show_bookmarks_bar { "Hide bookmarks bar" } else { "Show bookmarks bar" }, Action::ToggleBookmarksBar).key("Ctrl+Shift+B"),
            MenuItem::new(&format!("Zoom in ({}%)", (zoom * 100.0).round()), Action::ZoomIn).key("Ctrl++").sep(),
            MenuItem::new("Zoom out", Action::ZoomOut).key("Ctrl+-"),
            MenuItem::new("Reset zoom", Action::ZoomReset).key("Ctrl+0"),
            MenuItem::new("Find in page", Action::Find).key("Ctrl+F").sep(),
            MenuItem::new("Save page as PDF…", Action::PrintPdf).key("Ctrl+P"),
            MenuItem::new("Page text", Action::ViewText),
            MenuItem::new("Developer tools", Action::ToggleDevtools).key("F12"),
            MenuItem::new(if self.settings.javascript { "Disable JavaScript" } else { "Enable JavaScript" }, Action::ToggleJavaScript).sep(),
            MenuItem::new(if self.settings.block_trackers { "Allow ads & trackers" } else { "Block ads & trackers" }, Action::ToggleTrackerBlocking),
            MenuItem::new("Settings", Action::Settings).key("Ctrl+,").sep(),
            MenuItem::new("About RabbitHole", Action::About),
            MenuItem::new("Quit", Action::Quit).key("Ctrl+Q").sep(),
        ];
        let m = self.show_menu_at(items, r.right() - 280.0 * chrome.s, r.bottom() + 4.0 * chrome.s);
        self.menu = Some(m);
        self.request_redraw();
    }

    /// Right-click on a tab (or the "+" button).
    fn open_tab_menu(&mut self, tab: Option<usize>, x: f32, y: f32) {
        let mut items = vec![MenuItem::new("New tab", Action::NewTab).key("Ctrl+T")];
        if let Some(i) = tab {
            let pinned = self.tabs[i].pinned;
            items.push(MenuItem::new("Reload", Action::ReloadTab(i)).sep());
            items.push(MenuItem::new("Duplicate", Action::DuplicateTab));
            items.push(MenuItem::new(if pinned { "Unpin tab" } else { "Pin tab" }, Action::PinTab(i)));
            items.push(MenuItem::new("Close tab", Action::CloseTabAt(i)).key("Ctrl+W").sep());
            items.push(MenuItem::new("Close other tabs", Action::CloseOtherTabs(i)).disabled(self.tabs.len() < 2));
            items.push(MenuItem::new("Close tabs to the right", Action::CloseTabsToRight(i)).disabled(i + 1 >= self.tabs.len()));
            if i != self.active {
                self.activate_tab(i);
            }
        }
        items.push(MenuItem::new("Reopen closed tab", Action::ReopenClosedTab).key("Ctrl+Shift+T").disabled(self.closed.len() == 0).sep());
        self.show_context_menu(items, x, y);
    }

    /// Right-click on the address bar.
    fn open_url_menu(&mut self, x: f32, y: f32) {
        if self.ui_focus != UiFocus::Url {
            self.focus_url_field();
        }
        let has_sel = self.url_field.selection().is_some();
        let items = vec![
            MenuItem::new("Cut", Action::UrlCut).key("Ctrl+X").disabled(!has_sel),
            MenuItem::new("Copy", Action::UrlCopy).key("Ctrl+C").disabled(!has_sel),
            MenuItem::new("Paste", Action::UrlPaste).key("Ctrl+V"),
            MenuItem::new("Paste and go", Action::UrlPasteAndGo).key("Ctrl+Shift+V"),
            MenuItem::new("Select all", Action::UrlSelectAll).key("Ctrl+A").sep(),
        ];
        self.show_context_menu(items, x, y);
    }

    pub(crate) fn open_context_menu(&mut self, link: Option<Url>, image: Option<Url>, has_sel: bool, x: f32, y: f32) {
        let (back, fwd) = self.tab().map(|t| (t.can_go_back(), t.can_go_forward())).unwrap_or((false, false));
        let mut items = Vec::new();
        if let Some(l) = &link {
            items.push(MenuItem::new("Open link in new tab", Action::OpenInNewTab(l.clone())));
            items.push(MenuItem::new("Open link in new window", Action::OpenInNewWindow(l.clone())));
            items.push(MenuItem::new("Open link in private window", Action::OpenInPrivateWindow(l.clone())));
            items.push(MenuItem::new("Save link as…", Action::SaveUrl(l.clone())));
            items.push(MenuItem::new("Copy link address", Action::CopyLink(l.clone())));
        }
        if let Some(img) = &image {
            items.push(MenuItem::new("Open image in new tab", Action::OpenInNewTab(img.clone())).sep());
            items.push(MenuItem::new("Save image as…", Action::SaveUrl(img.clone())));
            items.push(MenuItem::new("Copy image address", Action::CopyLink(img.clone())));
        }
        if has_sel {
            items.push(MenuItem::new("Copy", Action::CopySelection).key("Ctrl+C").sep());
            items.push(MenuItem::new(&format!("Search {} for the selection", self.settings.search_engine.name()), Action::SearchSelection));
        }
        let first_nav = items.len();
        items.push(MenuItem::new("Back", Action::Back).key("Alt+Left").disabled(!back));
        items.push(MenuItem::new("Forward", Action::Forward).key("Alt+Right").disabled(!fwd));
        items.push(MenuItem::new("Reload", Action::Reload).key("F5"));
        if first_nav > 0 {
            items[first_nav] = items[first_nav].clone().sep();
        }
        items.push(MenuItem::new("Bookmark this page", Action::ToggleBookmark).key("Ctrl+D").sep());
        items.push(MenuItem::new("Save page as PDF…", Action::PrintPdf).key("Ctrl+P"));
        items.push(MenuItem::new("Copy page address", Action::CopyPageUrl));
        items.push(MenuItem::new("View page text", Action::ViewText));
        items.push(MenuItem::new("Inspect", Action::Inspect(x, y)).key("Ctrl+Shift+C").sep());
        self.show_context_menu(items, x, y);
    }

    /// Show a `<select>` popup for the page (`rect` in document px).
    pub(crate) fn open_select_dropdown(&mut self, select: NodeId, options: Vec<(String, bool, bool)>, rect: Rect) {
        let chrome = self.chrome();
        let rect = if rect.w > 0.0 || rect.h > 0.0 { self.doc_to_screen(rect) } else { Rect::new(self.cursor.x as f32, self.cursor.y as f32, 0.0, 0.0) };
        let mut selected = None;
        let items: Vec<MenuItem> = options
            .iter()
            .enumerate()
            .map(|(i, (label, sel, disabled))| {
                if *sel {
                    selected = Some(i);
                }
                let text = if *sel { format!("✓ {label}") } else { format!("   {label}") };
                MenuItem::new(&text, Action::SelectOption(select, i)).disabled(*disabled)
            })
            .collect();
        if items.is_empty() {
            return;
        }
        let win = self.window_size();
        let mut menu = Menu::new(rect.x, rect.bottom(), items, chrome.s, &self.engine.fonts, self.ui_font, win.width as f32, win.height as f32);
        menu.rect.w = menu.rect.w.max(rect.w);
        menu.hover = selected;
        if let Some(sel) = selected {
            if sel >= menu.max_visible {
                menu.first_visible = sel + 1 - menu.max_visible;
            }
        }
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
            Action::NewWindow => self.open_window(None, false),
            Action::NewPrivateWindow => self.open_window(None, true),
            Action::CloseTabAt(i) => self.close_tab(i, event_loop),
            Action::ReopenClosedTab => self.reopen_closed_tab(),
            Action::DuplicateTab => self.duplicate_tab(idx),
            Action::PinTab(i) => self.toggle_pin(i),
            Action::CloseOtherTabs(i) => self.close_other_tabs(i, false, event_loop),
            Action::CloseTabsToRight(i) => self.close_other_tabs(i, true, event_loop),
            Action::ReloadTab(i) => self.reload_tab(i),
            Action::Back => self.go_history(-1),
            Action::Forward => self.go_history(1),
            Action::Reload => self.reload(false),
            Action::Bookmarks => self.navigate_tab(idx, "about:bookmarks", true),
            Action::History => self.navigate_tab(idx, "about:history", true),
            Action::Downloads => self.navigate_tab(idx, "about:downloads", true),
            Action::Settings => self.navigate_tab(idx, "about:settings", true),
            Action::About => self.navigate_tab(idx, "about:version", true),
            Action::ZoomIn => self.set_zoom(self.tab().map(|t| t.zoom).unwrap_or(1.0) + 0.1),
            Action::ZoomOut => self.set_zoom(self.tab().map(|t| t.zoom).unwrap_or(1.0) - 0.1),
            Action::ZoomReset => self.set_zoom(self.settings.default_zoom),
            Action::Find => self.open_find(),
            Action::PrintPdf => self.print_pdf(),
            Action::Quit => self.shutdown(event_loop),
            Action::OpenInNewTab(u) => self.follow_link(u, true),
            Action::OpenInNewWindow(u) => self.open_window(Some(&u.to_string()), false),
            Action::OpenInPrivateWindow(u) => self.open_window(Some(&u.to_string()), true),
            Action::CopyLink(u) => self.clipboard_set(&u.to_string()),
            Action::SaveUrl(u) => self.send_active(ToContent::SaveUrl(u)),
            Action::CopySelection => self.send_active(ToContent::CopySelection),
            Action::SearchSelection => {
                self.pending_search = true;
                self.send_active(ToContent::CopySelection);
            }
            Action::ViewText => self.navigate_tab(idx, "about:text", true),
            Action::CopyPageUrl => {
                if let Some(u) = self.tab().and_then(|t| t.current_url.as_ref()).map(|u| u.to_string()) {
                    self.clipboard_set(&u);
                }
            }
            Action::SelectOption(node, index) => self.send_active(ToContent::SelectOption { node, index }),
            Action::ToggleBookmark => self.toggle_bookmark(),
            Action::ToggleBookmarksBar => self.toggle_bookmarks_bar(),
            Action::ToggleJavaScript => self.toggle_javascript(),
            Action::ToggleTrackerBlocking => self.toggle_tracker_blocking(),
            Action::ToggleDevtools => self.toggle_devtools(),
            Action::Inspect(x, y) => self.inspect_at(x, y),
            Action::UrlCut => {
                if let Some(t) = self.url_field.selected_text().map(|s| s.to_string()) {
                    self.clipboard_set(&t);
                    self.url_field.delete_selection();
                    self.on_url_text_changed(false);
                }
            }
            Action::UrlCopy => {
                if let Some(t) = self.url_field.selected_text().map(|s| s.to_string()) {
                    self.clipboard_set(&t);
                }
            }
            Action::UrlPaste => {
                if let Some(t) = self.clipboard_get() {
                    self.ui_focus = UiFocus::Url;
                    self.url_field.insert(&single_line(&t), None);
                    self.on_url_text_changed(false);
                }
            }
            Action::UrlPasteAndGo => self.paste_and_go(),
            Action::UrlSelectAll => {
                self.ui_focus = UiFocus::Url;
                self.url_field.select_all();
            }
            Action::OpenBookmark(url, new_tab) => {
                if new_tab {
                    self.new_tab(Some(&url), true);
                } else {
                    self.navigate_tab(idx, &url, true);
                }
            }
            Action::RemoveBookmark(url) => {
                self.bookmarks = crate::bookmarks::Bookmarks::load(&self.profile.bookmarks_path());
                self.bookmarks.remove(&url);
                self.bookmarks.save(&self.profile.bookmarks_path());
                self.request_redraw();
            }
        }
    }

    /// Navigate to the clipboard contents (URL or search).
    pub(crate) fn paste_and_go(&mut self) {
        let Some(t) = self.clipboard_get() else { return };
        let target = self.resolve_input(&single_line(&t));
        self.ui_focus = UiFocus::Page;
        self.close_popups();
        let idx = self.active;
        self.navigate_tab(idx, &target, true);
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
    /// the page is busy, and animated.
    fn ui_scroll_key(&mut self, event: &KeyEvent) -> bool {
        let Some(tab) = self.tab() else { return false };
        let st = tab.state;
        let m = self.modifiers;
        if st.editing || st.key_listeners || st.focused || m.alt_key() || m.super_key() {
            return false;
        }
        let ctrl = m.control_key();
        let vp = self.css_viewport();
        let page_step = (vp.height * 0.875).max(LINE_SCROLL);
        let shift = m.shift_key();
        match &event.logical_key {
            Key::Named(NamedKey::ArrowDown) if !ctrl => self.scroll_by_smooth(0.0, LINE_SCROLL),
            Key::Named(NamedKey::ArrowUp) if !ctrl => self.scroll_by_smooth(0.0, -LINE_SCROLL),
            Key::Named(NamedKey::ArrowLeft) if !ctrl => self.scroll_by_smooth(-LINE_SCROLL, 0.0),
            Key::Named(NamedKey::ArrowRight) if !ctrl => self.scroll_by_smooth(LINE_SCROLL, 0.0),
            Key::Named(NamedKey::PageDown) if !ctrl => self.scroll_by_smooth(0.0, page_step),
            Key::Named(NamedKey::PageUp) if !ctrl => self.scroll_by_smooth(0.0, -page_step),
            Key::Named(NamedKey::Space) if !ctrl => self.scroll_by_smooth(0.0, if shift { -page_step } else { page_step }),
            Key::Named(NamedKey::Home) => self.scroll_to_smooth(0.0, 0.0),
            Key::Named(NamedKey::End) => self.scroll_to_smooth(0.0, f32::MAX),
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
                    self.menu = None;
                    self.context_menu = None;
                    self.dropdown = None;
                    self.request_redraw();
                }
                Key::Named(NamedKey::ArrowDown) | Key::Named(NamedKey::ArrowUp) => {
                    let down = matches!(event.logical_key, Key::Named(NamedKey::ArrowDown));
                    if let Some(menu) = self.dropdown.as_mut().or(self.menu.as_mut()).or(self.context_menu.as_mut()) {
                        menu.step(down);
                    }
                    self.request_redraw();
                }
                Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space) => {
                    let action = self
                        .dropdown
                        .as_ref()
                        .or(self.menu.as_ref())
                        .or(self.context_menu.as_ref())
                        .and_then(|m| m.hover.and_then(|h| m.items.get(h)).filter(|i| i.enabled).map(|i| i.action.clone()));
                    self.menu = None;
                    self.context_menu = None;
                    self.dropdown = None;
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
                    "w" if shift => return self.shutdown(event_loop),
                    "w" => return self.close_tab(idx, event_loop),
                    "t" if shift => return self.reopen_closed_tab(),
                    "t" => {
                        self.new_tab(None, false);
                        return;
                    }
                    "n" => return self.open_window(None, shift),
                    "l" if self.ui_focus != UiFocus::Console => return self.focus_url_field(),
                    "k" | "e" if !shift => {
                        // Focus the address bar for a search.
                        self.focus_url_field();
                        self.url_field.set_text("? ");
                        self.url_edited = true;
                        return;
                    }
                    "r" => return self.reload(shift),
                    "f" => return self.open_find(),
                    "g" => return self.find_step(if shift { -1 } else { 1 }),
                    "d" => return self.toggle_bookmark(),
                    "p" => return self.print_pdf(),
                    "s" => return self.print_pdf(),
                    "h" => return self.navigate_tab(idx, "about:history", true),
                    "," => return self.navigate_tab(idx, "about:settings", true),
                    "b" if shift => return self.toggle_bookmarks_bar(),
                    "j" if shift => return self.show_panel(Panel::Console),
                    "i" if shift => return self.toggle_devtools(),
                    "c" if shift => return self.show_panel(Panel::Elements),
                    "e" if shift => return self.show_panel(Panel::Network),
                    "j" => return self.navigate_tab(idx, "about:downloads", true),
                    "o" if shift => return self.navigate_tab(idx, "about:bookmarks", true),
                    "v" if shift && self.ui_focus == UiFocus::Url => return self.paste_and_go(),
                    "u" if shift => {
                        self.show_stats = !self.show_stats;
                        self.request_redraw();
                        return;
                    }
                    "+" | "=" => return self.set_zoom(self.tab().map(|t| t.zoom).unwrap_or(1.0) + 0.1),
                    "-" | "_" => return self.set_zoom(self.tab().map(|t| t.zoom).unwrap_or(1.0) - 0.1),
                    "0" => return self.set_zoom(self.settings.default_zoom),
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
            match &event.logical_key {
                Key::Named(NamedKey::Tab) => {
                    if !self.tabs.is_empty() {
                        let n = self.tabs.len();
                        let next = if shift { (idx + n - 1) % n } else { (idx + 1) % n };
                        self.activate_tab(next);
                    }
                    return;
                }
                Key::Named(NamedKey::PageDown) | Key::Named(NamedKey::PageUp) => {
                    let n = self.tabs.len();
                    let down = matches!(event.logical_key, Key::Named(NamedKey::PageDown));
                    if shift {
                        // Move the tab.
                        let to = if down { (idx + 1).min(n - 1) } else { idx.saturating_sub(1) };
                        self.move_tab(idx, to);
                    } else if n > 0 {
                        self.activate_tab(if down { (idx + 1) % n } else { (idx + n - 1) % n });
                    }
                    return;
                }
                Key::Named(NamedKey::F4) => return self.close_tab(idx, event_loop),
                _ => {}
            }
        }
        match &event.logical_key {
            Key::Named(NamedKey::F5) => return self.reload(ctrl || shift),
            Key::Named(NamedKey::F6) => return self.focus_url_field(),
            Key::Named(NamedKey::F11) => return self.toggle_fullscreen(),
            Key::Named(NamedKey::F12) => return self.toggle_devtools(),
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
            Key::Named(NamedKey::Home) if alt => {
                let home = self.settings.homepage.clone();
                return self.navigate_tab(idx, &home, true);
            }
            Key::Named(NamedKey::BrowserBack) => return self.go_history(-1),
            Key::Named(NamedKey::BrowserForward) => return self.go_history(1),
            Key::Named(NamedKey::BrowserRefresh) => return self.reload(false),
            Key::Character(c) if alt && c.to_lowercase() == "d" => return self.focus_url_field(),
            _ => {}
        }

        match self.ui_focus {
            UiFocus::Url => self.handle_url_key(event, ctrl, shift),
            UiFocus::Find => self.handle_find_key(event, ctrl, shift),
            UiFocus::Console => self.handle_console_key(event, ctrl, shift),
            UiFocus::Prompt => self.handle_prompt_key(event, ctrl, shift),
            UiFocus::Page => {
                if matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
                    self.status = None;
                    if self.tab().map(|t| t.is_loading()).unwrap_or(false) {
                        self.stop_loading();
                        return;
                    }
                    if self.devtools_open && self.panel != Panel::Console {
                        self.send_active(ToContent::InspectNode(None));
                    }
                }
                if matches!(event.logical_key, Key::Named(NamedKey::ArrowDown) | Key::Named(NamedKey::ArrowUp)) && self.elements_key(matches!(event.logical_key, Key::Named(NamedKey::ArrowDown))) {
                    return;
                }
                if self.ui_scroll_key(event) {
                    return;
                }
                self.scroll_anim = None;
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
                self.on_url_text_changed(true);
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
            UiFocus::Prompt => {
                if let Some(p) = self.save_prompt.as_mut() {
                    p.field.insert(text, None);
                }
            }
            UiFocus::Page => self.send_active(ToContent::Text(text.to_string())),
        }
        self.request_redraw();
    }

    /// The typed text changed: refresh suggestions and (when `typed`, i.e.
    /// characters were added at the end) complete inline.
    fn on_url_text_changed(&mut self, typed: bool) {
        self.url_edited = true;
        let q = self.url_field.text.clone();
        self.suggestions = omnibox::suggest(&q, &self.history, &self.bookmarks, self.settings.search_engine, 8);
        self.suggestion_sel = None;
        if typed && self.url_field.caret == self.url_field.text.len() && self.url_field.selection().is_none() {
            if let Some(rest) = omnibox::inline_completion(&q, &self.suggestions) {
                let start = self.url_field.text.len();
                self.url_field.text.push_str(&rest);
                self.url_field.anchor = Some(start);
                self.url_field.caret = self.url_field.text.len();
                // Enter goes where the completed text points.
                if let Some(first) = self.suggestions.first_mut() {
                    if first.kind != omnibox::SuggestionKind::Bookmark && first.kind != omnibox::SuggestionKind::History {
                        first.kind = omnibox::SuggestionKind::Url;
                        first.url = self.url_field.text.clone();
                        first.title.clear();
                    }
                }
            }
        }
        self.request_redraw();
    }

    fn handle_url_key(&mut self, event: &KeyEvent, ctrl: bool, shift: bool) {
        match &event.logical_key {
            Key::Named(NamedKey::Enter) => {
                // Ctrl+Enter: www.<text>.com
                if ctrl && !self.url_field.text.contains(['.', ':', '/', ' ']) && !self.url_field.text.is_empty() {
                    let t = format!("www.{}.com", self.url_field.text.trim());
                    self.url_field.set_text(&t);
                    self.suggestion_sel = None;
                } else if self.suggestion_sel.is_none() && self.url_field.selection().is_some() && self.url_field.selection().map(|(_, e)| e) == Some(self.url_field.text.len()) {
                    // Accept the inline completion.
                    self.url_field.clear_selection();
                }
                return self.submit_url_field();
            }
            Key::Named(NamedKey::Escape) => {
                // First Esc closes the suggestions, the second restores the URL.
                if !self.suggestions.is_empty() {
                    self.suggestions.clear();
                    self.suggestion_sel = None;
                } else if self.url_edited {
                    self.url_edited = false;
                    self.sync_url_field();
                    self.url_field.select_all();
                } else {
                    self.ui_focus = UiFocus::Page;
                    self.close_popups();
                    self.sync_url_field();
                }
            }
            Key::Named(NamedKey::ArrowDown) if !self.suggestions.is_empty() => {
                let n = self.suggestions.len();
                self.suggestion_sel = Some(self.suggestion_sel.map(|i| (i + 1) % n).unwrap_or(0));
                self.preview_suggestion();
            }
            Key::Named(NamedKey::ArrowUp) if !self.suggestions.is_empty() => {
                let n = self.suggestions.len();
                self.suggestion_sel = Some(self.suggestion_sel.map(|i| (i + n - 1) % n).unwrap_or(n - 1));
                self.preview_suggestion();
            }
            Key::Named(NamedKey::Delete) if shift && self.suggestion_sel.is_some() => {
                // Shift+Delete removes a history suggestion.
                if let Some(s) = self.suggestion_sel.and_then(|i| self.suggestions.get(i)).filter(|s| s.kind == omnibox::SuggestionKind::History).cloned() {
                    self.history.remove_url(&s.url, &self.profile.history_path());
                    self.suggestions.retain(|x| x.url != s.url);
                    self.suggestion_sel = None;
                }
            }
            Key::Named(NamedKey::Backspace) => {
                // With an inline completion selected, Backspace removes just it.
                self.url_field.backspace(ctrl);
                self.on_url_text_changed(false);
            }
            Key::Named(NamedKey::Delete) => {
                self.url_field.delete(ctrl);
                self.on_url_text_changed(false);
            }
            Key::Named(NamedKey::ArrowLeft) => self.url_field.move_left(shift, ctrl),
            Key::Named(NamedKey::ArrowRight) => self.url_field.move_right(shift, ctrl),
            Key::Named(NamedKey::Home) => self.url_field.home(shift),
            Key::Named(NamedKey::End) => self.url_field.end(shift),
            Key::Named(NamedKey::Tab) => {
                // Tab accepts the inline completion.
                if self.url_field.selection().map(|(_, e)| e) == Some(self.url_field.text.len()) {
                    self.url_field.clear_selection();
                    self.on_url_text_changed(false);
                }
            }
            Key::Named(NamedKey::Space) => {
                self.url_field.insert(" ", None);
                self.on_url_text_changed(true);
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
                        self.on_url_text_changed(false);
                    }
                }
                "v" => {
                    if let Some(t) = self.clipboard_get() {
                        self.url_field.insert(&single_line(&t), None);
                        self.on_url_text_changed(false);
                    }
                }
                _ => {}
            },
            _ => {
                if let Some(text) = event.text.as_ref() {
                    if !ctrl && !text.chars().any(|c| c.is_control()) {
                        self.url_field.insert(text, None);
                        self.on_url_text_changed(true);
                    }
                }
            }
        }
        self.request_redraw();
    }

    /// Show the highlighted suggestion's URL in the field.
    fn preview_suggestion(&mut self) {
        if let Some(s) = self.suggestion_sel.and_then(|i| self.suggestions.get(i)) {
            let text = if s.kind == omnibox::SuggestionKind::Search { s.title.split(" — ").next().unwrap_or("").to_string() } else { s.url.clone() };
            self.url_field.set_text(&text);
        }
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
                            tab.find.field.insert(&single_line(&t), None);
                        }
                        changed = true;
                    }
                }
                _ => {}
            },
            _ => {
                if let Some(text) = event.text.as_ref() {
                    if !ctrl && !text.chars().any(|c| c.is_control()) {
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

    fn handle_prompt_key(&mut self, event: &KeyEvent, ctrl: bool, shift: bool) {
        match &event.logical_key {
            Key::Named(NamedKey::Enter) => return self.answer_save_prompt(true),
            Key::Named(NamedKey::Escape) => return self.answer_save_prompt(false),
            _ => {}
        }
        let clip = if ctrl && matches!(&event.logical_key, Key::Character(c) if c.to_lowercase() == "v") { self.clipboard_get() } else { None };
        let Some(p) = self.save_prompt.as_mut() else { return };
        let f = &mut p.field;
        match &event.logical_key {
            Key::Named(NamedKey::Backspace) => f.backspace(ctrl),
            Key::Named(NamedKey::Delete) => f.delete(ctrl),
            Key::Named(NamedKey::ArrowLeft) => f.move_left(shift, ctrl),
            Key::Named(NamedKey::ArrowRight) => f.move_right(shift, ctrl),
            Key::Named(NamedKey::Home) => f.home(shift),
            Key::Named(NamedKey::End) => f.end(shift),
            Key::Named(NamedKey::Space) => f.insert(" ", None),
            Key::Character(c) if ctrl => match c.to_lowercase().as_str() {
                "a" => f.select_all(),
                "v" => {
                    if let Some(t) = clip {
                        f.insert(&single_line(&t), None);
                    }
                }
                _ => {}
            },
            _ => {
                if let Some(text) = event.text.as_ref() {
                    if !ctrl && !text.chars().any(|c| c.is_control()) {
                        f.insert(text, None);
                    }
                }
            }
        }
        self.request_redraw();
    }

    // ----------------------------------------------------------------- find

    pub(crate) fn open_find(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let was_open = tab.find.open;
        tab.find.open = true;
        tab.find.field.select_all();
        self.ui_focus = UiFocus::Find;
        // Reopening searches again for the remembered text.
        if !was_open && !tab.find.field.text.is_empty() {
            self.refresh_find();
        }
        let i = self.active;
        self.sync_view(i);
        self.request_redraw();
    }

    pub(crate) fn close_find(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.find.open = false;
            tab.find.count = 0;
            tab.send(ToContent::FindClose);
        }
        self.ui_focus = UiFocus::Page;
        let i = self.active;
        self.sync_view(i);
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
        if !self.tab().map(|t| t.find.open).unwrap_or(false) {
            return self.open_find();
        }
        self.send_active(ToContent::FindStep(delta));
    }
}

/// Clipboard text for a one-line field.
fn single_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
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
