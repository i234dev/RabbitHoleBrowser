# Speedometer 3.1: 0.1.39 against 0.1.38 (20 November 2026)

Paired runs on 20 November 2026 (19:41–19:51 machine time), Speedometer 3.1
(`speedometer-3.1-1386415be8fe`, served locally), 3 iterations per run, all
suites, JIT on (default), one core (`taskset -c 8`), Intel Core i7-12700H under
WSL2. Both binaries are the `shot` example built with the same
alignment-normalized flags: A is the released 0.1.38 source `bdff454`, B is
source `7c9af779` (0.1.39 code before the version bump). The order rotates every
repetition. All 16 runs completed without benchmark errors.

| Rep | A geomean (ms) | B geomean (ms) | A/B |
|---|---:|---:|---:|
| 1 | 279.51 | 276.83 | 1.010 |
| 2 | 273.84 | 276.04 | 0.992 |
| 3 | 274.08 | 273.85 | 1.001 |
| 4 | 274.15 | 275.25 | 0.996 |
| 5 | 274.02 | 273.35 | 1.002 |
| 6 | 273.92 | 276.42 | 0.991 |
| 7 | 277.04 | 274.96 | 1.008 |
| 8 | 274.77 | 274.71 | 1.000 |

Median **1.0005×** (B faster in 5 of 8 pairs): no measurable difference.
Per-suite medians range from 0.983 (TodoMVC-Vue, TodoMVC-JavaScript-ES5) to
1.013 (TodoMVC-Angular-Complex-DOM).

A first version of the `DOMTokenList` indexed getter, a separate object kind,
measured 0.9906 on six DOM-heavy suites; it was replaced by a per-object flag
on ordinary host objects before this measurement.
