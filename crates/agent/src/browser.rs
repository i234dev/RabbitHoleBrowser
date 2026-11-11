//! The agent's browser: isolated contexts (cookie jar + localStorage),
//! tabs with frames, stable refs, events (dialogs, popups, downloads),
//! console and network logs, and the operations behind the tools.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rabbithole_engine::dom::{Document, NodeId};
use rabbithole_engine::net::{CookieJar, HeaderProfile, NetLog, Request, Url};
use rabbithole_engine::scripting::ScriptOutcome;
use rabbithole_engine::{DialogPolicy, Engine, Loaded, LocalStorage, Modifiers, Page, Viewport};
use serde_json::Value;

use crate::ax::{self, clip_text, esc, Ax, BuildOptions, BuildStats, FrameView, GeoMap, Kind};
use crate::overlay::{self, Overlay, Reach};
use crate::refs::{Ref, RefMap};
use crate::snapshot::{self, RenderCtx};
use crate::wait::{self, Settled};

/// Startup configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Persistent profile of the default context (cookies, localStorage);
    /// `None` = in-memory only.
    pub profile_dir: Option<PathBuf>,
    /// Identify as RabbitHoleBot and respect robots.txt (default true).
    pub bot: bool,
    pub viewport: Viewport,
    pub javascript: bool,
    /// Download images while loading (screenshots load them on demand).
    pub images: bool,
    /// Cap of the auto-wait after loads and actions.
    pub settle_cap: Duration,
    /// Only these domains (and subdomains) may be loaded.
    pub allowlist: Vec<String>,
    /// Scan system fonts (faithful screenshots; slower startup).
    pub system_fonts: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            profile_dir: None,
            bot: true,
            viewport: Viewport { width: 1280.0, height: 800.0 },
            javascript: true,
            images: false,
            settle_cap: Duration::from_secs(5),
            allowlist: Vec::new(),
            system_fonts: false,
        }
    }
}

/// Output of an operation: text (and optionally a PNG).
#[derive(Debug, Default, Clone)]
pub struct Out {
    pub text: String,
    pub png: Option<Vec<u8>>,
}

impl Out {
    pub fn text(t: impl Into<String>) -> Out {
        Out { text: t.into(), png: None }
    }
}

/// An isolated browsing context.
pub struct Context {
    pub name: String,
    pub cookies: Arc<Mutex<CookieJar>>,
    storage: Arc<Mutex<LocalStorage>>,
    dir: PathBuf,
    persistent: bool,
}

impl Context {
    fn new(name: &str, dir: PathBuf, persistent: bool) -> Context {
        let _ = std::fs::create_dir_all(&dir);
        let jar = if persistent { CookieJar::load(&dir.join("cookies.txt")).unwrap_or_default() } else { CookieJar::new() };
        if !persistent {
            let _ = std::fs::remove_file(dir.join("localstorage.txt"));
        }
        let storage = Arc::new(Mutex::new(LocalStorage::load(dir.join("localstorage.txt"))));
        Context { name: name.to_string(), cookies: Arc::new(Mutex::new(jar)), storage, dir, persistent }
    }

    fn save_cookies(&self) {
        if self.persistent {
            if let Ok(mut j) = self.cookies.lock() {
                let _ = j.save(&self.dir.join("cookies.txt"));
            }
        }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        if !self.persistent {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// A document in a tab (frame 0 = the top-level page).
pub struct Frame {
    pub page: Page,
    /// (parent frame, iframe element).
    pub parent: Option<(usize, NodeId)>,
}

/// A download that was not saved.
#[derive(Debug, Clone)]
pub struct Download {
    pub id: u32,
    pub name: String,
    pub url: String,
    pub size: Option<u64>,
    pub content_type: String,
    request: Request,
}

pub struct Tab {
    pub id: u32,
    pub ctx: usize,
    pub frames: Vec<Frame>,
    pub refs: RefMap,
    history: Vec<Request>,
    hist_pos: usize,
    pub netlog: Arc<NetLog>,
    pub console: VecDeque<(String, String)>,
    console_reported_errors: usize,
    /// Events not yet reported to the agent.
    pub events: Vec<String>,
    pub invalid: Vec<(usize, NodeId)>,
    pub dialog_policy: DialogPolicy,
    pub opener: Option<u32>,
    /// Popup not loaded yet.
    pub pending: Option<Url>,
    /// Network id of the last request when the current document loaded.
    pub load_net_id: u64,
    pub status: u16,
}

impl Tab {
    fn new(id: u32, ctx: usize) -> Tab {
        Tab {
            id,
            ctx,
            frames: Vec::new(),
            refs: RefMap::default(),
            history: Vec::new(),
            hist_pos: 0,
            netlog: NetLog::new(),
            console: VecDeque::new(),
            console_reported_errors: 0,
            events: Vec::new(),
            invalid: Vec::new(),
            dialog_policy: DialogPolicy { confirm: false, prompt: None },
            opener: None,
            pending: None,
            load_net_id: 0,
            status: 0,
        }
    }

    pub fn page(&self) -> Option<&Page> {
        self.frames.first().map(|f| &f.page)
    }
}

/// Fence marking page-derived content (prompt-injection safety).
pub fn fence(body: &str) -> String {
    let mut buf = [0u8; 4];
    rabbithole_engine::scripting::fill_random(&mut buf);
    let nonce: String = buf.iter().map(|b| format!("{b:02x}")).collect();
    let clean = body.replace(['⟦', '⟧'], "|");
    format!("⟦page {nonce}⟧\n{clean}\n⟦/page {nonce}⟧")
}

/// The browser driven by an agent.
pub struct Agent {
    pub engine: Engine,
    pub cfg: Config,
    pub contexts: Vec<Context>,
    pub tabs: Vec<Tab>,
    pub active: Option<usize>,
    current_ctx: usize,
    next_tab: u32,
    next_download: u32,
    pub downloads: Vec<Download>,
    /// Mutating calls, for replay/export.
    pub action_log: Vec<(String, Value, String)>,
    pub live: Option<crate::live::Live>,
    fonts_loaded: bool,
}

fn temp_dir(name: &str) -> PathBuf {
    let mut b = [0u8; 4];
    rabbithole_engine::scripting::fill_random(&mut b);
    std::env::temp_dir().join(format!("rabbithole-agent-{}-{}-{:02x}{:02x}{:02x}{:02x}", std::process::id(), name.replace(['/', '\\'], "_"), b[0], b[1], b[2], b[3]))
}

/// Short form of a URL for messages.
pub fn short_url(u: &str) -> String {
    clip_text(u, 100)
}

impl Agent {
    pub fn new(cfg: Config) -> Agent {
        let mut engine = if cfg.system_fonts { Engine::new() } else { Engine::new_bundled_fonts_only() };
        engine.scripting = cfg.javascript;
        engine.loader.load_scripts = cfg.javascript;
        engine.loader.load_images = cfg.images;
        engine.loader.load_fonts = cfg.images;
        engine.loader.fetcher.set_header_profile(if cfg.bot { HeaderProfile::Bot } else { HeaderProfile::Browser });
        let default = match &cfg.profile_dir {
            Some(d) => Context::new("default", d.clone(), true),
            None => Context::new("default", temp_dir("default"), false),
        };
        let fonts_loaded = cfg.system_fonts;
        let mut a = Agent {
            engine,
            cfg,
            contexts: vec![default],
            tabs: Vec::new(),
            active: None,
            current_ctx: usize::MAX,
            next_tab: 0,
            next_download: 0,
            downloads: Vec::new(),
            action_log: Vec::new(),
            live: None,
            fonts_loaded,
        };
        a.activate_ctx(0);
        a
    }

    /// Switch identity: bot (default) or normal browser headers.
    pub fn set_bot(&mut self, bot: bool) {
        self.cfg.bot = bot;
        self.engine.loader.fetcher.set_header_profile(if bot { HeaderProfile::Bot } else { HeaderProfile::Browser });
    }

    pub fn set_robots(&mut self, on: bool) {
        self.engine.loader.fetcher.respect_robots = on;
    }

    fn activate_ctx(&mut self, ci: usize) {
        if self.current_ctx == ci {
            return;
        }
        self.engine.share_local_storage(self.contexts[ci].storage.clone());
        self.engine.loader.fetcher.cookies = Some(self.contexts[ci].cookies.clone());
        self.current_ctx = ci;
    }

    /// Point the engine at a tab's context, network log and dialog policy.
    fn prepare(&mut self, ti: usize) {
        let ci = self.tabs[ti].ctx;
        self.activate_ctx(ci);
        self.engine.loader.fetcher.log = Some(self.tabs[ti].netlog.clone());
        self.engine.dialog_policy = self.tabs[ti].dialog_policy.clone();
        let policy = self.tabs[ti].dialog_policy.clone();
        for f in &mut self.tabs[ti].frames {
            f.page.script.dialog_policy = policy.clone();
        }
    }

    /// Index of the active tab.
    pub fn tab_index(&self) -> Result<usize, String> {
        let ti = self.active.ok_or("No page open: call open with a url first.")?;
        if self.tabs[ti].frames.is_empty() && self.tabs[ti].pending.is_none() {
            return Err("No page open in this tab: call open with a url first.".into());
        }
        Ok(ti)
    }

    pub fn context_index(&mut self, name: &str) -> usize {
        if let Some(i) = self.contexts.iter().position(|c| c.name == name) {
            return i;
        }
        self.contexts.push(Context::new(name, temp_dir(name), false));
        self.contexts.len() - 1
    }

    pub fn new_tab(&mut self, ctx: usize) -> usize {
        self.next_tab += 1;
        self.tabs.push(Tab::new(self.next_tab, ctx));
        self.tabs.len() - 1
    }

    fn allowed(&self, url: &Url) -> bool {
        if self.cfg.allowlist.is_empty() || !url.is_http() {
            return true;
        }
        let h = url.host.to_ascii_lowercase();
        self.cfg.allowlist.iter().any(|d| {
            let d = d.trim().trim_start_matches("*.").to_ascii_lowercase();
            h == d || h.ends_with(&format!(".{d}"))
        })
    }

    // ------------------------------------------------------------ loading

    /// Load a request into a tab's top frame (or a child frame). Returns a
    /// short report of what happened (navigation, download, error).
    pub fn navigate(&mut self, ti: usize, req: Request, push_history: bool) -> Result<String, String> {
        self.prepare(ti);
        if !self.allowed(&req.url) {
            return Err(format!("Navigation blocked: {} is not in the domain allowlist.", req.url.host));
        }
        let vp = self.cfg.viewport;
        let mut req = req;
        let mut report = String::new();
        let mut hops = 0;
        let page = loop {
            hops += 1;
            let loaded = match req.url.scheme.as_str() {
                "about" if req.url.path == "blank" => Ok(Loaded::Page(self.engine.loader.load_html("", req.url.clone(), vp))),
                _ => self.engine.loader.load_request(&req, vp),
            };
            let loaded = match loaded {
                Ok(l) => l,
                Err(e) => {
                    let msg = e.to_string();
                    let mut m = format!("Navigation to {} failed: {msg}", short_url(&req.url.to_string()));
                    if msg.contains("HTTP 401") || msg.contains("HTTP 403") {
                        m.push_str(". The site refused the request (login required or bot blocked).");
                    } else if msg.contains("HTTP 429") {
                        m.push_str(". Rate limited: wait before retrying.");
                    }
                    return Err(m);
                }
            };
            match loaded {
                Loaded::Download(stream) => {
                    self.next_download += 1;
                    let d = Download {
                        id: self.next_download,
                        name: stream.suggested_filename(),
                        url: stream.url.to_string(),
                        size: stream.content_length,
                        content_type: stream.content_type.clone(),
                        request: req.clone(),
                    };
                    drop(stream);
                    let msg = format!(
                        "Download #{} offered: \"{}\" ({}, {}) from {} — not saved. Save it with tabs action=download id={} path=FILE.",
                        d.id,
                        esc(&d.name),
                        d.size.map(human_size).unwrap_or_else(|| "size unknown".into()),
                        d.content_type,
                        short_url(&d.url),
                        d.id
                    );
                    self.downloads.push(d);
                    self.tabs[ti].events.push(msg.clone());
                    return Ok(msg);
                }
                Loaded::Page(p) => {
                    let mut page = self.engine.build_page(p, vp);
                    // Page-requested redirects (script, meta refresh).
                    let s = wait::settle(&self.engine, &mut page, Some(&self.tabs[ti].netlog), self.cfg.settle_cap);
                    self.collect_events(ti, &s);
                    if let Some(nav) = s.navigation {
                        if hops < 6 {
                            report.push_str(&format!("(redirected by the page to {})\n", short_url(&nav.request.url.to_string())));
                            self.drain_console(ti, &mut page);
                            req = nav.request;
                            continue;
                        }
                    }
                    if let Some(p) = s.pending {
                        report.push_str(&format!("Note: page not fully settled after {:.1} s: waiting for {p}\n", s.elapsed.as_secs_f32()));
                    }
                    break page;
                }
            }
        };
        let mut page = page;
        self.drain_console(ti, &mut page);
        let tab = &mut self.tabs[ti];
        tab.status = page.status;
        tab.frames = vec![Frame { page, parent: None }];
        tab.refs.new_document();
        tab.invalid.clear();
        tab.pending = None;
        tab.load_net_id = tab.netlog.last_id();
        if push_history {
            tab.history.truncate(tab.hist_pos + if tab.history.is_empty() { 0 } else { 1 });
            tab.history.push(req.clone());
            tab.hist_pos = tab.history.len() - 1;
        }
        self.load_frames(ti);
        self.contexts[self.tabs[ti].ctx].save_cookies();
        Ok(report)
    }

    /// Load the iframes of a tab (same- and cross-origin), depth ≤ 3.
    fn load_frames(&mut self, ti: usize) {
        let mut i = 0;
        while i < self.tabs[ti].frames.len() && self.tabs[ti].frames.len() < 12 {
            let depth = {
                let mut d = 0;
                let mut cur = self.tabs[ti].frames[i].parent;
                while let Some((p, _)) = cur {
                    d += 1;
                    cur = self.tabs[ti].frames[p].parent;
                }
                d
            };
            if depth >= 3 {
                i += 1;
                continue;
            }
            let (iframes, base, geo) = {
                let page = &self.tabs[ti].frames[i].page;
                let doc = &page.doc;
                let list: Vec<(NodeId, Option<String>, Option<String>)> = doc
                    .descendants(Document::ROOT)
                    .filter(|&n| doc.is_element(n, "iframe") && crate::ax::displayed(page, n))
                    .map(|n| (n, doc.attr(n, "src").map(|s| s.to_string()), doc.attr(n, "srcdoc").map(|s| s.to_string())))
                    .collect();
                (list, page.url.clone(), GeoMap::build(page))
            };
            for (node, src, srcdoc) in iframes {
                if self.tabs[ti].frames.len() >= 12 {
                    break;
                }
                let size = geo.get(node).map(|g| g.raw).filter(|r| r.w > 10.0 && r.h > 10.0);
                let Some(size) = size else { continue };
                let vp = Viewport { width: size.w, height: size.h };
                let page = if let Some(html) = srcdoc {
                    self.engine.load_html(&html, Url::parse("about:srcdoc").unwrap_or(base.clone()), vp)
                } else if let Some(src) = src {
                    let Some(url) = base.join(src.trim()) else { continue };
                    if !url.is_http() && url.scheme != "data" || !self.allowed(&url) {
                        continue;
                    }
                    let req = Request::get(url).with_referer(base.clone());
                    match self.engine.loader.load_request(&req, vp) {
                        Ok(Loaded::Page(p)) => self.engine.build_page(p, vp),
                        _ => continue,
                    }
                } else {
                    continue;
                };
                let mut page = page;
                let _ = wait::settle(&self.engine, &mut page, Some(&self.tabs[ti].netlog), Duration::from_millis(1500));
                self.drain_console(ti, &mut page);
                self.tabs[ti].frames.push(Frame { page, parent: Some((i, node)) });
            }
            i += 1;
        }
    }

    fn collect_events(&mut self, ti: usize, s: &Settled) {
        self.collect_side_effects(ti, &s.dialogs, &s.new_windows);
    }

    fn collect_side_effects(&mut self, ti: usize, dialogs: &[rabbithole_engine::Dialog], windows: &[Url]) {
        let policy = self.tabs[ti].dialog_policy.clone();
        for d in dialogs {
            let answer = match d.kind {
                "alert" => "dismissed".to_string(),
                "confirm" => {
                    if policy.confirm {
                        "accepted (returned true)".into()
                    } else {
                        "dismissed (returned false; repeat the action with dialog=accept to accept)".into()
                    }
                }
                _ => match &policy.prompt {
                    Some(t) if t.is_empty() => "accepted with the default value".into(),
                    Some(t) => format!("answered \"{}\"", esc(t)),
                    None => "cancelled (returned null; repeat with dialog=accept or dialog=\"text\")".into(),
                },
            };
            self.tabs[ti].events.push(format!("Dialog {}(\"{}\") → {answer}", d.kind, esc(&clip_text(&d.message, 200))));
        }
        for u in windows {
            let opener = self.tabs[ti].id;
            let ctx = self.tabs[ti].ctx;
            let ni = self.new_tab(ctx);
            self.tabs[ni].pending = Some(u.clone());
            self.tabs[ni].opener = Some(opener);
            let id = self.tabs[ni].id;
            self.tabs[ti].events.push(format!("New tab t{id} opened: {} (switch with tabs action=switch id={id})", short_url(&u.to_string())));
        }
    }

    fn drain_console(&mut self, ti: usize, page: &mut Page) {
        let tab = &mut self.tabs[ti];
        for (l, m) in page.take_console() {
            if tab.console.len() >= 500 {
                tab.console.pop_front();
            }
            tab.console.push_back((l, m));
        }
    }

    fn drain_all_consoles(&mut self, ti: usize) {
        let mut lines = Vec::new();
        for f in &mut self.tabs[ti].frames {
            lines.extend(f.page.take_console());
        }
        let tab = &mut self.tabs[ti];
        for l in lines {
            if tab.console.len() >= 500 {
                tab.console.pop_front();
            }
            tab.console.push_back(l);
        }
        let errors = tab.console.iter().filter(|(l, _)| l == "error").count();
        if errors > tab.console_reported_errors {
            let n = errors - tab.console_reported_errors;
            let last = tab.console.iter().rev().find(|(l, _)| l == "error").map(|(_, m)| clip_text(m, 120)).unwrap_or_default();
            tab.events.push(format!("{n} new JS error(s), last: \"{}\" (debug console=true)", esc(&last)));
            tab.console_reported_errors = errors;
        }
    }

    /// Ensure the active tab's pending popup is loaded.
    pub fn ensure_loaded(&mut self, ti: usize) -> Result<(), String> {
        if let Some(u) = self.tabs[ti].pending.take() {
            self.navigate(ti, Request::get(u), true)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------ the tree

    fn frame_views(frames: &[Frame]) -> Vec<FrameView<'_>> {
        frames
            .iter()
            .enumerate()
            .map(|(i, f)| FrameView {
                page: &f.page,
                children: frames.iter().enumerate().filter_map(|(j, g)| g.parent.filter(|(p, _)| *p == i).map(|(_, n)| (n, j))).collect(),
            })
            .collect()
    }

    /// Build the semantic tree of a tab.
    pub fn tree(&mut self, ti: usize, include_hidden: bool) -> (Ax, BuildStats, Vec<Overlay>) {
        let tab = &mut self.tabs[ti];
        let page = &tab.frames[0].page;
        let geo = ax::geometry(page);
        let overlays = overlay::detect(page, &geo);
        let main = if page.doc.descendants(Document::ROOT).any(|n| page.doc.is_element(n, "main") || page.doc.attr(n, "role") == Some("main")) {
            None
        } else {
            crate::read::main_content(page).filter(|&m| Some(m) != page.doc.body())
        };
        let opts = BuildOptions { include_hidden, invalid: tab.invalid.clone(), overlays: overlays.iter().map(|o| o.node).collect(), main };
        let views = Self::frame_views(&tab.frames);
        let (root, stats) = ax::build(&views, &mut tab.refs, &opts);
        (root, stats, overlays)
    }

    /// Flat items of the current tree (for diffs).
    pub fn items(&mut self, ti: usize) -> Vec<((usize, NodeId, u16, bool), String)> {
        let (root, _, _) = self.tree(ti, false);
        snapshot::flat_items(&root)
    }

    /// Header line of a page.
    pub fn page_line(&self, ti: usize) -> String {
        let tab = &self.tabs[ti];
        let Some(page) = tab.page() else { return "[page] (none)".into() };
        let max_scroll = (page.content_height() - page.viewport.height).max(0.0);
        let mut s = format!("[page] \"{}\"  {}  status={}", esc(&clip_text(page.title.trim(), 120)), esc(&page.url.to_string()), page.status);
        if max_scroll > 0.0 {
            s.push_str(&format!("  scroll={}/{}", page.page_scroll.1.round(), max_scroll.round()));
        }
        if tab.frames.len() > 1 {
            s.push_str(&format!("  frames={}", tab.frames.len() - 1));
        }
        s
    }

    /// Render a snapshot of the active tab.
    pub fn snapshot_text(&mut self, ti: usize, max_tokens: usize, focus: Option<&str>, include_hidden: bool, urls: bool) -> Result<String, String> {
        let (root, stats, overlays) = self.tree(ti, include_hidden);
        let page = &self.tabs[ti].frames[0].page;
        let viewport = rabbithole_engine::gfx::Rect::new(page.page_scroll.0, page.page_scroll.1, page.viewport.width, page.viewport.height);
        let notes: HashMap<NodeId, String> = overlays.iter().map(|o| (o.node, o.note())).collect();
        let header = self.page_line(ti);
        let focus_node = match focus {
            Some(r) => Some(resolve_in(&self.tabs[ti].refs, r, &self.tabs[ti].frames)?),
            None => None,
        };
        let tab = &mut self.tabs[ti];
        let mut ctx = RenderCtx { refs: &mut tab.refs, viewport, overlay_notes: &notes };
        let (lines, cut) = match (focus, focus_node) {
            (Some(r), Some((frame, node))) => {
                let sub = find_ax(&root, frame, node).ok_or_else(|| format!("{r} is not part of the visible page (hidden or removed)"))?;
                snapshot::render_budget(sub, &mut ctx, max_tokens, urls, true)
            }
            _ => snapshot::render_budget(&root, &mut ctx, max_tokens, urls, false),
        };
        let mut body = header;
        body.push('\n');
        body.push_str(&lines.join("\n"));
        let mut out = fence(&body);
        let mut notes = Vec::new();
        if lines.is_empty() && focus.is_none() {
            notes.push("The page shows no content: it may be rendered by scripts that failed or still load (debug console=true shows errors; read or network may still help).".to_string());
        }
        if cut {
            notes.push(format!("Snapshot cut at max_tokens={max_tokens}: expand a section with snapshot ref=@eN, or use read for the main text."));
        }
        if !include_hidden && (stats.hidden_text_chars > 40 || stats.hidden_items > 0) {
            notes.push(format!(
                "Hidden content not shown: {} chars of text, {} controls (display:none, invisible, off-screen, same color as background…). Use hidden=true to list it marked.",
                stats.hidden_text_chars, stats.hidden_items
            ));
        }
        if let Some(o) = overlays.iter().find(|o| o.covers >= 0.3) {
            let r = self.tabs[ti].refs.lookup(0, o.node).map(|r| format!("@e{r}")).unwrap_or_default();
            notes.push(format!("An overlay {r} covers the page: deal with it first{}.", if o.cookie { " (click ref=\"cookies\" rejects cookies)" } else { "" }));
        }
        notes.extend(self.page_notes(ti));
        if !notes.is_empty() {
            out.push('\n');
            out.push_str(&notes.join("\n"));
        }
        Ok(out)
    }

    /// Captcha / login / error notes about the current page.
    fn page_notes(&mut self, ti: usize) -> Vec<String> {
        let mut notes = Vec::new();
        let tab = &mut self.tabs[ti];
        let Some(f) = tab.frames.first() else { return notes };
        let page = &f.page;
        let text = page.doc.body().map(|b| page.doc.text_content(b)).unwrap_or_default();
        let lower = text.to_lowercase();
        let wall = ["captcha", "are you a robot", "are you human", "verify you are human", "unusual traffic", "checking your browser", "just a moment", "access denied", "bot detection", "please enable javascript and cookies", "cf-challenge", "attention required", "made by a human", "confirm you are human", "complete the following challenge", "bots use"];
        let looks_wall = wall.iter().any(|w| lower.contains(w)) || page.doc.descendants(Document::ROOT).any(|n| {
            page.doc.attr(n, "class").map(|c| c.contains("g-recaptcha") || c.contains("h-captcha") || c.contains("cf-turnstile")).unwrap_or(false)
                || page.doc.attr(n, "src").map(|s| s.contains("recaptcha") || s.contains("hcaptcha") || s.contains("challenges.cloudflare")).unwrap_or(false)
        });
        if page.status >= 400 {
            let mut m = format!("HTTP status {}", page.status);
            if looks_wall {
                m.push_str(": the page looks like a captcha/bot-check wall; an automated agent cannot pass it");
            }
            notes.push(m + ".");
        } else if looks_wall && text.len() < 3000 {
            notes.push("The page looks like a captcha/bot-check wall.".into());
        }
        // Login walls.
        let pw = page.doc.descendants(Document::ROOT).find(|&n| page.doc.is_element(n, "input") && page.doc.attr(n, "type").map(|t| t.eq_ignore_ascii_case("password")).unwrap_or(false) && crate::ax::displayed(page, n));
        if let Some(pw) = pw {
            if text.len() < 2500 || page.status == 401 {
                let form = page.form_of(pw);
                let r = match form {
                    Some(f) => format!("@f{}", tab.refs.assign_form(0, f)),
                    None => format!("@e{}", tab.refs.assign(0, pw, "textbox", "password")),
                };
                notes.push(format!("Page requires login (form detected at {r}; fill ref={r} to see its fields)."));
            }
        }
        notes
    }

    // --------------------------------------------------------------- refs

    /// Resolve a ref of the active tab, with helpful errors.
    pub fn resolve(&mut self, ti: usize, r: &str) -> Result<(usize, NodeId), String> {
        let tab = &self.tabs[ti];
        match resolve_in(&tab.refs, r, &tab.frames) {
            Ok(x) => Ok(x),
            Err(e) => {
                // Suggest a similar element when it was re-rendered.
                if let Some(Ref::Element(n)) = Ref::parse(r) {
                    if let Some(entry) = tab.refs.get(n).cloned() {
                        if entry.generation == tab.refs.generation {
                            let (root, _, _) = self.tree(ti, false);
                            let mut best: Option<(u32, String, String, u8)> = None;
                            root.visit(&mut |a, _| {
                                if let Some(rr) = a.reference {
                                    if rr == n {
                                        return;
                                    }
                                    let score = if a.role == entry.role && a.name == entry.name {
                                        3
                                    } else if a.name == entry.name && !a.name.is_empty() {
                                        2
                                    } else if a.role == entry.role && !entry.name.is_empty() && a.name.to_lowercase().contains(&entry.name.to_lowercase()) {
                                        1
                                    } else {
                                        0
                                    };
                                    if score > 0 && best.as_ref().map(|b| score > b.3).unwrap_or(true) {
                                        best = Some((rr, a.role.clone(), a.name.clone(), score));
                                    }
                                }
                            });
                            if let Some((rr, role, name, _)) = best {
                                return Err(format!("{e} Similar candidate: @e{rr} {role} \"{}\"", esc(&name)));
                            }
                        }
                    }
                }
                Err(e)
            }
        }
    }

    // ------------------------------------------------------------ actions

    /// Run the follow-up of an action: navigation, popups, dialogs,
    /// settle, then report the diff (or the new page).
    pub fn finish_action(&mut self, ti: usize, frame: usize, before: Vec<((usize, NodeId, u16, bool), String)>, mut outcome: ScriptOutcome, request: Option<Request>, what: &str, target: Option<(usize, NodeId)>) -> Result<String, String> {
        let mut lines = Vec::new();
        let dialogs = std::mem::take(&mut outcome.dialogs);
        let windows = std::mem::take(&mut outcome.new_windows);
        self.collect_side_effects(ti, &dialogs, &windows);
        let mut request = request.or(outcome.navigation.take().map(|n| n.request));
        let mut settled = None;
        if request.is_none() {
            let s = {
                let tab = &mut self.tabs[ti];
                let log = tab.netlog.clone();
                wait::settle(&self.engine, &mut tab.frames[frame].page, Some(&log), self.cfg.settle_cap)
            };
            self.collect_events(ti, &s);
            request = s.navigation.as_ref().map(|n| n.request.clone());
            settled = Some(s);
        }
        if let Some(req) = request {
            if frame == 0 {
                let from = self.tabs[ti].frames[0].page.url.clone();
                let report = self.navigate(ti, req, true)?;
                if report.starts_with("Download #") {
                    lines.push(format!("{what}: {report}"));
                    return Ok(lines.join("\n"));
                }
                let page = &self.tabs[ti].frames[0].page;
                let same_doc = page.url.without_fragment() == from.without_fragment();
                lines.push(format!("{what} → navigated to {} (status {}){}", esc(&page.url.to_string()), page.status, if same_doc { " (same URL reloaded)" } else { "" }));
                if !report.trim().is_empty() {
                    lines.push(report.trim().to_string());
                }
                let snap = self.snapshot_text(ti, 1500, None, false, false)?;
                lines.push(snap);
                return Ok(lines.join("\n"));
            }
            // Navigation inside an iframe: reload that frame.
            let vp = self.tabs[ti].frames[frame].page.viewport;
            self.prepare(ti);
            match self.engine.loader.load_request(&req, vp) {
                Ok(Loaded::Page(p)) => {
                    let mut page = self.engine.build_page(p, vp);
                    let _ = wait::settle(&self.engine, &mut page, Some(&self.tabs[ti].netlog), self.cfg.settle_cap);
                    lines.push(format!("{what} → frame navigated to {}", esc(&page.url.to_string())));
                    self.tabs[ti].frames[frame].page = page;
                    self.tabs[ti].refs.drop_frame(frame);
                }
                Ok(Loaded::Download(s)) => lines.push(format!("{what} → frame download {} (not saved)", esc(&s.suggested_filename()))),
                Err(e) => lines.push(format!("{what} → frame navigation failed: {e}")),
            }
        }
        self.drain_all_consoles(ti);
        let after = self.items(ti);
        let d = snapshot::diff(&before, &after, 40);
        let mut head = what.to_string();
        if let Some(s) = &settled {
            if let Some(p) = &s.pending {
                head.push_str(&format!(" (not settled after {:.1} s: waiting for {p})", s.elapsed.as_secs_f32()));
            }
        }
        lines.push(head);
        if d.is_empty() {
            // Silent failure detection.
            let focus_changed = false;
            let mut why = vec!["no DOM change".to_string(), "no navigation".to_string()];
            if let Some((f, n)) = target {
                let page = &self.tabs[ti].frames[f].page;
                let listens = page.doc.ancestor_or_self(n, |a| {
                    page.script.host.as_ref().map(|h| h.element_has_any_listener(a)).unwrap_or(false) || page.doc.attr(a, "onclick").is_some()
                });
                if listens.is_none() {
                    why.push("no event listeners on the target".into());
                }
            }
            let has_events = !self.tabs[ti].events.is_empty();
            if !focus_changed && !has_events {
                lines.push(format!("⚠ The action had no visible effect: {}.", why.join(", ")));
            } else {
                lines.push("No change in the page content.".into());
            }
        } else if d.changed + d.added + d.removed > 60 || (d.total_after > 0 && (d.added + d.removed) * 2 > d.total_after + 20) {
            lines.push(format!("Page changed substantially ({} added, {} removed, {} changed). New snapshot:", d.added, d.removed, d.changed));
            lines.push(self.snapshot_text(ti, 1500, None, false, false)?);
        } else {
            lines.push(fence(&d.lines.join("\n")));
        }
        // API hints for content that just appeared.
        let hints = crate::netinfo::hints(&self.tabs[ti], &d.lines);
        lines.extend(hints);
        Ok(lines.join("\n"))
    }

    /// Click (or hover) an element like a user.
    pub fn click(&mut self, ti: usize, r: &str, hover_only: bool, force: bool) -> Result<String, String> {
        if r.trim().eq_ignore_ascii_case("cookies") || r.trim().eq_ignore_ascii_case("@cookies") {
            return self.reject_cookies(ti);
        }
        let (frame, node) = self.resolve(ti, r)?;
        let rr = r.trim().trim_start_matches('@').to_string();
        let label = self.describe_ref(ti, frame, node);
        let before = self.items(ti);
        self.prepare(ti);
        let tab = &mut self.tabs[ti];
        let page = &mut tab.frames[frame].page;
        // Disabled controls.
        if crate::ax::states_of(page, node, "", false).iter().any(|s| s == "disabled") && !hover_only {
            return Err(format!("@{rr} {label} is disabled."));
        }
        // Scroll into view (main frame) and find a point that hits it.
        let geo = GeoMap::build(page);
        if frame == 0 {
            if let Some(g) = geo.get(node) {
                if !g.fixed {
                    let vh = page.viewport.height;
                    let visible = g.rect.y >= page.page_scroll.1 && g.rect.bottom() <= page.page_scroll.1 + vh;
                    if !visible {
                        let max = (page.content_height() - vh).max(0.0);
                        let y = (g.rect.y + g.rect.h / 2.0 - vh / 2.0).clamp(0.0, max);
                        self.engine.set_page_scroll(page, page.page_scroll.0, y);
                        let o = self.engine.dispatch_window_event(page, "scroll");
                        drop(o);
                    }
                }
            }
        }
        let geo = GeoMap::build(page);
        let reach = if frame == 0 { overlay::reach(page, &geo, node) } else { Reach::Ancestor(0.0, 0.0) };
        let outcome;
        let mut request = None;
        let verb = if hover_only { "Hovered" } else { "Clicked" };
        match reach {
            Reach::Covered(c) if !force => {
                let cr = tab.refs.assign(frame, c, "overlay", "");
                let name = cover_name(page, c);
                return Err(format!("@{rr} {label} is not clickable: covered by @e{cr} ({name}). Close or dismiss it first{}, or pass force=true.", if overlay::is_cookie_text(&page.doc.text_content(c)) { " (click ref=\"cookies\" rejects cookies)" } else { "" }));
            }
            Reach::Invisible if !force => {
                return Err(format!("@{rr} {label} is not visible (no box, zero size or hidden). Pass force=true to dispatch the event anyway."));
            }
            Reach::Point(x, y) if !hover_only => {
                let up = self.engine.click_at(page, x, y, Modifiers::default());
                let mut o = up.outcome;
                request = crate::actions::click_request(&self.engine, page, up.action, &mut o);
                outcome = o;
            }
            Reach::Point(x, y) | Reach::Ancestor(x, y) if hover_only => {
                outcome = self.engine.pointer_move(page, x, y, Modifiers::default());
            }
            _ if hover_only => {
                let o = self.engine.dispatch_event(page, node, "mouseover", rabbithole_engine::scripting::mouse_init(0.0, 0.0, 0, false, false, false, false));
                outcome = o;
            }
            _ => {
                let (action, o) = self.engine.click_element(page, node, None, Modifiers::default());
                let mut o = o;
                request = crate::actions::click_request(&self.engine, page, action, &mut o);
                outcome = o;
            }
        }
        self.finish_action(ti, frame, before, outcome, request, &format!("{verb} @{rr} {label}"), Some((frame, node)))
    }

    /// The cookie-banner shortcut: always chooses REJECT.
    fn reject_cookies(&mut self, ti: usize) -> Result<String, String> {
        let (root, _, overlays) = self.tree(ti, false);
        let banner = overlays.iter().find(|o| o.cookie).map(|o| o.node);
        // Candidate buttons: inside the banner if found, else any control
        // in a cookie-ish container.
        const REJECT: &[&str] = &[
            "reject all", "reject", "decline", "deny", "refuse", "only necessary", "necessary only", "essential only", "only essential", "rechazar", "rechazar todo",
            "no acepto", "denegar", "solo necesarias", "ablehnen", "alle ablehnen", "nur notwendige", "refuser", "tout refuser", "continuer sans accepter", "rifiuta",
            "rejeitar", "weiger", "do not accept", "continue without accepting", "no, thanks", "no thanks",
        ];
        let page = &self.tabs[ti].frames[0].page;
        let doc = &page.doc;
        let mut found: Option<(u32, String)> = None;
        root.visit(&mut |a, _| {
            if found.is_some() || a.kind != Kind::Control || a.frame != 0 || !matches!(a.role.as_str(), "button" | "link" | "clickable") {
                return;
            }
            let inside = match banner {
                Some(b) => b == a.node || doc.is_ancestor(b, a.node),
                None => doc.ancestor_or_self(a.node, |x| overlay::is_cookie_text(&format!("{} {}", doc.attr(x, "id").unwrap_or(""), doc.attr(x, "class").unwrap_or("")))).is_some(),
            };
            let name = a.name.to_lowercase();
            if inside && REJECT.iter().any(|w| name.contains(w)) && !name.contains("accept all") {
                if let Some(r) = a.reference {
                    found = Some((r, a.name.clone()));
                }
            }
        });
        // Cookie banners inside iframes (consent frameworks).
        if found.is_none() {
            root.visit(&mut |a, _| {
                if found.is_some() || a.kind != Kind::Control || a.frame == 0 {
                    return;
                }
                let name = a.name.to_lowercase();
                if REJECT.iter().any(|w| name.contains(w)) {
                    if let Some(r) = a.reference {
                        found = Some((r, a.name.clone()));
                    }
                }
            });
        }
        match found {
            Some((r, _)) => self.click(ti, &format!("@e{r}"), false, false),
            None => match banner {
                Some(b) => {
                    let r = self.tabs[ti].refs.assign(0, b, "dialog", "");
                    Err(format!("Cookie banner @e{r} has no reject/decline option; choose manually (this shortcut never accepts)."))
                }
                None => Err("No cookie banner found on this page.".into()),
            },
        }
    }

    pub fn describe_ref(&mut self, ti: usize, frame: usize, node: NodeId) -> String {
        let page = &self.tabs[ti].frames[frame].page;
        let role = self.tabs[ti].refs.lookup(frame, node).and_then(|r| self.tabs[ti].refs.get(r)).map(|e| (e.role.clone(), e.name.clone()));
        match role {
            Some((role, name)) if !name.is_empty() => format!("{role} \"{}\"", esc(&clip_text(&name, 60))),
            Some((role, _)) => role,
            None => format!("<{}>", page.doc.tag(node).unwrap_or("?")),
        }
    }

    /// Record a mutating call.
    pub fn log_action(&mut self, tool: &str, args: &Value) {
        let url = self.active.and_then(|ti| self.tabs[ti].page().map(|p| p.url.to_string())).unwrap_or_default();
        self.action_log.push((tool.to_string(), args.clone(), url));
    }

    /// Flush pending events of a tab as lines.
    pub fn take_events(&mut self, ti: usize) -> Vec<String> {
        std::mem::take(&mut self.tabs[ti].events)
    }

    /// Load system fonts once (screenshots).
    pub fn ensure_fonts(&mut self) {
        if !self.fonts_loaded {
            self.engine.fonts.load_system_fonts();
            self.fonts_loaded = true;
        }
    }

    /// Save a session (cookies, localStorage, open tabs) to a directory.
    pub fn save_session(&mut self, dir: &Path) -> Result<String, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let ci = self.active.map(|t| self.tabs[t].ctx).unwrap_or(0);
        self.activate_ctx(ci);
        if let Ok(mut ls) = self.contexts[ci].storage.lock() {
            ls.save();
        }
        let jar = self.contexts[ci].cookies.lock().map_err(|_| "cookie jar poisoned")?.to_netscape_with_session();
        std::fs::write(dir.join("cookies.txt"), jar).map_err(|e| e.to_string())?;
        let ls = self.contexts[ci].dir.join("localstorage.txt");
        let _ = std::fs::copy(&ls, dir.join("localstorage.txt"));
        let urls: Vec<String> = self.tabs.iter().filter(|t| t.ctx == ci).filter_map(|t| t.page().map(|p| p.url.to_string())).collect();
        std::fs::write(dir.join("tabs.txt"), urls.join("\n")).map_err(|e| e.to_string())?;
        Ok(format!("Session of context \"{}\" saved to {} ({} cookies, {} tabs).", self.contexts[ci].name, dir.display(), self.contexts[ci].cookies.lock().map(|j| j.len()).unwrap_or(0), urls.len()))
    }

    /// Restore a saved session into a (new or named) context.
    pub fn load_session(&mut self, dir: &Path, ctx_name: &str) -> Result<(usize, Vec<String>), String> {
        let jar = CookieJar::load(&dir.join("cookies.txt")).map_err(|e| format!("cannot read {}: {e}", dir.join("cookies.txt").display()))?;
        let ci = self.context_index(ctx_name);
        *self.contexts[ci].cookies.lock().map_err(|_| "poisoned")? = jar;
        let target = self.contexts[ci].dir.join("localstorage.txt");
        let _ = std::fs::copy(dir.join("localstorage.txt"), &target);
        *self.contexts[ci].storage.lock().map_err(|_| "poisoned")? = LocalStorage::load(target);
        let urls = std::fs::read_to_string(dir.join("tabs.txt")).unwrap_or_default().lines().map(|s| s.to_string()).filter(|s| !s.is_empty()).collect();
        Ok((ci, urls))
    }

    pub fn back_forward(&mut self, ti: usize, delta: i32) -> Result<String, String> {
        let tab = &self.tabs[ti];
        let pos = tab.hist_pos as i32 + delta;
        if pos < 0 || pos as usize >= tab.history.len() {
            return Err(if delta < 0 { "No previous page in history.".into() } else { "No next page in history.".into() });
        }
        let req = tab.history[pos as usize].clone();
        self.tabs[ti].hist_pos = pos as usize;
        self.navigate(ti, req, false)
    }

    pub fn reload(&mut self, ti: usize) -> Result<String, String> {
        let tab = &self.tabs[ti];
        let req = tab.history.get(tab.hist_pos).cloned().or_else(|| tab.page().map(|p| Request::get(p.url.clone()))).ok_or("nothing to reload")?;
        self.navigate(ti, req, false)
    }

    /// Download offered earlier, saved explicitly.
    pub fn save_download(&mut self, id: u32, path: &Path) -> Result<String, String> {
        let d = self.downloads.iter().find(|d| d.id == id).cloned().ok_or_else(|| format!("no download #{id}"))?;
        let resp = self.engine.loader.fetcher.download(&d.request, path, |_, _| true).map_err(|e| e.to_string())?;
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        Ok(format!("Saved download #{id} \"{}\" ({}, {}) to {}", esc(&d.name), human_size(size), resp.content_type, path.display()))
    }

    /// Approximate resident memory (Linux), for reports.
    pub fn rss_kb() -> Option<u64> {
        let s = std::fs::read_to_string("/proc/self/statm").ok()?;
        let pages: u64 = s.split_whitespace().nth(1)?.parse().ok()?;
        Some(pages * 4)
    }
}

/// Name of a covering element for messages.
fn cover_name(page: &Page, c: NodeId) -> String {
    let doc = &page.doc;
    let role = match doc.attr(c, "role") {
        Some(r) => r.to_string(),
        None if doc.is_element(c, "dialog") => "dialog".into(),
        None => doc.tag(c).unwrap_or("element").to_string(),
    };
    let name = ax::accessible_name(page, c, "dialog");
    let name = if name.is_empty() {
        doc.descendants(c)
            .find(|&d| matches!(doc.tag(d), Some("h1" | "h2" | "h3" | "h4" | "p" | "strong")))
            .map(|d| ax::content_text(page, d))
            .unwrap_or_else(|| ax::content_text(page, c))
    } else {
        name
    };
    format!("{role} '{}'", esc(&clip_text(&name, 60)))
}

/// Resolve a ref string against a tab's refs and frames.
fn resolve_in(refs: &RefMap, r: &str, frames: &[Frame]) -> Result<(usize, NodeId), String> {
    let parsed = Ref::parse(r).ok_or_else(|| format!("\"{r}\" is not a ref (expected @e12 or @f1)."))?;
    let (frame, node, generation, desc) = match parsed {
        Ref::Element(n) => {
            let e = refs.get(n).ok_or_else(|| format!("@e{n} is unknown: take a snapshot to get current refs."))?;
            (e.frame, e.node, e.generation, format!("{} \"{}\"", e.role, esc(&clip_text(&e.name, 50))))
        }
        Ref::Form(n) => {
            let (f, node, g) = refs.get_form(n).ok_or_else(|| format!("@f{n} is unknown: list forms with fill (no values)."))?;
            (f, node, g, "form".to_string())
        }
    };
    if generation != refs.generation {
        return Err(format!("{parsed} ({desc}) belongs to a previous page; the tab has navigated since. Take a new snapshot."));
    }
    let Some(f) = frames.get(frame) else { return Err(format!("{parsed} belongs to a frame that is gone.")) };
    if !f.page.doc.is_connected(node) {
        return Err(format!("{parsed} ({desc}) no longer exists (page re-rendered)."));
    }
    Ok((frame, node))
}

/// Find the Ax node for (frame, node).
pub fn find_ax(a: &Ax, frame: usize, node: NodeId) -> Option<&Ax> {
    if a.frame == frame && a.node == node && a.kind != Kind::Break {
        return Some(a);
    }
    a.children.iter().find_map(|c| find_ax(c, frame, node))
}

pub fn human_size(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / 1048576.0)
    }
}
