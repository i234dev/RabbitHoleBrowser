# RabbitHole Browser

**A web browser written from scratch in Rust.** No Chromium, no WebKit, no Gecko,
no system webview. RabbitHole ships its own HTML parser, CSS parser and cascade,
layout engine, font rasterization and a software rasterizer, and paints the
result straight into a native window on Linux, macOS and Windows.

It can be used three ways:

- as a **desktop browser** (a native window, drawn entirely by our own code),
- as a **command line tool** (headless screenshots, text extraction, link listing, tree dumps),
- as an **MCP server** so LLM agents such as Claude can read and screenshot the web through it.

![about:home rendered by RabbitHole](docs/about-home.png)

![Wikipedia rendered by RabbitHole](docs/wikipedia.png)

_Both images were produced by `rabbithole screenshot`: every pixel comes from RabbitHole's own rasterizer._

## Status (v0.1.5)

RabbitHole is young but already runs real, heavy sites: Google (search and
sign-in), YouTube (playback with sound and AV1/VP9 video), Wikipedia, GitHub,
Reddit (infinite scroll), BBC, Hacker News, the Svelte playground (WebAssembly
bundler in a Worker) and Speedometer 3.1 end to end. Everything below is our own
code unless noted in [Dependencies](#dependencies).

### Conformance and benchmarks

| Suite | Result | Notes |
|---|---|---|
| **test262** (ECMAScript) | **98.7 %** (45,243 / 45,848 run) | 99.8 % excluding Intl; committed baseline in `crates/js/tests/test262_baseline.txt`; Temporal and other unfinished proposals skipped |
| **WPT testharness** (DOM, HTML, URL, Encoding, IndexedDB, …) | **82.1 %** (107,993 / 131,464 subtests, 2,843 pages) | committed baseline in `tools/wpt/` |
| **WPT CSS reftests** (pixel comparison) | **61.1 %** (11,983 / 19,627) | committed baseline in `crates/engine/tests/wpt-css-baseline.txt`; CSS2 79.6 %, color 79.4 %, css-ui 91.1 %, css-text-decor 75.5 %, flexbox 61.2 % |
| **WebAssembly spec testsuite** | **99.99 %** (66,747 / 66,757) | every proposal at 100 % except two outdated threads/descriptor cases |
| html5lib tree construction | 1,935 / 1,945 | |
| WHATWG URL tests | 899 / 899 (+ 278 / 278 setters) | |
| Acid2 | **0 pixels** different from the reference | |
| Acid3 | 96 / 100 | |
| Framework suite (React 18/19, Vue 3, Svelte 5, Lit 3, Preact, Solid, Alpine, htmx, jQuery) | 66 / 66 | real apps driven with simulated input |
| Speedometer 3.1 | 0.148 | headless on a loaded 3-core VM; tracked for regressions, not a fair comparison |

## Features

**Parsing & styling**

- Spec HTML tokenizer and tree builder (WHATWG parsing algorithm, fragments,
  templates, declarative shadow DOM), XML parser for XHTML/SVG documents,
  WHATWG URL parser with IDNA, the full Encoding Standard.
- CSS: selectors level 4 (`:is`/`:where`/`:has`/`:not`, `:host`/`::slotted`),
  cascade layers (`@layer`), `@media`/`@supports`/`@font-face`/
  `@keyframes`, custom properties, `calc()`/`min()`/`max()`/`clamp()` mixing
  percentages and lengths, modern colors (`oklch`, `lab`, `color-mix`,
  `light-dark`), containment, CSS transitions and animations.
- Incremental restyle and relayout (only dirty subtrees, including shadow
  trees), so pages that mutate the DOM constantly stay responsive.

**Layout & painting**

- Block, inline (with block-in-inline splitting), floats, CSS 2.1 tables,
  flexbox, grid, positioning (relative/absolute/fixed/
  sticky), stacking contexts, vertical writing modes, Unicode bidi.
- Own software rasterizer: anti-aliased paths and strokes, gradients, shadows,
  per-corner radii, 2D/3D-flattened transforms, filters and `backdrop-filter`,
  masks, opacity layers, the top layer for modal dialogs.
- Text shaping with OpenType features (ligatures, kerning, Arabic joining,
  right-to-left runs), color emoji (COLR/CPAL, CBDT, sbix), web fonts
  (TTF/OTF/WOFF/WOFF2), system font discovery.
- Form controls drawn with Chrome's look and `appearance` support; images in
  PNG, JPEG, GIF/APNG/WebP (animated) and AVIF, responsive images and lazy
  loading; own SVG renderer.

**JavaScript and WebAssembly**

- Own JavaScript engine (`crates/js`): lazy parser and compiler, register-based
  interpreter with hidden classes and inline caches, 8-byte NaN-boxed values,
  size-class allocator, mark-and-sweep GC, ES2023 standard library,
  ES modules, own regular expression engine, and a baseline x86-64 JIT with
  loop OSR (`--jit=on`, off by default while it is tuned).
- Own WebAssembly engine (`crates/wasm`): validator, interpreter and a
  baseline x86-64 compiler; the `WebAssembly` JS API in pages and workers.
- Web platform (`crates/script`): the full WebIDL surface of DOM, HTML, CSSOM,
  Shadow DOM and custom elements, events (pointer, keyboard, wheel), observers
  (Mutation/Intersection/Resize), canvas 2D, fetch/XHR with CORS, WebSocket,
  EventSource, Streams, IndexedDB, Web Crypto, Workers, iframes with
  same-origin scripting and `postMessage`, history and Navigation API.
- Experimental WebGL1: own software shader evaluator, clipping and triangle
  rasterizer, vertex/index buffers, uniforms, depth testing, RGBA8 nearest
  2D textures, canvas presentation and `readPixels`. See [supported operations
  and limits](docs/webgl.md); this is a rendering subset, not full WebGL conformance.
- Service Worker lifecycle: classic scripts run in an isolated worker realm,
  with installation/activation, `waitUntil`, updates, `skipWaiting` and
  `clients.claim`. Registrations and origin-scoped Cache API data live in memory
  within an engine instance. Fetch interception and offline navigation are pending;
  see [the lifecycle milestone](docs/service-workers.md).

**Media**

- Audio output on Windows (WASAPI), Linux (PulseAudio/PipeWire/ALSA loaded at
  run time) and macOS (CoreAudio); MP3, AAC, FLAC, Vorbis, Opus, WAV.
- `<audio>`/`<video>` and Media Source Extensions (YouTube plays), AV1 and VP9
  video in pure Rust, H.264 and HE-AAC through Windows Media Foundation, Web
  Audio basics.

**Networking & the browser**

- Own HTTP/2 (HPACK, flow control) and HTTP/1.1 over rustls, memory + disk HTTP
  cache, cookies with SameSite/partitioning, CORS, gzip/deflate/brotli/zstd,
  WebSocket and EventSource clients.
- Desktop UI drawn by the engine: tabs (pinning, drag, reopen), omnibox with
  suggestions, history, bookmarks (Netscape HTML import/export), downloads with
  pause/resume, settings, session restore, private windows, print to PDF,
  developer tools (console, elements, network), automatic handling of cookie
  consent dialogs (`about:autocookies`), crash isolation per tab.

## Limitations (for now)

- WebGL1 is experimental: limited GLSL ES 1.00 and nearest 2D textures;
  no framebuffer objects, blending, mipmaps or general shader control flow.
  WebGL2 contexts return `null`; WebGPU is unavailable. This does not establish
  Three.js compatibility.
- Service Workers do not yet intercept fetches or provide offline navigation.
  Registrations and Cache API entries are not persisted to disk; module Service Workers,
  worker messaging and client enumeration are unfinished.
- No Temporal API; Intl is partial.
- The JIT is off by default (it is not yet faster than the interpreter on
  Speedometer) and disabled on Windows until tested there.
- Speedometer score is far from Chrome's; JavaScript speed is the main gap.
- H.264/HE-AAC video needs Windows (Media Foundation).

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
`PageUp`/`PageDown`/`Space`/`Home`/`End` scroll, `Ctrl+T`/`Ctrl+W` tabs,
`Ctrl+F` find, `Ctrl+D` bookmark, `Ctrl+H` history, `Ctrl+J` downloads,
`Ctrl+Shift+J` JavaScript console, `Ctrl+±` zoom, `F11` fullscreen, `Ctrl+Q`
quit. Typing something that is not a URL searches DuckDuckGo.

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

# Evaluate JavaScript in the page after its own scripts ran
rabbithole js https://news.ycombinator.com/ 'document.querySelectorAll(".titleline").length'

# Every page command accepts --no-js, --wait <ms> and --verbose
rabbithole text https://example.com --no-js

rabbithole --help
```

### MCP server (Model Context Protocol)

`rabbithole mcp` speaks JSON-RPC 2.0 over stdio and exposes a browser built
for agents (see [docs/AI_INTERFACE.md](docs/AI_INTERFACE.md) for the full
specification): a compact semantic snapshot with stable refs (`@e12`) instead
of HTML or screenshots, actions by ref that auto-wait and return a diff of
what changed, read mode, forms as a unit, network log with API hints, events
for dialogs/popups/downloads, iframes and shadow DOM flattened, hidden text
removed, page content fenced against prompt injection.

| Tool         | Purpose                                                              |
|--------------|----------------------------------------------------------------------|
| `open`       | Load a URL (or back/forward/reload) and return a snapshot             |
| `snapshot`   | Semantic tree with refs; expand one element; token budget             |
| `read`       | Main content as Markdown, links as refs                               |
| `find`       | Elements by text, role/name or CSS selector                           |
| `click`      | Click/hover by ref; `ref="cookies"` rejects a cookie banner           |
| `type`       | Type text with key events, press keys                                 |
| `fill`       | List forms, fill fields by label/name, validate and submit            |
| `scroll`     | Scroll, scroll until visible, load more                               |
| `screenshot` | PNG with refs drawn over interactive elements (on demand)             |
| `network`    | Requests, JSON response bodies, API hints                             |
| `tabs`       | Tabs/popups, contexts, downloads, session save/load                   |
| `debug`      | eval, console/errors, action log export, live view                    |

The same tools work from a shell: `rabbithole agent -c "open https://example.com" -c "read"`
or interactively (`rabbithole agent`). The MCP server, the headless
subcommands and the agent API send the same standard browser headers as the
GUI by default; `--bot` (or `open bot=true`) switches to the honest
`RabbitHoleBot` identity, which also respects `robots.txt`.

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

| Crate              | Role |
|--------------------|------|
| `crates/dom`       | Arena DOM, mutation log, top layer. |
| `crates/html`      | Spec HTML tokenizer/tree builder, XML parser, entities. |
| `crates/css`       | CSS tokenizer, selectors, stylesheets, media queries. |
| `crates/style`     | UA stylesheet, cascade (with layers), computed values, incremental restyle. |
| `crates/layout`    | Box tree, block/inline/flex/grid/table layout, display list, incremental relayout. |
| `crates/gfx`       | Software rasterizer, fonts and shaping, image decoding, canvas 2D backend. |
| `crates/webgl`     | Experimental own software WebGL shader evaluator, clipping, rasterization and texture sampling. |
| `crates/svg`       | SVG parsing and rendering. |
| `crates/net`       | URL, HTTP/2 + HTTP/1.1, cache, cookies, CORS, WebSocket, EventSource. |
| `crates/js`        | JavaScript engine: parser, compiler, interpreter, GC, builtins, regex, JIT front-end. |
| `crates/jit`       | Shared W^X executable memory and x86-64 assembler (JS and wasm JITs). |
| `crates/wasm`      | WebAssembly decoder, validator, interpreter and baseline compiler. |
| `crates/wasm-js`   | The `WebAssembly` JavaScript API. |
| `crates/script`    | Web platform bindings for the JS engine (DOM, CSSOM, events, workers, …). |
| `crates/media`     | Audio output, demuxers/decoders, media elements, MSE. |
| `crates/engine`    | Glue: load, parse, style, layout, paint, scripting, frames, input, pixel diff. |
| `crates/agent`     | Agent interface: semantic snapshots, refs, actions, read mode, tools. |
| `crates/wpt`       | Web Platform Tests runner (`rh-wpt`). |
| `app`              | The `rabbithole` binary: GUI (winit + softbuffer), CLI and MCP server. |

Pipeline: `net` fetches bytes → `html` builds a `dom::Document` → `css` parses
stylesheets → `style` computes a `ComputedStyle` per element → `layout`
positions boxes and emits a display list → `gfx` rasterizes it → the app blits
the canvas into the window (or a PNG, or an MCP response).

Threads in the desktop browser: the UI thread owns the window, the chrome
(tabs, address bar, menus, find bar, console) and presents frames; every tab's
engine and page live on a content thread of its own (`app/src/gui/content`),
fed with input/navigation messages and answering with rasterized frames,
title, URL, cursor, console lines and navigation requests. Frames are painted
taller than the viewport, so scrolling is instant even while a page runs a
long script; a page busy for more than a few seconds gets a "not responding"
bar (wait / stop) instead of freezing the window. Pages are shown as soon as
the document and CSS are ready; images and scripts arrive afterwards.
`RH_UI_STATS=1` logs UI-thread stalls, Ctrl+Shift+U shows them on screen, and
`rabbithole ui-bench <URL> [--legacy]` measures them headlessly.

## Dependencies

External crates are limited to platform plumbing and codecs, not browser
functionality: `winit` (windows/input), `softbuffer` (presenting pixels),
`rustls`/`ring` (TLS), `image`, `zune-jpeg`, `rav1d`/`avif-parse` and
`moxcms` (image decoding and color management), `rustybuzz` and `fontdue`
(OpenType shaping and glyph outlines), `symphonia`, `opus-decoder` and
`rusty_vp9` (audio/video decoding), compression crates (`flate2`,
`brotli-decompressor`, `ruzstd`), and `serde_json` for MCP messages.

## Roadmap

See [the development plan](docs/ROADMAP.md) for the next phases and their
validation criteria, and [the JIT record](docs/jit-design.md) for implementation
details and measurements.

- JavaScript speed: make the JIT a net win and turn it on; cheaper calls,
  closures and generators.
- WPT DOM and CSS pass rates up (CSSOM view, `document.write`, speculative
  parsing, text, masking); Intl and Temporal later.
- Extend experimental WebGL1 with tested shader and rendering capabilities;
  design a coherent WebGL2 milestone before exposing a WebGL2 context.
- Service Worker fetch interception, offline navigation and persistent storage;
  Temporal and full Intl later.
- macOS and Linux packaging; JIT enabled on Windows after real-hardware testing.

## License

MIT — see [LICENSE](LICENSE). Bundled DejaVu fonts are distributed under their
own license, see `assets/fonts/LICENSE-DejaVu.txt`.
