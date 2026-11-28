# Fixed content during scrolling: 0.2.52 (28 November 2026)

## Change

While the user interface scrolls the last painted frame ahead of the content
thread's repaint, `position: fixed` content painted on top of the rest of the
page (a viewport-fixed suffix of the display list) is composited as a separate
layer in viewport coordinates instead of moving with the page pixels. When
scrolling content paints above fixed content, or an effect depends on the
pixels underneath (filters, blending, backdrop filters), the frame keeps the
previous single-layer behaviour. `RH_FIXED_LAYER_TRACE=1` logs which case a
tab is in. `position: sticky` is not covered yet.

## Windows measurement

Measured by the video work on 28 November 2026 on TheOtherSide under Windows
11 (Intel i7-12700H, CPUs 9-12), native GUI, continuous wheel scrolling in a
local page with a `top: 0` header and a `bottom: 0` bar and no scroll
handlers, while script kept the content thread busy (four 1,450 ms blocks).
A capture accompanied each wheel step; the centre of each bar was compared
byte for byte with the resting image.

| Binary | Captures | Header identical | Bottom bar identical | Both |
|---|---:|---:|---:|---:|
| 0.2.51 | 45 | 2 | 2 | 2 |
| 0.2.52 candidate (`9e63f140`) | 45 | 45 | 45 | 45 |

A single 4 s block gave 3/45 and 28/45: after 2.5 s the browser's own
unresponsive-page notice covers the top 40 px of the viewport. macOS was not
measured.

## Compatibility

App 155, layout 408 and script 301 tests pass; CSS reftests 16,578 / 19,627
with no differences against 0.2.51; WPT `css/css-position`,
`css/css-scroll-anchoring`, `css/css-scroll-snap`, `css/cssom-view`,
`html/semantics`, `media-source` and `mimesniff` (1,117 pages): 6,211 /
10,675 subtests, the same as 0.2.51, none lost.

## Speedometer 3.1

Paired runs on 28 November 2026 (Europe/Madrid) on TheOtherSide (WSL2 Linux
x86-64), CPU 8, 8 repetitions, same method as the
[0.2.48 record](speedometer-2026-11-26-v0.2.48.md): A = 0.2.51 integration
`6f256aa1` (sha256 `d2168c25…`), B = 0.2.52 integration `9e63f140` (sha256
`5124d0db…`). A first series on the same binaries, taken while the Windows
side of the laptop was busy, was slower for both (geomean 198.5 / 202.7 ms,
pairs from 0.93 to 1.11) and is not used.

| Rep | A geomean (ms) | B geomean (ms) | A/B |
|---|---:|---:|---:|
| 1 | 196.02 | 196.62 | 0.997 |
| 2 | 193.30 | 195.27 | 0.990 |
| 3 | 197.56 | 194.28 | 1.017 |
| 4 | 191.72 | 192.74 | 0.995 |
| 5 | 194.64 | 193.13 | 1.008 |
| 6 | 190.57 | 192.55 | 0.990 |
| 7 | 194.83 | 192.11 | 1.014 |
| 8 | 192.57 | 191.16 | 1.007 |

Median **1.002×** (4 of 8 pairs): no measurable change.
