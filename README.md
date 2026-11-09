# RabbitHole Browser

**A web browser written from scratch in Rust.** No Chromium, no WebKit, no Gecko,
no system webview. RabbitHole ships its own HTML parser, CSS parser and cascade,
layout engine, font rasterization and a software rasterizer, and paints the
result straight into a native window on Linux, macOS and Windows.

It can be used three ways:

- as a **desktop browser** (a native window, drawn entirely by our own code),
- as a **command line tool** (headless screenshots, text extraction, link listing, tree dumps),
- as an **MCP server** so LLM agents such as Claude can read and screenshot the web through it.

> Screenshot: _coming soon_ (`rabbithole screenshot https://example.com -o docs/example.png`).

## Features

- HTML5-style tokenizer and forgiving tree builder (implied tags, void elements,
  raw text elements, character references, inline SVG/MathML nesting).
- CSS parser: selectors (descendant/child/sibling combinators, attribute
  operators, `:nth-child`, `:not`, `:is`), specificity, `!important`, `@media`
  queries (width/height/orientation), `@import`, `@supports`.
- Cascade and inheritance producing computed styles, with a built-in
  user-agent stylesheet and presentational attribute support.
- Block and inline layout: margins (with collapsing), padding, borders,
  `inline-block`, line breaking, text alignment, lists with markers, images.
- Software rasterizer with anti-aliased glyphs, rounded corners, borders,
  image scaling and clipping. Bundled DejaVu fonts make rendering identical on
  every platform.
- Networking: own URL parser/resolver, HTTP/HTTPS with pure-Rust TLS (rustls),
  gzip, redirects, `file:`, `data:` and `about:` URLs, charset detection.
- Browser chrome drawn by the engine itself: back/forward/reload/home, URL bar
  with search fallback, status bar, scrollbar, link hover and keyboard navigation.

## Limitations (for now)

- **No JavaScript.** Pages render as the server sends them.
- CSS coverage is partial: no grid; flexbox, floats, tables and positioning are
  simplified (absolute/fixed elements are not rendered).
- No complex text shaping (Latin/Cyrillic/Greek etc. render fine; Arabic and
  Indic scripts do not), no emoji font.
- No tabs, bookmarks, cookies or form submission yet.

## Building

Requires a recent stable Rust toolchain.

```sh
cargo build --release
./target/release/rabbithole
```

The GUI uses `winit` and `softbuffer`; on Linux you need an X11 or Wayland
session (the usual `libxkbcommon`/`wayland` client libraries are loaded at
runtime). The headless subcommands and the MCP server do not need a display.

Machines without a system C toolchain (no `gcc`/`cc`) can build with Zig as
the linker: see `scripts/dev-env.sh`.

## Usage

### Desktop browser

```sh
rabbithole                       # opens about:home
rabbithole https://example.com
```

Keyboard: `Ctrl+L`/`F6` focus the URL bar, `Enter` navigate, `Esc` cancel,
`F5`/`Ctrl+R` reload, `Alt+←`/`Alt+→` or `Backspace` history, arrows /
`PageUp`/`PageDown`/`Space`/`Home`/`End` scroll, `Ctrl+Q` quit. Typing
something that is not a URL searches DuckDuckGo.

### Command line

```sh
# Headless screenshot (PNG)
rabbithole screenshot https://example.com -o example.png
rabbithole screenshot https://example.com --width 1024 --height 768 --full
rabbithole screenshot https://example.com --scroll 600 --no-images

# Readable text of a page (Markdown-ish)
rabbithole text https://example.com

# All links as "text<TAB>href"
rabbithole links https://example.com

# Parsed DOM and/or layout tree
rabbithole dump https://example.com --dom
rabbithole dump https://example.com --layout

rabbithole --help
```

### MCP server (Model Context Protocol)

`rabbithole mcp` speaks JSON-RPC 2.0 over stdio and exposes these tools:

| Tool         | Arguments                                            | Returns                                   |
|--------------|------------------------------------------------------|-------------------------------------------|
| `browse`     | `url`, `max_chars?`, `images?`                       | title, final URL and the page text        |
| `links`      | `url`, `filter?`                                     | `text — href` lines                       |
| `screenshot` | `url`, `width?`, `height?`, `full_page?`, `scroll_y?`| a PNG image plus a caption                |
| `fetch_html` | `url`, `max_chars?`                                  | raw decoded HTML source                   |
| `dom`        | `url`, `max_chars?`                                  | the parsed DOM tree                       |

Claude Code:

```sh
claude mcp add rabbithole -- /path/to/rabbithole mcp
```

Claude Desktop (or any MCP client), in its `mcpServers` configuration:

```json
{
  "mcpServers": {
    "rabbithole": {
      "command": "/path/to/rabbithole",
      "args": ["mcp"]
    }
  }
}
```

## Architecture

A Cargo workspace; each stage of the pipeline is its own crate:

| Crate            | Role                                                                 |
|------------------|----------------------------------------------------------------------|
| `crates/dom`     | Arena-based DOM (nodes, elements, attributes, traversal).            |
| `crates/html`    | HTML tokenizer, entity decoding and tree builder.                    |
| `crates/css`     | CSS tokenizer, selector parser/matcher, stylesheet and media queries.|
| `crates/style`   | User-agent stylesheet, value parsing, cascade and computed styles.   |
| `crates/gfx`     | Canvas + software rasterizer, fonts and glyph cache, image decoding. |
| `crates/net`     | URL parsing/resolution, fetching (HTTP(S)/file/data/about), charsets.|
| `crates/layout`  | Box tree, block/inline layout, display list, painting.               |
| `crates/engine`  | Glue: load → parse → style → layout → paint, hit testing, text dump. |
| `app`            | The `rabbithole` binary: GUI (winit + softbuffer), CLI and MCP server.|

Pipeline: `net` fetches bytes → `html` builds a `dom::Document` → `css` parses
stylesheets → `style` computes a `ComputedStyle` per element → `layout`
positions boxes and emits a display list → `gfx` rasterizes it → the app blits
the canvas into the window (or a PNG, or an MCP response).

External crates are limited to platform plumbing, not browser functionality:
`winit` (windows/input), `softbuffer` (presenting pixels), `fontdue` (TrueType
glyph rasterization), `image` (PNG/JPEG/GIF/WebP decoding), `ureq` + `rustls`
(HTTP and TLS), `serde_json` (MCP messages).

## Roadmap

- Floats, proper tables and a real flexbox implementation.
- Absolute/fixed positioning and `z-index` stacking.
- Background images, gradients, `border-radius` per corner, box shadows.
- Text selection, find in page, zoom, tabs, bookmarks and history persistence.
- Cookies and form submission (GET/POST).
- Incremental/asynchronous loading and image decoding off the main thread.
- A JavaScript engine (the long way down the rabbit hole).

## License

MIT — see [LICENSE](LICENSE). Bundled DejaVu fonts are distributed under their
own license, see `assets/fonts/LICENSE-DejaVu.txt`.
