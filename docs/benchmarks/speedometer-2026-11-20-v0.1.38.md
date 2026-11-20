# Speedometer 3.1: 0.1.38 against 0.1.37 (20 November 2026)

Paired runs on 20 November 2026 (15:18–15:28 machine time), Speedometer 3.1
(`speedometer-3.1-1386415be8fe`, served locally), 3 iterations per run, all
suites, JIT on (default), one core (`taskset -c 8`), Intel Core i7-12700H under
WSL2. Both binaries are the `shot` example built with the same
alignment-normalized flags: A is the released 0.1.37 source `30bd9a9`, B is
source `63bc7d26` (0.1.38 code before the version bump). The order rotates every
repetition. All 16 runs completed without benchmark errors.

| Rep | A geomean (ms) | B geomean (ms) | A/B |
|---|---:|---:|---:|
| 1 | 333.62 | 293.34 | 1.137 |
| 2 | 333.25 | 295.20 | 1.129 |
| 3 | 333.80 | 289.62 | 1.153 |
| 4 | 333.00 | 296.34 | 1.124 |
| 5 | 333.39 | 294.49 | 1.132 |
| 6 | 334.54 | 295.19 | 1.133 |
| 7 | 333.74 | 295.48 | 1.129 |
| 8 | 336.30 | 293.36 | 1.146 |

Median **1.133×** (B faster in 8 of 8 pairs).

| Suite | A/B | | Suite | A/B |
|---|---:|---|---|---:|
| NewsSite-Nuxt | 1.914 | | TodoMVC-JavaScript-ES5 | 1.053 |
| NewsSite-Next | 1.816 | | Editor-TipTap | 1.044 |
| TodoMVC-JavaScript-ES6-Webpack-Complex-DOM | 1.480 | | TodoMVC-React-Complex-DOM | 1.036 |
| TodoMVC-Svelte-Complex-DOM | 1.331 | | TodoMVC-Vue | 1.032 |
| TodoMVC-Preact-Complex-DOM | 1.226 | | TodoMVC-Angular-Complex-DOM | 1.031 |
| TodoMVC-Lit-Complex-DOM | 1.224 | | Perf-Dashboard | 1.020 |
| TodoMVC-WebComponents | 1.094 | | other suites | 0.980–1.007 |
| Editor-CodeMirror | 0.959 | | | |

Editor-CodeMirror measured 1.004 in an earlier run of nearly the same code
(before a grid layout-cache fix) and is listed as measured.

Main causes: background images that finish loading repaint instead of
relaying out the page; layout-cache entries made while measuring flex,
table and intrinsic sizes now serve the final layout (not inside grids);
stale static positions no longer break layout fingerprints; the DOM mutation
log no longer overflows into full restyles on large pages; innerHTML moves
parsed nodes instead of cloning them; attribute selectors match without
allocating.
