# Speedometer 3.1: 0.1.41 against 0.1.40 (21 November 2026)

Paired runs on 21 November 2026 (09:49–09:57 machine time), Speedometer 3.1
(`speedometer-3.1-1386415be8fe`, served locally), 3 iterations per run, all
suites, JIT on (default), one core (`taskset -c 8`), Intel Core i7-12700H under
WSL2. Both binaries are the `shot` example built with the same
alignment-normalized flags: A is the released 0.1.40 source `ee319ae`
(sha256 `4e30148b…`), B is source `0a940094` (0.1.41 code before the version
bump, sha256 `dfe13834…`). The order rotates every repetition. All 16 runs
completed without benchmark errors.

| Rep | A geomean (ms) | B geomean (ms) | A/B |
|---|---:|---:|---:|
| 1 | 258.46 | 241.55 | 1.070 |
| 2 | 257.36 | 240.10 | 1.072 |
| 3 | 255.44 | 241.59 | 1.057 |
| 4 | 257.34 | 239.44 | 1.075 |
| 5 | 255.51 | 239.94 | 1.065 |
| 6 | 256.48 | 239.64 | 1.070 |
| 7 | 255.21 | 239.07 | 1.068 |
| 8 | 256.73 | 240.97 | 1.065 |

Median **1.069×** (B faster in 8 of 8 pairs). Every suite is faster:
NewsSite-Next 1.230, NewsSite-Nuxt 1.157, TodoMVC-Angular-Complex-DOM 1.140,
React-Stockcharts-SVG 1.083, TodoMVC-ES6-Webpack-Complex-DOM 1.082, Lit 1.072,
Backbone 1.068, React-Complex 1.062, Preact 1.059, React-Redux 1.059,
WebComponents 1.056, Perf-Dashboard 1.053, Charts-chartjs 1.049, ES5 1.045,
Vue 1.034, Charts-observable-plot 1.027, Editor-CodeMirror 1.017,
TodoMVC-jQuery 1.010, Svelte 1.009, Editor-TipTap 1.007.

Main causes: plain functions create their `prototype` object on first use,
a call's scope and slots share one allocation, an intern cache sits in front
of the string table; built-in Web API scripts are compiled once per thread and
copied for each frame realm (realm setup of later frames 84 → 30 ms), the
style-sharing cache persists across restyles, and resource discovery skips
declarations that cannot reference anything.
