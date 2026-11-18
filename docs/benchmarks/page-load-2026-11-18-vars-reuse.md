# Page loads: reused custom property maps

Measured on **18 November 2026** (17:10–17:26 UTC). Base: source `8838d1c`
(0.1.32). Candidate: the base plus the change released in 0.1.33. The
[JSON record](page-load-2026-11-18-vars-reuse.json) holds both measurements,
the binary checksums and the per-page and per-suite ratios.

## What changed

When an element is restyled, its custom property map (`--x`) was rebuilt from
its parent's map and its own `--*` declarations, then compared value by value
with the previous map. Each element now remembers the inputs of its last
computation. If the parent's map is the same object and its own declarations
are unchanged, it returns the previous map directly, without a copy or a deep
comparison.

## Page loads

32 Speedometer workload pages, 16 rounds (512 paired loads, alternating
order, fresh process per load):

| | Base | Candidate | Ratio |
|---|---:|---:|---:|
| **Whole load (wall)** | 134.9 s | **130.3 s** | **1.035×** (272 of 512 loads faster) |
| Page construction | 80.3 s | 77.7 s | 1.032× |
| Load and layout | 108.3 s | 106.1 s | 1.020× |

The gain is concentrated on two pages. TodoMVC Backbone and jQuery
(Complex DOM) have the most custom properties and load **1.34×** and **1.33×**
faster (per-page median). The other 30 pages are within ±1.5 %, which is noise
level. The median paired ratio over all loads is 1.001×. First paint totals
(12.0 s against 9.7 s) are dominated by a few outliers (236 of 512 loads
faster), so they are not counted as a gain.

## Speedometer 3.1

Eight rotated rounds, JIT on: median ratio **1.003×** (5 of 8 rounds faster,
0.994–1.013), which is neutral. TodoMVC-Lit-Complex-DOM improves by 17 %. The
largest per-suite losses are Webpack Complex DOM (−1.7 %) and Chart.js
(−1.5 %). The instrumented binaries' scores
are not official release scores.

## Conditions

Identically built binaries (release, line tables, frame pointers, 64-byte
alignment), Linux x86-64 under WSL2 on an Intel Core i7-12700H, CPU 0, one
process at a time. Style and engine tests passed (292).
