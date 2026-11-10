//! RabbitHole Browser — a web browser written from scratch.
//!
//! Entry point: hand-rolled argument parsing dispatching to the GUI, the
//! headless CLI subcommands or the MCP server.

mod bookmarks;
mod cli;
mod gui;
mod history;
mod mcp;
mod profile;
mod scripting;
mod settings;

use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const HELP: &str = "RabbitHole Browser — a web browser written from scratch (no Chromium/WebKit/Gecko)

USAGE:
    rabbithole [URL...]                    Open the graphical browser (no URL: restore the last session)
    rabbithole screenshot <URL> [options]  Render a page headlessly to a PNG
    rabbithole diff <URL-A> <URL-B|PNG> [--out diff.png] [--width W] [--height H] [--tolerance T] [--max-percent P]
                                           Render two pages (or a page and a PNG) and compare them pixel by pixel
    rabbithole text <URL>                  Print the readable text of a page
    rabbithole links <URL>                 Print all links as `text<TAB>href`
    rabbithole dump <URL> [--dom] [--layout]
                                           Print the DOM and/or layout tree
    rabbithole cookies <URL>               Print the stored cookies that would be sent to URL
    rabbithole js <URL> <EXPRESSION>       Evaluate JavaScript in the loaded page and print the result
    rabbithole click <URL> <X> <Y>         Show what is under a viewport point and click it like a mouse
                                           (--scroll Y, --width/--height, --hit-only)
    rabbithole mcp [options]               Run as a Model Context Protocol server on stdio
    rabbithole agent [options] [-c CMD]... Agent tools from the shell (REPL on stdin without -c;
                                           --script FILE replays commands, --replay FILE.json checks a test)
    rabbithole ui-bench <URL> [--legacy]   Measure UI-thread stalls while a page loads (developer tool)
    rabbithole --help | --version

SCREENSHOT OPTIONS:
    -o, --output <FILE>   Output PNG path (default: screenshot.png)
    --width <PX>          Viewport width in CSS px (default: 1280)
    --height <PX>         Viewport height in CSS px (default: 800)
    --scale <F>           Device scale factor, e.g. 2 for HiDPI (default: 1)
    --full                Capture the whole page height (max 20000px)
    --scroll <PX>         Vertical scroll offset before capturing
    --no-images           Do not download images

AGENT / MCP OPTIONS:
    --browser-headers     Send normal Chrome-like headers instead of the RabbitHoleBot identity
                          (also skips robots.txt; accepted by every headless subcommand)
    --no-robots           Keep the bot identity but do not check robots.txt
    --ephemeral           In-memory profile (default: <profile>/agent, persistent)
    --profile <DIR>       Profile directory for the default context
    --allow <D1,D2>       Only load these domains
    --live                Serve a live view of the agent's tab on a local HTTP port
    --timeout <MS>        Cap of the auto-wait after loads and actions (default 5000)

COMMON OPTIONS (screenshot, text, links, dump, js):
    --no-js               Do not run page JavaScript
    --wait <MS>           Let timers run this long after load (default 1000 with JS)
    --verbose             Print the page's console output to stderr

The profile (cookies, history, bookmarks, session) lives in the platform
config directory under `rabbithole/` (override with RABBITHOLE_PROFILE).
";

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // Headless subcommands identify as a bot unless asked otherwise.
    const HEADLESS: &[&str] = &["screenshot", "text", "links", "dump", "cookies", "js", "click"];
    if args.first().map(|a| HEADLESS.contains(&a.as_str())).unwrap_or(false) {
        if let Some(i) = args.iter().position(|a| a == "--browser-headers") {
            args.remove(i);
            cli::set_browser_headers(true);
        }
    }
    let headless = args.first().map(|a| HEADLESS.contains(&a.as_str()) || a == "agent" || a == "mcp").unwrap_or(false);
    if headless {
        return run_headless(args);
    }
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("rabbithole: {msg}");
            ExitCode::FAILURE
        }
    }
}

/// Headless commands run on a worker thread with a large stack (deep
/// script recursion) and the process exits from the main thread without
/// tearing the worker's thread-locals down (script engine state is not
/// safe to destroy during process exit).
fn run_headless(args: Vec<String>) -> ExitCode {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("rabbithole-headless".into()).stack_size(256 << 20).spawn(move || {
        let r = run(&args);
        let _ = tx.send(r);
        loop {
            std::thread::park();
        }
    });
    if spawned.is_err() {
        eprintln!("rabbithole: cannot start the worker thread");
        return ExitCode::FAILURE;
    }
    let code = match rx.recv() {
        Ok(Ok(())) => 0,
        Ok(Err(msg)) => {
            eprintln!("rabbithole: {msg}");
            1
        }
        Err(_) => 101,
    };
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(code)
}

fn run(args: &[String]) -> Result<(), String> {
    let first = args.first().map(String::as_str);
    match first {
        None => gui::run(Vec::new()),
        Some("-h") | Some("--help") | Some("help") => {
            print!("{HELP}");
            Ok(())
        }
        Some("-V") | Some("--version") | Some("version") => {
            println!("rabbithole {VERSION}");
            Ok(())
        }
        Some("screenshot") => cli::screenshot(&args[1..]),
        Some("diff") => cli::diff(&args[1..]),
        Some("text") => cli::text(&args[1..]),
        Some("links") => cli::links(&args[1..]),
        Some("dump") => cli::dump(&args[1..]),
        Some("cookies") => cli::cookies(&args[1..]),
        Some("js") => cli::js(&args[1..]),
        Some("click") => cli::click(&args[1..]),
        Some("mcp") => mcp::serve(&args[1..]),
        Some("agent") => cli::agent(&args[1..]),
        Some("ui-bench") => gui::bench::run(&args[1..]),
        Some(url) if url.starts_with('-') => Err(format!("unknown option `{url}` (try --help)")),
        Some(_) => gui::run(args.to_vec()),
    }
}
