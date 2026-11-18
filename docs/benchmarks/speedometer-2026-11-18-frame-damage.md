# Speedometer 3.1: damage-based repaint of frames

Measured on **18 November 2026** (13:11–13:25 UTC). Base: source `ab45758`
(0.1.29). Candidate: the base plus this change, released in 0.1.30. The
[JSON record](speedometer-2026-11-18-frame-damage.json) holds every pair.

## What changed

The top-level page repaints only the regions where its display list changed;
frames were painted whole into a new canvas every time they changed. Each
Speedometer suite runs in a frame. Frames painted at device scale 1 now keep
their canvas and use the same damage-based repaint. At other scales (HiDPI)
they are still painted whole.

## Result

| Statistic | Result |
|---|---:|
| Rounds with the candidate faster | **8 / 8** |
| Median paired ratio | **1.023×** |
| Range of paired ratios | 1.008–1.040× |

Preact and Svelte (Complex-DOM) 1.06×, NewsSite 1.04–1.05×,
Charts-observable-plot 1.05×, CodeMirror 1.03×; TodoMVC-ES5 0.98×.

## Conditions and correctness

Same method as the [0.1.23 measurement](speedometer-2026-11-18-layout-cache.md).
The test `damage_repainted_frames_match_full_paints` compares every damage
repaint of a frame with a full paint (identical pixels); engine and layout
tests passed (526).
