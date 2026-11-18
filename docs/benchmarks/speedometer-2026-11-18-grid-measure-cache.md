# Speedometer 3.1: measuring layouts reuse published grid layouts

Measured on **18 November 2026** (03:07–03:24 UTC). Base: source
`89f489a` plus the stretched flex item layout cache released in 0.1.23.
Candidate: the base plus this change, released in 0.1.24. The
[JSON record](speedometer-2026-11-18-grid-measure-cache.json) holds every
pair and per-suite ratio.

## What changed

Grid items are first measured without publishing their used grid tracks (the
values `getComputedStyle` reports) and then laid out publishing them. The
layout cache required both to match, so measurement and final layout
invalidated each other on every relayout. The geometry is the same either
way: a measuring layout now reuses an entry that also published the tracks.
The reverse is still a miss, so published tracks are never missing.

## Result

| Statistic | Result |
|---|---:|
| Rounds with the candidate faster | **8 / 8** |
| Median paired ratio (base geomean / candidate geomean) | **1.068×** |
| Range of paired ratios | 1.065–1.163× |
| Mean score, base / candidate | 2.206 / 2.379 |
| NewsSite-Next / NewsSite-Nuxt | 1.56× / 1.57× |
| Editor-TipTap / TodoMVC-Svelte-Complex-DOM | 1.15× / 1.09× |

No suite was more than 0.8 % slower.

## Conditions

Same method as the [0.1.23 measurement](speedometer-2026-11-18-layout-cache.md):
complete Speedometer 3.1 (`1386415be8fe`), JIT on, identical build flags with
every function and branch target aligned to 64 bytes, Linux x86-64 under WSL2
on an Intel Core i7-12700H, CPU 0, one run at a time, order rotated.

## Correctness

A complete Speedometer run with `RH_VERIFY_INCREMENTAL=1` found no
difference from full layouts. The regression test
`measured_grid_items_reuse_published_layouts` has 0 cache hits without the
change and 240 with it.
