//! Headless command line subcommands: screenshot, text, links, dump.

use rabbithole_engine::{Engine, Page, Viewport};

/// Hard cap on the height of a full-page capture.
pub const MAX_FULL_HEIGHT: u32 = 20_000;

/// Options for a headless capture.
#[derive(Debug, Clone)]
pub struct CaptureOptions {
    pub width: u32,
    pub height: u32,
    pub full_page: bool,
    pub scroll_y: f32,
    pub images: bool,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        CaptureOptions {
            width: 1280,
            height: 800,
            full_page: false,
            scroll_y: 0.0,
            images: true,
        }
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

/// Load `url` and render it into an RGBA buffer according to `opts`.
pub fn capture(engine: &mut Engine, url: &str, opts: &CaptureOptions) -> Result<Capture, String> {
    engine.load_images = opts.images;
    let width = opts.width.max(1);
    let viewport_h = opts.height.max(1);
    let viewport = Viewport {
        width: width as f32,
        height: viewport_h as f32,
    };
    let page = engine.load(url, viewport).map_err(|e| e.to_string())?;
    let height = if opts.full_page {
        (page.content_height().ceil() as u32).clamp(1, MAX_FULL_HEIGHT)
    } else {
        viewport_h
    };
    let mut canvas = rabbithole_engine::gfx::Canvas::new(width, height);
    canvas.clear(rabbithole_engine::gfx::Color::WHITE);
    let max_scroll = (page.content_height() - height as f32).max(0.0);
    let scroll_y = if opts.full_page { 0.0 } else { opts.scroll_y.clamp(0.0, max_scroll) };
    engine.render(&page, &mut canvas, 0.0, scroll_y);
    Ok(Capture {
        rgba: canvas.to_rgba8(),
        width,
        height,
        title: page.title.clone(),
        url: page.url.to_string(),
    })
}

/// Encode a capture as PNG bytes.
pub fn encode_png(cap: &Capture) -> Result<Vec<u8>, String> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::write_buffer_with_format(
        &mut out,
        &cap.rgba,
        cap.width,
        cap.height,
        image::ExtendedColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .map_err(|e| format!("PNG encoding failed: {e}"))?;
    Ok(out.into_inner())
}

/// Load a page for text-oriented commands (images disabled).
fn load_text_page(engine: &mut Engine, url: &str) -> Result<Page, String> {
    engine.load_images = false;
    let viewport = Viewport {
        width: 1280.0,
        height: 800.0,
    };
    engine.load(url, viewport).map_err(|e| e.to_string())
}

/// Minimal option parser: returns the positional arguments and a list of
/// (flag, value) pairs. Flags listed in `takes_value` consume the next arg.
fn parse_args<'a>(args: &'a [String], takes_value: &[&str]) -> Result<(Vec<&'a str>, Vec<(&'a str, &'a str)>), String> {
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
    positional
        .first()
        .copied()
        .ok_or_else(|| format!("usage: rabbithole {cmd} <URL> [options]"))
}

/// `rabbithole screenshot <URL> [-o out.png] [--width W] [--height H] [--full] [--scroll Y] [--no-images]`
pub fn screenshot(args: &[String]) -> Result<(), String> {
    let (positional, flags) = parse_args(args, &["output", "width", "height", "scroll"])?;
    let url = url_arg(&positional, "screenshot")?;
    let opts = CaptureOptions {
        width: parse_num(&flags, "width", 1280u32)?,
        height: parse_num(&flags, "height", 800u32)?,
        full_page: has_flag(&flags, "full"),
        scroll_y: parse_num(&flags, "scroll", 0.0f32)?,
        images: !has_flag(&flags, "no-images"),
    };
    let output = flag(&flags, "output").unwrap_or("screenshot.png");
    let mut engine = Engine::new();
    let cap = capture(&mut engine, url, &opts)?;
    image::save_buffer(output, &cap.rgba, cap.width, cap.height, image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("could not write {output}: {e}"))?;
    eprintln!("{} ({}x{}) -> {output}", cap.title, cap.width, cap.height);
    Ok(())
}

/// `rabbithole text <URL>`
pub fn text(args: &[String]) -> Result<(), String> {
    let (positional, _flags) = parse_args(args, &[])?;
    let url = url_arg(&positional, "text")?;
    let mut engine = Engine::new();
    let page = load_text_page(&mut engine, url)?;
    println!("{}", page.text());
    Ok(())
}

/// `rabbithole links <URL>`
pub fn links(args: &[String]) -> Result<(), String> {
    let (positional, _flags) = parse_args(args, &[])?;
    let url = url_arg(&positional, "links")?;
    let mut engine = Engine::new();
    let page = load_text_page(&mut engine, url)?;
    for link in page.links() {
        let text = link.text.split_whitespace().collect::<Vec<_>>().join(" ");
        println!("{text}\t{}", link.href);
    }
    Ok(())
}

/// `rabbithole dump <URL> [--dom] [--layout]` (both when neither is given)
pub fn dump(args: &[String]) -> Result<(), String> {
    let (positional, flags) = parse_args(args, &[])?;
    let url = url_arg(&positional, "dump")?;
    let want_dom = has_flag(&flags, "dom");
    let want_layout = has_flag(&flags, "layout");
    let (want_dom, want_layout) = if !want_dom && !want_layout { (true, true) } else { (want_dom, want_layout) };
    let mut engine = Engine::new();
    let page = load_text_page(&mut engine, url)?;
    if want_dom {
        println!("=== DOM: {} ===", page.url);
        println!("{}", page.dom_dump());
    }
    if want_layout {
        println!("=== Layout ({}x{}) ===", page.content_width(), page.content_height());
        println!("{}", page.layout_dump());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_flags_and_positionals() {
        let args = s(&["https://x", "-o", "a.png", "--width", "640", "--full", "--scroll=10"]);
        let (pos, flags) = parse_args(&args, &["output", "width", "height", "scroll"]).unwrap();
        assert_eq!(pos, vec!["https://x"]);
        assert_eq!(flag(&flags, "output"), Some("a.png"));
        assert_eq!(parse_num(&flags, "width", 0u32).unwrap(), 640);
        assert_eq!(parse_num(&flags, "scroll", 0.0f32).unwrap(), 10.0);
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
}
