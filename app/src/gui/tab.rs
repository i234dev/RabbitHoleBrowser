//! Per-tab state.

use crate::gui::input::TextBuffer;
use rabbithole_engine::dom::NodeId;
use rabbithole_engine::gfx::Rect;
use rabbithole_engine::net::Url;
use rabbithole_engine::{ControlKind, Page, Selection};

/// Find-in-page state.
#[derive(Debug, Default)]
pub struct FindState {
    pub open: bool,
    pub field: TextBuffer,
    pub matches: Vec<Rect>,
    pub current: usize,
}

/// A text control being edited.
#[derive(Debug)]
pub struct ControlEdit {
    pub node: NodeId,
    pub buf: TextBuffer,
    pub kind: ControlKind,
    pub max_len: Option<usize>,
}

pub struct Tab {
    pub id: u64,
    pub page: Option<Page>,
    pub title: String,
    /// URL being shown or loaded (for the address bar).
    pub current_url: Option<Url>,
    pub history: Vec<Url>,
    pub index: usize,
    /// Scroll position in CSS px.
    pub scroll: (f32, f32),
    pub zoom: f32,
    /// Incremented per navigation; stale load results are dropped.
    pub generation: u64,
    pub loading: Option<Url>,
    pub selection: Option<Selection>,
    pub find: FindState,
    pub edit: Option<ControlEdit>,
    /// Keyboard-focused element (focus ring).
    pub focus: Option<NodeId>,
    /// The page was laid out for a different viewport.
    pub needs_relayout: bool,
}

impl Tab {
    pub fn new(id: u64) -> Tab {
        Tab {
            id,
            page: None,
            title: String::from("New tab"),
            current_url: None,
            history: Vec::new(),
            index: 0,
            scroll: (0.0, 0.0),
            zoom: 1.0,
            generation: 0,
            loading: None,
            selection: None,
            find: FindState::default(),
            edit: None,
            focus: None,
            needs_relayout: false,
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
        self.loading.is_some()
    }

    /// Maximum vertical scroll for a CSS viewport height.
    pub fn max_scroll(&self, viewport_h: f32) -> f32 {
        let content = self.page.as_ref().map(|p| p.content_height()).unwrap_or(0.0);
        (content - viewport_h).max(0.0)
    }

    pub fn max_scroll_x(&self, viewport_w: f32) -> f32 {
        let content = self.page.as_ref().map(|p| p.content_width()).unwrap_or(0.0);
        (content - viewport_w).max(0.0)
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

    /// Clear transient per-page UI state when a new page replaces the old one.
    pub fn reset_page_state(&mut self) {
        self.selection = None;
        self.edit = None;
        self.focus = None;
        self.find.matches.clear();
        self.find.current = 0;
        self.scroll = (0.0, 0.0);
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
