# Speedometer 3.1: direct copy of unscaled images

Measured on **18 November 2026**. Base: `89f489a` plus the layout changes
released in 0.1.23–0.1.26. Candidate: the base plus this change, released in
0.1.27. The [JSON record](speedometer-2026-11-18-image-copy.json) holds every
pair.

## What changed

Images drawn at their own size at whole-pixel offsets — notably each
Speedometer suite's iframe and canvases composited into the page on every
paint — were resampled pixel by pixel (bilinear, four reads in floating
point). In that case every device pixel is exactly one source pixel, so the
pixels are now copied and blended directly. The unit test
`unscaled_image_copy_matches_sampling` checks that every pixel equals the
sampled result. RGBA exports are also written in place.

## Result

| Statistic | Result |
|---|---:|
| Rounds with the candidate faster | **8 / 8** |
| Median paired ratio | **1.020×** |
| Range of paired ratios | 1.003–1.030× |
| Suites faster | 19 of 20 (jQuery unchanged) |

React-Stockcharts-SVG 1.045×, Editor-CodeMirror 1.039×, NewsSite 1.04×,
Preact and WebComponents 1.033×.

## Conditions

Same method as the [0.1.23 measurement](speedometer-2026-11-18-layout-cache.md):
complete Speedometer 3.1, JIT on, identical builds with 64-byte alignment,
WSL2 on an Intel Core i7-12700H, CPU 0, rotated order.
