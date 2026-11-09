//! RabbitHole Browser — a web browser written from scratch.
//!
//! Entry point: hand-rolled argument parsing dispatching to the GUI, the
//! headless CLI subcommands or the MCP server.

mod cli;
mod gui;
mod mcp;

use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const HELP: &str = "RabbitHole Browser — a web browser written from scratch (no Chromium/WebKit/Gecko)

USAGE:
    rabbithole [URL]                       Open the graphical browser (default: about:home)
    rabbithole screenshot <URL> [options]  Render a page headlessly to a PNG
    rabbithole text <URL> [--no-images]    Print the readable text of a page
    rabbithole links <URL>                 Print all links as `text<TAB>href`
    rabbithole dump <URL> [--dom] [--layout]
                                           Print the DOM and/or layout tree
    rabbithole mcp                         Run as a Model Context Protocol server on stdio
    rabbithole --help | --version

SCREENSHOT OPTIONS:
    -o, --output <FILE>   Output PNG path (default: screenshot.png)
    --width <PX>          Viewport width (default: 1280)
    --height <PX>         Viewport height (default: 800)
    --full                Capture the whole page height (max 20000px)
    --scroll <PX>         Vertical scroll offset before capturing
    --no-images           Do not download images
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
        None => gui::run("about:home".to_string()),
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
        Some("mcp") => mcp::serve(),
        Some(url) if url.starts_with('-') => Err(format!("unknown option `{url}` (try --help)")),
        Some(url) => gui::run(url.to_string()),
    }
}
