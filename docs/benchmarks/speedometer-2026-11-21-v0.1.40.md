# Speedometer 3.1: 0.1.40 against 0.1.39 (21 November 2026)

Paired runs on 21 November 2026 (05:29–05:39 machine time), Speedometer 3.1
(`speedometer-3.1-1386415be8fe`, served locally), 3 iterations per run, all
suites, JIT on (default), one core (`taskset -c 8`), Intel Core i7-12700H under
WSL2. Both binaries are the `shot` example built with the same
alignment-normalized flags: A is the released 0.1.39 source `b9ffcab`, B is
source `9b4e7cc9` (0.1.40 code before the version bump). The order rotates every
repetition. All 16 runs completed without benchmark errors.

| Rep | A geomean (ms) | B geomean (ms) | A/B |
|---|---:|---:|---:|
| 1 | 288.32 | 257.01 | 1.122 |
| 2 | 284.65 | 256.20 | 1.111 |
| 3 | 284.79 | 257.96 | 1.104 |
| 4 | 285.39 | 253.67 | 1.125 |
| 5 | 285.54 | 255.10 | 1.119 |
| 6 | 286.44 | 256.13 | 1.118 |
| 7 | 286.86 | 255.65 | 1.122 |
| 8 | 285.09 | 255.63 | 1.115 |

Median **1.119×** (B faster in 8 of 8 pairs). Every suite is faster:
TodoMVC-ES6-Webpack-Complex-DOM 1.224, TodoMVC-Angular-Complex-DOM 1.222,
React-Stockcharts-SVG 1.220, Charts-chartjs 1.190, TodoMVC-ES5 1.135,
Editor-CodeMirror 1.134, Perf-Dashboard 1.132, TodoMVC-Preact 1.131,
Charts-observable-plot 1.112, Editor-TipTap 1.109, TodoMVC-jQuery 1.105,
TodoMVC-Backbone 1.104, TodoMVC-React-Redux 1.102, TodoMVC-React 1.101,
TodoMVC-Vue 1.100, NewsSite-Next 1.094, NewsSite-Nuxt 1.077, TodoMVC-Svelte
1.033, TodoMVC-WebComponents 1.024, TodoMVC-Lit 1.014.

Main causes: the JavaScript allocator prefetches its next free block, the
collector prefetches during marking and sweeping, short strings keep their
code units inline, more objects may be allocated between collections (the
bytes trigger is unchanged; collector time over three iterations fell from
5.2 s to 1.8 s); custom-property maps are layered over the parent's instead
of copied; `querySelector`/`getElementById` use attribute and class indexes.

A first measurement attempt the same morning compared 0.1.39 with a stale
binary (the build had been refused by the disk guard) and is not reported.
