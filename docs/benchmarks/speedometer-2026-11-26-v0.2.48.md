# Speedometer 3.1: 0.2.48 against 0.2.47 (26 November 2026)

Paired runs on 26 November 2026, same method as the
[0.2.47 record](speedometer-2026-11-25-v0.2.47.md): alignment-normalized
release builds of the `shot` example, one core (CPU 8), rotated order, three
Speedometer iterations per run, started below load 2 with the shared build lock
held. A is the released 0.2.47 source `21bc106b` (sha256 `490d92cb…`), B is
the 0.2.48 integration with the 48 MiB code limit (`739c925c` content, sha256
`b1a539eb…`).

| Rep | A geomean (ms) | B geomean (ms) | A/B |
|---|---:|---:|---:|
| 1 | 193.58 | 189.30 | 1.023 |
| 2 | 194.21 | 189.01 | 1.028 |
| 3 | 201.32 | 191.45 | 1.052 |
| 4 | 195.44 | 189.07 | 1.034 |
| 5 | 195.52 | 188.99 | 1.035 |
| 6 | 194.30 | 196.58 | 0.988 |
| 7 | 194.56 | 187.84 | 1.036 |
| 8 | 194.04 | 188.35 | 1.030 |

Median **1.032×** (B faster in 7 of 8 pairs). Per-suite medians:
TodoMVC-jQuery 1.133×, TodoMVC-React-Redux 1.119×, React-Stockcharts-SVG
1.109×, TodoMVC-React 1.088×, TodoMVC-Lit 1.036×, Perf-Dashboard 1.034×,
NewsSite-Nuxt 1.027×, TodoMVC-WebComponents 1.027×, Charts-chartjs 1.023×,
TodoMVC-Angular 1.023×, TodoMVC-Preact 1.018×, Charts-observable-plot 1.013×,
TodoMVC-ES6-Webpack 1.012×, NewsSite-Next 1.010×, TodoMVC-Vue 1.008×,
TodoMVC-ES5 0.998×, Editor-TipTap 0.996×, TodoMVC-Svelte 0.995×,
Editor-CodeMirror 0.991×, TodoMVC-Backbone 0.987×.

## The code limit

The same integration with the previous 16 MiB per-thread code limit measured
0.997× (3 of 8 pairs; charts 0.906×, Perf-Dashboard 0.833×). A traced
single-iteration run showed why: 0.2.47 compiled 1,345 functions and rejected
none; the integration compiled 865 and rejected 469 for the code limit, so the
suites that run last stayed in the interpreter. Run alone (six alternating
rounds), Perf-Dashboard and both chart suites were faster with the integration
than with 0.2.47 (Perf-Dashboard 393 → 378 ms, chartjs 761 → 742 ms). With
48 MiB the traced run compiles 1,340 functions and rejects none.

## What changed

- Compiled functions call compiled functions and return to them in native
  code.
- `hasOwnProperty`, `Object.is`, `Array.isArray`, `Math.round` and `isNaN`
  are computed inline for the common cases.
- The megamorphic cache is also indexed by prototype and caches named element
  stores; cached additions find the target shape by id.
- `typeof`, for-in steps, object and closure creation and more megamorphic
  reads run in native code.
