# Speedometer 3.1: fast hashing of layout fingerprints

Measured on **18 November 2026** (03:59–04:16 UTC). Base: `89f489a` plus the
layout cache changes released in 0.1.23 and 0.1.24. Candidate: the base plus
this change, released in 0.1.25. The
[JSON record](speedometer-2026-11-18-fingerprint-hash.json) holds every pair.

## What changed

The layout cache proves that a box's contents do not depend on its height by
fingerprinting their geometry. After 0.1.24, profiling NewsSite-Next showed
41 % of its samples in that fingerprint, mostly in SipHash. Fingerprints now
use a multiply-rotate hasher. Equivalence still also requires equal content
height, collapsed margins and floats.

A variant that additionally remembered failed comparisons was slower
(median 0.989×, NewsSite 0.88×) and was discarded.

## Result

| Statistic | Result |
|---|---:|
| Rounds with the candidate faster | **8 / 8** |
| Median paired ratio | **1.039×** |
| Range of paired ratios | 1.005–1.055× |
| NewsSite-Next / NewsSite-Nuxt | 1.42× / 1.43× |

No other suite changed by more than ±2.8 %.

## Conditions and correctness

Same method as the [0.1.23 measurement](speedometer-2026-11-18-layout-cache.md):
complete Speedometer 3.1, JIT on, identical builds with 64-byte alignment,
WSL2 on an Intel Core i7-12700H, CPU 0, rotated order. A complete run with
`RH_VERIFY_INCREMENTAL=1` found no difference from full layouts.
