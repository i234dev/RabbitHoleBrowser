# Speedometer 3.1: cached box-shadow blur

Measured on **18 November 2026** (05:54–06:08 UTC). Base: `89f489a` plus the
changes released in 0.1.23–0.1.27. Candidate: the base plus this change,
released in 0.1.28. The [JSON record](speedometer-2026-11-18-shadow-cache.json)
holds every pair.

## What changed

Profiling showed that repainting the suites' frames took 16 % of the CPU
during the run, and 68 % of that was blurring outset box shadows again on
every paint. The blurred mask of a shadow depends only on its geometry; it
is now cached per thread, keyed by the exact bits of that geometry (shadow
and border boxes, radii, blur radius and the clipped region), and reused when
the box is repainted in place. The cache holds up to 32 MiB per thread.

## Result

| Statistic | Result |
|---|---:|
| Rounds with the candidate faster | **8 / 8** |
| Median paired ratio | **1.218×** |
| Range of paired ratios | 1.185–1.233× |
| Mean score, base / candidate | 2.57 / 3.13 |

Every TodoMVC suite improved (1.08×–1.86×: WebComponents 1.86×, Svelte
1.75×, Preact 1.72×, Backbone 1.56×, Vue 1.49×, ES5 1.48×, Lit 1.38×); the
TodoMVC app has a large blurred shadow on its main container. Other suites
were unchanged (±1.2 %).

## Conditions and correctness

Same method as the [0.1.23 measurement](speedometer-2026-11-18-layout-cache.md).
The unit test `repeated_box_shadows_match_first_paint` checks that a repaint
from the cache is identical and that a shadow moved by one pixel equals the
original shifted; gfx, engine and layout tests passed (568).
