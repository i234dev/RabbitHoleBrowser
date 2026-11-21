//! Per-tab state on the UI thread. The page itself lives on the tab's
//! content thread; the UI keeps what it needs to draw and route input.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::gui::content::{ContentHandle, DomRow, Frame, NodeDetails, PageState, ToContent, ViewSize};
use crate::gui::input::TextBuffer;
use crate::scripting::ConsoleLog;
use rabbithole_engine::gfx::Image;
use rabbithole_engine::net::Url;

/// Requests kept per tab for the devtools Network view.
pub const MAX_NETWORK: usize = 1000;

/// Devtools state of a tab (Elements and Network views).
#[derive(Debug, Default)]
pub struct Inspector {
    pub network: Vec<rabbithole_engine::net::NetEvent>,
    /// When the current page started loading (request times are shown
    /// relative to it).
    pub dom: Vec<DomRow>,
    pub details: NodeDetails,
    /// Selected row in the Elements / Network list.
    pub selected: Option<usize>,
    /// First visible row of the Elements / Network list.
    pub scroll: usize,
    pub network_scroll: usize,
    pub network_selected: Option<usize>,
}

/// Find-in-page state (the matches live on the content thread).
#[derive(Debug, Default)]
pub struct FindState {
    pub open: bool,
    pub field: TextBuffer,
    pub count: usize,
    pub current: usize,
}

pub struct Tab {
    pub id: u64,
    /// Pinned tabs sit first in the strip, icon-only, and survive "close
    /// other tabs".
    pub pinned: bool,
    /// The tab's content thread (None only in unit tests).
    pub content: Option<ContentHandle>,
    /// Latest painted frame.
    pub frame: Option<Frame>,
    pub title: String,
    /// URL being shown (for the address bar).
    pub current_url: Option<Url>,
    pub history: Vec<Url>,
    pub index: usize,
    /// The document each history entry belongs to (`document` values;
    /// 0 or missing: unknown, e.g. restored entries). Entries of the
    /// current document are traversed without reloading it.
    pub history_docs: Vec<u64>,
    /// The document currently shown (incremented per commit; 0: none).
    pub document: u64,
    /// Scroll position in CSS px (the UI is the authority; the content
    /// thread follows).
    pub scroll: (f32, f32),
    pub zoom: f32,
    /// Navigation requested, document not committed yet.
    pub loading: Option<Url>,
    /// Loading after the commit (scripts, images).
    pub load_phase: Option<&'static str>,
    pub find: FindState,
    /// JavaScript console messages of the current page.
    pub console: ConsoleLog,
    /// Console input line.
    pub console_input: TextBuffer,
    /// Console scroll offset in visual rows from the bottom.
    pub console_scroll: usize,
    /// A selected log message takes keyboard focus from the input field.
    pub console_selected: Option<u64>,
    pub console_expanded: BTreeSet<u64>,
    pub favicon: Option<Arc<Image>>,
    /// Input routing hints from the page.
    pub state: PageState,
    /// Cursor over the page ("default", "pointer", "text").
    pub cursor: &'static str,
    pub hover_link: Option<Url>,
    /// View size last sent to the content thread.
    pub view: Option<ViewSize>,
    /// The "not responding" bar was dismissed for the busy period that
    /// started at this time (content clock, ms).
    pub hang_dismissed: Option<u64>,
    pub inspector: Inspector,
    /// When the last navigation started (spinner animation phase).
    pub load_started: Option<std::time::Instant>,
    /// The page's cookie dialog was handled automatically (URL-bar
    /// indicator), and when.
    pub consent: Option<(crate::autocookies::ConsentEvent, std::time::Instant)>,
}

impl Tab {
    pub fn new(id: u64) -> Tab {
        Tab {
            id,
            pinned: false,
            content: None,
            frame: None,
            title: String::from("New tab"),
            current_url: None,
            history: Vec::new(),
            index: 0,
            history_docs: Vec::new(),
            document: 0,
            scroll: (0.0, 0.0),
            zoom: 1.0,
            loading: None,
            load_phase: None,
            find: FindState::default(),
            console: ConsoleLog::default(),
            console_input: TextBuffer::default(),
            console_scroll: 0,
            console_selected: None,
            console_expanded: BTreeSet::new(),
            favicon: None,
            state: PageState::default(),
            cursor: "default",
            hover_link: None,
            view: None,
            hang_dismissed: None,
            inspector: Inspector::default(),
            load_started: None,
            consent: None,
        }
    }

    /// Send a message to the content thread.
    pub fn send(&self, msg: ToContent) {
        if let Some(c) = &self.content {
            c.send(msg);
        }
    }

    pub fn display_url(&self) -> String {
        match (&self.loading, &self.current_url) {
            (Some(u), _) => u.to_string(),
            (None, Some(u)) => u.to_string(),
            _ => String::new(),
        }
    }

    pub fn display_title(&self) -> &str {
        if self.title.trim().is_empty() {
            "New tab"
        } else {
            self.title.trim()
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.index > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.index + 1 < self.history.len()
    }

    pub fn is_loading(&self) -> bool {
        self.loading.is_some() || self.load_phase.is_some()
    }

    /// Document size (CSS px) of the last frame.
    pub fn content_size(&self) -> (f32, f32) {
        self.frame.as_ref().map(|f| f.content).unwrap_or((0.0, 0.0))
    }

    /// Maximum vertical scroll for a CSS viewport height.
    pub fn max_scroll(&self, viewport_h: f32) -> f32 {
        (self.content_size().1 - viewport_h).max(0.0)
    }

    pub fn max_scroll_x(&self, viewport_w: f32) -> f32 {
        (self.content_size().0 - viewport_w).max(0.0)
    }

    /// Push a URL onto the history, truncating any forward entries. The
    /// entry belongs to the current document.
    pub fn push_history(&mut self, url: Url) {
        if self.history.get(self.index) == Some(&url) && !self.history.is_empty() {
            self.set_entry_document(self.index);
            return;
        }
        if !self.history.is_empty() {
            self.history.truncate(self.index + 1);
        }
        self.history.push(url);
        self.index = self.history.len() - 1;
        self.set_entry_document(self.index);
    }

    /// Replace the current entry's URL (redirects, `replaceState`); it now
    /// belongs to the current document.
    pub fn replace_history(&mut self, url: Url) {
        if self.history.is_empty() {
            return self.push_history(url);
        }
        let i = self.index.min(self.history.len() - 1);
        self.history[i] = url;
        self.set_entry_document(i);
    }

    fn set_entry_document(&mut self, i: usize) {
        self.history_docs.resize(self.history.len(), 0);
        if let Some(d) = self.history_docs.get_mut(i) {
            *d = self.document;
        }
    }

    /// The document moved itself `delta` entries through its own history,
    /// now showing `url`. Entries the list does not have for this document
    /// become a replacement of the current one.
    pub fn follow_traversal(&mut self, delta: isize, url: Url) {
        let target = self.index as isize + delta;
        if target >= 0 && self.same_document_entry(target as usize) {
            self.index = target as usize;
            self.history[self.index] = url.clone();
        } else {
            self.replace_history(url.clone());
        }
        self.current_url = Some(url);
    }

    /// A new document was committed.
    pub fn new_document(&mut self) {
        self.document += 1;
    }

    /// Whether history entry `target` belongs to the document shown, so
    /// going there is a same-document traversal (no reload).
    pub fn same_document_entry(&self, target: usize) -> bool {
        self.document != 0
            && self.loading.is_none()
            && self.history.len() == self.history_docs.len()
            && self.history_docs.get(target) == Some(&self.document)
            && self.history_docs.get(self.index) == Some(&self.document)
    }

    /// Clear per-page UI state when a new document replaces the old one.
    pub fn reset_page_state(&mut self) {
        self.find.count = 0;
        self.find.current = 0;
        self.scroll = (0.0, 0.0);
        self.clear_console();
        self.favicon = None;
        self.state = PageState::default();
        self.cursor = "default";
        self.hover_link = None;
        self.inspector.dom.clear();
        self.inspector.details = NodeDetails::default();
        self.inspector.selected = None;
        self.inspector.scroll = 0;
    }

    pub fn clear_console(&mut self) {
        self.console.clear();
        self.console_scroll = 0;
        self.console_selected = None;
        self.console_expanded.clear();
    }

    /// Drop UI references to messages removed by the bounded log or clear.
    pub fn prune_console_state(&mut self) {
        let console = &self.console;
        self.console_expanded.retain(|id| console.get(*id).is_some());
        if self.console_selected.is_some_and(|id| self.console.get(id).is_none()) {
            self.console_selected = None;
        }
        if self.console.is_empty() {
            self.console_scroll = 0;
        }
    }

    /// Copy the original message, including newlines, or the input selection.
    pub fn console_copy_text(&self) -> Option<&str> {
        match self.console_selected {
            Some(id) => self.console.get(id).map(|(_, text)| text.as_str()),
            None => self.console_input.selected_text(),
        }
    }

    /// Snapshot every retained message in order, preserving each raw payload.
    /// A single LF separates messages; existing newlines are not normalized.
    pub fn console_copy_all_text(&self) -> Option<String> {
        if self.console.is_empty() {
            return None;
        }
        Some(self.console.lines().map(|(_, text)| text.as_str()).collect::<Vec<_>>().join("\n"))
    }

    /// Record a network request (oldest dropped past the limit).
    pub fn log_request(&mut self, ev: rabbithole_engine::net::NetEvent) {
        self.inspector.network.push(ev);
        if self.inspector.network.len() > MAX_NETWORK {
            self.inspector.network.remove(0);
        }
    }

    /// The back/forward list as strings (session saving).
    pub fn history_strings(&self) -> Vec<String> {
        self.history.iter().map(|u| u.to_string()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_selection_survives_append_and_clears_after_eviction_or_navigation() {
        let mut t = Tab::new(1);
        t.console.push("error", "original\nstack");
        let id = t.console.entries().next().unwrap().0;
        t.console_selected = Some(id);
        t.console_expanded.insert(id);
        t.console_input.set_text("input selection");
        t.console_input.select_all();
        t.console.push("log", "next");
        t.prune_console_state();
        assert_eq!(t.console_copy_text(), Some("original\nstack"));
        assert!(t.console_expanded.contains(&id));
        for _ in 0..crate::scripting::CONSOLE_CAPACITY {
            t.console.push("log", "duplicate\nmessage");
        }
        t.prune_console_state();
        assert_eq!(t.console_selected, None);
        assert!(t.console_expanded.is_empty());
        assert_eq!(t.console_copy_text(), Some("input selection"));
        let id = t.console.entries().next().unwrap().0;
        t.console_selected = Some(id);
        t.console_expanded.insert(id);
        t.console_scroll = 12;
        t.reset_page_state();
        assert!(t.console.is_empty());
        assert_eq!(t.console_selected, None);
        assert!(t.console_expanded.is_empty());
        assert_eq!(t.console_scroll, 0);
        t.console.push("error", "new document\nstack");
        assert!(t.console.entries().next().unwrap().0 > id);
        t.console_selected = Some(t.console.entries().next().unwrap().0);
        t.console_expanded.insert(t.console_selected.unwrap());
        t.clear_console();
        assert_eq!(t.console_selected, None);
        assert!(t.console_expanded.is_empty());
        assert_eq!(t.console_scroll, 0);
    }

    #[test]
    fn history_push() {
        let mut t = Tab::new(1);
        let a = Url::parse("https://a/").unwrap();
        let b = Url::parse("https://b/").unwrap();
        let c = Url::parse("https://c/").unwrap();
        t.push_history(a.clone());
        t.push_history(b.clone());
        assert!(t.can_go_back() && !t.can_go_forward());
        t.index = 0;
        t.push_history(c.clone());
        assert_eq!(t.history, vec![a, c]);
        assert_eq!(t.index, 1);
        // Pushing the current URL again is a no-op (reload).
        t.push_history(t.history[1].clone());
        assert_eq!(t.history.len(), 2);
    }
}
