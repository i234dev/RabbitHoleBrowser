# Speedometer 3.1: 0.1.37 against 0.1.36 (20 November 2026)

Paired runs on 20 November 2026 (00:54–01:05 machine time), Speedometer 3.1
(`speedometer-3.1-1386415be8fe`, served locally), 3 iterations per run, all
suites, JIT on (default), one core (`taskset -c 8`), Intel Core i7-12700H under
WSL2. Both binaries are the `shot` example built with the same
alignment-normalized flags: A is the released 0.1.36 source `b657ae5`, B is
source `a9055e5` (0.1.37 code before the version bump). The order rotates every
repetition. All 16 runs completed without benchmark errors.

| Rep | A geomean (ms) | B geomean (ms) | A/B |
|---|---:|---:|---:|
| 1 | 314.82 | 320.63 | 0.982 |
| 2 | 314.93 | 314.78 | 1.000 |
| 3 | 315.88 | 313.77 | 1.007 |
| 4 | 315.32 | 315.41 | 1.000 |
| 5 | 317.94 | 326.93 | 0.973 |
| 6 | 317.10 | 317.44 | 0.999 |
| 7 | 320.61 | 314.80 | 1.018 |
| 8 | 315.56 | 316.63 | 0.997 |

Median **0.9993×**: no measurable difference overall (B faster in 3 of 8
pairs). Per-suite medians range from 0.971 (TodoMVC-JavaScript-ES6-Webpack)
and 0.977 (NewsSite-Next) to 1.043 (TodoMVC-Svelte).

Before the 0.1.37 performance commits the same comparison on six DOM-heavy
suites was 0.9815; the vertical writing-mode paths were then kept off
horizontal pages and an allocation was removed from CSS math function parsing.
