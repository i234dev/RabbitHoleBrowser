//! The engine ties everything together: fetch → parse → style → layout →
//! paint, plus hit testing, scrolling state and text extraction.
//!
//! Loading is split in two stages so a GUI can keep its event loop alive:
//! a [`Loader`] (`Clone + Send`) fetches and decodes every resource on a
//! background thread and yields a [`LoadedPage`] (plain data), and the
//! [`Engine`] turns it into a [`Page`] (styles, layout, display list) on the
//! thread that owns the fonts and paints.

pub mod forms;
mod home;
pub mod scripting;
pub mod select;
mod text;

pub use select::{Selection, TextRun};

pub use forms::{ChosenFile, ControlKind};
pub use rabbithole_net::{Body, Request, StreamedResponse};

use rabbithole_css::{parse_media_query, MatchState, MediaQuery, PseudoElement, Rule};
use rabbithole_dom::{Document, NodeId};
use rabbithole_gfx::{Canvas, Color, FontContext, Image, Rect};
use rabbithole_layout::{DisplayList, LayoutBox, LayoutContext, LayoutTree, ScrollInfo};
use rabbithole_net::{decode_text, Destination, FetchError, Fetcher, Response, Url};
use rabbithole_style::{FontFace, MediaEnv, StyleOptions, StyledTree, Stylesheet};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

pub use rabbithole_css as css;
pub use rabbithole_dom as dom;
pub use rabbithole_gfx as gfx;
pub use rabbithole_html as html;
pub use rabbithole_layout as layout;
pub use rabbithole_net as net;
pub use rabbithole_style as style;
pub use rabbithole_svg as svg;
pub use rabbithole_script as script;
pub use scripting::{Dialog, LocalStorage, PendingNavigation, ScriptOutcome, ScriptState};

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

impl From<FetchError> for EngineError {
    fn from(e: FetchError) -> Self {
        EngineError::Fetch(e.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
}

// ================================================================== Loader

/// Shared caches (thread-safe) for decoded images, stylesheet text and
/// font files across page loads.
#[derive(Default)]
struct Caches {
    images: HashMap<String, Option<Arc<Image>>>,
    css: HashMap<String, Arc<str>>,
    fonts: HashMap<String, Option<Arc<Vec<u8>>>>,
}

/// Resource loader: fetches a document and everything it references.
/// Cheap to clone; safe to move to a background thread.
#[derive(Clone)]
pub struct Loader {
    pub fetcher: Fetcher,
    pub load_images: bool,
    pub load_stylesheets: bool,
    pub load_fonts: bool,
    pub load_scripts: bool,
    pub max_images: usize,
    pub max_background_images: usize,
    pub max_fonts: usize,
    /// Page whose subresources are being fetched (referer / Sec-Fetch-Site).
    pub initiator: Option<Url>,
    caches: Arc<Mutex<Caches>>,
}

/// An external or inline script in document order.
#[derive(Debug, Clone)]
pub struct ScriptSource {
    pub node: NodeId,
    pub url: Option<Url>,
    pub source: String,
    pub is_module: bool,
    pub defer: bool,
    pub is_async: bool,
}

/// A fetched web font (from `@font-face`).
#[derive(Debug, Clone)]
pub struct LoadedFont {
    pub face: FontFace,
    pub data: Arc<Vec<u8>>,
}

/// Everything needed to build a [`Page`]; plain data, `Send`.
pub struct LoadedPage {
    pub url: Url,
    pub base_url: Url,
    pub doc: Document,
    pub title: String,
    pub status: u16,
    pub content_type: String,
    pub sheets: Vec<Stylesheet>,
    pub images: Vec<(NodeId, Arc<Image>)>,
    pub background_images: Vec<(String, Arc<Image>)>,
    pub fonts: Vec<LoadedFont>,
    pub scripts: Vec<ScriptSource>,
    /// Viewport used for media queries while discovering resources.
    pub viewport: Viewport,
}

const MAX_IMPORT_DEPTH: usize = 4;

/// Result of navigating to a URL: a page to display, or a download.
pub enum Loaded {
    Page(LoadedPage),
    /// The response is not displayable (binary, or `Content-Disposition:
    /// attachment`); the body is still unread so it can be streamed to disk.
    Download(StreamedResponse),
}

/// Can a response of this content type be rendered as a page?
pub fn is_displayable(content_type: &str) -> bool {
    let ct = content_type.trim().to_ascii_lowercase();
    ct.is_empty()
        || ct.starts_with("text/")
        || ct.starts_with("image/")
        || ct.starts_with("application/xhtml")
        || ct.contains("json")
        || ct.contains("xml")
        || ct.contains("javascript")
        || ct.contains("ecmascript")
}

impl Default for Loader {
    fn default() -> Self {
        Loader {
            fetcher: Fetcher::default(),
            load_images: true,
            load_stylesheets: true,
            load_fonts: true,
            load_scripts: true,
            max_images: 120,
            max_background_images: 40,
            max_fonts: 16,
            initiator: None,
            caches: Arc::new(Mutex::new(Caches::default())),
        }
    }
}

impl Loader {
    pub fn new() -> Loader {
        Loader::default()
    }

    /// Fetch and prepare a page. `url` may be anything a user would type.
    pub fn load(&self, url: &str, viewport: Viewport) -> Result<LoadedPage, EngineError> {
        let url = Url::parse(url).ok_or_else(|| EngineError::BadUrl(url.to_string()))?;
        if url.scheme == "about" {
            let html = match url.path.as_str() {
                "home" | "" => home::HOME_HTML,
                _ => "",
            };
            return Ok(self.load_html(html, url, viewport));
        }
        let resp = self.fetcher.fetch(&url)?;
        Ok(self.load_response(resp, viewport))
    }

    /// Navigate with an arbitrary request (form POST, link with referer).
    /// Distinguishes pages from downloads.
    pub fn load_request(&self, req: &Request, viewport: Viewport) -> Result<Loaded, EngineError> {
        if req.url.scheme == "about" {
            return self.load(&req.url.to_string(), viewport).map(Loaded::Page);
        }
        let stream = self.fetcher.request_stream(req)?;
        let attachment = stream
            .header("content-disposition")
            .map(|cd| cd.trim_start().to_ascii_lowercase().starts_with("attachment"))
            .unwrap_or(false);
        if attachment || !is_displayable(&stream.content_type) {
            return Ok(Loaded::Download(stream));
        }
        let resp = stream.into_response(self.fetcher.max_body)?;
        if resp.status >= 400 && resp.body.is_empty() {
            return Err(EngineError::Fetch(format!("HTTP {}", resp.status)));
        }
        Ok(Loaded::Page(self.load_response(resp, viewport)))
    }

    /// Turn an HTTP(-like) response into a page based on its content type.
    pub fn load_response(&self, resp: Response, viewport: Viewport) -> LoadedPage {
        let ct = resp.content_type.as_str();
        let url = resp.url.clone();
        let mut page = if ct.starts_with("text/html") || ct.starts_with("application/xhtml") || ct.is_empty() {
            let html = decode_text(&resp.body, resp.charset.as_deref());
            self.load_html(&html, url, viewport)
        } else if ct.starts_with("image/") {
            let html = format!(
                "<html><head><title>{0}</title></head><body style=\"margin:0;background:#222;display:flex;align-items:center;justify-content:center;min-height:100vh\"><img src=\"{0}\" alt=\"image\"></body></html>",
                escape_html(&url.to_string())
            );
            self.load_html(&html, url, viewport)
        } else if ct.starts_with("text/") || ct.contains("json") || ct.contains("xml") || ct.contains("javascript") {
            let text = decode_text(&resp.body, resp.charset.as_deref());
            let html = format!(
                "<html><head><title>{}</title></head><body><pre style=\"white-space:pre-wrap;overflow-wrap:anywhere\">{}</pre></body></html>",
                escape_html(&url.to_string()),
                escape_html(&text)
            );
            self.load_html(&html, url, viewport)
        } else {
            let html = format!(
                "<html><head><title>Download</title></head><body><h1>Cannot display this content</h1><p><code>{}</code> ({} bytes) is of type <code>{}</code>.</p></body></html>",
                escape_html(&url.to_string()),
                resp.body.len(),
                escape_html(ct)
            );
            self.load_html(&html, url, viewport)
        };
        page.status = resp.status;
        page.content_type = resp.content_type;
        page
    }

    /// Prepare a page from HTML source with the given document URL.
    pub fn load_html(&self, html: &str, url: Url, viewport: Viewport) -> LoadedPage {
        if self.initiator.as_ref() != Some(&url) {
            let bound = Loader { initiator: Some(url.clone()), ..self.clone() };
            return bound.load_html(html, url, viewport);
        }
        let doc = rabbithole_html::parse(html);
        let title = doc.title().unwrap_or_default();
        let base_url = doc
            .find_element("base")
            .and_then(|b| doc.attr(b, "href"))
            .and_then(|h| url.join(h))
            .unwrap_or_else(|| url.clone());

        let debug = std::env::var("RH_DEBUG_FETCH").is_ok();
        let t = std::time::Instant::now();
        let sheets = self.collect_stylesheets(&doc, &base_url);
        if debug {
            eprintln!("[load] stylesheets {:?}", t.elapsed());
        }
        let env = media_env(viewport, self.load_scripts);

        // A throw-away cascade on this thread to discover referenced
        // resources (background images, web fonts). Styles are recomputed
        // by the engine on the UI thread.
        let base_str = base_url.to_string();
        let opts = StyleOptions {
            state: &MatchState::default(),
            resolve_url: &|base, raw| resolve_url(base, raw),
            document_base: &base_str,
        };
        let t = std::time::Instant::now();
        let styles = rabbithole_style::style_tree_with(&doc, &sheets, &env, &opts);
        if debug {
            eprintln!("[load] discovery cascade {:?}", t.elapsed());
        }
        let t = std::time::Instant::now();
        let images = if self.load_images { self.load_images(&doc, &styles, &base_url) } else { Vec::new() };
        if debug {
            eprintln!("[load] images {:?}", t.elapsed());
        }
        let t = std::time::Instant::now();
        let background_images = if self.load_images { self.load_background_images(&styles) } else { Vec::new() };
        if debug {
            eprintln!("[load] background images {:?}", t.elapsed());
        }
        let t = std::time::Instant::now();
        let fonts = if self.load_fonts { self.load_fonts(&styles) } else { Vec::new() };
        if debug {
            eprintln!("[load] fonts {:?}", t.elapsed());
        }
        let t = std::time::Instant::now();
        let scripts = if self.load_scripts { self.collect_scripts(&doc, &base_url) } else { Vec::new() };
        if debug {
            eprintln!("[load] scripts {:?}", t.elapsed());
        }
        drop(styles);

        LoadedPage {
            url,
            base_url,
            doc,
            title,
            status: 200,
            content_type: "text/html".into(),
            sheets,
            images,
            background_images,
            fonts,
            scripts,
            viewport,
        }
    }

    // ------------------------------------------------------------ styles

    pub(crate) fn collect_stylesheets(&self, doc: &Document, base: &Url) -> Vec<Stylesheet> {
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

        let mut to_fetch: Vec<Url> = Vec::new();
        {
            let caches = self.caches.lock().unwrap();
            for s in &sources {
                if let Src::Link(u, _) = s {
                    let key = u.without_fragment().to_string();
                    if !caches.css.contains_key(&key) && !to_fetch.iter().any(|x| x.without_fragment().to_string() == key) {
                        to_fetch.push(u.clone());
                    }
                }
            }
        }
        self.fetch_css_into_cache(&to_fetch);

        let mut sheets = Vec::new();
        for s in sources {
            match s {
                Src::Inline(text, media) => {
                    let mut sheet = rabbithole_css::parse_stylesheet(&text);
                    sheet.base_url = Some(base.to_string());
                    self.resolve_imports(&mut sheet, base, 0);
                    sheets.push(wrap_media(sheet, media.as_deref()));
                }
                Src::Link(url, media) => {
                    let key = url.without_fragment().to_string();
                    let text = self.caches.lock().unwrap().css.get(&key).cloned();
                    if let Some(text) = text {
                        let mut sheet = rabbithole_css::parse_stylesheet(&text);
                        sheet.base_url = Some(url.to_string());
                        self.resolve_imports(&mut sheet, &url, 0);
                        sheets.push(wrap_media(sheet, media.as_deref()));
                    }
                }
            }
        }
        sheets
    }

    fn fetch_css_into_cache(&self, urls: &[Url]) {
        if urls.is_empty() {
            return;
        }
        let results = self.fetcher.fetch_many_for(urls, 6, Destination::Style, self.initiator.as_ref());
        let mut caches = self.caches.lock().unwrap();
        for (u, r) in urls.iter().zip(results) {
            let key = u.without_fragment().to_string();
            match r {
                Ok(resp) if resp.status < 400 => {
                    let text = decode_text(&resp.body, resp.charset.as_deref());
                    caches.css.insert(key, Arc::from(text));
                }
                _ => {
                    caches.css.insert(key, Arc::from(""));
                }
            }
        }
    }

    /// Replace `@import` rules with the imported sheet's rules (wrapped in
    /// their media query), fetching as needed, up to a fixed depth.
    fn resolve_imports(&self, sheet: &mut Stylesheet, sheet_url: &Url, depth: usize) {
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
        let to_fetch: Vec<Url> = {
            let caches = self.caches.lock().unwrap();
            imports
                .iter()
                .map(|(_, u, _)| u.clone())
                .filter(|u| !caches.css.contains_key(&u.without_fragment().to_string()))
                .collect()
        };
        self.fetch_css_into_cache(&to_fetch);

        let mut new_rules = Vec::with_capacity(sheet.rules.len());
        let mut import_iter = imports.into_iter().peekable();
        for (i, rule) in std::mem::take(&mut sheet.rules).into_iter().enumerate() {
            if let Some((idx, url, query)) = import_iter.peek().cloned() {
                if idx == i {
                    import_iter.next();
                    let text = self.caches.lock().unwrap().css.get(&url.without_fragment().to_string()).cloned();
                    if let Some(text) = text {
                        let mut imported = rabbithole_css::parse_stylesheet(&text);
                        imported.base_url = Some(url.to_string());
                        self.resolve_imports(&mut imported, &url, depth + 1);
                        // Nested sheets keep their own base URL through a
                        // wrapper sheet: flatten rules but url() values were
                        // not rewritten, so remember the base per rule group.
                        new_rules.push(Rule::Media {
                            query,
                            rules: rebase_rules(imported.rules, &url.to_string()),
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

    fn load_images(&self, doc: &Document, styles: &StyledTree, base: &Url) -> Vec<(NodeId, Arc<Image>)> {
        let mut wanted: Vec<(NodeId, Url)> = Vec::new();
        for n in doc.descendants(Document::ROOT) {
            if !doc.is_element(n, "img") && !doc.is_element(n, "input") {
                continue;
            }
            if doc.is_element(n, "input") && doc.attr(n, "type").map(|t| !t.eq_ignore_ascii_case("image")).unwrap_or(true) {
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
        let caches = self.caches.lock().unwrap();
        wanted
            .into_iter()
            .filter_map(|(n, u)| caches.images.get(&u.to_string()).cloned().flatten().map(|img| (n, img)))
            .collect()
    }

    fn load_background_images(&self, styles: &StyledTree) -> Vec<(String, Arc<Image>)> {
        let mut keys: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        let mut consider = |s: &rabbithole_style::ComputedStyle| {
            for layer in &s.background_layers {
                if let rabbithole_style::BackgroundImage::Url(u) = &layer.image {
                    if seen.insert(u.clone()) {
                        keys.push(u.clone());
                    }
                }
            }
            if let rabbithole_style::Content::Items(items) = &s.content {
                for it in items {
                    if let rabbithole_style::ContentItem::Url(u) = it {
                        if seen.insert(u.clone()) {
                            keys.push(u.clone());
                        }
                    }
                }
            }
        };
        for s in styles.styles.iter().flatten() {
            consider(s);
        }
        for s in styles.before.iter().flatten().chain(styles.after.iter().flatten()) {
            consider(s);
        }
        keys.truncate(self.max_background_images);
        let pairs: Vec<(String, Url)> = keys
            .into_iter()
            .filter_map(|k| Url::parse_absolute(&k).map(|u| (k, u)))
            .filter(|(_, u)| u.scheme != "javascript")
            .collect();
        let urls: Vec<Url> = pairs.iter().map(|(_, u)| u.clone()).collect();
        self.fetch_images_into_cache(&urls);
        let caches = self.caches.lock().unwrap();
        pairs
            .into_iter()
            .filter_map(|(k, u)| caches.images.get(&u.to_string()).cloned().flatten().map(|img| (k, img)))
            .collect()
    }

    fn fetch_images_into_cache(&self, urls: &[Url]) {
        let mut to_fetch: Vec<Url> = Vec::new();
        {
            let caches = self.caches.lock().unwrap();
            for u in urls {
                let key = u.to_string();
                if !caches.images.contains_key(&key) && !to_fetch.iter().any(|x| x.to_string() == key) {
                    to_fetch.push(u.clone());
                }
            }
        }
        if to_fetch.is_empty() {
            return;
        }
        let mut fetcher = self.fetcher.clone();
        fetcher.max_body = 16 * 1024 * 1024;
        let debug = std::env::var("RH_DEBUG_FETCH").is_ok();
        let t0 = std::time::Instant::now();
        let results = fetcher.fetch_many_for(&to_fetch, 8, Destination::Image, self.initiator.as_ref());
        if debug {
            eprintln!("[fetch] {} images in {:?}", to_fetch.len(), t0.elapsed());
            for (u, r) in to_fetch.iter().zip(results.iter()) {
                match r {
                    Ok(resp) => eprintln!("[fetch]   {} -> {} {} {}B", u, resp.status, resp.content_type, resp.body.len()),
                    Err(e) => eprintln!("[fetch]   {} -> ERROR {}", u, e),
                }
            }
        }
        let mut caches = self.caches.lock().unwrap();
        for (u, r) in to_fetch.iter().zip(results) {
            let td = std::time::Instant::now();
            let decoded = match r {
                Ok(resp) if resp.status < 400 => decode_image(&resp.body, &resp.content_type, &u.path).map(Arc::new),
                _ => None,
            };
            if debug {
                eprintln!("[decode] {:?} {} -> {}", td.elapsed(), u, decoded.as_ref().map(|i| format!("{}x{}", i.width, i.height)).unwrap_or_else(|| "none".into()));
            }
            caches.images.insert(u.to_string(), decoded);
        }
    }

    // ------------------------------------------------------------- fonts

    fn load_fonts(&self, styles: &StyledTree) -> Vec<LoadedFont> {
        if styles.font_faces.is_empty() {
            return Vec::new();
        }
        // Only fetch families that some element actually asks for.
        let mut used: HashSet<String> = HashSet::new();
        for s in styles
            .styles
            .iter()
            .flatten()
            .chain(styles.before.iter().flatten())
            .chain(styles.after.iter().flatten())
        {
            for f in &s.font_family_list {
                used.insert(f.to_ascii_lowercase());
            }
        }
        let mut picked: Vec<(FontFace, Url)> = Vec::new();
        for face in &styles.font_faces {
            if !used.contains(&face.family.to_ascii_lowercase()) {
                continue;
            }
            // Prefer woff2, then woff, then ttf/otf; skip eot/svg.
            let mut best: Option<(u8, &str)> = None;
            for src in &face.sources {
                let fmt = src.format.as_deref().map(|f| f.to_ascii_lowercase()).unwrap_or_else(|| {
                    let lower = src.url.to_ascii_lowercase();
                    let path = lower.split('?').next().unwrap_or("");
                    if path.ends_with(".woff2") {
                        "woff2".into()
                    } else if path.ends_with(".woff") {
                        "woff".into()
                    } else if path.ends_with(".ttf") || path.ends_with(".otf") {
                        "truetype".into()
                    } else {
                        String::new()
                    }
                });
                let rank = match fmt.as_str() {
                    "woff2" | "woff2-variations" => 3,
                    "woff" => 2,
                    "truetype" | "opentype" | "truetype-variations" => 1,
                    _ => 0,
                };
                if rank > 0 && best.map(|(r, _)| rank > r).unwrap_or(true) {
                    best = Some((rank, src.url.as_str()));
                }
            }
            if let Some((_, url)) = best {
                if let Some(u) = Url::parse_absolute(url) {
                    picked.push((face.clone(), u));
                }
            }
            if picked.len() >= self.max_fonts {
                break;
            }
        }
        let urls: Vec<Url> = picked.iter().map(|(_, u)| u.clone()).collect();
        let to_fetch: Vec<Url> = {
            let caches = self.caches.lock().unwrap();
            urls.iter().filter(|u| !caches.fonts.contains_key(&u.to_string())).cloned().collect()
        };
        if !to_fetch.is_empty() {
            let mut fetcher = self.fetcher.clone();
            fetcher.max_body = 8 * 1024 * 1024;
            let results = fetcher.fetch_many_for(&to_fetch, 6, Destination::Font, self.initiator.as_ref());
            let mut caches = self.caches.lock().unwrap();
            for (u, r) in to_fetch.iter().zip(results) {
                let data = match r {
                    Ok(resp) if resp.status < 400 && !resp.body.is_empty() => Some(Arc::new(resp.body)),
                    _ => None,
                };
                caches.fonts.insert(u.to_string(), data);
            }
        }
        let caches = self.caches.lock().unwrap();
        picked
            .into_iter()
            .filter_map(|(face, u)| caches.fonts.get(&u.to_string()).cloned().flatten().map(|data| LoadedFont { face, data }))
            .collect()
    }

    // ----------------------------------------------------------- scripts

    fn collect_scripts(&self, doc: &Document, base: &Url) -> Vec<ScriptSource> {
        let mut out = Vec::new();
        let mut external: Vec<(usize, Url)> = Vec::new();
        for n in doc.descendants(Document::ROOT) {
            let Some(el) = doc.element(n) else { continue };
            if el.tag != "script" {
                continue;
            }
            let ty = el.attr("type").map(|t| t.trim().to_ascii_lowercase()).unwrap_or_default();
            let is_module = ty == "module";
            let is_js = ty.is_empty()
                || is_module
                || matches!(
                    ty.as_str(),
                    "text/javascript" | "application/javascript" | "text/ecmascript" | "application/ecmascript" | "text/jscript" | "module"
                );
            if !is_js || el.attr("nomodule").is_some() {
                continue;
            }
            let idx = out.len();
            out.push(ScriptSource {
                node: n,
                url: None,
                source: String::new(),
                is_module,
                defer: el.attr("defer").is_some() || is_module,
                is_async: el.attr("async").is_some(),
            });
            match el.attr("src").map(str::trim).filter(|s| !s.is_empty()) {
                Some(src) => match base.join(src) {
                    Some(u) => {
                        out[idx].url = Some(u.clone());
                        external.push((idx, u));
                    }
                    None => {
                        out.pop();
                    }
                },
                None => out[idx].source = doc.text_content(n),
            }
        }
        if !external.is_empty() {
            let urls: Vec<Url> = external.iter().map(|(_, u)| u.clone()).collect();
            let mut fetcher = self.fetcher.clone();
            fetcher.max_body = 8 * 1024 * 1024;
            let results = fetcher.fetch_many_for(&urls, 6, Destination::Script, self.initiator.as_ref());
            for ((idx, _), r) in external.iter().zip(results) {
                match r {
                    Ok(resp) if resp.status < 400 => {
                        out[*idx].source = decode_text(&resp.body, resp.charset.as_deref());
                    }
                    _ => out[*idx].source.clear(),
                }
            }
        }
        out
    }

    /// Drop cached images, stylesheets and fonts.
    pub fn clear_caches(&self) {
        *self.caches.lock().unwrap() = Caches::default();
    }
}

// ================================================================== Engine

/// Owns the fonts and builds/paints pages. Single-threaded.
pub struct Engine {
    pub fonts: Rc<FontContext>,
    pub loader: Loader,
    /// Run page scripts (JavaScript) when building pages.
    pub scripting: bool,
    /// Echo `console.*` output and script errors to stderr.
    pub console_to_stderr: bool,
    /// Origin-scoped `localStorage` shared by all pages.
    pub local_storage: std::cell::RefCell<LocalStorage>,
    /// Monotonic clock origin for timers / `performance.now()`.
    pub start: std::time::Instant,
    registered_fonts: HashSet<String>,
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
    pub status: u16,
    pub sheets: Vec<Stylesheet>,
    pub styles: StyledTree,
    pub layout: LayoutTree,
    pub display_list: DisplayList,
    pub images: HashMap<NodeId, Arc<Image>>,
    pub background_images: HashMap<String, Arc<Image>>,
    pub scripts: Vec<ScriptSource>,
    pub viewport: Viewport,
    /// Canvas background (propagated from `html`/`body`).
    pub background: Color,
    /// Page scroll position used for sticky/fixed placement in the
    /// current display list.
    pub page_scroll: (f32, f32),
    /// Scroll offsets of inner scroll containers.
    pub scroll_offsets: HashMap<NodeId, (f32, f32)>,
    /// Dynamic selector state (hover/focus/active/target).
    pub state: MatchState,
    /// Whether any stylesheet uses dynamic pseudo-classes (so hover
    /// changes need a restyle).
    pub has_dynamic_rules: bool,
    /// JavaScript state (host, pending navigation, console, ...).
    pub script: ScriptState,
    /// Hash of the document's stylesheet sources (to detect changes).
    pub sheet_signature: u64,
    /// Scroll position requested by scripts (consumed by the UI).
    pub requested_scroll: Option<(f32, f32)>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Engine {
        let fonts = FontContext::new();
        fonts.load_system_fonts();
        Engine {
            fonts: Rc::new(fonts),
            loader: Loader::new(),
            scripting: true,
            console_to_stderr: std::env::var("RH_JS_LOG").is_ok(),
            local_storage: std::cell::RefCell::new(LocalStorage::default()),
            start: std::time::Instant::now(),
            registered_fonts: HashSet::new(),
        }
    }

    /// Engine without system font discovery (faster startup for tests/CLI).
    pub fn new_bundled_fonts_only() -> Engine {
        Engine {
            fonts: Rc::new(FontContext::new()),
            loader: Loader::new(),
            scripting: true,
            console_to_stderr: std::env::var("RH_JS_LOG").is_ok(),
            local_storage: std::cell::RefCell::new(LocalStorage::default()),
            start: std::time::Instant::now(),
            registered_fonts: HashSet::new(),
        }
    }

    /// Persist `localStorage` to a file (loaded now, saved after script runs).
    pub fn set_local_storage_path(&mut self, path: std::path::PathBuf) {
        *self.local_storage.borrow_mut() = LocalStorage::load(path);
    }

    /// Synchronous convenience: load + build.
    pub fn load(&mut self, url: &str, viewport: Viewport) -> Result<Page, EngineError> {
        let loaded = self.loader.load(url, viewport)?;
        Ok(self.build_page(loaded, viewport))
    }

    /// Synchronous convenience for HTML source.
    pub fn load_html(&mut self, html: &str, url: Url, viewport: Viewport) -> Page {
        let loaded = self.loader.load_html(html, url, viewport);
        self.build_page(loaded, viewport)
    }

    /// Turn loaded resources into a styled, laid out page.
    pub fn build_page(&mut self, loaded: LoadedPage, viewport: Viewport) -> Page {
        for f in &loaded.fonts {
            let key = format!("{}|{}|{}", f.face.family.to_ascii_lowercase(), f.face.weight.0, f.face.italic);
            if self.registered_fonts.insert(key) {
                self.fonts.add_font_data(&f.face.family, f.face.weight.0, f.face.italic, &f.data);
            }
        }
        let has_dynamic_rules = loaded.sheets.iter().any(sheet_has_dynamic_rules);
        let mut page = Page {
            url: loaded.url,
            base_url: loaded.base_url,
            doc: loaded.doc,
            title: loaded.title,
            status: loaded.status,
            sheets: loaded.sheets,
            styles: StyledTree { styles: Vec::new(), before: Vec::new(), after: Vec::new(), marker: Vec::new(), font_faces: Vec::new(), keyframes: HashMap::new(), root_font_size: 16.0 },
            layout: empty_layout(),
            display_list: DisplayList::default(),
            images: loaded.images.into_iter().collect(),
            background_images: loaded.background_images.into_iter().collect(),
            scripts: loaded.scripts,
            viewport,
            background: Color::TRANSPARENT,
            page_scroll: (0.0, 0.0),
            scroll_offsets: HashMap::new(),
            state: MatchState::default(),
            has_dynamic_rules,
            script: ScriptState::default(),
            requested_scroll: None,
            sheet_signature: 0,
        };
        page.snapshot_form_defaults();
        page.sheet_signature = stylesheet_signature(&page.doc);
        self.restyle(&mut page);
        if self.scripting && !page.scripts.is_empty() {
            // Keep what the scripts asked for during load (redirects via
            // `location`, `window.open`, dialogs) for the embedder to apply.
            let out = self.run_page_scripts(&mut page);
            if out.navigation.is_some() {
                page.script.pending_navigation = out.navigation;
            }
            page.script.new_windows.extend(out.new_windows);
            page.script.dialogs.extend(out.dialogs);
            if out.scroll_to.is_some() {
                page.requested_scroll = out.scroll_to;
            }
        }
        // `<meta http-equiv="refresh" content="N; url=...">` (N <= 5 s is
        // followed right away; longer delays are left to the user).
        if page.script.pending_navigation.is_none() {
            if let Some(target) = meta_refresh_target(&page, self.scripting) {
                page.script.pending_navigation = Some(PendingNavigation {
                    request: Request::get(target).with_referer(page.url.clone()),
                    replace: true,
                });
            }
        }
        page
    }

    /// Take the navigation a page requested by itself (script redirect or
    /// meta refresh), if any.
    pub fn take_page_navigation(&self, page: &mut Page) -> Option<PendingNavigation> {
        page.script.pending_navigation.take()
    }

    /// Re-run style + layout + display list (after DOM, viewport or
    /// selector-state changes).
    pub fn restyle(&self, page: &mut Page) {
        // Scripts may add/remove `<style>`/`<link>` elements or insert rules
        // (CSS-in-JS): re-collect the sheets when their sources changed.
        if page.script.host.is_some() || page.script.scripts_ran {
            let sig = stylesheet_signature(&page.doc);
            if sig != page.sheet_signature {
                page.sheet_signature = sig;
                page.sheets = self.loader.collect_stylesheets(&page.doc, &page.base_url);
                page.has_dynamic_rules = page.sheets.iter().any(sheet_has_dynamic_rules);
            }
        }
        let env = media_env(page.viewport, self.scripting);
        let base_str = page.base_url.to_string();
        let opts = StyleOptions {
            state: &page.state,
            resolve_url: &|base, raw| resolve_url(base, raw),
            document_base: &base_str,
        };
        page.styles = rabbithole_style::style_tree_with(&page.doc, &page.sheets, &env, &opts);
        page.background = canvas_background(&page.doc, &page.styles);
        rasterize_inline_svgs(&page.doc, &page.styles, &self.fonts, &mut page.images);
        self.relayout_only(page);
    }

    /// Current computed style of one element without a full restyle
    /// (stylesheets are re-collected first if their sources changed).
    pub fn computed_style_now(&self, page: &mut Page, node: NodeId) -> Option<Rc<rabbithole_style::ComputedStyle>> {
        let sig = stylesheet_signature(&page.doc);
        if sig != page.sheet_signature {
            page.sheet_signature = sig;
            page.sheets = self.loader.collect_stylesheets(&page.doc, &page.base_url);
            page.has_dynamic_rules = page.sheets.iter().any(sheet_has_dynamic_rules);
        }
        let env = media_env(page.viewport, self.scripting);
        let base_str = page.base_url.to_string();
        let opts = StyleOptions {
            state: &page.state,
            resolve_url: &|base, raw| resolve_url(base, raw),
            document_base: &base_str,
        };
        rabbithole_style::style_chain(&page.doc, &page.sheets, &env, &opts, node)
    }

    /// Re-run layout + display list (styles unchanged).
    pub fn relayout_only(&self, page: &mut Page) {
        let Page { doc, styles, images, background_images, viewport, scroll_offsets, page_scroll, layout, display_list, .. } = page;
        let ctx = LayoutContext {
            doc,
            styles,
            fonts: &self.fonts,
            images,
            background_images,
            viewport_width: viewport.width,
            viewport_height: viewport.height,
            scroll_offsets,
            page_scroll: *page_scroll,
        };
        *layout = rabbithole_layout::layout(&ctx);
        *display_list = rabbithole_layout::build_display_list(layout, &ctx);
    }

    /// Rebuild only the display list (after scrolling: sticky/fixed boxes
    /// and inner scroll offsets).
    pub fn rebuild_display_list(&self, page: &mut Page) {
        let Page { doc, styles, images, background_images, viewport, scroll_offsets, page_scroll, layout, display_list, .. } = page;
        let ctx = LayoutContext {
            doc,
            styles,
            fonts: &self.fonts,
            images,
            background_images,
            viewport_width: viewport.width,
            viewport_height: viewport.height,
            scroll_offsets,
            page_scroll: *page_scroll,
        };
        *display_list = rabbithole_layout::build_display_list(layout, &ctx);
    }

    /// Change the viewport (window resize).
    pub fn relayout(&mut self, page: &mut Page, viewport: Viewport) {
        page.viewport = viewport;
        self.restyle(page);
    }

    /// Update the page scroll position (sticky/fixed elements follow it).
    /// Cheap: only the display list is rebuilt, and only when the page has
    /// sticky or fixed boxes or inner scroll offsets.
    pub fn set_page_scroll(&self, page: &mut Page, x: f32, y: f32) {
        if page.page_scroll == (x, y) {
            return;
        }
        page.page_scroll = (x, y);
        if page.layout.has_sticky_or_fixed() {
            self.rebuild_display_list(page);
        }
    }

    /// Scroll the innermost scroll container under (x, y) (document
    /// coordinates) by (dx, dy). Returns false when there is none (the
    /// caller should scroll the page instead) or it is already at its limit.
    pub fn scroll_at(&self, page: &mut Page, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        let Some(info) = page.layout.scroll_container_at(x, y) else {
            return false;
        };
        let max_x = (info.content_width - info.viewport.w).max(0.0);
        let max_y = (info.content_height - info.viewport.h).max(0.0);
        let nx = (info.offset.0 + dx).clamp(0.0, max_x);
        let ny = (info.offset.1 + dy).clamp(0.0, max_y);
        if (nx, ny) == info.offset {
            return false;
        }
        page.scroll_offsets.insert(info.node, (nx, ny));
        self.relayout_only(page);
        true
    }

    /// Set the hovered element (None = nothing). Restyles only when the
    /// page has dynamic rules; returns true when a repaint is needed.
    pub fn set_hover(&self, page: &mut Page, node: Option<NodeId>) -> bool {
        if page.state.hovered == node {
            return false;
        }
        page.state.hovered = node;
        if page.has_dynamic_rules {
            self.restyle(page);
            true
        } else {
            false
        }
    }

    pub fn set_focus(&self, page: &mut Page, node: Option<NodeId>) -> bool {
        if page.state.focused == node {
            return false;
        }
        page.state.focused = node;
        if page.has_dynamic_rules {
            self.restyle(page);
        }
        true
    }

    /// Paint the page region starting at (scroll_x, scroll_y) into `canvas`.
    pub fn render(&self, page: &Page, canvas: &mut Canvas, scroll_x: f32, scroll_y: f32) {
        self.render_scaled(page, canvas, scroll_x, scroll_y, 1.0);
    }

    /// Paint with a device scale (HiDPI / zoom): the page is laid out in
    /// CSS pixels for a viewport of `canvas size / scale` and painted
    /// magnified by `scale`. Scroll offsets are in CSS pixels.
    pub fn render_scaled(&self, page: &Page, canvas: &mut Canvas, scroll_x: f32, scroll_y: f32, scale: f32) {
        canvas.clear(Color::WHITE);
        if page.background.a > 0 {
            let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
            canvas.fill_rect(full, page.background);
        }
        if (scale - 1.0).abs() > 1e-3 {
            canvas.push_transform([scale, 0.0, 0.0, scale, 0.0, 0.0]);
            rabbithole_layout::paint(&page.display_list, canvas, &self.fonts, scroll_x, scroll_y);
            canvas.pop_transform();
        } else {
            rabbithole_layout::paint(&page.display_list, canvas, &self.fonts, scroll_x, scroll_y);
        }
    }
}

fn empty_layout() -> LayoutTree {
    LayoutTree::empty()
}

/// Decode a raster image, or rasterize an SVG document at its intrinsic
/// size (×2 for crispness when scaled, capped at 2048 px).
fn decode_image(bytes: &[u8], content_type: &str, path: &str) -> Option<Image> {
    let looks_svg = content_type.contains("svg")
        || path.to_ascii_lowercase().ends_with(".svg")
        || (bytes.len() > 5 && bytes.iter().take(512).any(|&b| b == b'<') && std::str::from_utf8(&bytes[..bytes.len().min(2048)]).map(|s| s.contains("<svg")).unwrap_or(false));
    if looks_svg {
        let text = decode_text(bytes, None);
        let tree = rabbithole_svg::SvgTree::parse(&text)?;
        let (w, h) = tree.resolve_size(None, None);
        let scale = if w.max(h) < 512.0 { 2.0 } else { 1.0 };
        let (pw, ph) = ((w * scale).round().clamp(1.0, 2048.0) as u32, (h * scale).round().clamp(1.0, 2048.0) as u32);
        thread_local! {
            static SVG_FONTS: FontContext = FontContext::new();
        }
        return Some(SVG_FONTS.with(|fonts| tree.rasterize(fonts, pw, ph)));
    }
    Image::decode(bytes)
}

/// Rasterize inline `<svg>` elements of a document into images keyed by
/// node, so layout/paint treat them as replaced images.
fn rasterize_inline_svgs(doc: &Document, styles: &StyledTree, fonts: &FontContext, images: &mut HashMap<NodeId, Arc<Image>>) {
    let mut roots = Vec::new();
    for n in doc.descendants(Document::ROOT) {
        if doc.is_element(n, "svg") && styles.get(n).is_some() {
            // Skip svg nested inside another svg.
            let nested = doc.ancestor_or_self(n, |a| a != n && doc.is_element(a, "svg")).is_some();
            if !nested {
                roots.push(n);
            }
        }
    }
    for n in roots {
        if images.contains_key(&n) {
            continue;
        }
        let style_of = |node: NodeId, prop: &str| -> Option<String> {
            let s = styles.get(node)?;
            match prop {
                "color" => Some(css_color(s.color)),
                "opacity" if s.opacity < 1.0 => Some(format!("{}", s.opacity)),
                "display" if s.display.is_none() => Some("none".into()),
                "visibility" if s.visibility == rabbithole_style::Visibility::Hidden => Some("hidden".into()),
                _ => None,
            }
        };
        let Some(tree) = rabbithole_svg::SvgTree::from_dom(doc, n, &style_of) else { continue };
        let st = styles.get(n).cloned();
        let css_w = st.as_ref().and_then(|s| match s.width { rabbithole_style::Length::Px(v) => Some(v), _ => None });
        let css_h = st.as_ref().and_then(|s| match s.height { rabbithole_style::Length::Px(v) => Some(v), _ => None });
        let (w, h) = tree.resolve_size(css_w, css_h);
        if w <= 0.0 || h <= 0.0 {
            continue;
        }
        let scale = if w.max(h) < 256.0 { 2.0 } else { 1.0 };
        let (pw, ph) = ((w * scale).round().clamp(1.0, 2048.0) as u32, (h * scale).round().clamp(1.0, 2048.0) as u32);
        images.insert(n, Arc::new(tree.rasterize(fonts, pw, ph)));
    }
}

fn css_color(c: Color) -> String {
    format!("rgba({}, {}, {}, {})", c.r, c.g, c.b, c.a as f32 / 255.0)
}

fn media_env(viewport: Viewport, scripting: bool) -> MediaEnv {
    MediaEnv { width: viewport.width, height: viewport.height, screen: true, scripting }
}

fn resolve_url(base: &str, raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.starts_with("data:") {
        return if raw.is_empty() { None } else { Some(raw.to_string()) };
    }
    let base = Url::parse_absolute(base)?;
    base.join(raw).map(|u| u.to_string())
}

fn wrap_media(sheet: Stylesheet, media: Option<&str>) -> Stylesheet {
    match media.map(str::trim).filter(|m| !m.is_empty()) {
        None => sheet,
        Some(m) => Stylesheet {
            base_url: sheet.base_url.clone(),
            rules: vec![Rule::Media { query: parse_media_query(m), rules: sheet.rules }],
        },
    }
}

/// Imported sheets live inside the importing sheet, but their `url()`
/// values must resolve against their own location: rewrite relative URLs
/// in declarations to absolute ones.
fn rebase_rules(rules: Vec<Rule>, base: &str) -> Vec<Rule> {
    use rabbithole_css::Token;
    fn rewrite(decls: &mut [rabbithole_css::Declaration], base: &str) {
        for d in decls {
            let mut i = 0;
            while i < d.value.len() {
                match &d.value[i] {
                    Token::Url(u) => {
                        if let Some(abs) = resolve_url(base, u) {
                            d.value[i] = Token::Url(abs);
                        }
                    }
                    Token::Function(f) if f.eq_ignore_ascii_case("url") => {
                        if let Some(Token::String(s)) = d.value.get(i + 1) {
                            if let Some(abs) = resolve_url(base, s) {
                                d.value[i + 1] = Token::String(abs);
                            }
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
        }
    }
    rules
        .into_iter()
        .map(|r| match r {
            Rule::Style(mut s) => {
                rewrite(&mut s.declarations, base);
                Rule::Style(s)
            }
            Rule::Media { query, rules } => Rule::Media { query, rules: rebase_rules(rules, base) },
            Rule::Supports { matches, rules } => Rule::Supports { matches, rules: rebase_rules(rules, base) },
            Rule::FontFace(mut d) => {
                rewrite(&mut d, base);
                Rule::FontFace(d)
            }
            other => other,
        })
        .collect()
}

/// Target of a `<meta http-equiv="refresh">` with a short delay.
fn in_noscript(doc: &rabbithole_dom::Document, mut n: NodeId) -> bool {
    while let Some(p) = doc.parent(n) {
        if doc.element(p).map(|e| e.tag == "noscript").unwrap_or(false) {
            return true;
        }
        n = p;
    }
    false
}

fn meta_refresh_target(page: &Page, scripting: bool) -> Option<Url> {
    let doc = &page.doc;
    let head = doc.head()?;
    for n in doc.descendants(head) {
        let Some(el) = doc.element(n) else { continue };
        if el.tag != "meta" || !el.attr("http-equiv").map(|h| h.eq_ignore_ascii_case("refresh")).unwrap_or(false) {
            continue;
        }
        // With scripting on, `<noscript>` content is inert (the HTML parser
        // would treat it as raw text), so its refreshes must not fire.
        if scripting && in_noscript(doc, n) {
            continue;
        }
        let content = el.attr("content")?.trim();
        let (delay, rest) = match content.find(|c: char| c == ';' || c == ',') {
            Some(i) => (&content[..i], content[i + 1..].trim()),
            None => (content, ""),
        };
        let delay: f32 = delay.trim().parse().unwrap_or(0.0);
        if delay > 5.0 || rest.is_empty() {
            return None;
        }
        let url = rest.trim_start_matches(|c: char| c.is_ascii_whitespace());
        let url = if url.len() >= 4 && url[..4].eq_ignore_ascii_case("url=") { &url[4..] } else { url };
        let url = url.trim().trim_matches(|c| c == '\'' || c == '"');
        if url.is_empty() {
            return None;
        }
        let target = page.resolve(url)?;
        return (target.without_fragment() != page.url.without_fragment()).then_some(target);
    }
    None
}

/// Hash of every `<style>` text and stylesheet `<link>` (href + media) in
/// document order.
fn stylesheet_signature(doc: &Document) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for n in doc.descendants(Document::ROOT) {
        let Some(el) = doc.element(n) else { continue };
        match el.tag.as_str() {
            "style" => {
                0u8.hash(&mut h);
                el.attr("media").hash(&mut h);
                for c in doc.children(n) {
                    if let Some(t) = doc.text(c) {
                        t.hash(&mut h);
                    }
                }
            }
            "link" if el.attr("rel").map(|r| r.to_ascii_lowercase().split_ascii_whitespace().any(|x| x == "stylesheet")).unwrap_or(false) => {
                1u8.hash(&mut h);
                el.attr("href").hash(&mut h);
                el.attr("media").hash(&mut h);
                el.attr("disabled").is_some().hash(&mut h);
            }
            _ => {}
        }
    }
    h.finish()
}

fn sheet_has_dynamic_rules(sheet: &Stylesheet) -> bool {
    use rabbithole_css::selector::{Compound, Pseudo};
    fn compound_dynamic(c: &Compound) -> bool {
        c.pseudos.iter().any(|p| match p {
            Pseudo::Hover | Pseudo::Active | Pseudo::Focus | Pseudo::Target => true,
            Pseudo::Not(l) | Pseudo::Is(l) | Pseudo::Where(l) => l.iter().any(|s| s.compounds.iter().any(compound_dynamic)),
            Pseudo::Has(l) => l.iter().any(|(_, s)| s.compounds.iter().any(compound_dynamic)),
            _ => false,
        })
    }
    fn walk(rules: &[Rule]) -> bool {
        rules.iter().any(|r| match r {
            Rule::Style(s) => s.selectors.iter().any(|sel| sel.compounds.iter().any(compound_dynamic)),
            Rule::Media { rules, .. } | Rule::Supports { rules, .. } => walk(rules),
            _ => false,
        })
    }
    walk(&sheet.rules)
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

    /// Element at document coordinates (page-scrolled content only).
    pub fn node_at(&self, x: f32, y: f32) -> Option<NodeId> {
        self.layout.node_at(x, y)
    }

    /// Element at viewport coordinates, checking fixed boxes first.
    pub fn node_at_viewport(&self, vx: f32, vy: f32) -> Option<NodeId> {
        self.layout.node_at_with_scroll(vx, vy, self.page_scroll)
    }

    /// Resolved href of the link under the document point, if any.
    pub fn link_at(&self, x: f32, y: f32) -> Option<Url> {
        let node = self.node_at(x, y)?;
        self.link_of(node)
    }

    /// Resolved href of the link under the viewport point, if any.
    pub fn link_at_viewport(&self, vx: f32, vy: f32) -> Option<Url> {
        let node = self.node_at_viewport(vx, vy)?;
        self.link_of(node)
    }

    /// The link (`a[href]` ancestor) of a node.
    pub fn link_of(&self, node: NodeId) -> Option<Url> {
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

    /// Document-space rectangle of the element with `id` (or `<a name>`).
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
        self.rect_of(node)
    }

    /// Border-box rect of an element (document coordinates); climbs to a
    /// boxed ancestor for elements without a box.
    pub fn rect_of(&self, node: NodeId) -> Option<Rect> {
        let mut cur = Some(node);
        while let Some(n) = cur {
            if let Some(b) = find_box(&self.layout.root, n) {
                return Some(b.rect);
            }
            cur = self.doc.parent(n);
        }
        None
    }

    /// Scroll container under a document point.
    pub fn scroll_container_at(&self, x: f32, y: f32) -> Option<ScrollInfo> {
        self.layout.scroll_container_at(x, y)
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
                text = self
                    .doc
                    .descendants(n)
                    .filter_map(|c| self.doc.attr(c, "alt").or_else(|| self.doc.attr(c, "title")).or_else(|| self.doc.attr(c, "aria-label")))
                    .next()
                    .unwrap_or("")
                    .to_string();
            }
            out.push(Link { text, href: url });
        }
        out
    }

    /// Readable text of the page as Markdown-ish plain text.
    pub fn text(&self) -> String {
        text::extract(&self.doc, &self.styles, &self.base_url)
    }

    pub fn dom_dump(&self) -> String {
        self.doc.dump()
    }

    pub fn layout_dump(&self) -> String {
        self.layout.dump()
    }

    /// Style of a pseudo-element (helper for tooling).
    pub fn pseudo_style(&self, node: NodeId, which: PseudoElement) -> Option<&Rc<rabbithole_style::ComputedStyle>> {
        self.styles.pseudo(node, which)
    }

    /// `<img>` elements that failed to load (diagnostics).
    pub fn missing_images(&self) -> Vec<NodeId> {
        self.doc
            .descendants(Document::ROOT)
            .filter(|&n| self.doc.is_element(n, "img") && !self.images.contains_key(&n) && self.styles.get(n).is_some())
            .collect()
    }

    /// Cursor hint for a document point: "pointer" over links/buttons,
    /// "text" over selectable text, else "default".
    pub fn cursor_at(&self, x: f32, y: f32) -> &'static str {
        let Some(node) = self.node_at(x, y) else { return "default" };
        if self.link_of(node).is_some() {
            return "pointer";
        }
        let mut cur = Some(node);
        while let Some(n) = cur {
            if let Some(s) = self.styles.get(n) {
                if s.cursor_pointer {
                    return "pointer";
                }
                break;
            }
            cur = self.doc.parent(n);
        }
        if self.doc.is_text(node) {
            return "text";
        }
        "default"
    }
}
