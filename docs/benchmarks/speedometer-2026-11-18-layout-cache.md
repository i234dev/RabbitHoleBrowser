# Speedometer 3.1: layout cache for measured and stretched flex items

Measured on **18 November 2026** (02:31–02:51 UTC). Base: source
`89f489af94e236302a0b6d5a69df9198655d6508` (0.1.21). Candidate: the same
source plus the layout cache change released in 0.1.23. The
[JSON record](speedometer-2026-11-18-layout-cache.json) holds every pair and
per-suite ratio.

## What changed

A flex item is laid out once without a definite height (to measure it) and
again stretched to its line's height. The layout cache kept a single entry
per box, so both passes missed on every relayout. When the second pass places
every descendant exactly as the first (compared by a fingerprint of the
contents' geometry, content height, margins and floats), the cache entry now
serves both heights. Profiling NewsSite-Next showed 90 % of its cache misses
came from this alternation.

## Result

| Statistic | Result |
|---|---:|
| Rounds with the candidate faster | **8 / 8** |
| Median paired ratio (base geomean / candidate geomean) | **1.056×** |
| Range of paired ratios | 1.044–1.061× |
| Mean score, base / candidate | 2.120 / 2.234 |
| NewsSite-Next / NewsSite-Nuxt (median ratio) | **1.70× / 1.72×** |

The other 18 suites stay within ±1.3 %.

## Conditions

- Speedometer 3.1 at suite commit `1386415be8fe`, all 20 default suites,
  three iterations per run, served from a local copy; JIT on (default).
- Headless `shot` builds of both sources with identical flags: release, line
  tables, frame pointers, and every function and branch target aligned to
  64 bytes, so code placement cannot favour either binary.
- Linux x86-64 under WSL2 on an Intel Core i7-12700H, pinned to CPU 0, one
  run at a time, base/candidate order rotated per round.

## Correctness

A complete Speedometer run with `RH_VERIFY_INCREMENTAL=1`, which compares
every incremental layout with a full layout, found no difference. The
regression test `stretched_flex_items_are_reused_from_the_layout_cache`
fails without the change (240 misses) and passes with it.
