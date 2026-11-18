# Page loads: lazy fonts and screened font fallback

Measured on **18 November 2026** (15:15–15:26 UTC). Base: source `9819455`
(0.1.30). Candidate: the base plus the font changes released in 0.1.31. The
[JSON record](page-load-2026-11-18-fonts.json) holds the totals and every
page's median.

## What changed

- The seven bundled fonts were parsed when the engine started; fontdue builds
  every glyph outline while parsing. They are now parsed on first use.
- Looking for a fallback font for a character loaded each candidate font in
  full just to ask whether it maps the character. Candidates are now checked
  in their character map first, and only a font that maps it is loaded.

## Benchmark

[`tools/bench/page_load.py`](../../tools/bench/page_load.py) loads each of the
32 Speedometer 3.1 workload pages (TodoMVC in every framework, the NewsSite
apps, the editors and the charts) from a local copy, in a fresh `shot` process
per load, with both builds in alternating order, four rounds (128 paired
loads). Times include engine start-up, fetching, parsing, scripts, style,
layout and the first paint of a 1280×900 viewport.

## Result

| Phase | Base total | Candidate total | Ratio |
|---|---:|---:|---:|
| **Whole load (wall)** | 53.6 s | **39.7 s** | **1.35×** (128 of 128 loads faster) |
| Engine start-up (fonts) | 16.3 s | 0.4 s | 43× |
| Build page (parse, scripts, style) | 17.4 s | 21.7 s | 0.80× |
| Load and layout | 27.5 s | 31.5 s | 0.87× |
| First paint | 4.5 s | 3.5 s | 1.28× |

Fonts are now parsed when a page first uses them, so part of the start-up cost
moves into building and laying out the page; the whole load is still about
110 ms shorter per page. NewsSite-Next loads in a median 332 ms instead of
581 ms.

## Conditions and correctness

Identically built binaries (release, line tables, frame pointers, 64-byte
alignment), Linux x86-64 under WSL2 on an Intel Core i7-12700H with its
installed system fonts, CPU 0, one load at a time. The gfx tests
`fallback_faces_map_their_characters` (the chosen face maps the character and
the choice does not depend on which faces were loaded) and
`bundled_faces_load_lazily_and_parse` cover the changes; gfx, layout and engine
tests passed (574).
