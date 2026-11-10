//! Headless command line subcommands: screenshot, text, links, dump, cookies.

use std::sync::{Arc, Mutex};

use rabbithole_engine::net::{CookieJar, Request, Url};
use rabbithole_engine::{Engine, Loaded, Page, Viewport};

use crate::profile::Profile;

/// Hard cap on the height of a full-page capture (CSS px).
pub const MAX_FULL_HEIGHT: u32 = 20_000;

/// Options for a headless capture.
#[derive(Debug, Clone)]
pub struct CaptureOptions {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub full_page: bool,
    pub scroll_y: f32,
    pub images: bool,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        CaptureOptions { width: 1280, height: 800, scale: 1.0, full_page: false, scroll_y: 0.0, images: true }
    }
}

/// Result of a headless capture: tightly packed RGBA8 pixels.
pub struct Capture {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub title: String,
    pub url: String,
}

/// An engine wired to the persistent cookie jar of the profile.
pub struct Session {
    pub engine: Engine,
    pub profile: Profile,
    pub cookies: Arc<Mutex<CookieJar>>,
    /// How long to let timers run after load (ms) so script-rendered
    /// content settles.
    pub wait_ms: u64,
    /// Print page console output to stderr.
    pub verbose: bool,
}

/// Script-related command line options shared by all subcommands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JsOptions {
    pub enabled: bool,
    pub wait_ms: u64,
    pub verbose: bool,
}

impl JsOptions {
    /// From `--no-js`, `--wait <ms>` and `--verbose` (default wait when JS
    /// is on: `default_wait`).
    pub fn from_flags(flags: &[(&str, &str)], default_wait: u64) -> Result<JsOptions, String> {
        let enabled = !has_flag(flags, "no-js");
        let wait_ms = parse_num(flags, "wait", if enabled { default_wait } else { 0 })?;
        Ok(JsOptions { enabled, wait_ms, verbose: has_flag(flags, "verbose") })
    }
}

impl Session {
    /// `system_fonts`: scan platform fonts (slower start; needed for
    /// faithful screenshots).
    pub fn new(system_fonts: bool) -> Session {
        let profile = Profile::locate();
        let cookies = profile.load_cookies();
        let mut engine = if system_fonts { Engine::new() } else { Engine::new_bundled_fonts_only() };
        engine.loader.fetcher.cookies = Some(cookies.clone());
        engine.set_local_storage_path(profile.local_storage_path());
        Session { engine, profile, cookies, wait_ms: 1000, verbose: false }
    }

    /// Apply `--no-js` / `--wait` / `--verbose`.
    pub fn with_js(mut self, js: JsOptions) -> Session {
        self.engine.scripting = js.enabled;
        self.engine.loader.load_scripts = js.enabled;
        self.wait_ms = js.wait_ms;
        self.verbose = js.verbose;
        self
    }

    pub fn save(&self) {
        self.profile.save_cookies(&self.cookies);
    }

    /// Load a page with an arbitrary request; downloads are an error here.
    pub fn load_request(&mut self, req: &Request, viewport: Viewport) -> Result<Page, String> {
        let loaded = self.engine.loader.load_request(req, viewport).map_err(|e| e.to_string())?;
        let mut page = match loaded {
            Loaded::Page(p) => self.engine.build_page(p, viewport),
            Loaded::Download(s) => return Err(format!("{} is a download ({}), not a page", s.url, s.content_type)),
        };
        if page.has_scripts() && self.wait_ms > 0 {
            // Script-driven redirects (`location.href = ...`) are followed once.
            if let Some(nav) = crate::scripting::settle(&self.engine, &mut page, self.wait_ms) {
                self.flush_console(&mut page);
                let loaded = self.engine.loader.load_request(&nav.request, viewport).map_err(|e| e.to_string())?;
                if let Loaded::Page(p) = loaded {
                    page = self.engine.build_page(p, viewport);
                    let _ = crate::scripting::settle(&self.engine, &mut page, self.wait_ms);
                }
            }
        }
        self.flush_console(&mut page);
        self.save();
        Ok(page)
    }

    /// Print (when verbose) and clear the page's console messages.
    pub fn flush_console(&self, page: &mut Page) {
        for (level, msg) in page.take_console() {
            if self.verbose || level == "error" && std::env::var("RH_JS_LOG").is_ok() {
                eprintln!("[console.{level}] {msg}");
            }
        }
    }
}

/// Load `url` and render it into an RGBA buffer according to `opts`.
pub fn capture(session: &mut Session, url: &str, opts: &CaptureOptions) -> Result<Capture, String> {
    session.engine.loader.load_images = opts.images;
    let scale = if opts.scale.is_finite() && opts.scale > 0.0 { opts.scale } else { 1.0 };
    let width = opts.width.max(1);
    let viewport_h = opts.height.max(1);
    let viewport = Viewport { width: width as f32, height: viewport_h as f32 };
    let parsed = Url::parse(url).ok_or_else(|| format!("invalid URL: {url}"))?;
    let page = session.load_request(&Request::get(parsed), viewport)?;
    let css_height = if opts.full_page { (page.content_height().ceil() as u32).clamp(1, MAX_FULL_HEIGHT) } else { viewport_h };
    let px_w = ((width as f32) * scale).round().max(1.0) as u32;
    let px_h = ((css_height as f32) * scale).round().max(1.0) as u32;
    let mut canvas = rabbithole_engine::gfx::Canvas::new(px_w, px_h);
    let max_scroll = (page.content_height() - css_height as f32).max(0.0);
    let scroll_y = if opts.full_page { 0.0 } else { opts.scroll_y.clamp(0.0, max_scroll) };
    session.engine.render_scaled(&page, &mut canvas, 0.0, scroll_y, scale);
    Ok(Capture { rgba: canvas.to_rgba8(), width: px_w, height: px_h, title: page.title.clone(), url: page.url.to_string() })
}

/// Encode a capture as PNG bytes.
pub fn encode_png(cap: &Capture) -> Result<Vec<u8>, String> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::write_buffer_with_format(&mut out, &cap.rgba, cap.width, cap.height, image::ExtendedColorType::Rgba8, image::ImageFormat::Png)
        .map_err(|e| format!("PNG encoding failed: {e}"))?;
    Ok(out.into_inner())
}

pub const TEXT_VIEWPORT: Viewport = Viewport { width: 1280.0, height: 800.0 };

/// Load a page for text-oriented commands (images disabled).
pub fn load_text_page(session: &mut Session, url: &str) -> Result<Page, String> {
    session.engine.loader.load_images = false;
    if url.trim_start().starts_with('<') {
        // Inline HTML (handy for tests and quick experiments).
        let base = Url::parse("about:blank").ok_or("bad base")?;
        let mut page = session.engine.load_html(url, base, TEXT_VIEWPORT);
        let _ = crate::scripting::settle(&session.engine, &mut page, session.wait_ms);
        session.flush_console(&mut page);
        return Ok(page);
    }
    session.engine.loader.load_fonts = false;
    let parsed = Url::parse(url).ok_or_else(|| format!("invalid URL: {url}"))?;
    session.load_request(&Request::get(parsed), TEXT_VIEWPORT)
}

/// Minimal option parser: returns the positional arguments and a list of
/// (flag, value) pairs. Flags listed in `takes_value` consume the next arg.
pub fn parse_args<'a>(args: &'a [String], takes_value: &[&str]) -> Result<(Vec<&'a str>, Vec<(&'a str, &'a str)>), String> {
    let mut positional = Vec::new();
    let mut flags = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if let Some(stripped) = a.strip_prefix("--") {
            if let Some((k, v)) = stripped.split_once('=') {
                flags.push((k, v));
            } else if takes_value.contains(&stripped) {
                i += 1;
                let v = args.get(i).ok_or_else(|| format!("--{stripped} needs a value"))?;
                flags.push((stripped, v.as_str()));
            } else {
                flags.push((stripped, ""));
            }
        } else if a == "-o" {
            i += 1;
            let v = args.get(i).ok_or_else(|| "-o needs a value".to_string())?;
            flags.push(("output", v.as_str()));
        } else if a.starts_with('-') && a.len() > 1 {
            return Err(format!("unknown option `{a}`"));
        } else {
            positional.push(a);
        }
        i += 1;
    }
    Ok((positional, flags))
}

fn flag<'a>(flags: &[(&'a str, &'a str)], name: &str) -> Option<&'a str> {
    flags.iter().find(|(k, _)| *k == name).map(|(_, v)| *v)
}

fn has_flag(flags: &[(&str, &str)], name: &str) -> bool {
    flags.iter().any(|(k, _)| *k == name)
}

fn parse_num<T: std::str::FromStr>(flags: &[(&str, &str)], name: &str, default: T) -> Result<T, String> {
    match flag(flags, name) {
        None => Ok(default),
        Some(v) => v.parse().map_err(|_| format!("invalid value for --{name}: {v}")),
    }
}

fn url_arg<'a>(positional: &[&'a str], cmd: &str) -> Result<&'a str, String> {
    positional.first().copied().ok_or_else(|| format!("usage: rabbithole {cmd} <URL> [options]"))
}

/// `rabbithole screenshot <URL> [-o out.png] [--width W] [--height H] [--scale S] [--full] [--scroll Y] [--no-images]`
pub fn screenshot(args: &[String]) -> Result<(), String> {
    let (positional, flags) = parse_args(args, &["output", "width", "height", "scroll", "scale", "wait"])?;
    let url = url_arg(&positional, "screenshot")?;
    let js = JsOptions::from_flags(&flags, 1000)?;
    let opts = CaptureOptions {
        width: parse_num(&flags, "width", 1280u32)?,
        height: parse_num(&flags, "height", 800u32)?,
        scale: parse_num(&flags, "scale", 1.0f32)?,
        full_page: has_flag(&flags, "full"),
        scroll_y: parse_num(&flags, "scroll", 0.0f32)?,
        images: !has_flag(&flags, "no-images"),
    };
    let output = flag(&flags, "output").unwrap_or("screenshot.png");
    let mut session = Session::new(true).with_js(js);
    let cap = capture(&mut session, url, &opts)?;
    image::save_buffer(output, &cap.rgba, cap.width, cap.height, image::ExtendedColorType::Rgba8).map_err(|e| format!("could not write {output}: {e}"))?;
    eprintln!("{} ({}x{}) -> {output}", cap.title, cap.width, cap.height);
    Ok(())
}

/// `rabbithole text <URL>`
pub fn text(args: &[String]) -> Result<(), String> {
    let (positional, flags) = parse_args(args, &["wait"])?;
    let url = url_arg(&positional, "text")?;
    let mut session = Session::new(false).with_js(JsOptions::from_flags(&flags, 1000)?);
    let page = load_text_page(&mut session, url)?;
    emit(&page.text());
    Ok(())
}

/// `rabbithole links <URL>`
pub fn links(args: &[String]) -> Result<(), String> {
    let (positional, flags) = parse_args(args, &["wait"])?;
    let url = url_arg(&positional, "links")?;
    let mut session = Session::new(false).with_js(JsOptions::from_flags(&flags, 1000)?);
    let page = load_text_page(&mut session, url)?;
    for link in page.links() {
        let text = link.text.split_whitespace().collect::<Vec<_>>().join(" ");
        emit(&format!("{text}\t{}", link.href));
    }
    Ok(())
}

/// `rabbithole dump <URL> [--dom] [--layout]` (both when neither is given)
pub fn dump(args: &[String]) -> Result<(), String> {
    let (positional, flags) = parse_args(args, &["wait"])?;
    let url = url_arg(&positional, "dump")?;
    let js = JsOptions::from_flags(&flags, 1000)?;
    let want_dom = has_flag(&flags, "dom");
    let want_layout = has_flag(&flags, "layout");
    let (want_dom, want_layout) = if !want_dom && !want_layout { (true, true) } else { (want_dom, want_layout) };
    let mut session = Session::new(false).with_js(js);
    let page = load_text_page(&mut session, url)?;
    if want_dom {
        emit(&format!("=== DOM: {} ===", page.url));
        emit(&page.dom_dump());
    }
    if want_layout {
        emit(&format!("=== Layout ({}x{}) ===", page.content_width(), page.content_height()));
        emit(&page.layout_dump());
    }
    Ok(())
}

/// `rabbithole js <URL> <EXPRESSION> [--wait ms]`: evaluate JavaScript in
/// the loaded page (after its own scripts ran) and print the result.
pub fn js(args: &[String]) -> Result<(), String> {
    let (positional, flags) = parse_args(args, &["wait"])?;
    let (url, expr) = match positional.as_slice() {
        [u, rest @ ..] if !rest.is_empty() => (*u, rest.join(" ")),
        _ => return Err("usage: rabbithole js <URL> <EXPRESSION> [--wait ms] [--verbose]".into()),
    };
    let mut js = JsOptions::from_flags(&flags, 1000)?;
    js.enabled = true;
    let mut session = Session::new(false).with_js(js);
    let mut page = load_text_page(&mut session, url)?;
    let result = session.engine.eval_in_page(&mut page, &expr);
    let _ = crate::scripting::settle(&session.engine, &mut page, 0);
    session.flush_console(&mut page);
    emit(&result);
    Ok(())
}

/// `rabbithole cookies <URL>`: the stored cookies that would be sent.
pub fn cookies(args: &[String]) -> Result<(), String> {
    let (positional, _flags) = parse_args(args, &[])?;
    let url = url_arg(&positional, "cookies")?;
    let parsed = Url::parse(url).ok_or_else(|| format!("invalid URL: {url}"))?;
    let profile = Profile::locate();
    let jar = profile.load_cookies();
    let guard = jar.lock().map_err(|_| "cookie jar poisoned".to_string())?;
    let lines = cookie_lines(&guard, &parsed);
    if lines.is_empty() {
        emit("(no cookies)");
    }
    for l in lines {
        emit(&l);
    }
    Ok(())
}

/// Cookies of a jar that apply to `url`, one `name=value  (domain path flags)` per line.
pub fn cookie_lines(jar: &CookieJar, url: &Url) -> Vec<String> {
    let host = url.host.to_ascii_lowercase();
    jar.cookies()
        .iter()
        .filter(|c| !c.is_expired() && (host == c.domain || host.ends_with(&format!(".{}", c.domain))))
        .map(|c| {
            let mut flags = Vec::new();
            if c.secure {
                flags.push("Secure");
            }
            if c.http_only {
                flags.push("HttpOnly");
            }
            let exp = c.expires.map(|e| format!("expires={e}")).unwrap_or_else(|| "session".into());
            format!("{}={}\t{} {} {} {}", c.name, c.value, c.domain, c.path, exp, flags.join(","))
        })
        .collect()
}

/// Write a line to stdout; a closed pipe (`| head`) ends the process
/// quietly instead of panicking.
fn emit(text: &str) {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    if out.write_all(text.as_bytes()).and_then(|_| out.write_all(b"\n")).and_then(|_| out.flush()).is_err() {
        std::process::exit(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_flags_and_positionals() {
        let args = s(&["https://x", "-o", "a.png", "--width", "640", "--full", "--scroll=10", "--scale", "2"]);
        let (pos, flags) = parse_args(&args, &["output", "width", "height", "scroll", "scale"]).unwrap();
        assert_eq!(pos, vec!["https://x"]);
        assert_eq!(flag(&flags, "output"), Some("a.png"));
        assert_eq!(parse_num(&flags, "width", 0u32).unwrap(), 640);
        assert_eq!(parse_num(&flags, "scroll", 0.0f32).unwrap(), 10.0);
        assert_eq!(parse_num(&flags, "scale", 1.0f32).unwrap(), 2.0);
        assert!(has_flag(&flags, "full"));
        assert!(!has_flag(&flags, "no-images"));
    }

    #[test]
    fn rejects_bad_values() {
        let args = s(&["u", "--width", "abc"]);
        let (_, flags) = parse_args(&args, &["width"]).unwrap();
        assert!(parse_num(&flags, "width", 1u32).is_err());
        assert!(parse_args(&s(&["--width"]), &["width"]).is_err());
        assert!(parse_args(&s(&["-x"]), &[]).is_err());
    }

    #[test]
    fn cookie_listing() {
        let url = Url::parse("https://www.example.com/a").unwrap();
        let mut jar = CookieJar::new();
        jar.store(rabbithole_engine::net::Cookie::parse("a=1; Domain=example.com; Path=/; Secure", &url).unwrap());
        jar.store(rabbithole_engine::net::Cookie::parse("b=2", &Url::parse("https://other.org/").unwrap()).unwrap());
        let lines = cookie_lines(&jar, &url);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("a=1\texample.com / session Secure"));
    }
}
