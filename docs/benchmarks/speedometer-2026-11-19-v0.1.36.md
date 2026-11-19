# Speedometer 3.1: 0.1.36 against 0.1.35 (19 November 2026)

Paired runs on 19 November 2026 (14:38–14:57 machine time), Speedometer 3.1
(`speedometer-3.1-1386415be8fe`, served locally), 3 iterations per run, all
suites, JIT on (default), one core (`taskset -c 8`), Intel Core i7-12700H under
WSL2. Both binaries are the `shot` example built with the same
alignment-normalized flags: A is the released 0.1.35 source `6283119`, B is
source `89d437d` (0.1.36 code before the version bump). The order of A and B
rotates every repetition. All 16 runs completed without benchmark errors.

| Rep | A geomean (ms) | B geomean (ms) | A/B |
|---|---:|---:|---:|
| 1 | 317.21 | 314.36 | 1.009 |
| 2 | 319.11 | 315.99 | 1.010 |
| 3 | 319.37 | 314.55 | 1.015 |
| 4 | 319.67 | 315.50 | 1.013 |
| 5 | 317.63 | 316.95 | 1.002 |
| 6 | 320.37 | 314.52 | 1.019 |
| 7 | 318.42 | 315.97 | 1.008 |
| 8 | 317.75 | 316.30 | 1.005 |

Median **1.0095×** (B faster in 8 of 8 pairs). Per-suite medians range from
0.988 (TodoMVC-WebComponents) to 1.050 (TodoMVC-Preact); NewsSite-Next and
NewsSite-Nuxt are 1.031 and 1.033, which recovers the NewsSite cost recorded
for [0.1.35](speedometer-2026-11-19-v0.1.35.md).

## Notes on the harness

- The `shot` example of B finishes idle waits by spinning their last 1.5 ms
  (commit `04f4a32`). In 0.1.35 an image installed from the cache queued a
  `load` event that was never dispatched; the pending event made the `shot`
  loop poll without sleeping. 0.1.36 dispatches that event, so the loop now
  sleeps between benchmark steps, and on this VM each wake-up arrived
  0.3–1 ms late. This affects only the `shot` measurement tool, not the GUI
  browser.
- Earlier runs of the 0.1.36 integration were invalid: an iframe that changed
  only its URL fragment was reloaded, which made Speedometer report errors in
  about half of the runs (fixed in `8167136`; the bug also exists in 0.1.35,
  where it rarely triggered).
