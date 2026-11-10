//! Per-tab state on the UI thread. The page itself lives on the tab's
//! content thread; the UI keeps what it needs to draw and route input.

use std::sync::Arc;

use crate::gui::content::{ContentHandle, Frame, PageState, ToContent, ViewSize};
use crate::gui::input::TextBuffer;
use crate::scripting::ConsoleLog;
use rabbithole_engine::gfx::Image;
use rabbithole_engine::net::Url;

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
    /// The tab's content thread (None only in unit tests).
    pub content: Option<ContentHandle>,
    /// Latest painted frame.
    pub frame: Option<Frame>,
    pub title: String,
    /// URL being shown (for the address bar).
    pub current_url: Option<Url>,
    pub history: Vec<Url>,
    pub index: usize,
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
    /// Console scroll offset in lines from the bottom.
    pub console_scroll: usize,
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
}

impl Tab {
    pub fn new(id: u64) -> Tab {
        Tab {
            id,
            content: None,
            frame: None,
            title: String::from("New tab"),
            current_url: None,
            history: Vec::new(),
            index: 0,
            scroll: (0.0, 0.0),
            zoom: 1.0,
            loading: None,
            load_phase: None,
            find: FindState::default(),
            console: ConsoleLog::default(),
            console_input: TextBuffer::default(),
            console_scroll: 0,
            favicon: None,
            state: PageState::default(),
            cursor: "default",
            hover_link: None,
            view: None,
            hang_dismissed: None,
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

    /// Push a URL onto the history, truncating any forward entries.
    pub fn push_history(&mut self, url: Url) {
        if self.history.get(self.index) == Some(&url) && !self.history.is_empty() {
            return;
        }
        if !self.history.is_empty() {
            self.history.truncate(self.index + 1);
        }
        self.history.push(url);
        self.index = self.history.len() - 1;
    }

    /// Clear per-page UI state when a new document replaces the old one.
    pub fn reset_page_state(&mut self) {
        self.find.count = 0;
        self.find.current = 0;
        self.scroll = (0.0, 0.0);
        self.console.clear();
        self.console_scroll = 0;
        self.favicon = None;
        self.state = PageState::default();
        self.cursor = "default";
        self.hover_link = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
