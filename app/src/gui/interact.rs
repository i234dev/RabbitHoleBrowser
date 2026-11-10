//! Mouse and keyboard handling.

use std::time::{Duration, Instant};

use rabbithole_engine::dom::NodeId;
use rabbithole_engine::gfx::Rect;
use rabbithole_engine::net::Url;
use rabbithole_engine::scripting::{mouse_init, plain_init};
use rabbithole_engine::ControlKind;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::CursorIcon;

use super::chrome::{Action, Menu, MenuItem, ToolButton};
use super::{App, MouseDown, UiFocus};

const LINE_SCROLL: f32 = 40.0;
const DRAG_THRESHOLD: f64 = 4.0;

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
        // Drag selection.
        if let Some(md) = self.mouse_down.as_mut() {
            if md.button == MouseButton::Left {
                let dist = ((self.cursor.x - md.pos.x).powi(2) + (self.cursor.y - md.pos.y).powi(2)).sqrt();
                if dist > DRAG_THRESHOLD {
                    md.dragged = true;
                }
                if md.dragged {
                    if let (Some(anchor), Some(css)) = (md.doc, self.to_css_clamped(self.cursor)) {
                        let cur = self.to_doc(css);
                        let sel = self.page().map(|p| p.select_between(&self.engine.fonts, anchor, cur));
                        if let (Some(sel), Some(tab)) = (sel, self.tabs.get_mut(self.active)) {
                            tab.selection = if sel.rects.is_empty() { None } else { Some(sel) };
                        }
                        self.request_redraw();
                        return;
                    }
                }
            }
        }
        self.update_hover();
    }

    /// CSS coordinates of a window position, clamped into the page area
    /// (for drag selection outside the page).
    fn to_css_clamped(&self, pos: winit::dpi::PhysicalPosition<f64>) -> Option<(f32, f32)> {
        let area = self.chrome().page_area();
        let s = self.page_scale();
        let x = (pos.x as f32).clamp(area.x, area.right() - 1.0);
        let y = (pos.y as f32).clamp(area.y, area.bottom() - 1.0);
        Some(((x - area.x) / s, (y - area.y) / s))
    }

    /// Recompute hover state: toolbar buttons, tabs, page links/cursor.
    pub(crate) fn update_hover(&mut self) {
        let (x, y) = (self.cursor.x as f32, self.cursor.y as f32);
        let chrome = self.chrome();
        let mut icon = CursorIcon::Default;
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
        if chrome.url_field().contains(x, y) || (chrome.find_open && chrome.find_field().contains(x, y)) {
            icon = CursorIcon::Text;
        } else if button.is_some() || hover_tab.is_some() || hover_plus {
            icon = CursorIcon::Pointer;
        }

        let popup_open = self.menu.is_some() || self.context_menu.is_some() || self.dropdown.is_some();
        let mut link = None;
        if !popup_open {
            if let Some(css) = self.to_css(self.cursor) {
                let doc = self.to_doc(css);
                let node = self.page().and_then(|p| p.node_at_viewport(css.0, css.1));
                let prev = self.tabs.get(self.active).and_then(|t| t.hover_node);
                if prev != node && self.page().map(|p| p.has_scripts()).unwrap_or(false) {
                    if let Some(tab) = self.tabs.get_mut(self.active) {
                        tab.hover_node = node;
                    }
                    if let Some(p) = prev {
                        self.page_mouse_event(p, "mouseout", css, 0);
                    }
                    if let Some(n) = node {
                        self.page_mouse_event(n, "mouseover", css, 0);
                    }
                }
                if let Some(page) = self.tabs.get_mut(self.active).and_then(|t| t.page.as_mut()) {
                    if self.engine.set_hover(page, node) {
                        self.page_dirty = true;
                        redraw = true;
                    }
                    link = node.and_then(|n| page.link_of(n));
                    icon = match page.cursor_at(doc.0, doc.1) {
                        "pointer" => CursorIcon::Pointer,
                        "text" => CursorIcon::Text,
                        _ => CursorIcon::Default,
                    };
                    if link.is_some() {
                        icon = CursorIcon::Pointer;
                    }
                    if let Some(n) = node {
                        if matches!(page.control_kind(n), ControlKind::Text | ControlKind::Password | ControlKind::TextArea | ControlKind::Other) {
                            icon = CursorIcon::Text;
                        }
                    }
                }
            }
        }
        if icon != self.cursor_icon {
            self.cursor_icon = icon;
            if let Some(w) = &self.window {
                w.set_cursor(icon);
            }
        }
        if link != self.hover_link {
            self.hover_link = link;
            redraw = true;
        }
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

        // Page.
        self.blur_url_field();
        if self.ui_focus == UiFocus::Console {
            self.ui_focus = UiFocus::Page;
        }
        if self.ui_focus == UiFocus::Find {
            self.ui_focus = UiFocus::Page;
        }
        let Some(css) = self.to_css(self.cursor) else { return };
        let doc = self.to_doc(css);
        let node = self.page().and_then(|p| p.node_at_viewport(css.0, css.1));
        let link = node.and_then(|n| self.page().and_then(|p| p.link_of(n)));

        let button_code: u8 = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            _ => 0,
        };
        if let Some(n) = node {
            self.page_mouse_event(n, "mousedown", css, button_code);
        }
        if button == MouseButton::Right {
            let prevented = node.map(|n| self.page_mouse_event(n, "contextmenu", css, 2).default_prevented).unwrap_or(false);
            if !prevented {
                self.open_context_menu(link, x, y);
            }
            return;
        }
        if button == MouseButton::Middle {
            let prevented = node.map(|n| self.page_mouse_event(n, "auxclick", css, 1).default_prevented).unwrap_or(false);
            if let (Some(l), false) = (link, prevented) {
                self.follow_link(l, true);
            }
            return;
        }
        if button != MouseButton::Left {
            return;
        }
        // Scripts may have changed the page: hit-test again.
        let node = self.page().and_then(|p| p.node_at_viewport(css.0, css.1));
        let link = node.and_then(|n| self.page().and_then(|p| p.link_of(n)));

        // Double click → word selection.
        let now = Instant::now();
        let double = self
            .last_click
            .map(|(t, p)| now.duration_since(t) < Duration::from_millis(400) && (p.x - self.cursor.x).abs() < 5.0 && (p.y - self.cursor.y).abs() < 5.0)
            .unwrap_or(false);
        self.last_click = Some((now, self.cursor));

        // Form controls.
        let control = node.and_then(|n| self.page().and_then(|p| p.control_of(n)));
        if let Some(c) = control {
            let kind = self.page().map(|p| p.control_kind(c)).unwrap_or(ControlKind::Other);
            match kind {
                ControlKind::Text | ControlKind::Password | ControlKind::TextArea | ControlKind::Other => {
                    self.begin_edit(c);
                    self.place_caret_from_click(c, doc);
                    return;
                }
                ControlKind::Checkbox | ControlKind::Radio => {
                    self.end_edit();
                    self.page_mouse_event(c, "mouseup", css, 0);
                    if !self.page_mouse_event(c, "click", css, 0).default_prevented {
                        self.toggle_control_with_events(c);
                    }
                    return;
                }
                ControlKind::Select => {
                    self.end_edit();
                    self.open_select_dropdown(c);
                    return;
                }
                ControlKind::Submit => {
                    self.page_mouse_event(c, "mouseup", css, 0);
                    let out = self.page_mouse_event(c, "click", css, 0);
                    if !out.default_prevented && out.navigation.is_none() {
                        self.submit_form(c, Some(c));
                    }
                    return;
                }
                ControlKind::Reset => {
                    self.end_edit();
                    self.page_mouse_event(c, "mouseup", css, 0);
                    if !self.page_mouse_event(c, "click", css, 0).default_prevented {
                        let form = self.page().and_then(|p| p.form_of(c));
                        if let Some(f) = form {
                            self.mutate_page(|p| p.reset_form(f));
                            self.dispatch_to(f, "reset", plain_init(true));
                        }
                    }
                    return;
                }
                ControlKind::File => {
                    self.set_status("File uploads are not supported yet");
                    return;
                }
                ControlKind::Button | ControlKind::Hidden => {}
            }
        }
        self.end_edit();
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.focus = None;
        }
        if double {
            if let Some(n) = node {
                self.page_mouse_event(n, "dblclick", css, 0);
            }
            let sel = self.page().map(|p| p.select_word_at(&self.engine.fonts, doc.0, doc.1));
            if let Some(tab) = self.tabs.get_mut(self.active) {
                tab.selection = sel.filter(|s| !s.rects.is_empty());
            }
            self.request_redraw();
            return;
        }
        self.mouse_down = Some(MouseDown { button, pos: self.cursor, doc: Some(doc), link, dragged: false });
        self.mouse_down_node = node;
    }

    /// Dispatch a mouse event to a page node (viewport CSS coordinates).
    pub(crate) fn page_mouse_event(&mut self, node: NodeId, event_type: &str, css: (f32, f32), button: u8) -> rabbithole_engine::ScriptOutcome {
        let m = self.modifiers;
        let init = mouse_init(css.0, css.1, button, m.control_key(), m.shift_key(), m.alt_key(), m.super_key());
        self.dispatch_to(node, event_type, init)
    }

    pub(crate) fn handle_mouse_up(&mut self, button: MouseButton) {
        let Some(md) = self.mouse_down.take() else { return };
        if md.button != button {
            return;
        }
        if md.dragged {
            return; // selection stays
        }
        if let Some(tab) = self.tabs.get_mut(self.active) {
            if tab.selection.take().is_some() {
                self.request_redraw();
            }
        }
        // Click on a plain element: tell scripts first; a prevented click
        // (or one that navigated by itself) skips the link default action.
        let mut prevented = false;
        if let (Some(node), Some(css)) = (self.mouse_down_node.take(), self.to_css(self.cursor)) {
            self.page_mouse_event(node, "mouseup", css, 0);
            let out = self.page_mouse_event(node, "click", css, 0);
            prevented = out.default_prevented || out.navigation.is_some();
        }
        if let (Some(link), false) = (md.link, prevented) {
            let new_tab = self.modifiers.control_key() || self.modifiers.super_key();
            self.follow_link(link, new_tab);
        }
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
            let doc = self.to_doc(css);
            let scrolled_inner = match self.tabs.get_mut(self.active).and_then(|t| t.page.as_mut()) {
                Some(page) => self.engine.scroll_at(page, doc.0, doc.1, dx, dy),
                None => false,
            };
            if scrolled_inner {
                self.mark_page_dirty();
                return;
            }
        }
        self.scroll_by(dx, dy);
    }

    fn place_caret_from_click(&mut self, control: NodeId, doc: (f32, f32)) {
        let Some(page) = self.page() else { return };
        let Some(rect) = page.rect_of(control) else { return };
        let fonts = &self.engine.fonts;
        let text = page.control_text(control);
        let rel_x = doc.0 - rect.x;
        // Measure prefixes with the control's own run positions via caret_x.
        let mut best = text.len();
        let mut best_d = f32::MAX;
        for (i, _) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
            if let Some(cx) = page.caret_x(fonts, control, i) {
                let d = (cx - rel_x).abs();
                if d < best_d {
                    best_d = d;
                    best = i;
                }
            }
        }
        if let Some(edit) = self.tabs.get_mut(self.active).and_then(|t| t.edit.as_mut()) {
            edit.buf.set_caret(best, false);
        }
        self.request_redraw();
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
            MenuItem::new("Clear cookies", Action::ClearCookies).sep(),
            MenuItem::new("About RabbitHole", Action::About),
            MenuItem::new("Quit               Ctrl+Q", Action::Quit).sep(),
        ];
        let win = self.window_size();
        let menu = Menu::new(r.right() - 240.0 * chrome.s, r.bottom() + 4.0 * chrome.s, items, chrome.s, &self.engine.fonts, self.ui_font, win.width as f32, win.height as f32);
        self.menu = Some(menu);
        self.request_redraw();
    }

    fn open_context_menu(&mut self, link: Option<Url>, x: f32, y: f32) {
        let chrome = self.chrome();
        let has_sel = self.tab().map(|t| t.selection.is_some()).unwrap_or(false);
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
        self.request_redraw();
    }

    fn open_select_dropdown(&mut self, select: NodeId) {
        let chrome = self.chrome();
        let Some(page) = self.page() else { return };
        let options = page.select_options(select);
        let rect = page.rect_of(select).map(|r| self.doc_to_screen(r)).unwrap_or(Rect::new(self.cursor.x as f32, self.cursor.y as f32, 0.0, 0.0));
        let items: Vec<MenuItem> = options
            .iter()
            .enumerate()
            .map(|(i, (label, _, selected, disabled))| {
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
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.focus = Some(select);
        }
        self.dropdown = Some(menu);
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
            Action::CopySelection => {
                if let Some(text) = self.tab().and_then(|t| t.selection.as_ref()).map(|s| s.text.clone()) {
                    self.clipboard_set(&text);
                }
            }
            Action::ViewText => self.navigate_tab(idx, "about:text", true),
            Action::CopyPageUrl => {
                if let Some(u) = self.tab().and_then(|t| t.current_url.as_ref()).map(|u| u.to_string()) {
                    self.clipboard_set(&u);
                }
            }
            Action::SelectOption(node, i) => self.select_option_with_events(node, i),
            Action::ToggleBookmark => self.toggle_bookmark(),
            Action::ToggleJavaScript => self.toggle_javascript(),
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

    /// Send `keydown`/`keyup` to the page (focused control, else `body`).
    /// Returns true when a listener called `preventDefault()`.
    fn page_key_event(&mut self, event: &KeyEvent) -> bool {
        if self.ui_focus != UiFocus::Page || self.menu.is_some() || self.context_menu.is_some() || self.dropdown.is_some() {
            return false;
        }
        let Some(page) = self.page() else { return false };
        if !page.has_scripts() {
            return false;
        }
        let target = self.tab().and_then(|t| t.focus).or_else(|| page.doc.body());
        let Some(target) = target else { return false };
        let key = key_name(&event.logical_key);
        let code = format!("{:?}", event.physical_key).trim_start_matches("Code(").trim_end_matches(')').to_string();
        let m = self.modifiers;
        let init = rabbithole_engine::scripting::key_init(&key, &code, m.control_key(), m.shift_key(), m.alt_key(), m.super_key(), event.repeat);
        let ty = if event.state == ElementState::Pressed { "keydown" } else { "keyup" };
        let out = self.dispatch_to(target, ty, init);
        out.default_prevented || out.navigation.is_some()
    }

    pub(crate) fn handle_key(&mut self, event: &KeyEvent, event_loop: &ActiveEventLoop) {
        if event.state != ElementState::Pressed {
            self.page_key_event(event);
            return;
        }
        // Browser-level shortcuts are never cancellable by pages.
        let browser_shortcut = (self.modifiers.control_key() || self.modifiers.super_key())
            && matches!(&event.logical_key, Key::Character(c) if matches!(c.to_lowercase().as_str(), "q" | "w" | "t" | "l" | "r" | "j" | "h"))
            || matches!(&event.logical_key, Key::Named(NamedKey::F5 | NamedKey::F6 | NamedKey::F11));
        if !browser_shortcut && self.page_key_event(event) {
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

        // Global shortcuts.
        if ctrl {
            if let Key::Character(c) = &event.logical_key {
                match c.to_lowercase().as_str() {
                    "q" => return self.shutdown(event_loop),
                    "w" => return self.close_tab(idx, event_loop),
                    "t" => {
                        self.new_tab(None, false);
                        return;
                    }
                    "l" => return self.focus_url_field(),
                    "r" => return self.reload(shift),
                    "f" => return self.open_find(),
                    "d" => return self.toggle_bookmark(),
                    "h" => return self.navigate_tab(idx, "about:history", true),
                    "j" if shift => return self.toggle_console(),
                    "j" => return self.navigate_tab(idx, "about:downloads", true),
                    "o" if shift => return self.navigate_tab(idx, "about:bookmarks", true),
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
                if self.tab().map(|t| t.edit.is_some()).unwrap_or(false) {
                    self.handle_control_key(event, ctrl, shift);
                } else {
                    self.handle_page_key(event, ctrl, shift, event_loop);
                }
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
            UiFocus::Page => {
                let editing = self.tab().map(|t| t.edit.is_some()).unwrap_or(false);
                if editing {
                    if let Some(edit) = self.tabs.get_mut(self.active).and_then(|t| t.edit.as_mut()) {
                        let max = edit.max_len;
                        edit.buf.insert(text, max);
                    }
                    self.commit_edit();
                }
            }
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

    fn handle_control_key(&mut self, event: &KeyEvent, ctrl: bool, shift: bool) {
        let (node, kind) = match self.tab().and_then(|t| t.edit.as_ref()) {
            Some(e) => (e.node, e.kind),
            None => return,
        };
        let mut changed = false;
        {
            let edit = self.tabs.get_mut(self.active).and_then(|t| t.edit.as_mut()).unwrap();
            match &event.logical_key {
                Key::Named(NamedKey::Enter) => {
                    if kind == ControlKind::TextArea && !ctrl {
                        edit.buf.insert("\n", edit.max_len);
                        changed = true;
                    } else {
                        self.submit_form(node, None);
                        return;
                    }
                }
                Key::Named(NamedKey::Escape) => {
                    self.end_edit();
                    return;
                }
                Key::Named(NamedKey::Tab) => {
                    self.focus_next(!shift);
                    return;
                }
                Key::Named(NamedKey::Backspace) => {
                    edit.buf.backspace(ctrl);
                    changed = true;
                }
                Key::Named(NamedKey::Delete) => {
                    edit.buf.delete(ctrl);
                    changed = true;
                }
                Key::Named(NamedKey::ArrowLeft) => edit.buf.move_left(shift, ctrl),
                Key::Named(NamedKey::ArrowRight) => edit.buf.move_right(shift, ctrl),
                Key::Named(NamedKey::Home) => edit.buf.home(shift),
                Key::Named(NamedKey::End) => edit.buf.end(shift),
                Key::Named(NamedKey::Space) => {
                    edit.buf.insert(" ", edit.max_len);
                    changed = true;
                }
                Key::Character(c) if ctrl => match c.to_lowercase().as_str() {
                    "a" => edit.buf.select_all(),
                    "c" => {
                        if let Some(t) = edit.buf.selected_text().map(|s| s.to_string()) {
                            self.clipboard_set(&t);
                            return;
                        }
                    }
                    "x" => {
                        if let Some(t) = edit.buf.selected_text().map(|s| s.to_string()) {
                            edit.buf.delete_selection();
                            changed = true;
                            self.clipboard_set(&t);
                        }
                    }
                    "v" => {
                        if let Some(t) = self.clipboard_get() {
                            if let Some(edit) = self.tabs.get_mut(self.active).and_then(|t| t.edit.as_mut()) {
                                let max = edit.max_len;
                                edit.buf.insert(&t, max);
                            }
                            changed = true;
                        }
                    }
                    _ => {}
                },
                _ => {
                    if let Some(text) = event.text.as_ref() {
                        if !ctrl {
                            edit.buf.insert(text, edit.max_len);
                            changed = true;
                        }
                    }
                }
            }
        }
        if changed {
            self.commit_edit();
        }
        self.request_redraw();
    }

    fn handle_page_key(&mut self, event: &KeyEvent, ctrl: bool, shift: bool, _event_loop: &ActiveEventLoop) {
        let vp = self.css_viewport();
        let page_step = (vp.height - LINE_SCROLL).max(LINE_SCROLL);
        match &event.logical_key {
            Key::Named(NamedKey::ArrowDown) => self.scroll_by(0.0, LINE_SCROLL),
            Key::Named(NamedKey::ArrowUp) => self.scroll_by(0.0, -LINE_SCROLL),
            Key::Named(NamedKey::ArrowLeft) => self.scroll_by(-LINE_SCROLL, 0.0),
            Key::Named(NamedKey::ArrowRight) => self.scroll_by(LINE_SCROLL, 0.0),
            Key::Named(NamedKey::PageDown) => self.scroll_by(0.0, page_step),
            Key::Named(NamedKey::PageUp) => self.scroll_by(0.0, -page_step),
            Key::Named(NamedKey::Space) => {
                let focused_checkbox = self.tab().and_then(|t| t.focus).filter(|&n| {
                    self.page().map(|p| matches!(p.control_kind(n), ControlKind::Checkbox | ControlKind::Radio)).unwrap_or(false)
                });
                if let Some(n) = focused_checkbox {
                    self.toggle_control_with_events(n);
                } else {
                    self.scroll_by(0.0, if shift { -page_step } else { page_step });
                }
            }
            Key::Named(NamedKey::Home) => self.set_scroll(0.0, 0.0),
            Key::Named(NamedKey::End) => self.set_scroll(0.0, f32::MAX),
            Key::Named(NamedKey::Backspace) => self.go_history(if shift { 1 } else { -1 }),
            Key::Named(NamedKey::Tab) => self.focus_next(!shift),
            Key::Named(NamedKey::Enter) => {
                let focused = self.tab().and_then(|t| t.focus);
                if let Some(n) = focused {
                    let link = self.page().and_then(|p| p.link_of(n));
                    if let Some(l) = link {
                        self.follow_link(l, ctrl);
                    } else if let Some(kind) = self.page().map(|p| p.control_kind(n)) {
                        match kind {
                            ControlKind::Submit => self.submit_form(n, Some(n)),
                            ControlKind::Checkbox | ControlKind::Radio => self.toggle_control_with_events(n),
                            ControlKind::Select => self.open_select_dropdown(n),
                            _ => {}
                        }
                    }
                }
            }
            Key::Named(NamedKey::Escape) => {
                if self.tab().map(|t| t.is_loading()).unwrap_or(false) {
                    self.stop_loading();
                } else if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.selection = None;
                    tab.focus = None;
                    self.status = None;
                    self.request_redraw();
                }
            }
            Key::Character(c) if ctrl => match c.to_lowercase().as_str() {
                "c" => {
                    if let Some(text) = self.tab().and_then(|t| t.selection.as_ref()).map(|s| s.text.clone()) {
                        self.clipboard_set(&text);
                    }
                }
                "a" => {
                    // Select all text on the page.
                    let sel = self.page().map(|p| {
                        let h = p.content_height();
                        let w = p.content_width();
                        p.select_between(&self.engine.fonts, (0.0, 0.0), (w, h))
                    });
                    if let Some(tab) = self.tabs.get_mut(self.active) {
                        tab.selection = sel.filter(|s| !s.rects.is_empty());
                    }
                    self.request_redraw();
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// Move keyboard focus to the next/previous link or control.
    fn focus_next(&mut self, forward: bool) {
        let Some(page) = self.page() else { return };
        let doc = &page.doc;
        let focusables: Vec<NodeId> = doc
            .descendants(rabbithole_engine::dom::Document::ROOT)
            .filter(|&n| page.styles.get(n).is_some())
            .filter(|&n| {
                rabbithole_engine::forms::is_interactive(doc, n) || (doc.is_element(n, "a") && doc.attr(n, "href").is_some())
            })
            .collect();
        if focusables.is_empty() {
            return;
        }
        let current = self.tab().and_then(|t| t.edit.as_ref().map(|e| e.node).or(t.focus));
        let pos = current.and_then(|c| focusables.iter().position(|&n| n == c));
        let next = match (pos, forward) {
            (Some(p), true) => focusables[(p + 1) % focusables.len()],
            (Some(p), false) => focusables[(p + focusables.len() - 1) % focusables.len()],
            (None, true) => focusables[0],
            (None, false) => focusables[focusables.len() - 1],
        };
        let kind = page.control_kind(next);
        let rect = page.rect_of(next);
        self.end_edit();
        if matches!(kind, ControlKind::Text | ControlKind::Password | ControlKind::TextArea | ControlKind::Other) && rabbithole_engine::forms::is_interactive(&self.page().unwrap().doc, next) {
            self.begin_edit(next);
        } else {
            if let Some(page) = self.tabs.get_mut(self.active).and_then(|t| t.page.as_mut()) {
                self.engine.set_focus(page, Some(next));
            }
            if let Some(tab) = self.tabs.get_mut(self.active) {
                tab.focus = Some(next);
            }
        }
        if let Some(r) = rect {
            self.scroll_into_view(r);
        }
        self.mark_page_dirty();
    }

    // ----------------------------------------------------------------- find

    pub(crate) fn open_find(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        tab.find.open = true;
        tab.find.field.select_all();
        self.ui_focus = UiFocus::Find;
        self.needs_relayout = true;
        self.mark_page_dirty();
    }

    pub(crate) fn close_find(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.find.open = false;
            tab.find.matches.clear();
        }
        self.ui_focus = UiFocus::Page;
        self.needs_relayout = true;
        self.mark_page_dirty();
    }

    pub(crate) fn refresh_find(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let query = tab.find.field.text.clone();
        let matches = tab.page.as_ref().map(|p| p.find_text(&self.engine.fonts, &query)).unwrap_or_default();
        tab.find.current = tab.find.current.min(matches.len().saturating_sub(1));
        tab.find.matches = matches;
        if let Some(r) = tab.find.matches.get(tab.find.current).copied() {
            self.scroll_into_view(r);
        }
        self.request_redraw();
    }

    pub(crate) fn find_step(&mut self, delta: isize) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let n = tab.find.matches.len();
        if n == 0 {
            return;
        }
        tab.find.current = ((tab.find.current as isize + delta).rem_euclid(n as isize)) as usize;
        let r = tab.find.matches[tab.find.current];
        self.scroll_into_view(r);
        self.request_redraw();
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
