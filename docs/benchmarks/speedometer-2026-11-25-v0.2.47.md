# Speedometer 3.1: 0.2.47 against 0.2.46 (25 November 2026)

Paired runs on 25 November 2026 (22:34–22:43 machine time, Europe/Madrid),
same method as the [0.2.46 record](speedometer-2026-11-25-v0.2.46.md):
alignment-normalized release builds of the `shot` example, one core (CPU 8),
rotated order, three Speedometer iterations per run, started below load 2 with
the shared build lock held. A is the released 0.2.46 source `191c5eb4`
(sha256 `454bd003…`), B is the 0.2.47 integration `a74f87d9` (sha256
`01af86f0…`). The delivery adds only `149890ae`, which allocates the
profiler's sample buffers when profiling starts instead of reserving them
statically; it does not run when profiling is off.

| Rep | A geomean (ms) | B geomean (ms) | A/B |
|---|---:|---:|---:|
| 1 | 236.10 | 205.07 | 1.151 |
| 2 | 236.04 | 195.39 | 1.208 |
| 3 | 236.50 | 203.49 | 1.162 |
| 4 | 233.70 | 200.37 | 1.166 |
| 5 | 238.35 | 194.11 | 1.228 |
| 6 | 236.33 | 203.34 | 1.162 |
| 7 | 231.87 | 203.24 | 1.141 |
| 8 | 234.99 | 203.15 | 1.157 |

Median **1.162×** (B faster in 8 of 8 pairs). Per-suite medians:
Charts-observable-plot 1.544×, Perf-Dashboard 1.377×, TodoMVC-Svelte 1.327×,
TodoMVC-WebComponents 1.325×, Charts-chartjs 1.313×, TodoMVC-Lit 1.263×,
Editor-TipTap and TodoMVC-Preact 1.245×, NewsSite-Nuxt 1.170×, NewsSite-Next
1.161×, TodoMVC-ES6-Webpack 1.138×, TodoMVC-Backbone 1.113×, TodoMVC-ES5
1.094×, Editor-CodeMirror 1.082×, TodoMVC-Vue 1.076×, TodoMVC-React 1.058×,
React-Stockcharts-SVG 1.039×, TodoMVC-Angular and TodoMVC-React-Redux 1.025×,
TodoMVC-jQuery 0.999×.

## What changed

- Child frames, inline SVG and canvas 2D are rasterized when the page is
  presented instead of inside the measured script work; canvas 2D records its
  drawing and executes it when pixels are needed.
- Text shaping reuses faces and shape plans; page scripts compiled once are
  shared between realms; young frame loads are polled every 4 ms.
- Ordinal list markers are renumbered only after a changed item.

## Network and loading (not measured by Speedometer)

Measured on 25 November on TheOtherSide, medians of five interleaved runs,
0.2.46 tree against the network branch:

| Page or case | Before | After |
|---|---:|---:|
| GitHub repository page, load + layout | 2,302 ms | 959 ms |
| Wikipedia article ("United States"), load + layout | 1,621 ms | 986 ms |
| YouTube home, load + layout | 1,976 ms | 1,644 ms |
| 10 `sendBeacon` calls (time blocked) | 2,029 ms | 1 ms |
| 40-module import graph, 50 ms per request | 2,103 ms | 421 ms |
| Cache API, 100 entries: `match` / `put` | 823 / 467 ms | 4 / 5 ms |
| Trivial iframe | 41.9 ms | 17.6 ms |
