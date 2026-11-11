//! Images after the first paint: responsive selection (`srcset`, `sizes`,
//! `<picture>`/`<source>` by media and type, device pixel ratio), lazy
//! loading (`loading="lazy"`, driven by scroll), script-driven changes
//! (`img.src = ...`, lazy-loader `data-src` swaps, `new Image()`),
//! background fetch + decode with `load`/`error` events, and animated
//! image playback on the engine tick.

use crate::{Engine, Page, Viewport};
use rabbithole_css::{parse_media_query, MediaAlternative, MediaEnv, MediaFeature, MediaQuery};
use rabbithole_dom::{Document, NodeId};
use rabbithole_gfx::{Image, Rect};
use rabbithole_layout::LayoutBox;
use rabbithole_net::Url;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Lazy images start loading this far (CSS px) outside the viewport.
const LAZY_MARGIN: f32 = 1250.0;

// ============================================================ selection

/// One `srcset` candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct SrcCandidate {
    pub url: String,
    /// `Nw` descriptor.
    pub width: Option<f32>,
    /// `Nx` descriptor.
    pub density: Option<f32>,
}

/// Parse a `srcset` attribute (HTML "parse a srcset attribute": URLs may
/// contain commas; descriptors end at a comma outside parentheses).
pub fn parse_srcset(input: &str) -> Vec<SrcCandidate> {
    let s = input.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < s.len() {
        while i < s.len() && (s[i].is_ascii_whitespace() || s[i] == b',') {
            i += 1;
        }
        if i >= s.len() {
            break;
        }
        let start = i;
        while i < s.len() && !s[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut url = &input[start..i];
        let mut descriptors = String::new();
        if url.ends_with(',') {
            url = url.trim_end_matches(',');
        } else {
            let mut depth = 0;
            while i < s.len() {
                let c = s[i];
                if c == b'(' {
                    depth += 1;
                } else if c == b')' {
                    depth -= 1;
                } else if c == b',' && depth <= 0 {
                    i += 1;
                    break;
                }
                descriptors.push(c as char);
                i += 1;
            }
        }
        if url.is_empty() {
            continue;
        }
        let mut cand = SrcCandidate { url: url.to_string(), width: None, density: None };
        let mut bad = false;
        for d in descriptors.split_ascii_whitespace() {
            let (num, unit) = d.split_at(d.len() - 1);
            let v: Option<f32> = num.parse().ok();
            match (unit, v) {
                ("w" | "W", Some(v)) if v > 0.0 && cand.width.is_none() && cand.density.is_none() => cand.width = Some(v),
                ("x" | "X", Some(v)) if v > 0.0 && cand.width.is_none() && cand.density.is_none() => cand.density = Some(v),
                ("h" | "H", Some(_)) => {}
                _ => bad = true,
            }
        }
        if !bad {
            out.push(cand);
        }
    }
    out
}

/// Evaluate a media query list with device-pixel-ratio support
/// (`min-resolution`, `-webkit-min-device-pixel-ratio`, ...).
pub fn media_matches(query: &str, vp: Viewport, dpr: f32, scripting: bool) -> bool {
    let q = query.trim();
    if q.is_empty() {
        return true;
    }
    let mq = parse_media_query(q);
    let env = MediaEnv { width: vp.width, height: vp.height, screen: true, scripting };
    if mq.alternatives.is_empty() {
        return true;
    }
    mq.alternatives.iter().any(|alt| {
        let one = |f: &MediaFeature| -> bool {
            if let MediaFeature::Other(name, value) = f {
                if let Some(res) = resolution_value(value) {
                    match name.as_str() {
                        "min-resolution" | "-webkit-min-device-pixel-ratio" | "min-device-pixel-ratio" | "min--moz-device-pixel-ratio" => return dpr + 1e-3 >= res,
                        "max-resolution" | "-webkit-max-device-pixel-ratio" | "max-device-pixel-ratio" | "max--moz-device-pixel-ratio" => return dpr <= res + 1e-3,
                        "resolution" | "-webkit-device-pixel-ratio" | "device-pixel-ratio" => return (dpr - res).abs() < 1e-3,
                        _ => {}
                    }
                }
            }
            let single = MediaQuery { alternatives: vec![MediaAlternative { negated: false, media_type: None, features: vec![f.clone()] }] };
            single.evaluate(&env)
        };
        let type_ok = match alt.media_type.as_deref() {
            None | Some("all") | Some("screen") => true,
            _ => false,
        };
        let r = type_ok && alt.features.iter().all(one);
        r != alt.negated
    })
}

fn resolution_value(v: &str) -> Option<f32> {
    let v = v.trim().to_ascii_lowercase();
    if let Some(n) = v.strip_suffix("dppx") {
        return n.trim().parse().ok();
    }
    if let Some(n) = v.strip_suffix("dpcm") {
        return n.trim().parse::<f32>().ok().map(|d| d * 2.54 / 96.0);
    }
    if let Some(n) = v.strip_suffix("dpi") {
        return n.trim().parse::<f32>().ok().map(|d| d / 96.0);
    }
    if let Some(n) = v.strip_suffix('x') {
        return n.trim().parse().ok();
    }
    v.parse().ok()
}

/// Evaluate a CSS length as used in `sizes` (px, em, rem, vw, vh, vmin,
/// vmax, ch, and `calc()`/`min()`/`max()`/`clamp()` expressions).
pub fn eval_length(src: &str, vp: Viewport) -> Option<f32> {
    let toks = tokenize_len(src)?;
    let mut p = LenParser { toks: &toks, pos: 0, vp };
    let v = p.expr()?;
    if p.pos != toks.len() {
        return None;
    }
    Some(v)
}

#[derive(Debug, Clone, PartialEq)]
enum LTok {
    Num(f32, String),
    Op(char),
    Open,
    Func(String),
    Close,
    Comma,
}

fn tokenize_len(s: &str) -> Option<Vec<LTok>> {
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch.is_ascii_digit() || ch == '.' || ((ch == '-' || ch == '+') && i + 1 < c.len() && (c[i + 1].is_ascii_digit() || c[i + 1] == '.') && !matches!(out.last(), Some(LTok::Num(..)) | Some(LTok::Close))) {
            let st = i;
            i += 1;
            while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.') {
                i += 1;
            }
            let n: f32 = c[st..i].iter().collect::<String>().parse().ok()?;
            let us = i;
            while i < c.len() && (c[i].is_ascii_alphabetic() || c[i] == '%') {
                i += 1;
            }
            out.push(LTok::Num(n, c[us..i].iter().collect::<String>().to_ascii_lowercase()));
        } else if ch.is_ascii_alphabetic() || ch == '-' && i + 1 < c.len() && c[i + 1].is_ascii_alphabetic() {
            let st = i;
            while i < c.len() && (c[i].is_ascii_alphanumeric() || c[i] == '-') {
                i += 1;
            }
            let name: String = c[st..i].iter().collect::<String>().to_ascii_lowercase();
            if i < c.len() && c[i] == '(' {
                i += 1;
                out.push(LTok::Func(name));
            } else {
                return None;
            }
        } else {
            match ch {
                '(' => out.push(LTok::Open),
                ')' => out.push(LTok::Close),
                ',' => out.push(LTok::Comma),
                '+' | '-' | '*' | '/' => out.push(LTok::Op(ch)),
                _ => return None,
            }
            i += 1;
        }
    }
    Some(out)
}

struct LenParser<'a> {
    toks: &'a [LTok],
    pos: usize,
    vp: Viewport,
}

impl LenParser<'_> {
    fn expr(&mut self) -> Option<f32> {
        let mut v = self.term()?;
        while let Some(LTok::Op(op @ ('+' | '-'))) = self.toks.get(self.pos) {
            let op = *op;
            self.pos += 1;
            let r = self.term()?;
            v = if op == '+' { v + r } else { v - r };
        }
        Some(v)
    }
    fn term(&mut self) -> Option<f32> {
        let mut v = self.factor()?;
        while let Some(LTok::Op(op @ ('*' | '/'))) = self.toks.get(self.pos) {
            let op = *op;
            self.pos += 1;
            let r = self.factor()?;
            v = if op == '*' { v * r } else if r != 0.0 { v / r } else { return None };
        }
        Some(v)
    }
    fn factor(&mut self) -> Option<f32> {
        let t = self.toks.get(self.pos)?.clone();
        self.pos += 1;
        match t {
            LTok::Num(n, unit) => {
                let vw = self.vp.width / 100.0;
                let vh = self.vp.height / 100.0;
                Some(match unit.as_str() {
                    "" | "px" => n,
                    "em" | "rem" => n * 16.0,
                    "ch" | "ex" => n * 8.0,
                    "vw" | "svw" | "lvw" | "dvw" | "%" => n * vw,
                    "vh" | "svh" | "lvh" | "dvh" => n * vh,
                    "vmin" => n * vw.min(vh),
                    "vmax" => n * vw.max(vh),
                    "pt" => n * 4.0 / 3.0,
                    "cm" => n * 96.0 / 2.54,
                    "mm" => n * 96.0 / 25.4,
                    "in" => n * 96.0,
                    _ => return None,
                })
            }
            LTok::Open => {
                let v = self.expr()?;
                (self.toks.get(self.pos) == Some(&LTok::Close)).then(|| self.pos += 1)?;
                Some(v)
            }
            LTok::Op('-') => Some(-self.factor()?),
            LTok::Func(name) => {
                let mut args = vec![self.expr()?];
                while self.toks.get(self.pos) == Some(&LTok::Comma) {
                    self.pos += 1;
                    args.push(self.expr()?);
                }
                (self.toks.get(self.pos) == Some(&LTok::Close)).then(|| self.pos += 1)?;
                match name.as_str() {
                    "calc" | "-webkit-calc" if args.len() == 1 => Some(args[0]),
                    "min" => args.into_iter().reduce(f32::min),
                    "max" => args.into_iter().reduce(f32::max),
                    "clamp" if args.len() == 3 => Some(args[1].clamp(args[0], args[2].max(args[0]))),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

/// Evaluate a `sizes` attribute to a CSS pixel width (`auto` uses
/// `layout_width` when known). Defaults to `100vw`.
pub fn eval_sizes(sizes: &str, vp: Viewport, dpr: f32, layout_width: Option<f32>) -> f32 {
    for item in split_top_level(sizes, ',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        // The size is the last top-level component; the rest is a media
        // condition.
        let (cond, len) = match last_component(item) {
            Some(i) => (item[..i].trim(), item[i..].trim()),
            None => ("", item),
        };
        if len.eq_ignore_ascii_case("auto") {
            if cond.is_empty() || media_matches(cond, vp, dpr, true) {
                if let Some(w) = layout_width.filter(|w| *w > 0.0) {
                    return w;
                }
            }
            continue;
        }
        if !cond.is_empty() && !media_matches(cond, vp, dpr, true) {
            continue;
        }
        if let Some(v) = eval_length(len, vp) {
            if v >= 0.0 {
                return v;
            }
        }
    }
    vp.width
}

fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c == sep && depth <= 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// Byte index where the last top-level whitespace-separated component
/// starts (None when the item is a single component).
fn last_component(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut depth = 0i32;
    for i in (0..b.len()).rev() {
        match b[i] {
            b')' => depth += 1,
            b'(' => depth -= 1,
            c if c.is_ascii_whitespace() && depth == 0 => return Some(i + 1),
            _ => {}
        }
    }
    None
}

/// The resource an element should show.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageChoice {
    /// URL as written in the document (resolve against the base URL).
    pub url: String,
    /// Image pixels per CSS pixel (natural size = pixels / density).
    pub density: f32,
}

/// Pick a candidate for `dpr`: the smallest density that is at least the
/// device pixel ratio, else the densest one.
pub fn choose_candidate(cands: &[SrcCandidate], sizes_px: f32, dpr: f32) -> Option<ImageChoice> {
    let sizes_px = if sizes_px > 0.0 { sizes_px } else { 1.0 };
    let mut best: Option<ImageChoice> = None;
    let mut fallback: Option<ImageChoice> = None;
    for c in cands {
        let d = match (c.width, c.density) {
            (Some(w), _) => w / sizes_px,
            (_, Some(x)) => x,
            _ => 1.0,
        };
        let choice = ImageChoice { url: c.url.clone(), density: d };
        if d + 1e-3 >= dpr {
            if best.as_ref().map(|b| d < b.density).unwrap_or(true) {
                best = Some(choice);
            }
        } else if fallback.as_ref().map(|f| d > f.density).unwrap_or(true) {
            fallback = Some(choice);
        }
    }
    best.or(fallback)
}

fn non_empty<'a>(doc: &'a Document, n: NodeId, attr: &str) -> Option<&'a str> {
    doc.attr(n, attr).map(str::trim).filter(|s| !s.is_empty())
}

/// Is the `src` a lazy-loading placeholder (empty, or an inline data URI
/// while a `data-src`/`data-srcset` holds the real image)?
fn is_placeholder(src: Option<&str>) -> bool {
    match src {
        None => true,
        Some(s) => s.starts_with("data:") || s == "#" || s.eq_ignore_ascii_case("about:blank"),
    }
}

/// Choose what an image-bearing element (`img`, `input type=image`,
/// `video poster`) should display for the viewport and pixel ratio.
pub fn select_image(doc: &Document, n: NodeId, vp: Viewport, dpr: f32, scripting: bool, layout_width: Option<f32>) -> Option<ImageChoice> {
    let el = doc.element(n)?;
    match el.tag.as_str() {
        "video" => non_empty(doc, n, "poster").map(|u| ImageChoice { url: u.to_string(), density: 1.0 }),
        "input" => {
            if !el.attr("type").map(|t| t.eq_ignore_ascii_case("image")).unwrap_or(false) {
                return None;
            }
            non_empty(doc, n, "src").map(|u| ImageChoice { url: u.to_string(), density: 1.0 })
        }
        "img" => {
            // <picture>: the first matching <source> wins.
            if let Some(p) = doc.parent(n).filter(|&p| doc.is_element(p, "picture")) {
                for s in doc.child_elements(p) {
                    if s == n {
                        break;
                    }
                    if !doc.is_element(s, "source") {
                        continue;
                    }
                    let Some(srcset) = non_empty(doc, s, "srcset").or_else(|| non_empty(doc, s, "data-srcset")) else { continue };
                    if let Some(m) = non_empty(doc, s, "media") {
                        if !media_matches(m, vp, dpr, scripting) {
                            continue;
                        }
                    }
                    if let Some(t) = non_empty(doc, s, "type") {
                        if !rabbithole_gfx::image::supports_mime(t) {
                            continue;
                        }
                    }
                    let cands = parse_srcset(srcset);
                    if cands.is_empty() {
                        continue;
                    }
                    let sizes = non_empty(doc, s, "sizes").or_else(|| non_empty(doc, n, "sizes")).map(|z| eval_sizes(z, vp, dpr, layout_width)).unwrap_or(vp.width);
                    if let Some(c) = choose_candidate(&cands, sizes, dpr) {
                        return Some(c);
                    }
                }
            }
            let src = non_empty(doc, n, "src");
            let mut srcset = non_empty(doc, n, "srcset");
            let mut src_eff = src;
            if is_placeholder(src) {
                // Common lazy-loader conventions (used when no script swaps
                // them in, e.g. with JavaScript off or unsupported libraries).
                if srcset.is_none() {
                    srcset = non_empty(doc, n, "data-srcset").or_else(|| non_empty(doc, n, "data-lazy-srcset"));
                }
                for a in ["data-src", "data-lazy-src", "data-original", "data-lazy", "data-url"] {
                    if let Some(v) = non_empty(doc, n, a) {
                        src_eff = Some(v);
                        break;
                    }
                }
            }
            if let Some(ss) = srcset {
                let mut cands = parse_srcset(ss);
                if !cands.is_empty() {
                    // `src` is an implicit 1x candidate when no `w` is used.
                    if let Some(s) = src_eff.filter(|s| !s.starts_with("data:") || src_eff != src) {
                        if cands.iter().all(|c| c.width.is_none()) && !cands.iter().any(|c| c.density.unwrap_or(1.0) == 1.0) {
                            cands.push(SrcCandidate { url: s.to_string(), width: None, density: Some(1.0) });
                        }
                    }
                    let sizes = non_empty(doc, n, "sizes").map(|z| eval_sizes(z, vp, dpr, layout_width)).unwrap_or_else(|| layout_width.unwrap_or(vp.width));
                    if let Some(c) = choose_candidate(&cands, sizes, dpr) {
                        return Some(c);
                    }
                }
            }
            src_eff.map(|u| ImageChoice { url: u.to_string(), density: 1.0 })
        }
        "object" => object_image_url(doc, n).map(|u| ImageChoice { url: u.to_string(), density: 1.0 }),
        _ => None,
    }
}

/// The resource of an `<object>` that could be displayed as an image: its
/// `data` URL unless `type` names something that is not an image (HTML
/// documents and plugin content render the fallback content instead).
fn object_image_url<'d>(doc: &'d Document, n: NodeId) -> Option<&'d str> {
    let data = non_empty(doc, n, "data")?;
    if let Some(t) = non_empty(doc, n, "type") {
        let t = t.to_ascii_lowercase();
        if !t.starts_with("image/") {
            return None;
        }
    }
    Some(data)
}

/// Give an image the natural size implied by its `srcset` density.
pub(crate) fn apply_density(img: Arc<Image>, density: f32) -> Arc<Image> {
    if (density - 1.0).abs() < 1e-3 || density <= 0.0 || img.broken {
        return img;
    }
    let w = (img.natural_width as f32 / density).round().max(1.0) as u32;
    let h = (img.natural_height as f32 / density).round().max(1.0) as u32;
    Arc::new((*img).clone().with_natural_size(w, h))
}

/// Elements whose image is chosen by [`select_image`].
pub(crate) fn is_image_element(doc: &Document, n: NodeId) -> bool {
    match doc.tag(n) {
        Some("img") => true,
        Some("video") => doc.attr(n, "poster").is_some(),
        Some("input") => doc.attr(n, "type").map(|t| t.eq_ignore_ascii_case("image")).unwrap_or(false),
        Some("object") => object_image_url(doc, n).is_some(),
        _ => false,
    }
}

fn is_lazy(doc: &Document, n: NodeId) -> bool {
    doc.attr(n, "loading").map(|l| l.trim().eq_ignore_ascii_case("lazy")).unwrap_or(false)
}

// ============================================================ state

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    /// `loading=lazy` and not near the viewport yet.
    Deferred,
    Loading,
    Loaded,
    Failed,
}

#[derive(Debug, Clone)]
struct Req {
    /// Absolute URL (cache key).
    url: String,
    density: f32,
    status: Status,
}

/// Per-page image bookkeeping (lives in [`Page`]).
pub struct ImageState {
    reqs: HashMap<NodeId, Req>,
    in_flight: HashSet<String>,
    tx: Sender<Vec<String>>,
    rx: Receiver<Vec<String>>,
    /// Elements whose source scripts changed (including detached
    /// `new Image()` elements, which are not in the tree).
    script_nodes: Vec<NodeId>,
    /// `load` / `error` events waiting to be fired.
    events: Vec<(NodeId, &'static str)>,
    next_frame: Option<Instant>,
    /// The engine's clock origin (for `next_script_due` in engine ms).
    pub(crate) clock: Option<Instant>,
    /// Background-image URLs being fetched after the first layout.
    bg_pending: HashSet<String>,
    /// Page scroll at the last sync (scroll-driven syncs are throttled).
    last_sync_scroll: (f32, f32),
    /// Favicon URL once requested.
    favicon_url: Option<String>,
    /// Decoded favicon (`<link rel=icon>` or `/favicon.ico`).
    pub favicon: Option<Arc<Image>>,
}

impl Default for ImageState {
    fn default() -> Self {
        let (tx, rx) = channel();
        ImageState {
            reqs: HashMap::new(),
            in_flight: HashSet::new(),
            tx,
            rx,
            script_nodes: Vec::new(),
            events: Vec::new(),
            next_frame: None,
            clock: None,
            favicon_url: None,
            favicon: None,
            bg_pending: HashSet::new(),
            last_sync_scroll: (0.0, 0.0),
        }
    }
}

/// What scripts can observe about an image element.
#[derive(Debug, Clone, Default)]
pub struct ImageInfo {
    pub natural_width: u32,
    pub natural_height: u32,
    pub complete: bool,
    pub current_src: String,
}

impl ImageState {
    /// A script changed an image's source (`img.src = ...`).
    pub fn script_changed(&mut self, n: NodeId) {
        if !self.script_nodes.contains(&n) {
            self.script_nodes.push(n);
        }
    }

    /// Something to do on the next tick (fetch results, events, frames)?
    fn due(&self) -> Option<Instant> {
        if !self.events.is_empty() {
            return Some(Instant::now());
        }
        let poll = (!self.in_flight.is_empty()).then(|| Instant::now() + Duration::from_millis(25));
        match (poll, self.next_frame) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

fn collect_backgrounds(b: &LayoutBox, view: &Rect, have: &HashMap<String, Arc<Image>>, out: &mut Vec<String>) {
    if out.len() >= 64 {
        return;
    }
    if !b.style.background_layers.is_empty() && (!b.rect.intersect(view).is_empty()) {
        for l in &b.style.background_layers {
            if let rabbithole_style::BackgroundImage::Url(u) = &l.image {
                if !have.contains_key(u) && !out.contains(u) {
                    out.push(u.clone());
                }
            }
        }
    }
    for c in &b.children {
        collect_backgrounds(c, view, have, out);
    }
}

fn collect_rects(b: &LayoutBox, want: &HashSet<NodeId>, out: &mut HashMap<NodeId, Rect>) {
    if let Some(n) = b.node {
        if want.contains(&n) && !out.contains_key(&n) {
            out.insert(n, b.rect);
        }
    }
    for c in &b.children {
        collect_rects(c, want, out);
    }
}

impl Page {
    /// Script-visible state of an `<img>` (naturalWidth, complete, ...).
    pub fn image_info(&self, n: NodeId) -> ImageInfo {
        let img = self.images.get(&n).filter(|i| !i.broken);
        let req = self.image_state.reqs.get(&n);
        let has_src = self.doc.attr(n, "src").map(|s| !s.trim().is_empty()).unwrap_or(false) || self.doc.attr(n, "srcset").is_some();
        let complete = match req.map(|r| r.status) {
            Some(Status::Loaded) | Some(Status::Failed) => true,
            Some(Status::Loading) | Some(Status::Deferred) => false,
            None => img.is_some() || !has_src,
        };
        ImageInfo {
            natural_width: img.map(|i| i.natural_width).unwrap_or(0),
            natural_height: img.map(|i| i.natural_height).unwrap_or(0),
            complete,
            current_src: req.map(|r| r.url.clone()).unwrap_or_default(),
        }
    }

    /// When the image pipeline next needs a tick (engine ms).
    pub(crate) fn image_due(&self) -> Option<f64> {
        let at = self.image_state.due()?;
        let origin = self.image_state.clock?;
        Some(at.saturating_duration_since(origin).as_secs_f64() * 1000.0)
    }

    /// The page's favicon, once loaded.
    pub fn favicon(&self) -> Option<Arc<Image>> {
        self.image_state.favicon.clone()
    }

    /// Images still being fetched in the background.
    pub fn images_in_flight(&self) -> usize {
        self.image_state.in_flight.len()
    }
}

impl Engine {
    /// Set the device pixel ratio used for `srcset` / resolution queries
    /// (HiDPI scale × zoom). Takes effect on the next restyle.
    pub fn set_device_pixel_ratio(&mut self, dpr: f32) {
        let dpr = if dpr.is_finite() && dpr > 0.0 { dpr } else { 1.0 };
        self.device_pixel_ratio = dpr;
        self.loader.device_pixel_ratio = dpr;
    }

    /// Reconcile image elements with what is loaded: (re)select sources,
    /// start background fetches, install cached results and defer lazy
    /// images that are far from the viewport. Returns true when
    /// `page.images` changed (a relayout is needed).
    pub(crate) fn sync_images(&self, page: &mut Page) -> bool {
        page.image_state.clock.get_or_insert(self.start);
        page.image_state.last_sync_scroll = page.page_scroll;
        let vp = page.viewport;
        let dpr = self.device_pixel_ratio;
        let scripting = self.scripting;
        let mut nodes: Vec<NodeId> = page
            .doc
            .shadow_including_descendants(Document::ROOT)
            .into_iter()
            .filter(|&n| is_image_element(&page.doc, n) && page.styles.get(n).is_some())
            .collect();
        let attached: HashSet<NodeId> = nodes.iter().copied().collect();
        let script_nodes = std::mem::take(&mut page.image_state.script_nodes);
        for &n in &script_nodes {
            if !attached.contains(&n) && page.doc.element(n).is_some() && !page.doc.is_connected(n) {
                nodes.push(n);
            }
        }
        // Layout rects for lazy candidates and `sizes="auto"`.
        let want: HashSet<NodeId> = nodes
            .iter()
            .copied()
            .filter(|&n| scripting && is_lazy(&page.doc, n) || page.doc.attr(n, "sizes").map(|s| s.contains("auto")).unwrap_or(false))
            .collect();
        let mut rects = HashMap::new();
        if !want.is_empty() {
            collect_rects(&page.layout.root, &want, &mut rects);
        }
        let view = Rect::new(page.page_scroll.0 - LAZY_MARGIN, page.page_scroll.1 - LAZY_MARGIN, vp.width + 2.0 * LAZY_MARGIN, vp.height + 2.0 * LAZY_MARGIN);
        let mut changed = false;
        let mut to_fetch: Vec<Url> = Vec::new();
        for n in nodes {
            let layout_w = rects.get(&n).map(|r| r.w);
            let choice = select_image(&page.doc, n, vp, dpr, scripting, layout_w)
                .and_then(|c| page.base_url.join(&c.url).filter(|u| u.scheme != "javascript").map(|u| (u, c.density)));
            let Some((url, density)) = choice else {
                if page.image_state.reqs.remove(&n).is_some() || page.images.contains_key(&n) {
                    if page.images.remove(&n).is_some() {
                        changed = true;
                    }
                }
                continue;
            };
            let key = url.to_string();
            let existing = page.image_state.reqs.get(&n).cloned();
            if let Some(r) = &existing {
                if r.url == key && r.status != Status::Deferred {
                    continue;
                }
            } else if page.images.contains_key(&n) && !script_nodes.contains(&n) {
                // Loaded by the page loader before the first layout.
                page.image_state.reqs.insert(n, Req { url: key, density, status: Status::Loaded });
                continue;
            }
            if !self.loader.load_images {
                if !page.images.get(&n).map(|i| i.broken).unwrap_or(false) {
                    page.images.insert(n, Arc::new(Image::broken()));
                    changed = true;
                }
                continue;
            }
            if scripting && is_lazy(&page.doc, n) && !script_nodes.contains(&n) && page.doc.is_connected(n) {
                let near = rects.get(&n).map(|r| !r.intersect(&view).is_empty() || r.is_empty() && r.y >= view.y && r.y <= view.bottom()).unwrap_or(false);
                if !near {
                    page.image_state.reqs.insert(n, Req { url: key, density, status: Status::Deferred });
                    continue;
                }
            }
            match self.loader.cached_image(&key) {
                Some(result) => {
                    let document = self.loader.is_document_resource(&key);
                    changed |= install(page, n, Some(result), density, &key, document);
                }
                None => {
                    page.image_state.reqs.insert(n, Req { url: key.clone(), density, status: Status::Loading });
                    if !page.image_state.in_flight.contains(&key) && !to_fetch.iter().any(|u| u.to_string() == key) {
                        to_fetch.push(url);
                    }
                }
            }
        }
        // CSS background images the page loader did not fetch (beyond its
        // cap, or added later by scripts / dynamic classes): load the ones
        // on boxes near the viewport.
        if self.loader.load_images {
            let mut bg: Vec<String> = Vec::new();
            collect_backgrounds(&page.layout.root, &view, &page.background_images, &mut bg);
            for key in bg {
                if page.image_state.bg_pending.contains(&key) || to_fetch.iter().any(|x| x.to_string() == key) {
                    continue;
                }
                let Some(u) = Url::parse_absolute(&key).filter(|u| u.scheme != "javascript") else { continue };
                match self.loader.cached_image(&key) {
                    Some(Some(img)) => {
                        page.background_images.insert(key, img);
                        changed = true;
                    }
                    Some(None) => {}
                    None => {
                        page.image_state.bg_pending.insert(key);
                        if !page.image_state.in_flight.contains(&u.to_string()) {
                            to_fetch.push(u);
                        }
                    }
                }
            }
        }
        if page.image_state.favicon_url.is_none() && self.loader.load_images {
            if let Some(u) = favicon_url(&page.doc, &page.base_url, &page.url) {
                let key = u.to_string();
                page.image_state.favicon_url = Some(key.clone());
                match self.loader.cached_image(&key) {
                    Some(r) => page.image_state.favicon = r,
                    None => {
                        if !page.image_state.in_flight.contains(&key) && !to_fetch.iter().any(|x| x.to_string() == key) {
                            to_fetch.push(u);
                        }
                    }
                }
            } else {
                page.image_state.favicon_url = Some(String::new());
            }
        }
        if !to_fetch.is_empty() {
            for u in &to_fetch {
                page.image_state.in_flight.insert(u.to_string());
            }
            let mut loader = self.loader.clone();
            loader.initiator = Some(page.url.clone());
            let tx = page.image_state.tx.clone();
            std::thread::spawn(move || {
                // Small batches so the first images show up early.
                for chunk in to_fetch.chunks(6) {
                    loader.fetch_images_into_cache(chunk);
                    if tx.send(chunk.iter().map(|u| u.to_string()).collect()).is_err() {
                        break;
                    }
                }
            });
        }
        changed
    }

    /// After scrolling: start lazy images (and background images) that came
    /// near the viewport.
    pub(crate) fn sync_lazy_images(&self, page: &mut Page) {
        let (lx, ly) = page.image_state.last_sync_scroll;
        let moved = (page.page_scroll.0 - lx).abs().max((page.page_scroll.1 - ly).abs());
        // Lazy images start 1250px ahead, so checking every 250px is plenty.
        if moved < 250.0 {
            return;
        }
        if self.sync_images(page) {
            self.relayout_only(page);
        }
    }

    /// Install finished background fetches. Returns true when the page's
    /// images changed.
    pub(crate) fn poll_images(&self, page: &mut Page) -> bool {
        let mut done: HashSet<String> = HashSet::new();
        while let Ok(batch) = page.image_state.rx.try_recv() {
            for k in batch {
                page.image_state.in_flight.remove(&k);
                done.insert(k);
            }
        }
        if done.is_empty() {
            return false;
        }
        if let Some(f) = page.image_state.favicon_url.clone() {
            if done.contains(&f) {
                page.image_state.favicon = self.loader.cached_image(&f).flatten();
            }
        }
        let mut bg_changed = false;
        for k in &done {
            if page.image_state.bg_pending.remove(k) {
                if let Some(Some(img)) = self.loader.cached_image(k) {
                    page.background_images.insert(k.clone(), img);
                    bg_changed = true;
                }
            }
        }
        let ready: Vec<(NodeId, String, f32)> = page
            .image_state
            .reqs
            .iter()
            .filter(|(_, r)| r.status == Status::Loading && done.contains(&r.url))
            .map(|(n, r)| (*n, r.url.clone(), r.density))
            .collect();
        let mut changed = bg_changed;
        for (n, key, density) in ready {
            let document = self.loader.is_document_resource(&key);
            changed |= install(page, n, self.loader.cached_image(&key), density, &key, document);
        }
        changed
    }

    /// Advance animated images that are on screen. Returns true when a
    /// frame changed (repaint needed).
    pub(crate) fn advance_animations(&self, page: &mut Page) -> bool {
        use rabbithole_layout::DisplayItem as D;
        let view = Rect::new(page.page_scroll.0, page.page_scroll.1, page.viewport.width, page.viewport.height);
        let now = Instant::now();
        let mut changed = false;
        let mut next: Option<Duration> = None;
        let mut seen: HashSet<*const Image> = HashSet::new();
        let mut fixed = 0i32;
        for it in &page.display_list.items {
            let (img, rect) = match it {
                D::PushFixed => {
                    fixed += 1;
                    continue;
                }
                D::PopFixed => {
                    fixed -= 1;
                    continue;
                }
                D::Image { image, rect, .. } => (image, *rect),
                D::ImageTiled { image, area, .. } => (image, *area),
                _ => continue,
            };
            if img.anim.is_none() || !seen.insert(Arc::as_ptr(img)) {
                continue;
            }
            // Transforms may move items; only skip clearly off-screen ones
            // outside fixed layers.
            if fixed == 0 && rect.intersect(&Rect::new(view.x - 200.0, view.y - 200.0, view.w + 400.0, view.h + 400.0)).is_empty() {
                continue;
            }
            let (c, n) = img.update_animation(now);
            changed |= c;
            if let Some(n) = n {
                next = Some(next.map(|x| x.min(n)).unwrap_or(n));
            }
        }
        page.image_state.next_frame = next.map(|d| now + d.max(Duration::from_millis(16)));
        changed
    }

    /// Memory/paint saver: images shown at less than a quarter of their
    /// pixel size are replaced by a box-filtered copy at twice the
    /// displayed resolution (natural size unchanged, so layout is too).
    /// Returns true when the display list was rebuilt.
    pub(crate) fn shrink_oversized_images(&self, page: &mut Page) -> bool {
        use rabbithole_layout::DisplayItem as D;
        let dpr = self.device_pixel_ratio.max(1.0);
        let mut need: HashMap<*const Image, f32> = HashMap::new();
        for it in &page.display_list.items {
            let (img, s) = match it {
                D::Image { image, rect, src, .. } if src.w > 0.0 && src.h > 0.0 => (image, (rect.w * dpr / src.w).max(rect.h * dpr / src.h)),
                D::ImageTiled { image, tile, .. } => (image, (tile.0 * dpr / image.width as f32).max(tile.1 * dpr / image.height as f32)),
                _ => continue,
            };
            let e = need.entry(Arc::as_ptr(img)).or_insert(0.0);
            *e = e.max(s);
        }
        let mut changed = false;
        for img in page.images.values_mut() {
            if img.anim.is_some() || img.broken || (img.width as u64 * img.height as u64) < 512 * 512 {
                continue;
            }
            let Some(&s) = need.get(&Arc::as_ptr(img)) else { continue };
            if s <= 0.0 || s >= 0.25 {
                continue;
            }
            let nw = ((img.width as f32 * s * 2.0).ceil() as u32).max(1);
            let nh = ((img.height as f32 * s * 2.0).ceil() as u32).max(1);
            *img = Arc::new(img.resized(nw, nh));
            changed = true;
        }
        if changed {
            self.rebuild_display_list(page);
        }
        changed
    }

    /// Fire queued `load`/`error` events on image elements.
    pub(crate) fn fire_image_events(&self, page: &mut Page) -> crate::ScriptOutcome {
        let events = std::mem::take(&mut page.image_state.events);
        let mut out = crate::ScriptOutcome::default();
        if page.script.host.is_none() {
            return out;
        }
        for (n, ev) in events {
            let o = self.dispatch_event(page, n, ev, rabbithole_script::EventInit { bubbles: false, cancelable: false, detail: None });
            out.relayout |= o.relayout;
            if o.navigation.is_some() {
                out.navigation = o.navigation;
            }
            out.new_windows.extend(o.new_windows);
            out.dialogs.extend(o.dialogs);
            if o.scroll_to.is_some() {
                out.scroll_to = o.scroll_to;
            }
        }
        out
    }
}

/// The page icon: the best `<link rel="icon">` (closest to 32px, SVG and
/// ICO accepted), else `/favicon.ico` on http(s) pages.
fn favicon_url(doc: &Document, base: &Url, page: &Url) -> Option<Url> {
    let mut best: Option<(f32, Url)> = None;
    for n in doc.descendants(Document::ROOT) {
        let Some(el) = doc.element(n) else { continue };
        if el.tag != "link" {
            continue;
        }
        let rel = el.attr("rel").unwrap_or("").to_ascii_lowercase();
        if !rel.split_ascii_whitespace().any(|r| r == "icon" || r == "apple-touch-icon") {
            continue;
        }
        let Some(u) = el.attr("href").map(str::trim).filter(|h| !h.is_empty()).and_then(|h| base.join(h)) else { continue };
        let size = el
            .attr("sizes")
            .and_then(|s| s.split_ascii_whitespace().next().and_then(|d| d.split(['x', 'X']).next()).and_then(|w| w.parse::<f32>().ok()))
            .unwrap_or(if rel.contains("apple") { 180.0 } else { 32.0 });
        let score = (size - 32.0).abs() + if rel.contains("apple") { 100.0 } else { 0.0 };
        if best.as_ref().map(|(b, _)| score < *b).unwrap_or(true) {
            best = Some((score, u));
        }
    }
    best.map(|(_, u)| u).or_else(|| page.is_http().then(|| page.join("/favicon.ico")).flatten())
}

/// Put a fetch result on an element (`None` = failed). Returns true when
/// the element's image changed. `document`: the resource is a document,
/// which an `<object>` shows as a nested browsing context (its content is
/// not rendered here: a transparent 300×150 box, the default object size)
/// instead of falling back to its children.
fn install(page: &mut Page, n: NodeId, result: Option<Option<Arc<Image>>>, density: f32, key: &str, document: bool) -> bool {
    let decoded = result.flatten();
    let decoded = match decoded {
        None if document && page.doc.is_element(n, "object") => Some(Arc::new(Image::new(1, 1, vec![0; 4]).with_natural_size(300, 150))),
        d => d,
    };
    let is_img = page.doc.is_element(n, "img") || page.doc.is_element(n, "input");
    let ok = decoded.is_some();
    page.image_state.reqs.insert(n, Req { url: key.to_string(), density, status: if ok { Status::Loaded } else { Status::Failed } });
    page.image_state.events.push((n, if ok { "load" } else { "error" }));
    match decoded {
        Some(img) => {
            page.images.insert(n, apply_density(img, density));
            true
        }
        None if is_img => {
            page.images.insert(n, Arc::new(Image::broken()));
            true
        }
        None => page.images.remove(&n).is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VP: Viewport = Viewport { width: 1000.0, height: 800.0 };

    #[test]
    fn srcset_parsing() {
        let c = parse_srcset("a.jpg 1x, b.jpg 2x");
        assert_eq!(c.len(), 2);
        assert_eq!(c[1], SrcCandidate { url: "b.jpg".into(), width: None, density: Some(2.0) });
        let c = parse_srcset("https://x/a,b.jpg 400w,\n https://x/c.jpg 800w");
        assert_eq!(c[0].url, "https://x/a,b.jpg");
        assert_eq!(c[0].width, Some(400.0));
        assert_eq!(c[1].url, "https://x/c.jpg");
        let c = parse_srcset("one.png");
        assert_eq!(c, vec![SrcCandidate { url: "one.png".into(), width: None, density: None }]);
        // Bad descriptors drop the candidate.
        assert!(parse_srcset("x.png 2q").is_empty());
    }

    #[test]
    fn sizes_and_lengths() {
        assert_eq!(eval_length("50vw", VP), Some(500.0));
        assert_eq!(eval_length("calc(100vw - 2rem)", VP), Some(968.0));
        assert_eq!(eval_length("min(300px, 50vw)", VP), Some(300.0));
        assert_eq!(eval_sizes("(max-width: 600px) 100vw, (max-width: 1200px) 50vw, 33vw", VP, 1.0, None), 500.0);
        assert_eq!(eval_sizes("(min-width: 1200px) 400px, calc(100vw - 20px)", VP, 1.0, None), 980.0);
        assert_eq!(eval_sizes("auto, 200px", VP, 1.0, Some(321.0)), 321.0);
        assert_eq!(eval_sizes("auto, 200px", VP, 1.0, None), 200.0);
        assert_eq!(eval_sizes("", VP, 1.0, None), 1000.0);
    }

    #[test]
    fn candidate_choice_by_dpr() {
        let c = parse_srcset("s.jpg 300w, m.jpg 600w, l.jpg 1200w");
        assert_eq!(choose_candidate(&c, 500.0, 1.0).unwrap().url, "m.jpg");
        assert_eq!(choose_candidate(&c, 500.0, 2.0).unwrap().url, "l.jpg");
        assert_eq!(choose_candidate(&c, 300.0, 1.0).unwrap().url, "s.jpg");
        // Nothing dense enough: the densest one.
        assert_eq!(choose_candidate(&c, 2000.0, 1.0).unwrap().url, "l.jpg");
        let x = parse_srcset("a.png, b.png 2x");
        assert_eq!(choose_candidate(&x, 100.0, 1.0).unwrap().url, "a.png");
        let b = choose_candidate(&x, 100.0, 2.0).unwrap();
        assert_eq!((b.url.as_str(), b.density), ("b.png", 2.0));
    }

    #[test]
    fn resolution_queries() {
        assert!(media_matches("(min-resolution: 2dppx)", VP, 2.0, true));
        assert!(!media_matches("(min-resolution: 2dppx)", VP, 1.0, true));
        assert!(media_matches("(-webkit-min-device-pixel-ratio: 1.5), (min-resolution: 144dpi)", VP, 1.5, true));
        assert!(media_matches("(max-width: 1200px)", VP, 1.0, true));
        assert!(!media_matches("(max-width: 600px)", VP, 1.0, true));
        assert!(media_matches("not print", VP, 1.0, true));
    }

    #[test]
    fn picture_source_selection() {
        let doc = rabbithole_html::parse(
            r#"<picture>
                 <source type="image/jxl" srcset="a.jxl">
                 <source media="(max-width: 600px)" srcset="small.webp">
                 <source type="image/avif" srcset="big.avif 1x, big2.avif 2x">
                 <img src="fallback.jpg">
               </picture>
               <img id=lazy src="data:image/gif;base64,R0lGOD" data-src="real.jpg">
               <img id=s srcset="a.jpg 1x, b.jpg 2x" src="a.jpg">"#,
        );
        let imgs: Vec<NodeId> = doc.descendants(Document::ROOT).filter(|&n| doc.is_element(n, "img")).collect();
        assert_eq!(select_image(&doc, imgs[0], VP, 1.0, true, None).unwrap().url, "big.avif");
        assert_eq!(select_image(&doc, imgs[0], VP, 2.0, true, None).unwrap().url, "big2.avif");
        let narrow = Viewport { width: 500.0, height: 800.0 };
        assert_eq!(select_image(&doc, imgs[0], narrow, 1.0, true, None).unwrap().url, "small.webp");
        assert_eq!(select_image(&doc, imgs[1], VP, 1.0, true, None).unwrap().url, "real.jpg");
        assert_eq!(select_image(&doc, imgs[2], VP, 2.0, true, None).unwrap().url, "b.jpg");
    }
}
