# Speedometer 3.1: memoized layout fingerprints

Measured on **18 November 2026**. Base: `89f489a` plus the layout cache
changes released in 0.1.23–0.1.25. Candidate: the base plus this change,
released in 0.1.26. The [JSON record](speedometer-2026-11-18-fingerprint-memo.json)
holds every pair.

## What changed

Each box keeps the fingerprint of its contents' geometry, relative to its
unscrolled content origin, until its contents are laid out again or moved
relative to it. A parent's fingerprint combines its children's positions with
their memoized fingerprints, so nested boxes no longer rehash whole subtrees.

## Result

| Statistic | Result |
|---|---:|
| Rounds with the candidate faster | **8 / 8** |
| Median paired ratio | **1.028×** |
| Range of paired ratios | 1.018–1.036× |
| NewsSite-Next / NewsSite-Nuxt | 1.26× / 1.26× |

No other suite changed by more than ±1 %.

## Conditions and correctness

Same method as the [0.1.23 measurement](speedometer-2026-11-18-layout-cache.md).
A complete Speedometer run with `RH_VERIFY_INCREMENTAL=1` found no difference
from full layouts; layout, engine and style tests passed (604).
