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
    rabbithole text <URL>                  Print the readable text of a page
    rabbithole links <URL>                 Print all links as `text<TAB>href`
    rabbithole dump <URL> [--dom] [--layout]
                                           Print the DOM and/or layout tree
    rabbithole cookies <URL>               Print the stored cookies that would be sent to URL
    rabbithole js <URL> <EXPRESSION>       Evaluate JavaScript in the loaded page and print the result
    rabbithole click <URL> <X> <Y>         Show what is under a viewport point and click it like a mouse
                                           (--scroll Y, --width/--height, --hit-only)
    rabbithole mcp                         Run as a Model Context Protocol server on stdio
    rabbithole --help | --version

SCREENSHOT OPTIONS:
    -o, --output <FILE>   Output PNG path (default: screenshot.png)
    --width <PX>          Viewport width in CSS px (default: 1280)
    --height <PX>         Viewport height in CSS px (default: 800)
    --scale <F>           Device scale factor, e.g. 2 for HiDPI (default: 1)
    --full                Capture the whole page height (max 20000px)
    --scroll <PX>         Vertical scroll offset before capturing
    --no-images           Do not download images

COMMON OPTIONS (screenshot, text, links, dump, js):
    --no-js               Do not run page JavaScript
    --wait <MS>           Let timers run this long after load (default 1000 with JS)
    --verbose             Print the page's console output to stderr

The profile (cookies, history, bookmarks, session) lives in the platform
config directory under `rabbithole/` (override with RABBITHOLE_PROFILE).
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("rabbithole: {msg}");
            ExitCode::FAILURE
        }
    }
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
        Some("text") => cli::text(&args[1..]),
        Some("links") => cli::links(&args[1..]),
        Some("dump") => cli::dump(&args[1..]),
        Some("cookies") => cli::cookies(&args[1..]),
        Some("js") => cli::js(&args[1..]),
        Some("click") => cli::click(&args[1..]),
        Some("mcp") => mcp::serve(),
        Some(url) if url.starts_with('-') => Err(format!("unknown option `{url}` (try --help)")),
        Some(_) => gui::run(args.to_vec()),
    }
}
