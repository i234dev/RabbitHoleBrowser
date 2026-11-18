# Page loads: shared custom property maps

Measured on **18 November 2026** (15:57–16:25 UTC). Base: source `6450efd`
(0.1.31). Candidate: the base plus the changes released in 0.1.32. The
[JSON record](page-load-2026-11-18-css-vars.json) holds both measurements.

## What changed

Every element carries the custom properties (`--x`) in effect. An element
whose rules redeclared a value it already inherits got a full copy of the
inherited map; pages built on large design-system stylesheets do this for
almost every element (TodoMVC "Complex DOM" loads 424 Spectrum stylesheets),
and copying, comparing and freeing those maps took about half of such a
page's load. The inherited map is now kept shared when the value is the same.
Length resolution is marked `#[inline]` so layout performance does not depend
on cross-crate inlining heuristics (a change in the style crate had moved it
out of line before).

## Page loads

[`tools/bench/page_load.py`](../../tools/bench/page_load.py), 32 Speedometer
workload pages, three rounds (96 paired loads, fresh process per load):

| | Base | Candidate | Ratio |
|---|---:|---:|---:|
| **Whole load (wall)** | 29.6 s | **25.1 s** | **1.18×** (77 of 96 loads faster) |
| Load and layout | 23.5 s | 20.2 s | 1.16× |
| First paint | 2.6 s | 2.2 s | 1.17× |

TodoMVC jQuery and Backbone (Complex DOM) load in about 610 ms instead of 880.

## Speedometer 3.1

Eight rotated rounds: median ratio **0.992×** (2 of 8 rounds faster);
TodoMVC-Svelte-Complex-DOM and Editor-TipTap lose the most. This small loss is
recorded here; the change is kept for the page-load gain. A fast hasher for
the same map was also tried: no further load gain and a larger Speedometer
loss, so it is not included.

## Conditions

Identically built binaries (release, line tables, frame pointers, 64-byte
alignment), Linux x86-64 under WSL2 on an Intel Core i7-12700H, CPU 0, one
process at a time. Style and engine tests passed (292).
