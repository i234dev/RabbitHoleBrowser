# Speedometer 3.1: 0.1.42 against 0.1.41 (21 November 2026)

Paired runs on 21 November 2026 (12:41–12:48 machine time), Speedometer 3.1
(`speedometer-3.1-1386415be8fe`, served locally), 3 iterations per run, all
suites, JIT on (default), one core (`taskset -c 8`), Intel Core i7-12700H under
WSL2, started when the one-minute load was below 2. Both binaries are the
`shot` example built with the same alignment-normalized flags: A is the
released 0.1.41 source `64ae951` (sha256 `8c321dd0…`), B is source `3bb98823`
(0.1.42 code before the version bump, sha256 `74f3df1e…`). The order rotates
every repetition. All 16 runs completed without benchmark errors.

| Rep | A geomean (ms) | B geomean (ms) | A/B |
|---|---:|---:|---:|
| 1 | 244.51 | 236.93 | 1.032 |
| 2 | 242.52 | 234.63 | 1.034 |
| 3 | 245.42 | 237.14 | 1.035 |
| 4 | 244.26 | 240.32 | 1.016 |
| 5 | 244.01 | 237.62 | 1.027 |
| 6 | 245.66 | 240.08 | 1.023 |
| 7 | 248.65 | 238.85 | 1.041 |
| 8 | 244.99 | 236.18 | 1.037 |

Median **1.033×** (B faster in 8 of 8 pairs). Per suite: TodoMVC-React-Complex
1.099, TodoMVC-React-Redux 1.076, Perf-Dashboard 1.074, React-Stockcharts-SVG
1.056, Charts-chartjs 1.052, NewsSite-Nuxt 1.052, Charts-observable-plot
1.051, NewsSite-Next 1.050, TodoMVC-Vue 1.038, TodoMVC-Angular 1.036; the
other suites 0.994–1.019.

Main causes: the baseline JIT handles more property-cache kinds in native
code (two-level prototype, string primitives, array length, absent
properties, an inline probe of the megamorphic cache) and strict equality
inline; the interpreter now records absent-property caches for objects whose
prototype lacks the key; symbol-keyed element reads are cached; `Heap::get`
is inlined.
