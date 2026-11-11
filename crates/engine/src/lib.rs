//! The engine ties everything together: fetch → parse → style → layout →
//! paint, plus hit testing, scrolling state and text extraction.
//!
//! Loading is split in two stages so a GUI can keep its event loop alive:
//! a [`Loader`] (`Clone + Send`) fetches and decodes every resource on a
//! background thread and yields a [`LoadedPage`] (plain data), and the
//! [`Engine`] turns it into a [`Page`] (styles, layout, display list) on the
//! thread that owns the fonts and paints.

pub mod animations;
pub mod diff;
pub mod forms;
pub mod frames;
mod home;
pub mod images;
pub mod incremental;
pub mod pointer;
pub mod scripting;
pub mod select;
mod text;

pub use select::{Selection, TextRun};

pub use forms::{ChosenFile, ControlKind};
pub use pointer::{ClickAction, Modifiers};
pub use rabbithole_net::{Body, Request, StreamedResponse};

use rabbithole_css::{parse_media_query, MatchState, MediaQuery, PseudoElement, Rule};
use rabbithole_dom::{Document, NodeId};
use rabbithole_gfx::{Canvas, Color, FontContext, Image, Rect};
use rabbithole_layout::{DisplayList, LayoutContext, LayoutTree, ScrollInfo};
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
pub use images::{choose_candidate, eval_sizes, media_matches, parse_srcset, select_image, ImageChoice, ImageInfo, SrcCandidate};
pub use scripting::{Dialog, DialogPolicy, LocalStorage, PendingNavigation, ScriptOutcome, ScriptState};

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
    /// Stylesheet URLs served over HTTP with a Content-Type other than
    /// `text/css`: ignored outside quirks mode (HTML §4.2.4 "link type
    /// stylesheet", fetch and process the linked resource).
    css_wrong_type: HashSet<String>,
    /// Image-request URLs that answered with a document (HTML / XML / text)
    /// instead of an image: an `<object>` showing one is a nested browsing
    /// context, not a failed load (no fallback content).
    documents: HashSet<String>,
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
    /// Device pixels per CSS pixel (picks `srcset` candidates).
    pub device_pixel_ratio: f32,
    /// Page whose subresources are being fetched (referer / Sec-Fetch-Site).
    pub initiator: Option<Url>,
    /// Fetch `<img>` and CSS background images before the page is built
    /// (otherwise the engine loads them in the background after the first
    /// layout, so the page can be shown sooner).
    pub prefetch_images: bool,
    /// Leave external scripts unfetched in [`LoadedPage::scripts`] (empty
    /// `source`, `url` set) so the page can be painted before they arrive;
    /// fetch them with [`Loader::fetch_script_sources`].
    pub defer_scripts: bool,
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

/// Largest external script body accepted.
const MAX_SCRIPT_BYTES: usize = 32 * 1024 * 1024;

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
            device_pixel_ratio: 1.0,
            initiator: None,
            prefetch_images: true,
            defer_scripts: false,
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
        let mut page = self.load_response(resp, viewport);
        inherit_fragment(&mut page.url, &url);
        Ok(page)
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
        let mut page = self.load_response(resp, viewport);
        inherit_fragment(&mut page.url, &req.url);
        Ok(Loaded::Page(page))
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
        // With scripting on, `<noscript>` content is raw text (HTML spec).
        let doc = rabbithole_html::parse_with_scripting(html, self.load_scripts);
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
        let images = if self.load_images && self.prefetch_images { self.load_images(&doc, &styles, &base_url, viewport) } else { Vec::new() };
        if debug {
            eprintln!("[load] images {:?}", t.elapsed());
        }
        let t = std::time::Instant::now();
        let background_images = if self.load_images && self.prefetch_images { self.load_background_images(&styles) } else { Vec::new() };
        if debug {
            eprintln!("[load] background images {:?}", t.elapsed());
        }
        let t = std::time::Instant::now();
        let fonts = if self.load_fonts { self.load_fonts(&styles) } else { Vec::new() };
        if debug {
            eprintln!("[load] fonts {:?}", t.elapsed());
        }
        let t = std::time::Instant::now();
        let scripts = if !self.load_scripts {
            Vec::new()
        } else if self.defer_scripts {
            self.plan_scripts(&doc, &base_url)
        } else {
            self.collect_scripts(&doc, &base_url)
        };
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
        #[derive(Clone, PartialEq, Eq, Hash)]
        enum Src {
            Inline(String, Option<String>),
            Link(Url, Option<String>),
        }
        // (source, shadow root scope or None for the document)
        let mut sources: Vec<(Src, Option<NodeId>)> = Vec::new();
        let scan = |root: NodeId, scope: Option<NodeId>, sources: &mut Vec<(Src, Option<NodeId>)>| {
            for n in doc.descendants(root) {
                let Some(el) = doc.element(n) else { continue };
                // With scripting enabled `<noscript>` content is inert raw text
                // for a real HTML parser: its style sheets must not apply (sites
                // like Google put `div,span{display:none}` there).
                if matches!(el.tag.as_str(), "style" | "link") && self.load_scripts && in_noscript(doc, n) {
                    continue;
                }
                if let Some((source, text)) = doc.sheet_overrides.get(&n) {
                    // Sheet modified through the CSSOM (insertRule, ...).
                    if el.tag == "link" || *source == doc.text_content(n) {
                        let media = el.attr("media").map(|m| m.to_string());
                        sources.push((Src::Inline(text.clone(), media), scope));
                        continue;
                    }
                }
                match el.tag.as_str() {
                    "style" => {
                        let media = el.attr("media").map(|m| m.to_string());
                        sources.push((Src::Inline(doc.text_content(n), media), scope));
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
                        sources.push((Src::Link(url, media), scope));
                    }
                    _ => {}
                }
            }
            // Constructed sheets adopted by the document / shadow root
            // come after its own sheets.
            if let Some(list) = doc.adopted_styles.get(&root) {
                for text in list {
                    sources.push((Src::Inline(text.clone(), None), scope));
                }
            }
        };
        scan(Document::ROOT, None, &mut sources);
        if let Some(list) = doc.adopted_styles.get(&Document::SCRIPT_FONT_FACES) {
            for text in list {
                sources.push((Src::Inline(text.clone(), None), None));
            }
        }
        for root in doc.connected_shadow_roots() {
            scan(root, Some(root), &mut sources);
        }

        let mut to_fetch: Vec<Url> = Vec::new();
        {
            let caches = self.caches.lock().unwrap();
            for (s, _) in &sources {
                if let Src::Link(u, _) = s {
                    let key = u.without_fragment().to_string();
                    if !caches.css.contains_key(&key) && !to_fetch.iter().any(|x| x.without_fragment().to_string() == key) {
                        to_fetch.push(u.clone());
                    }
                }
            }
        }
        self.fetch_css_into_cache(&to_fetch);

        let mut sheets: Vec<Stylesheet> = Vec::new();
        // Shadow trees of the same component share their sheets: parse each
        // (source) once and list every shadow root it is scoped to.
        let mut shadow_sheet_of: std::collections::HashMap<Src, usize> = std::collections::HashMap::new();
        for (s, scope) in sources {
            if let Some(root) = scope {
                if let Some(&i) = shadow_sheet_of.get(&s) {
                    sheets[i].shadow_scopes.push(root);
                    continue;
                }
            }
            let parsed = match &s {
                Src::Inline(text, media) => Some(parsed_sheet_cached(text, media.as_deref(), &format!("{}{base}", self.load_stylesheets as u8), || {
                    let mut sheet = rabbithole_css::parse_stylesheet(text);
                    sheet.base_url = Some(base.to_string());
                    self.resolve_imports(&mut sheet, base, 0);
                    wrap_media(sheet, media.as_deref())
                })),
                Src::Link(url, media) => {
                    let key = url.without_fragment().to_string();
                    let caches = self.caches.lock().unwrap();
                    let ignored = doc.quirks_mode != rabbithole_dom::QuirksMode::Quirks && caches.css_wrong_type.contains(&key);
                    let text = if ignored { None } else { caches.css.get(&key).cloned() };
                    drop(caches);
                    text.map(|text| {
                        parsed_sheet_cached(&text, media.as_deref(), &format!("{}{url}", self.load_stylesheets as u8), || {
                            let mut sheet = rabbithole_css::parse_stylesheet(&text);
                            sheet.base_url = Some(url.to_string());
                            self.resolve_imports(&mut sheet, url, 0);
                            wrap_media(sheet, media.as_deref())
                        })
                    })
                }
            };
            if let Some(mut sheet) = parsed {
                if let Some(root) = scope {
                    sheet.shadow_scopes = vec![root];
                    shadow_sheet_of.insert(s, sheets.len());
                }
                sheets.push(sheet);
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
                    let mime = resp.content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
                    if u.is_http() && !mime.is_empty() && mime != "text/css" {
                        caches.css_wrong_type.insert(key.clone());
                    }
                    let text = decode_text(&resp.body, resp.charset.as_deref());
                    caches.css.insert(key, Arc::from(text));
                }
                other => {
                    if std::env::var_os("RH_DEBUG_FETCH").is_some() {
                        match &other {
                            Ok(resp) => eprintln!("[fetch] css {u}: HTTP {}", resp.status),
                            Err(e) => eprintln!("[fetch] css {u}: {e}"),
                        }
                    }
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

    fn load_images(&self, doc: &Document, styles: &StyledTree, base: &Url, viewport: Viewport) -> Vec<(NodeId, Arc<Image>)> {
        let mut wanted: Vec<(NodeId, Url, f32)> = Vec::new();
        for n in doc.shadow_including_descendants(Document::ROOT) {
            if !images::is_image_element(doc, n) {
                continue;
            }
            if styles.get(n).is_none() {
                continue; // display:none
            }
            // `loading=lazy` waits for layout (the engine loads it when it
            // comes near the viewport); without scripting it is eager.
            if self.load_scripts && doc.attr(n, "loading").map(|l| l.trim().eq_ignore_ascii_case("lazy")).unwrap_or(false) {
                continue;
            }
            let Some(choice) = images::select_image(doc, n, viewport, self.device_pixel_ratio, self.load_scripts, None) else { continue };
            let Some(url) = base.join(&choice.url) else { continue };
            if url.scheme == "javascript" {
                continue;
            }
            wanted.push((n, url, choice.density));
            if wanted.len() >= self.max_images {
                break;
            }
        }
        let urls: Vec<Url> = wanted.iter().map(|(_, u, _)| u.clone()).collect();
        self.fetch_images_into_cache(&urls);
        let caches = self.caches.lock().unwrap();
        wanted
            .into_iter()
            .filter_map(|(n, u, d)| caches.images.get(&u.to_string()).cloned().flatten().map(|img| (n, images::apply_density(img, d))))
            .collect()
    }

    /// A finished fetch for `url` from the shared cache: `None` = not
    /// fetched yet, `Some(None)` = failed / undecodable.
    pub(crate) fn cached_image(&self, url: &str) -> Option<Option<Arc<Image>>> {
        self.caches.lock().unwrap().images.get(url).cloned()
    }

    /// Did `url` (fetched as an image) turn out to be a document?
    pub(crate) fn is_document_resource(&self, url: &str) -> bool {
        self.caches.lock().unwrap().documents.contains(url)
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

    pub(crate) fn fetch_images_into_cache(&self, urls: &[Url]) {
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
        fetcher.max_body = 32 * 1024 * 1024;
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
        // Decode in parallel without holding the cache lock (the UI thread
        // polls the cache while background loads run).
        let jobs: Vec<(Url, Option<Response>)> = to_fetch.into_iter().zip(results).map(|(u, r)| (u, r.ok().filter(|resp| resp.status < 400))).collect();
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8);
        let next = std::sync::atomic::AtomicUsize::new(0);
        let decoded: Mutex<Vec<(String, Option<Arc<Image>>)>> = Mutex::new(Vec::with_capacity(jobs.len()));
        std::thread::scope(|scope| {
            for _ in 0..threads.min(jobs.len()) {
                scope.spawn(|| loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((u, resp)) = jobs.get(i) else { break };
                    let td = std::time::Instant::now();
                    let img = resp.as_ref().and_then(|r| decode_image(&r.body, &r.content_type, &u.path)).map(Arc::new);
                    if debug {
                        eprintln!("[decode] {:?} {} -> {}", td.elapsed(), u, img.as_ref().map(|i| format!("{}x{}{}", i.width, i.height, if i.is_animated() { " animated" } else { "" })).unwrap_or_else(|| "none".into()));
                    }
                    decoded.lock().unwrap().push((u.to_string(), img));
                });
            }
        });
        let mut caches = self.caches.lock().unwrap();
        for (k, img) in decoded.into_inner().unwrap() {
            if img.is_none() {
                let doc_type = jobs.iter().find(|(u, _)| u.to_string() == k).and_then(|(_, r)| r.as_ref()).map(|r| {
                    let mime = r.content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
                    matches!(mime.as_str(), "text/html" | "application/xhtml+xml" | "text/xml" | "application/xml" | "text/plain" | "image/svg+xml") || mime.ends_with("+xml")
                });
                if doc_type == Some(true) {
                    caches.documents.insert(k.clone());
                }
            }
            caches.images.insert(k, img);
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
        let mut out = self.plan_scripts(doc, base);
        let initiator = self.initiator.clone().unwrap_or_else(|| base.clone());
        self.fetch_script_sources(&initiator, &mut out);
        out
    }

    /// Fetch the sources of external scripts planned without them (see
    /// [`Loader::defer_scripts`]). `page_url` is the document's URL.
    pub fn fetch_script_sources(&self, page_url: &Url, scripts: &mut [ScriptSource]) {
        let external: Vec<(usize, Url)> = scripts
            .iter()
            .enumerate()
            .filter(|(_, s)| s.source.is_empty())
            .filter_map(|(i, s)| s.url.clone().map(|u| (i, u)))
            .collect();
        if external.is_empty() {
            return;
        }
        let urls: Vec<Url> = external.iter().map(|(_, u)| u.clone()).collect();
        // App bundles of large sites exceed 8 MB (a video site's main
        // bundle is ~10 MB): allow as much as for documents.
        let mut fetcher = self.fetcher.clone();
        fetcher.max_body = MAX_SCRIPT_BYTES;
        let results = fetcher.fetch_many_for(&urls, 6, Destination::Script, Some(page_url));
        for ((idx, url), r) in external.iter().zip(results) {
            match r {
                Ok(resp) if resp.status < 400 => {
                    scripts[*idx].source = decode_text(&resp.body, resp.charset.as_deref());
                }
                Ok(resp) => {
                    if std::env::var_os("RH_DEBUG_SCRIPTS").is_some() {
                        eprintln!("[script] failed to load {url}: HTTP {}", resp.status);
                    }
                    scripts[*idx].source.clear();
                }
                Err(e) => {
                    if std::env::var_os("RH_DEBUG_SCRIPTS").is_some() {
                        eprintln!("[script] failed to load {url}: {e}");
                    }
                    scripts[*idx].source.clear();
                }
            }
        }
    }

    /// The document's scripts in order; inline ones with their source,
    /// external ones with only their URL.
    fn plan_scripts(&self, doc: &Document, base: &Url) -> Vec<ScriptSource> {
        let mut out = Vec::new();
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
                    Some(u) => out[idx].url = Some(u),
                    None => {
                        out.pop();
                    }
                },
                None => out[idx].source = doc.text_content(n),
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
    /// Origin-scoped `localStorage` shared by all pages (and by engines on
    /// other threads via [`Engine::share_local_storage`]).
    pub local_storage: Arc<Mutex<LocalStorage>>,
    /// Monotonic clock origin for timers / `performance.now()`.
    pub start: std::time::Instant,
    /// Device pixels per CSS pixel (see [`Engine::set_device_pixel_ratio`]).
    pub device_pixel_ratio: f32,
    /// How `confirm()`/`prompt()` are answered on newly built pages
    /// (before their scripts run).
    pub dialog_policy: scripting::DialogPolicy,
    registered_fonts: std::cell::RefCell<HashSet<String>>,
    /// Frame tree of all pages (what scripts see of other frames) and
    /// queued cross-frame messages.
    pub frames: std::cell::RefCell<frames::FrameRegistry>,
    /// Set while a frame's page is being built.
    pub(crate) frame_context: std::cell::RefCell<Option<frames::FrameContext>>,
    /// URLs (without fragment) of the documents loaded so far (pages and
    /// frames): links to them match `:visited`.
    pub visited: std::cell::RefCell<HashSet<String>>,
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
    /// Image loading after the first layout (lazy, srcset, scripts).
    pub image_state: images::ImageState,
    /// Hover / pressed-button state for pointer events.
    pub pointer: pointer::PointerState,
    /// Incremental style/layout state (see [`incremental`]).
    pub incr: incremental::IncrementalState,
    /// Engine-wide id of this document's browsing context.
    pub frame_id: u64,
    /// Nesting depth (0 = top-level page).
    pub frame_depth: u32,
    /// `window.name` from the hosting iframe's `name` attribute.
    pub frame_name: Option<String>,
    /// Inherited origin (about:blank / srcdoc frames).
    pub frame_origin: Option<String>,
    /// Documents of this page's `<iframe>` elements.
    pub frames: frames::Frames,
    /// Running CSS transitions and animations.
    pub anim: animations::AnimationState,
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
            local_storage: Arc::new(Mutex::new(LocalStorage::default())),
            start: std::time::Instant::now(),
            device_pixel_ratio: 1.0,
            dialog_policy: scripting::DialogPolicy::default(),
            registered_fonts: Default::default(),
            frames: Default::default(),
            frame_context: Default::default(),
            visited: Default::default(),
        }
    }

    /// Engine without system font discovery (faster startup for tests/CLI).
    pub fn new_bundled_fonts_only() -> Engine {
        Engine {
            fonts: Rc::new(FontContext::new()),
            loader: Loader::new(),
            scripting: true,
            console_to_stderr: std::env::var("RH_JS_LOG").is_ok(),
            local_storage: Arc::new(Mutex::new(LocalStorage::default())),
            start: std::time::Instant::now(),
            device_pixel_ratio: 1.0,
            dialog_policy: scripting::DialogPolicy::default(),
            registered_fonts: Default::default(),
            frames: Default::default(),
            frame_context: Default::default(),
            visited: Default::default(),
        }
    }

    /// Persist `localStorage` to a file (loaded now, saved after script runs).
    pub fn set_local_storage_path(&mut self, path: std::path::PathBuf) {
        *self.local_storage.lock().unwrap() = LocalStorage::load(path);
    }

    /// Use another engine's `localStorage` (engines on several threads,
    /// e.g. one per browser tab, then see the same data and file).
    pub fn share_local_storage(&mut self, storage: Arc<Mutex<LocalStorage>>) {
        self.local_storage = storage;
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
        let mut page = self.build_page_unscripted(loaded, viewport);
        self.finish_page_load(&mut page);
        page
    }

    /// First half of [`Engine::build_page`]: web fonts, style, layout and
    /// display list, without running scripts, so the page can be painted
    /// right away (progressive rendering). Call [`Engine::finish_page_load`]
    /// next (after filling `page.scripts` when they were deferred).
    /// Links of `page` whose destination is in the visited history (for
    /// `:visited`), recomputed when the DOM or the history changed.
    pub(crate) fn update_visited_links(&self, page: &mut Page) {
        let visited = self.visited.borrow();
        let key = (page.doc.epoch(), visited.len(), page.base_url.to_string());
        if page.incr.visited_key.as_ref() == Some(&key) {
            return;
        }
        let base = page.base_url.clone();
        let mut links = HashSet::new();
        if !visited.is_empty() {
            for n in page.doc.descendants(Document::ROOT) {
                let Some(el) = page.doc.element(n) else { continue };
                if !matches!(el.tag.as_str(), "a" | "area") {
                    continue;
                }
                let Some(href) = el.attr("href") else { continue };
                if let Some(u) = base.join(href.trim()) {
                    if visited.contains(&u.without_fragment().to_string()) {
                        links.insert(n);
                    }
                }
            }
        }
        if *page.state.visited != links {
            page.state.visited = std::sync::Arc::new(links);
        }
        page.incr.visited_key = Some(key);
    }

    pub fn build_page_unscripted(&self, loaded: LoadedPage, viewport: Viewport) -> Page {
        self.visited.borrow_mut().insert(loaded.url.without_fragment().to_string());
        for f in &loaded.fonts {
            let key = format!("{}|{}|{}", f.face.family.to_ascii_lowercase(), f.face.weight.0, f.face.italic);
            if self.registered_fonts.borrow_mut().insert(key) {
                self.fonts.add_font_data(&f.face.family, f.face.weight.0, f.face.italic, &f.data);
            }
        }
        let has_dynamic_rules = loaded.sheets.iter().any(sheet_has_dynamic_rules);
        let frame = self.frame_context.borrow_mut().take();
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
            pointer: pointer::PointerState::default(),
            sheet_signature: 0,
            image_state: images::ImageState::default(),
            incr: incremental::IncrementalState::default(),
            frame_id: frame.as_ref().map(|f| f.id).unwrap_or_else(frames::new_frame_id),
            frame_depth: frame.as_ref().map(|f| f.depth).unwrap_or(0),
            frame_name: frame.as_ref().and_then(|f| f.name.clone()),
            frame_origin: frame.and_then(|f| f.origin),
            frames: frames::Frames::default(),
            anim: animations::AnimationState::default(),
        };
        page.script.dialog_policy = self.dialog_policy.clone();
        page.sheet_signature = stylesheet_signature(&page.doc);
        self.restyle(&mut page);
        page
    }

    /// Second half of [`Engine::build_page`]: run the page's scripts (load
    /// events included), then apply `<meta http-equiv=refresh>`. What the
    /// scripts asked for (navigation, windows, dialogs, scrolling) is left
    /// on the page for the embedder.
    pub fn finish_page_load(&self, page: &mut Page) {
        if self.scripting && (!page.scripts.is_empty() || has_inline_handlers(&page.doc)) {
            // Keep what the scripts asked for during load (redirects via
            // `location`, `window.open`, dialogs) for the embedder to apply.
            let out = self.run_page_scripts(page);
            if out.navigation.is_some() {
                page.script.pending_navigation = out.navigation;
            }
            page.script.new_windows.extend(out.new_windows);
            page.script.dialogs.extend(out.dialogs);
            if out.scroll_to.is_some() {
                page.requested_scroll = out.scroll_to;
            }
        }
        self.shrink_oversized_images(page);
        // `<meta http-equiv="refresh" content="N; url=...">` (N <= 5 s is
        // followed right away; longer delays are left to the user).
        if page.script.pending_navigation.is_none() {
            if let Some(target) = meta_refresh_target(page, self.scripting) {
                page.script.pending_navigation = Some(PendingNavigation {
                    request: Request::get(target).with_referer(page.url.clone()),
                    replace: true,
                });
            }
        }
    }

    /// Take the navigation a page requested by itself (script redirect or
    /// meta refresh), if any.
    pub fn take_page_navigation(&self, page: &mut Page) -> Option<PendingNavigation> {
        page.script.pending_navigation.take()
    }

    /// Bring style, layout and the display list up to date (after DOM,
    /// viewport or selector-state changes). Incremental: only what changed
    /// is restyled, rebuilt and laid out again (see [`incremental`]).
    pub fn restyle(&self, page: &mut Page) {
        let before = page.incr.generation();
        let t = std::time::Instant::now();
        self.flush_style(page);
        let t_style = t.elapsed();
        self.relayout_only(page);
        if incremental::debug_enabled() {
            eprintln!("[incr] restyle: style {:?}, layout+dl {:?}", t_style, t.elapsed() - t_style);
        }
        // (Re)select image sources for the new viewport / DOM and start
        // lazy or script-requested loads; cached results show right away.
        // Nothing changed: nothing to select (scrolling syncs lazy images).
        if (page.incr.generation() != before || page.incr.take_dpr_change(self.device_pixel_ratio)) && self.sync_images(page) {
            self.relayout_only(page);
        }
        self.sync_frames(page);
    }

    /// Current computed style of one element: flushes pending style
    /// changes only (layout stays dirty until a geometry query or paint).
    pub fn computed_style_now(&self, page: &mut Page, node: NodeId) -> Option<Rc<rabbithole_style::ComputedStyle>> {
        self.flush_style(page);
        page.styles.get(node).cloned().or_else(|| self.unrendered_style(page, node))
    }

    /// Bring layout + display list up to date (images, scroll offsets or
    /// styles changed); only damaged boxes are laid out again.
    pub fn relayout_only(&self, page: &mut Page) {
        self.flush_display_list(page);
    }

    /// Rebuild only the display list (after scrolling: sticky/fixed boxes
    /// and inner scroll offsets).
    pub fn rebuild_display_list(&self, page: &mut Page) {
        let Page { doc, styles, images, background_images, viewport, scroll_offsets, page_scroll, layout, display_list, incr, .. } = page;
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
        let old = std::mem::replace(display_list, rabbithole_layout::build_display_list(layout, &ctx));
        incr.display_list_replaced(old);
        incr.mark_display_list_clean();
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
        if page.incr.depends_on_page_scroll(&page.layout) {
            self.rebuild_display_list(page);
        } else {
            // Only sticky boxes move: patch their translations in place.
            page.display_list.update_page_scroll((x, y), (page.viewport.width, page.viewport.height));
        }
        self.sync_lazy_images(page);
    }

    /// Scroll the innermost scroll container under (x, y) (document
    /// coordinates) by (dx, dy). Returns false when there is none (the
    /// caller should scroll the page instead) or it is already at its limit.
    pub fn scroll_at(&self, page: &mut Page, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        if let Some(scrolled) = self.frame_scroll_at(page, x, y, dx, dy) {
            return scrolled;
        }
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
        self.apply_scroll_offset(page, info.node, (nx, ny));
        true
    }

    /// Move a scroll container's contents to `offset` (already clamped).
    fn apply_scroll_offset(&self, page: &mut Page, node: NodeId, offset: (f32, f32)) {
        page.scroll_offsets.insert(node, offset);
        if !page.incr.dirty() && page.layout.set_scroll_offset(&page.doc, node, offset) {
            // Clean layout: scroll by translating the container's contents.
            page.incr.scrolled(node, offset);
            self.rebuild_display_list(page);
        } else {
            self.relayout_only(page);
        }
    }

    /// Scroll state of an element's box: (scroll offset, scroll size). A
    /// box that does not clip reports its padding box (nothing scrolled).
    pub fn element_scroll(&self, page: &Page, node: NodeId) -> Option<((f32, f32), (f32, f32))> {
        let b = page.layout.box_of(&page.doc, node)?;
        if b.clips {
            Some((b.scroll_offset, (b.scroll_width, b.scroll_height)))
        } else {
            let p = b.padding_rect();
            Some(((0.0, 0.0), (p.w, p.h)))
        }
    }

    /// Scroll an element's scroll container to `(x, y)` (clamped; e.g.
    /// `element.scrollTop = ...`). Returns whether it moved.
    pub fn set_element_scroll(&self, page: &mut Page, node: NodeId, x: f32, y: f32) -> bool {
        let Some(b) = page.layout.box_of(&page.doc, node) else { return false };
        if !b.scroll_container {
            return false;
        }
        let p = b.padding_rect();
        let max = ((b.scroll_width - p.w).max(0.0), (b.scroll_height - p.h).max(0.0));
        let to = (x.clamp(0.0, max.0), y.clamp(0.0, max.1));
        if to == b.scroll_offset {
            return false;
        }
        self.apply_scroll_offset(page, node, to);
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
        page.state.focus_visible = node.map(|n| page.focus_should_be_visible(n)).unwrap_or(false);
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

/// The response URL keeps the request's fragment unless a redirect gave
/// it its own (Fetch: "location URL's fragment").
fn inherit_fragment(url: &mut Url, requested: &Url) {
    if url.fragment.is_none() {
        url.fragment = requested.fragment.clone();
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
        let img = SVG_FONTS.with(|fonts| tree.rasterize(fonts, pw, ph));
        return Some(img.with_natural_size(w.round().max(1.0) as u32, h.round().max(1.0) as u32));
    }
    Image::decode(bytes)
}

/// Rasterize inline `<svg>` elements of a document into images keyed by
/// node, so layout/paint treat them as replaced images.
fn rasterize_inline_svgs(doc: &Document, styles: &StyledTree, fonts: &FontContext, images: &mut HashMap<NodeId, Arc<Image>>) {
    let mut roots = Vec::new();
    for n in doc.shadow_including_descendants(Document::ROOT) {
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
            shadow_scopes: sheet.shadow_scopes,
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

/// Does any element carry an inline event handler (`onclick="..."`)?
/// Such pages need a script host even without `<script>` elements.
fn has_inline_handlers(doc: &Document) -> bool {
    doc.descendants(Document::ROOT).any(|n| {
        doc.element(n)
            .map(|e| e.attrs.iter().any(|(k, _)| k.len() > 2 && k.starts_with("on")))
            .unwrap_or(false)
    })
}

/// Is the node inside a `<noscript>` element?
pub(crate) fn in_noscript(doc: &rabbithole_dom::Document, mut n: NodeId) -> bool {
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
    doc.adopted_styles.get(&Document::SCRIPT_FONT_FACES).hash(&mut h);
    let mut roots = vec![Document::ROOT];
    roots.extend(doc.connected_shadow_roots());
    for root in roots {
    root.hash(&mut h);
    doc.adopted_styles.get(&root).hash(&mut h);
    for n in doc.descendants(root) {
        let Some(el) = doc.element(n) else { continue };
        if let Some(o) = doc.sheet_overrides.get(&n) {
            o.hash(&mut h);
        }
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
    }
    h.finish()
}

fn sheet_has_dynamic_rules(sheet: &Stylesheet) -> bool {
    use rabbithole_css::selector::{Compound, Pseudo};
    fn compound_dynamic(c: &Compound) -> bool {
        c.pseudos.iter().any(|p| match p {
            Pseudo::Hover | Pseudo::Active | Pseudo::Focus | Pseudo::FocusVisible | Pseudo::FocusWithin | Pseudo::Target => true,
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

impl Page {
    /// Forget incremental state: the next restyle recomputes everything.
    pub fn invalidate_all(&mut self) {
        self.incr.invalidate_all();
    }

    /// Forget cached layout: the next relayout lays out everything.
    pub fn invalidate_layout(&mut self) {
        self.incr.invalidate_layout();
    }

    /// Should focus moving to `node` match `:focus-visible`? Like
    /// browsers: always for text-entry controls, otherwise only when the
    /// last user input was not a pointer press (keyboard navigation, or
    /// script focus before any click).
    pub fn focus_should_be_visible(&self, node: NodeId) -> bool {
        !self.pointer.pointer_modality || pointer::is_text_entry(&self.doc, node)
    }

    pub fn content_height(&self) -> f32 {
        self.layout.height
    }
    pub fn content_width(&self) -> f32 {
        self.layout.width
    }

    /// Element at document coordinates (with the current page scroll, so
    /// fixed boxes are found where they are painted).
    pub fn node_at(&self, x: f32, y: f32) -> Option<NodeId> {
        self.node_at_viewport(x - self.page_scroll.0, y - self.page_scroll.1)
    }

    /// Topmost element at viewport coordinates. Uses the hit regions
    /// recorded with the display list, so stacking order (z-index,
    /// positioned and fixed boxes), transforms, overflow clips,
    /// `visibility` and `pointer-events` match what is painted.
    pub fn node_at_viewport(&self, vx: f32, vy: f32) -> Option<NodeId> {
        self.display_list.hits.hit_test(vx, vy, self.page_scroll)
    }

    /// All elements under a viewport point, topmost first.
    pub fn nodes_at_viewport(&self, vx: f32, vy: f32) -> Vec<NodeId> {
        self.display_list.hits.hit_test_all(vx, vy, self.page_scroll)
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

    /// Vertical scroll position for navigating to `fragment` (HTML "scroll
    /// to the fragment"): the element with that id (or `<a name>`), else the
    /// top of the document for an empty fragment or `top`. Clamped to the
    /// scrollable range. `None` when the fragment names nothing.
    pub fn fragment_scroll(&self, fragment: &str) -> Option<f32> {
        let id = fragment.trim_start_matches('#');
        let id = rabbithole_net::percent_decode(id);
        let max = (self.content_height() - self.viewport.height).max(0.0);
        if !id.is_empty() {
            let node = self.doc.element_by_id(&id).or_else(|| {
                self.doc
                    .descendants(Document::ROOT)
                    .find(|&n| self.doc.is_element(n, "a") && self.doc.attr(n, "name") == Some(id.as_str()))
            });
            if let Some(r) = node.and_then(|n| self.rect_of(n)) {
                return Some(r.y.clamp(0.0, max));
            }
        }
        (id.is_empty() || id.eq_ignore_ascii_case("top")).then_some(0.0)
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
        if node >= self.doc.nodes.len() {
            return None;
        }
        self.incr.cached_rect(node, || {
            if self.layout.box_of(&self.doc, node).is_none() && self.doc.element(node).is_some() {
                // Inline elements: the union of their line fragments.
                if let Some(r) = self.incr.cached_inline_rect(node, || self.layout.inline_rect_index(&self.doc)) {
                    return Some(r);
                }
            }
            let mut cur = Some(node);
            while let Some(n) = cur {
                if let Some(b) = self.layout.box_of(&self.doc, n) {
                    return Some(b.rect);
                }
                cur = self.doc.flat_parent(n);
            }
            None
        })
    }

    /// Scroll container under a document point.
    pub fn scroll_container_at(&self, x: f32, y: f32) -> Option<ScrollInfo> {
        self.layout.scroll_container_at(x, y)
    }

    /// All links in document order (hidden ones excluded).
    pub fn links(&self) -> Vec<Link> {
        let mut out = Vec::new();
        for n in self.doc.shadow_including_descendants(Document::ROOT) {
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
            .shadow_including_descendants(Document::ROOT)
            .into_iter()
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
                if s.cursor_pointer() {
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

/// A parsed style sheet (with its imports resolved and media applied) for
/// this source text, media and base URL: re-collecting the sheets after a
/// script inserts or removes one (style-in-JS, Polymer / Shady CSS) parses
/// only the new ones.
fn parsed_sheet_cached(text: &str, media: Option<&str>, base: &str, parse: impl FnOnce() -> Stylesheet) -> Stylesheet {
    type Key = (Box<str>, Option<Box<str>>, Box<str>);
    thread_local! {
        static CACHE: std::cell::RefCell<(usize, HashMap<Key, Stylesheet>)> = std::cell::RefCell::new((0, HashMap::new()));
    }
    let key: Key = (text.into(), media.map(Into::into), base.into());
    if let Some(s) = CACHE.with(|c| c.borrow().1.get(&key).cloned()) {
        return s;
    }
    let sheet = parse();
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        // Bounded by source size; dropped wholesale when full.
        if c.0 + text.len() > 32 << 20 {
            c.0 = 0;
            c.1.clear();
        }
        c.0 += text.len();
        c.1.insert(key, sheet.clone());
    });
    sheet
}
