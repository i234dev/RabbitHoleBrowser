//! Offscreen renders of the browser chrome and the internal pages (no
//! window needed). The PNGs land in `$TMPDIR/rabbithole-snapshots/` for
//! visual inspection; the tests also check a few pixels.

use std::path::PathBuf;
use std::sync::Arc;

use rabbithole_engine::gfx::{Canvas, Color};
use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Viewport};

use super::content::{Frame, NodeDetails};
use super::omnibox;
use super::tab::Tab;
use super::{pages, App, GuiOptions, Panel, UiFocus};
use crate::bookmarks::Bookmarks;
use crate::history::History;
use crate::profile::Profile;
use crate::settings::Settings;

/// The chrome and page themes are process-wide: tests touching them run
/// one at a time.
static THEME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn out_dir() -> PathBuf {
    let d = std::env::temp_dir().join("rabbithole-snapshots");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn save_png(c: &Canvas, name: &str) -> PathBuf {
    let path = out_dir().join(name);
    let img = image::RgbaImage::from_raw(c.width, c.height, c.to_rgba8()).expect("buffer size");
    img.save(&path).expect("write png");
    path
}

fn headless_app(tag: &str) -> App {
    let dir = std::env::temp_dir().join(format!("rh-snap-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let profile = Profile::at(&dir);
    let sink: super::content::Sink = Arc::new(|_, _, _| {});
    App::new(Vec::new(), GuiOptions { private: false, new_window: true }, profile, sink)
}

/// Render HTML with our engine into a canvas.
fn render_html(html: &str, url: &str, w: u32, h: u32) -> Canvas {
    let mut engine = Engine::new_bundled_fonts_only();
    let mut page = engine.load_html(html, Url::parse(url).unwrap(), Viewport { width: w as f32, height: h as f32 });
    engine.flush_display_list(&mut page);
    let mut canvas = Canvas::new(w, h);
    engine.render(&page, &mut canvas, 0.0, 0.0);
    canvas
}

fn frame_from(canvas: &mut Canvas, content_h: f32) -> Frame {
    Frame {
        seq: 1,
        canvas: Arc::new(super::content::Pixels::from_canvas(canvas)),
        origin: (0.0, 0.0),
        scale: 1.0,
        scroll: (0.0, 0.0),
        content: (canvas.width as f32, content_h),
        background: Color::WHITE,
        scroll_containers: Arc::new(Vec::new()),
    }
}

fn tab(app: &mut App, title: &str, url: &str) -> usize {
    let id = app.next_tab_id;
    app.next_tab_id += 1;
    let mut t = Tab::new(id);
    t.title = title.into();
    let u = Url::parse(url).unwrap();
    t.current_url = Some(u.clone());
    t.history = vec![u];
    app.tabs.push(t);
    app.tabs.len() - 1
}

fn populate(app: &mut App) {
    tab(app, "Mail", "https://mail.example.org/inbox");
    app.tabs[0].pinned = true;
    tab(app, "Docs", "https://docs.example.org/");
    app.tabs[1].pinned = true;
    tab(app, "The Rust Programming Language", "https://doc.rust-lang.org/book/");
    let i = tab(app, "Loading page…", "http://insecure.example.com/");
    app.tabs[i].loading = Some(Url::parse("http://insecure.example.com/").unwrap());
    app.tabs[i].load_started = Some(std::time::Instant::now());
    tab(app, "RabbitHole settings", "about:settings");
    app.active = 2;
    app.url_field.set_text("https://doc.rust-lang.org/book/");
    app.bookmarks = Bookmarks::default();
    for (u, t) in [("https://www.rust-lang.org/", "Rust"), ("https://news.example.org/", "News"), ("https://github.com/", "GitHub"), ("https://en.wikipedia.org/", "Wikipedia")] {
        app.bookmarks.toggle(u, t);
    }
    app.settings.show_bookmarks_bar = true;
    let area = app.chrome().page_area();
    let mut page = render_html(&pages::settings_page(&Settings::default(), &[], "/home/user/Downloads", None, false), "about:settings", area.w as u32, area.h as u32);
    app.tabs[2].frame = Some(frame_from(&mut page, area.h * 2.0));
}

#[test]
fn chrome_light_and_dark() {
    let _guard = lock();
    let mut app = headless_app("chrome");
    populate(&mut app);
    app.paint(1200, 800);
    let p = save_png(&app.canvas, "chrome-light.png");
    let chrome = app.chrome();
    // Tab strip background and the active tab.
    let bar = chrome.tab_bar();
    let px = |c: &Canvas, x: f32, y: f32| Color::from_u32(c.data[y as usize * c.width as usize + x as usize]);
    assert_eq!(px(&app.canvas, bar.right() - 5.0, 6.0), super::chrome::LIGHT.tab_bar_bg, "{}", p.display());
    let (rects, _) = chrome.tab_rects(&app.pinned_flags());
    assert_eq!(px(&app.canvas, rects[2].x + rects[2].w / 2.0, rects[2].bottom() - 2.0), super::chrome::LIGHT.chrome_bg);
    // Pinned tabs are narrow.
    assert!(rects[0].w < 50.0 && rects[2].w > 100.0);
    // The bookmarks bar is shown with the page below it.
    assert!(chrome.bookmarks_bar().h > 0.0);
    assert!(chrome.page_area().y >= chrome.bookmarks_bar().bottom());

    // Dark theme.
    app.settings.theme = crate::settings::ThemeChoice::Dark;
    app.apply_theme();
    app.paint(1200, 800);
    save_png(&app.canvas, "chrome-dark.png");
    assert_eq!(px(&app.canvas, bar.right() - 5.0, 6.0), super::chrome::DARK.tab_bar_bg);
    app.settings.theme = crate::settings::ThemeChoice::Light;
    app.apply_theme();
}

#[test]
fn omnibox_suggestions_and_menus() {
    let _guard = lock();
    let mut app = headless_app("omnibox");
    populate(&mut app);
    let mut h = History::default();
    for (u, t) in [("https://github.com/rust-lang/rust", "rust-lang/rust"), ("https://github.com/", "GitHub"), ("https://gitlab.com/", "GitLab")] {
        h.add(u, t);
    }
    app.history = h;
    app.ui_focus = UiFocus::Url;
    app.url_field.set_text("git");
    app.suggestions = omnibox::suggest("git", &app.history, &app.bookmarks, app.settings.search_engine, 8);
    app.suggestion_sel = Some(1);
    app.paint(1200, 800);
    save_png(&app.canvas, "omnibox.png");
    assert!(app.suggestions.len() >= 3);
    // Context menu over the page.
    app.ui_focus = UiFocus::Page;
    app.suggestions.clear();
    app.open_context_menu(Some(Url::parse("https://example.com/a").unwrap()), Some(Url::parse("https://example.com/i.png").unwrap()), true, 400.0, 300.0);
    app.paint(1200, 800);
    save_png(&app.canvas, "context-menu.png");
    let m = app.context_menu.as_ref().unwrap();
    assert!(m.items.iter().any(|i| i.label == "Save image as…"));
    assert!(m.items.iter().any(|i| i.label == "Inspect"));
}

#[test]
fn devtools_panels() {
    let _guard = lock();
    let mut app = headless_app("devtools");
    populate(&mut app);
    app.settings.show_bookmarks_bar = false;
    app.devtools_open = true;
    let t = &mut app.tabs[2];
    t.console.push("log", "hello from the page");
    t.console.push("error", "TypeError: x is undefined");
    t.console.push("input", "> 1 + 1");
    t.console.push("result", "2");
    app.panel = Panel::Console;
    app.paint(1200, 800);
    save_png(&app.canvas, "devtools-console.png");
    // Elements.
    let t = &mut app.tabs[2];
    t.inspector.dom = (0..12)
        .map(|i| super::content::DomRow { node: i, depth: i % 4, label: if i % 3 == 2 { "\"some text\"".into() } else { format!("<div class=\"c{i}\">") }, is_text: i % 3 == 2 })
        .collect();
    t.inspector.selected = Some(3);
    t.inspector.details = NodeDetails {
        node: Some(3),
        styles: vec![("display".into(), "block".into()), ("color".into(), "rgb(32, 33, 36)".into()), ("font-size".into(), "16px".into())],
        attrs: vec![("class".into(), "c3".into())],
    };
    app.panel = Panel::Elements;
    app.paint(1200, 800);
    save_png(&app.canvas, "devtools-elements.png");
    // Network.
    let t = &mut app.tabs[2];
    for (i, (path, status, dest)) in [("/book/", 200, rabbithole_engine::net::Destination::Document), ("/book/style.css", 200, rabbithole_engine::net::Destination::Style), ("/missing.js", 404, rabbithole_engine::net::Destination::Script)].iter().enumerate() {
        t.log_request(rabbithole_engine::net::NetEvent {
            method: "GET".into(),
            url: Url::parse(&format!("https://doc.rust-lang.org{path}")).unwrap(),
            dest: *dest,
            status: Some(*status),
            content_type: "text/html".into(),
            size: Some(1000 * (i as u64 + 1)),
            duration: std::time::Duration::from_millis(40 + i as u64 * 10),
            error: None,
        });
    }
    app.panel = Panel::Network;
    app.paint(1200, 800);
    save_png(&app.canvas, "devtools-network.png");
    assert_eq!(super::console::resource_type(&app.tabs[2].inspector.network[1]), "stylesheet");
}

#[test]
fn cookie_menu_and_indicator() {
    let _guard = lock();
    let mut app = headless_app("cookies");
    populate(&mut app);
    app.settings.cookie_consent = crate::autocookies::ConsentMode::Reject;
    let ev = crate::autocookies::ConsentEvent {
        host: "doc.rust-lang.org".into(),
        url: "https://doc.rust-lang.org/book/".into(),
        outcome: rabbithole_engine::consent::ConsentOutcome::Rejected,
        choice: rabbithole_engine::consent::Choice::Reject,
        button: "Reject all".into(),
        vendor: Some("OneTrust".into()),
        in_frame: false,
        steps: vec!["Reject all".into()],
    };
    app.tabs[2].consent = Some((ev, std::time::Instant::now()));
    let chrome = app.chrome();
    assert!(chrome.consent_icon);
    assert!(chrome.url_text().right() <= chrome.consent_icon_rect().x);
    // Hover the indicator: tooltip in the status bar.
    let r = chrome.consent_icon_rect();
    app.cursor = winit::dpi::PhysicalPosition::new((r.x + r.w / 2.0) as f64, (r.y + r.h / 2.0) as f64);
    assert!(app.consent_tooltip().unwrap().contains("rejected automatically"));
    app.paint(1200, 800);
    let p = save_png(&app.canvas, "cookie-indicator.png");
    // The badge is painted (cookie colour inside the icon rect).
    let mut found = false;
    for y in r.y as usize..(r.y + r.h) as usize {
        for x in r.x as usize..(r.x + r.w) as usize {
            let c = Color::from_u32(app.canvas.data[y * app.canvas.width as usize + x]);
            if c.r == 0xd4 && c.g == 0x9a && c.b == 0x4f {
                found = true;
            }
        }
    }
    assert!(found, "cookie icon not painted: {}", p.display());
    // The main menu shows the current mode and the log entry.
    app.open_main_menu();
    app.paint(1200, 800);
    save_png(&app.canvas, "menu-cookies.png");
    let m = app.menu.as_ref().unwrap();
    assert!(m.items.iter().any(|i| i.label == "Cookie dialogs: Auto-reject"));
    assert!(m.items.iter().any(|i| i.label == "Handled cookie dialogs…"));
    assert!(m.items.len() <= m.max_visible, "menu fits without scrolling");
    app.menu = None;
}

#[test]
fn about_pages_render() {
    let _guard = lock();
    let mut h = History::default();
    h.add("https://www.rust-lang.org/", "Rust Programming Language");
    h.add("https://en.wikipedia.org/wiki/Rabbit", "Rabbit - Wikipedia");
    let mut b = Bookmarks::default();
    b.toggle("https://www.rust-lang.org/", "Rust");
    b.items.push(crate::bookmarks::Bookmark { url: "https://docs.example.org/".into(), title: "Docs".into(), folder: "Work".into() });
    let rows = vec![
        pages::DownloadRow { id: 1, url: "https://example.org/big.iso".into(), filename: "big.iso".into(), path: "/home/user/Downloads/big.iso".into(), done: 400 << 20, total: Some(1 << 30), state: "Downloading".into(), active: true, paused: false, resumable: true, finished: false, failed: false, speed: Some(5 << 20) },
        pages::DownloadRow { id: 2, url: "https://example.org/doc.pdf".into(), filename: "doc.pdf".into(), path: "/home/user/Downloads/doc.pdf".into(), done: 2 << 20, total: Some(2 << 20), state: "Completed".into(), active: false, paused: false, resumable: false, finished: true, failed: false, speed: None },
    ];
    let sites = vec![pages::SiteData { host: "example.org".into(), cookies: 3, zoom: Some(1.25) }];
    let mut ac = crate::autocookies::AutoCookies::default();
    for (host, outcome, button, vendor, t) in [
        ("www.bbc.example", rabbithole_engine::consent::ConsentOutcome::Rejected, "Reject additional cookies", "generic", 1_762_850_000u64),
        ("elpais.example", rabbithole_engine::consent::ConsentOutcome::Rejected, "Rechazar todo", "Didomi", 1_762_853_600),
        ("www.spiegel.example", rabbithole_engine::consent::ConsentOutcome::NoButton, "", "Sourcepoint", 1_762_857_200),
        ("www.lemonde.example", rabbithole_engine::consent::ConsentOutcome::Accepted, "Tout accepter", "", 1_762_860_800),
    ] {
        let ev = crate::autocookies::ConsentEvent { host: host.into(), url: format!("https://{host}/"), outcome, choice: rabbithole_engine::consent::Choice::Reject, button: button.into(), vendor: Some(vendor.into()).filter(|v: &String| !v.is_empty()), in_frame: false, steps: vec![] };
        ac.record(&ev, t);
    }
    ac.set_rule("spiegel.example", Some(crate::autocookies::SiteRule::Accept));
    ac.set_rule("bank.example", Some(crate::autocookies::SiteRule::Never));
    let cases: Vec<(&str, String)> = vec![
        ("about-autocookies.png", pages::autocookies_page(&ac, crate::autocookies::ConsentMode::Reject, "", None, false)),
        ("about-autocookies-search.png", pages::autocookies_page(&ac, crate::autocookies::ConsentMode::Reject, "rechazar", Some("elpais.example: Always reject"), false)),
        ("about-settings.png", pages::settings_page(&Settings::default(), &sites, "/home/user/Downloads", None, false)),
        ("about-history.png", pages::history_page(&h, "")),
        ("about-bookmarks.png", pages::bookmarks_page(&b, None, true, None)),
        ("about-downloads.png", pages::downloads_page(&rows, "/home/user/Downloads", false)),
        ("error-dns.png", pages::error_page("https://nowhere.invalid/", "I/O error: failed to lookup address information: Name or service not known")),
        ("error-tls.png", pages::error_page("https://expired.example.com/", "I/O error: invalid peer certificate: Expired")),
        ("error-offline.png", pages::error_page("https://example.com/", "I/O error: Network is unreachable (os error 101)")),
        ("error-blocked.png", pages::error_page("https://ads.tracker.example/", "blocked ad/tracker host: ads.tracker.example")),
        ("error-404.png", pages::error_page("https://example.com/missing", "HTTP error 404")),
    ];
    for (name, html) in cases {
        let c = render_html(&html, "about:blank", 1000, 760);
        let path = save_png(&c, name);
        // Not blank: several colours present.
        let mut colours = std::collections::HashSet::new();
        for p in c.data.iter().step_by(97) {
            colours.insert(*p);
        }
        assert!(colours.len() > 5, "{} looks blank", path.display());
    }
    // Dark variant of the settings page.
    pages::set_dark(true);
    let html = pages::settings_page(&Settings::default(), &sites, "/home/user/Downloads", None, false);
    pages::set_dark(false);
    let c = render_html(&html, "about:blank", 1000, 760);
    save_png(&c, "about-settings-dark.png");
    let bg = Color::from_u32(c.data[c.width as usize * 700 + 5]);
    assert!(bg.r < 0x40, "dark background: {bg:?}");
}
