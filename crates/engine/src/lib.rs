//! The engine ties everything together: fetch → parse → style → layout →
//! paint, plus hit testing and text extraction for CLI/MCP use.

mod home;
mod text;

use rabbithole_css::{parse_media_query, MediaQuery, Rule};
use rabbithole_dom::{Document, NodeData, NodeId};
use rabbithole_gfx::{Canvas, Color, FontContext, Image, Rect};
use rabbithole_layout::{DisplayList, LayoutBox, LayoutContext, LayoutTree};
use rabbithole_net::{decode_text, FetchError, Fetcher, Response, Url};
use rabbithole_style::{MediaEnv, StyledTree, Stylesheet};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub use rabbithole_css as css;
pub use rabbithole_dom as dom;
pub use rabbithole_gfx as gfx;
pub use rabbithole_html as html;
pub use rabbithole_layout as layout;
pub use rabbithole_net as net;
pub use rabbithole_style as style;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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
    /// Upper bound on `<img>` elements fetched per page.
    pub max_images: usize,
    /// Upper bound on CSS background images fetched per page.
    pub max_background_images: usize,
    image_cache: HashMap<String, Option<Rc<Image>>>,
    css_cache: HashMap<String, Rc<str>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub text: String,
    pub href: Url,
}

/// A loaded, styled and laid out document.
pub struct Page {
    pub url: Url,
    /// Base for relative URLs (`<base href>` or the document URL).
    pub base_url: Url,
    pub doc: Document,
    pub title: String,
    pub sheets: Vec<Stylesheet>,
    pub styles: StyledTree,
    pub layout: LayoutTree,
    pub display_list: DisplayList,
    pub images: HashMap<NodeId, Rc<Image>>,
    pub background_images: HashMap<String, Rc<Image>>,
    pub viewport: Viewport,
    /// Canvas background (propagated from `html`/`body`).
    pub background: Color,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

const MAX_IMPORT_DEPTH: usize = 4;

impl Engine {
    pub fn new() -> Engine {
        Engine {
            fonts: Rc::new(FontContext::new()),
            fetcher: Fetcher::default(),
            load_images: true,
            load_stylesheets: true,
            max_images: 80,
            max_background_images: 24,
            image_cache: HashMap::new(),
            css_cache: HashMap::new(),
        }
    }

    /// Fetch and fully prepare a page. `url` may be anything a user would
    /// type (bare domains get `https://`).
    pub fn load(&mut self, url: &str, viewport: Viewport) -> Result<Page, EngineError> {
        let url = Url::parse(url).ok_or_else(|| EngineError::BadUrl(url.to_string()))?;
        if url.scheme == "about" {
            let html = match url.path.as_str() {
                "home" | "" => home::HOME_HTML,
                _ => "",
            };
            return Ok(self.load_html(html, url, viewport));
        }
        let resp = self
            .fetcher
            .fetch(&url)
            .map_err(|e| EngineError::Fetch(e.to_string()))?;
        Ok(self.load_response(resp, viewport))
    }

    /// Turn an HTTP(-like) response into a page based on its content type.
    pub fn load_response(&mut self, resp: Response, viewport: Viewport) -> Page {
        let ct = resp.content_type.as_str();
        let url = resp.url.clone();
        if ct.starts_with("text/html") || ct.starts_with("application/xhtml") || ct.is_empty() {
            let html = decode_text(&resp.body, resp.charset.as_deref());
            return self.load_html(&html, url, viewport);
        }
        if ct.starts_with("image/") {
            let html = format!(
                "<html><head><title>{}</title></head><body style=\"margin:0;background:#222\"><img src=\"{}\" alt=\"image\"></body></html>",
                escape_html(&url.to_string()),
                escape_html(&url.to_string())
            );
            return self.load_html(&html, url, viewport);
        }
        if ct.starts_with("text/") || ct.contains("json") || ct.contains("xml") || ct.contains("javascript") {
            let text = decode_text(&resp.body, resp.charset.as_deref());
            let html = format!(
                "<html><head><title>{}</title></head><body><pre style=\"white-space:pre-wrap;word-wrap:break-word\">{}</pre></body></html>",
                escape_html(&url.to_string()),
                escape_html(&text)
            );
            return self.load_html(&html, url, viewport);
        }
        let html = format!(
            "<html><head><title>Unsupported content</title></head><body><h1>Cannot display this content</h1><p><code>{}</code> ({} bytes) is of type <code>{}</code>, which RabbitHole cannot render.</p></body></html>",
            escape_html(&url.to_string()),
            resp.body.len(),
            escape_html(ct)
        );
        self.load_html(&html, url, viewport)
    }

    /// Prepare a page from HTML source with the given base URL.
    pub fn load_html(&mut self, html: &str, url: Url, viewport: Viewport) -> Page {
        let doc = rabbithole_html::parse(html);
        let title = doc.title().unwrap_or_default();
        let base_url = doc
            .find_element("base")
            .and_then(|b| doc.attr(b, "href"))
            .and_then(|h| url.join(h))
            .unwrap_or_else(|| url.clone());

        let sheets = self.collect_stylesheets(&doc, &base_url);
        let env = media_env(viewport);
        let styles = rabbithole_style::style_tree(&doc, &sheets, &env);

        let images = if self.load_images {
            self.load_images(&doc, &styles, &base_url)
        } else {
            HashMap::new()
        };
        let background_images = if self.load_images {
            self.load_background_images(&styles, &base_url)
        } else {
            HashMap::new()
        };

        let (layout, display_list) = self.layout_page(&doc, &styles, &images, &background_images, viewport);
        let background = canvas_background(&doc, &styles);
        Page {
            url,
            base_url,
            doc,
            title,
            sheets,
            styles,
            layout,
            display_list,
            images,
            background_images,
            viewport,
            background,
        }
    }

    /// Re-run style + layout (after a viewport change).
    pub fn relayout(&mut self, page: &mut Page, viewport: Viewport) {
        page.viewport = viewport;
        let env = media_env(viewport);
        page.styles = rabbithole_style::style_tree(&page.doc, &page.sheets, &env);
        let (layout, display_list) =
            self.layout_page(&page.doc, &page.styles, &page.images, &page.background_images, viewport);
        page.layout = layout;
        page.display_list = display_list;
        page.background = canvas_background(&page.doc, &page.styles);
    }

    /// Paint the page region starting at (scroll_x, scroll_y) into `canvas`.
    pub fn render(&self, page: &Page, canvas: &mut Canvas, scroll_x: f32, scroll_y: f32) {
        canvas.clear(Color::WHITE);
        if page.background.a > 0 {
            let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
            canvas.fill_rect(full, page.background);
        }
        rabbithole_layout::paint(&page.display_list, canvas, &self.fonts, scroll_x, scroll_y);
    }

    fn layout_page(
        &self,
        doc: &Document,
        styles: &StyledTree,
        images: &HashMap<NodeId, Rc<Image>>,
        background_images: &HashMap<String, Rc<Image>>,
        viewport: Viewport,
    ) -> (LayoutTree, DisplayList) {
        let ctx = LayoutContext {
            doc,
            styles,
            fonts: &self.fonts,
            images,
            background_images,
            viewport_width: viewport.width,
            viewport_height: viewport.height,
        };
        let tree = rabbithole_layout::layout(&ctx);
        let list = rabbithole_layout::build_display_list(&tree, &ctx);
        (tree, list)
    }

    // ------------------------------------------------------------ styles

    fn collect_stylesheets(&mut self, doc: &Document, base: &Url) -> Vec<Stylesheet> {
        // Gather (source, media, origin url) in document order.
        enum Src {
            Inline(String, Option<String>),
            Link(Url, Option<String>),
        }
        let mut sources = Vec::new();
        for n in doc.descendants(Document::ROOT) {
            let Some(el) = doc.element(n) else { continue };
            match el.tag.as_str() {
                "style" => {
                    let media = el.attr("media").map(|m| m.to_string());
                    sources.push(Src::Inline(doc.text_content(n), media));
                }
                "link" => {
                    let rel = el.attr("rel").unwrap_or("").to_ascii_lowercase();
                    let is_sheet = rel.split_ascii_whitespace().any(|r| r == "stylesheet");
                    if !is_sheet || !self.load_stylesheets {
                        continue;
                    }
                    let Some(href) = el.attr("href") else { continue };
                    let Some(url) = base.join(href) else { continue };
                    let media = el.attr("media").map(|m| m.to_string());
                    sources.push(Src::Link(url, media));
                }
                _ => {}
            }
        }

        // Fetch external sheets in parallel (deduplicated, cached).
        let mut to_fetch: Vec<Url> = Vec::new();
        for s in &sources {
            if let Src::Link(u, _) = s {
                let key = u.without_fragment().to_string();
                if !self.css_cache.contains_key(&key) && !to_fetch.iter().any(|x| x.without_fragment().to_string() == key) {
                    to_fetch.push(u.clone());
                }
            }
        }
        self.fetch_css_into_cache(&to_fetch);

        let mut sheets = Vec::new();
        for s in sources {
            match s {
                Src::Inline(text, media) => {
                    let mut sheet = rabbithole_css::parse_stylesheet(&text);
                    self.resolve_imports(&mut sheet, base, 0);
                    sheets.push(wrap_media(sheet, media.as_deref()));
                }
                Src::Link(url, media) => {
                    let key = url.without_fragment().to_string();
                    if let Some(text) = self.css_cache.get(&key).cloned() {
                        let mut sheet = rabbithole_css::parse_stylesheet(&text);
                        self.resolve_imports(&mut sheet, &url, 0);
                        sheets.push(wrap_media(sheet, media.as_deref()));
                    }
                }
            }
        }
        sheets
    }

    fn fetch_css_into_cache(&mut self, urls: &[Url]) {
        if urls.is_empty() {
            return;
        }
        let results = self.fetcher.fetch_many(urls, 6);
        for (u, r) in urls.iter().zip(results) {
            let key = u.without_fragment().to_string();
            match r {
                Ok(resp) if resp.status < 400 => {
                    let text = decode_text(&resp.body, resp.charset.as_deref());
                    self.css_cache.insert(key, Rc::from(text));
                }
                _ => {
                    self.css_cache.insert(key, Rc::from(""));
                }
            }
        }
    }

    /// Replace `@import` rules with the imported sheet's rules (wrapped in
    /// their media query), fetching as needed, up to a fixed depth.
    fn resolve_imports(&mut self, sheet: &mut Stylesheet, sheet_url: &Url, depth: usize) {
        if depth >= MAX_IMPORT_DEPTH || !self.load_stylesheets {
            sheet.rules.retain(|r| !matches!(r, Rule::Import { .. }));
            return;
        }
        let imports: Vec<(usize, Url, MediaQuery)> = sheet
            .rules
            .iter()
            .enumerate()
            .filter_map(|(i, r)| match r {
                Rule::Import { href, query } => sheet_url.join(href).map(|u| (i, u, query.clone())),
                _ => None,
            })
            .collect();
        if imports.is_empty() {
            return;
        }
        let to_fetch: Vec<Url> = imports
            .iter()
            .map(|(_, u, _)| u.clone())
            .filter(|u| !self.css_cache.contains_key(&u.without_fragment().to_string()))
            .collect();
        self.fetch_css_into_cache(&to_fetch);

        let mut new_rules = Vec::with_capacity(sheet.rules.len());
        let mut import_iter = imports.into_iter().peekable();
        for (i, rule) in std::mem::take(&mut sheet.rules).into_iter().enumerate() {
            if let Some((idx, url, query)) = import_iter.peek().cloned() {
                if idx == i {
                    import_iter.next();
                    if let Some(text) = self.css_cache.get(&url.without_fragment().to_string()).cloned() {
                        let mut imported = rabbithole_css::parse_stylesheet(&text);
                        self.resolve_imports(&mut imported, &url, depth + 1);
                        new_rules.push(Rule::Media {
                            query,
                            rules: imported.rules,
                        });
                    }
                    continue;
                }
            }
            if !matches!(rule, Rule::Import { .. }) {
                new_rules.push(rule);
            }
        }
        sheet.rules = new_rules;
    }

    // ------------------------------------------------------------ images

    fn load_images(&mut self, doc: &Document, styles: &StyledTree, base: &Url) -> HashMap<NodeId, Rc<Image>> {
        let mut wanted: Vec<(NodeId, Url)> = Vec::new();
        for n in doc.descendants(Document::ROOT) {
            if !doc.is_element(n, "img") {
                continue;
            }
            if styles.get(n).is_none() {
                continue; // display:none
            }
            let Some(src) = image_source(doc, n) else { continue };
            let Some(url) = base.join(&src) else { continue };
            if url.scheme == "javascript" {
                continue;
            }
            wanted.push((n, url));
            if wanted.len() >= self.max_images {
                break;
            }
        }
        let urls: Vec<Url> = wanted.iter().map(|(_, u)| u.clone()).collect();
        self.fetch_images_into_cache(&urls);
        let mut out = HashMap::new();
        for (n, u) in wanted {
            if let Some(Some(img)) = self.image_cache.get(&u.to_string()) {
                out.insert(n, img.clone());
            }
        }
        out
    }

    fn load_background_images(&mut self, styles: &StyledTree, base: &Url) -> HashMap<String, Rc<Image>> {
        let mut keys: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        for s in styles.styles.iter().flatten() {
            if let Some(bg) = &s.background_image {
                if seen.insert(bg.clone()) {
                    keys.push(bg.clone());
                    if keys.len() >= self.max_background_images {
                        break;
                    }
                }
            }
        }
        let pairs: Vec<(String, Url)> = keys
            .into_iter()
            .filter_map(|k| base.join(&k).map(|u| (k, u)))
            .filter(|(_, u)| u.scheme != "javascript")
            .collect();
        let urls: Vec<Url> = pairs.iter().map(|(_, u)| u.clone()).collect();
        self.fetch_images_into_cache(&urls);
        let mut out = HashMap::new();
        for (k, u) in pairs {
            if let Some(Some(img)) = self.image_cache.get(&u.to_string()) {
                out.insert(k, img.clone());
            }
        }
        out
    }

    fn fetch_images_into_cache(&mut self, urls: &[Url]) {
        let mut to_fetch: Vec<Url> = Vec::new();
        for u in urls {
            let key = u.to_string();
            if !self.image_cache.contains_key(&key) && !to_fetch.iter().any(|x| x.to_string() == key) {
                to_fetch.push(u.clone());
            }
        }
        if to_fetch.is_empty() {
            return;
        }
        let mut fetcher = self.fetcher.clone();
        fetcher.max_body = 16 * 1024 * 1024;
        let results = fetcher.fetch_many(&to_fetch, 8);
        for (u, r) in to_fetch.iter().zip(results) {
            let decoded = match r {
                Ok(resp) if resp.status < 400 => Image::decode(&resp.body).map(Rc::new),
                _ => None,
            };
            self.image_cache.insert(u.to_string(), decoded);
        }
    }

    /// Drop cached images and stylesheets.
    pub fn clear_caches(&mut self) {
        self.image_cache.clear();
        self.css_cache.clear();
    }
}

fn media_env(viewport: Viewport) -> MediaEnv {
    MediaEnv {
        width: viewport.width,
        height: viewport.height,
        screen: true,
    }
}

fn wrap_media(sheet: Stylesheet, media: Option<&str>) -> Stylesheet {
    match media.map(str::trim).filter(|m| !m.is_empty()) {
        None => sheet,
        Some(m) => Stylesheet {
            rules: vec![Rule::Media {
                query: parse_media_query(m),
                rules: sheet.rules,
            }],
        },
    }
}

/// Pick the URL an `<img>` should load: `src`, else the first `srcset`
/// candidate, else common lazy-loading attributes.
fn image_source(doc: &Document, n: NodeId) -> Option<String> {
    let el = doc.element(n)?;
    if let Some(src) = el.attr("src").map(str::trim).filter(|s| !s.is_empty() && !s.starts_with("data:image/svg")) {
        return Some(src.to_string());
    }
    for attr in ["data-src", "data-lazy-src", "data-original"] {
        if let Some(v) = el.attr(attr).map(str::trim).filter(|s| !s.is_empty()) {
            return Some(v.to_string());
        }
    }
    if let Some(srcset) = el.attr("srcset").or_else(|| el.attr("data-srcset")) {
        let first = srcset.split(',').next()?.trim();
        let url = first.split_ascii_whitespace().next()?;
        if !url.is_empty() {
            return Some(url.to_string());
        }
    }
    None
}

/// The canvas background comes from `html`, or from `body` when `html`
/// has none (CSS 2.1 §14.2).
fn canvas_background(doc: &Document, styles: &StyledTree) -> Color {
    if let Some(html) = doc.html_element() {
        if let Some(s) = styles.get(html) {
            if !s.background_color.is_transparent() {
                return s.background_color;
            }
        }
    }
    if let Some(body) = doc.body() {
        if let Some(s) = styles.get(body) {
            if !s.background_color.is_transparent() {
                return s.background_color;
            }
        }
    }
    Color::TRANSPARENT
}

fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

fn find_box<'a>(b: &'a LayoutBox, node: NodeId) -> Option<&'a LayoutBox> {
    if b.node == Some(node) {
        return Some(b);
    }
    b.children.iter().find_map(|c| find_box(c, node))
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
        let node = self.node_at(x, y)?;
        let a = self.doc.ancestor_or_self(node, |n| {
            self.doc.element(n).map(|e| matches!(e.tag.as_str(), "a" | "area") && e.attr("href").is_some()).unwrap_or(false)
        })?;
        let href = self.doc.attr(a, "href")?;
        self.resolve(href)
    }

    /// Resolve a reference against the page's base URL.
    pub fn resolve(&self, href: &str) -> Option<Url> {
        let href = href.trim();
        if href.starts_with("javascript:") {
            return None;
        }
        self.base_url.join(href)
    }

    /// Document-space rectangle of the element with `id` (or `<a name>`),
    /// for fragment navigation.
    pub fn anchor_rect(&self, id: &str) -> Option<Rect> {
        let id = id.trim_start_matches('#');
        if id.is_empty() {
            return Some(Rect::new(0.0, 0.0, 0.0, 0.0));
        }
        let node = self.doc.element_by_id(id).or_else(|| {
            self.doc
                .descendants(Document::ROOT)
                .find(|&n| self.doc.is_element(n, "a") && self.doc.attr(n, "name") == Some(id))
        })?;
        // The element itself may be inline (no box); climb to a boxed ancestor.
        let mut cur = Some(node);
        while let Some(n) = cur {
            if let Some(b) = find_box(&self.layout.root, n) {
                return Some(b.rect);
            }
            cur = self.doc.parent(n);
        }
        None
    }

    /// All links in document order (hidden ones excluded).
    pub fn links(&self) -> Vec<Link> {
        let mut out = Vec::new();
        for n in self.doc.descendants(Document::ROOT) {
            let Some(el) = self.doc.element(n) else { continue };
            if el.tag != "a" {
                continue;
            }
            if self.styles.get(n).is_none() {
                continue;
            }
            let Some(href) = el.attr("href") else { continue };
            let Some(url) = self.resolve(href) else { continue };
            let mut text: String = self.doc.text_content(n).split_whitespace().collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                // Fall back to an image alt or title.
                text = self
                    .doc
                    .descendants(n)
                    .filter_map(|c| self.doc.attr(c, "alt").or_else(|| self.doc.attr(c, "title")))
                    .next()
                    .unwrap_or("")
                    .to_string();
            }
            out.push(Link { text, href: url });
        }
        out
    }

    /// Readable text of the page as Markdown-ish plain text (headings,
    /// paragraphs, lists, links), for CLI/MCP consumers.
    pub fn text(&self) -> String {
        text::extract(&self.doc, &self.styles, &self.base_url)
    }

    pub fn dom_dump(&self) -> String {
        self.doc.dump()
    }

    pub fn layout_dump(&self) -> String {
        self.layout.dump()
    }

    /// Element node ids of all `<img>` elements that failed to load (for
    /// diagnostics).
    pub fn missing_images(&self) -> Vec<NodeId> {
        self.doc
            .descendants(Document::ROOT)
            .filter(|&n| self.doc.is_element(n, "img") && !self.images.contains_key(&n) && self.styles.get(n).is_some())
            .collect()
    }
}

impl From<FetchError> for EngineError {
    fn from(e: FetchError) -> Self {
        EngineError::Fetch(e.to_string())
    }
}

#[allow(dead_code)]
fn _assert_node_data_used(_: NodeData) {}
