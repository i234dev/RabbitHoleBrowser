//! The engine ties everything together: fetch → parse → style → layout →
//! paint, plus hit testing and text extraction for CLI/MCP use.

use rabbithole_dom::{Document, NodeId};
use rabbithole_gfx::{Canvas, FontContext, Image, Rect};
use rabbithole_layout::{DisplayList, LayoutTree};
use rabbithole_net::{Fetcher, Url};
use rabbithole_style::{StyledTree, Stylesheet};
use std::collections::HashMap;
use std::rc::Rc;

pub use rabbithole_css as css;
pub use rabbithole_dom as dom;
pub use rabbithole_gfx as gfx;
pub use rabbithole_html as html;
pub use rabbithole_layout as layout;
pub use rabbithole_net as net;
pub use rabbithole_style as style;

#[derive(Debug, Clone)]
pub enum EngineError {
    BadUrl(String),
    Fetch(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::BadUrl(u) => write!(f, "invalid URL: {u}"),
            EngineError::Fetch(e) => write!(f, "fetch failed: {e}"),
        }
    }
}
impl std::error::Error for EngineError {}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
}

/// Shared, long-lived state: fonts, image cache, network settings.
pub struct Engine {
    pub fonts: Rc<FontContext>,
    pub fetcher: Fetcher,
    pub load_images: bool,
    pub load_stylesheets: bool,
    image_cache: HashMap<String, Option<Rc<Image>>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub text: String,
    pub href: Url,
}

/// A loaded, styled and laid out document.
pub struct Page {
    pub url: Url,
    pub doc: Document,
    pub title: String,
    pub sheets: Vec<Stylesheet>,
    pub styles: StyledTree,
    pub layout: LayoutTree,
    pub display_list: DisplayList,
    pub images: HashMap<NodeId, Rc<Image>>,
    pub background_images: HashMap<String, Rc<Image>>,
    pub viewport: Viewport,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Engine {
        Engine {
            fonts: Rc::new(FontContext::new()),
            fetcher: Fetcher::default(),
            load_images: true,
            load_stylesheets: true,
            image_cache: HashMap::new(),
        }
    }

    /// Fetch and fully prepare a page. `url` may be anything a user would
    /// type (bare domains get `https://`).
    pub fn load(&mut self, url: &str, viewport: Viewport) -> Result<Page, EngineError> {
        let _ = (url, viewport);
        unimplemented!()
    }

    /// Prepare a page from HTML source with the given base URL.
    pub fn load_html(&mut self, html: &str, base: Url, viewport: Viewport) -> Page {
        let _ = (html, base, viewport);
        unimplemented!()
    }

    /// Re-run style + layout (after a viewport change).
    pub fn relayout(&mut self, page: &mut Page, viewport: Viewport) {
        let _ = (page, viewport);
        unimplemented!()
    }

    /// Paint the page region starting at (scroll_x, scroll_y) into `canvas`.
    pub fn render(&self, page: &Page, canvas: &mut Canvas, scroll_x: f32, scroll_y: f32) {
        let _ = (page, canvas, scroll_x, scroll_y);
        unimplemented!()
    }
}

impl Page {
    pub fn content_height(&self) -> f32 {
        self.layout.height
    }
    pub fn content_width(&self) -> f32 {
        self.layout.width
    }

    /// Element at document coordinates.
    pub fn node_at(&self, x: f32, y: f32) -> Option<NodeId> {
        self.layout.node_at(x, y)
    }

    /// Resolved href of the link under the point, if any.
    pub fn link_at(&self, x: f32, y: f32) -> Option<Url> {
        let _ = (x, y);
        unimplemented!()
    }

    /// Document-space rectangle of the first element matching `#id` (for
    /// fragment navigation).
    pub fn anchor_rect(&self, id: &str) -> Option<Rect> {
        let _ = id;
        unimplemented!()
    }

    /// All links in document order.
    pub fn links(&self) -> Vec<Link> {
        unimplemented!()
    }

    /// Readable text of the page as Markdown-ish plain text (headings,
    /// paragraphs, lists, links), for CLI/MCP consumers.
    pub fn text(&self) -> String {
        unimplemented!()
    }

    pub fn dom_dump(&self) -> String {
        self.doc.dump()
    }

    pub fn layout_dump(&self) -> String {
        self.layout.dump()
    }
}
