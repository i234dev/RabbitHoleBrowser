# Web API measurements on 23 November 2026 at 16:08 CET

The last completed run of all 40,172 browser testharness URLs reached **82.63%** at `367874b4`, compared with **44.08%** on `a43c8e0`. Both used WPT `831baf28d226911e47b2454d69e0c9e7348dd4d1`. The later results below cover the listed directories. [Complete run by directory](wpt-apis-2026-11-23-367874b4.md)

## CSS support queries, shape margins and anchor alignment

These comparisons ran on TheOtherSide in an isolated loopback namespace, with a ci build, ten workers on CPUs 0–3 and 10–15, 5 s per page and 15 s for long pages. Each row compares named subtests on identical URLs and retains duplicate names.

| Directory | URLs | Earlier pass / total | Later pass / total | Gained | Lost |
|---|---:|---:|---:|---:|---:|
| css/css-anchor-position, reflected alignment after `929f4740` | 268 | 12,478 / 13,664 | 12,751 / 13,664 | 273 | 0 |
| css/css-shapes, support and interpolation after `929f4740` | 147 | 4,646 / 7,527 | 6,257 / 7,527 | 1,611 | 0 |
| css/css-anchor-position, then ordered fallbacks in `1fa7fca5` | 268 | 12,751 / 13,664 | 12,905 / 13,664 | 154 | 0 |
| css/css-shapes, ordered fallbacks | 147 | 6,257 / 7,527 | 6,257 / 7,527 | 0 | 0 |
| css/cssom, ordered fallbacks | 272 | 3,659 / 4,458 | 3,659 / 4,458 | 0 | 0 |
| css/css-conditional, ordered fallbacks | 235 | 2,192 / 3,065 | 2,192 / 3,065 | 0 | 0 |

`616943d5` preserves whitespace in feature queries and passes the original component values to the declaration and selector parsers. `a46f165c` interpolates mixed lengths and percentages and clamps the used shape margin after extrapolation. `0c153618` maps self-alignment through axis flips in the containing block’s writing mode. These three changes passed 72 engine regression tests and the CSS support-parser unit test. In our first alignment regression, the expectation for a flipped 30 × 20 box overlooked its new width of 20. The corrected regression checks both dimensions and the resulting position.

`1fa7fca5` ranks fitting positions by the space in the inset-modified containing block, includes the original position and preserves source order for ties. Its regression passed. The ordered runner SHA-256 was `5df721e5fee09e15e0827d9955aaa8d0b7eab4983686ac69b8c2900b813499b5`. All 880 source inputs matched the ordered control snapshot. The original WPT build differed only in the integration test file. The complete pixel comparison and Speedometer for these changes are pending. The layout risks include axis selection under orthogonal writing modes and extra trial layouts when alternatives compete.

## Full directory comparisons in the ongoing run at 4a3abc07

The same native URLs were present on both sides of every row. A page which reported no subtests contributes its harness result. This convention differs from the named-only CSS checks above. A loss means a pass in main changed status or disappeared, after normalizing ephemeral local ports. The following 20,610 URLs constitute a partial run.

| Directory | URLs | Main pass / total | Branch pass / total | Lost |
|---|---:|---:|---:|---:|
| FileAPI | 109 | 1,185 / 1,573 | 1,395 / 1,582 | 0 |
| IndexedDB | 448 | 2,494 / 3,167 | 2,547 / 3,167 | 0 |
| WebCryptoAPI | 333 | 15,179 / 67,288 | 48,548 / 70,607 | 0 |
| css | 8,001 | 90,663 / 181,117 | 106,351 / 180,975 | 14,972 |
| dom | 784 | 58,530 / 65,775 | 62,332 / 65,775 | 0 |
| fetch | 689 | 2,672 / 9,016 | 2,748 / 9,016 | 4 |
| html | 9,001 | 92,739 / 130,126 | 103,921 / 130,078 | 99 |
| service-workers | 302 | 680 / 2,281 | 682 / 2,281 | 0 |
| streams | 156 | 1,907 / 2,940 | 1,921 / 2,940 | 0 |
| webstorage | 54 | 1,172 / 1,284 | 1,172 / 1,284 | 0 |
| workers | 296 | 696 / 1,313 | 756 / 1,318 | 0 |
| xhr | 437 | 532 / 2,151 | 682 / 2,151 | 0 |

The CSS directory accounts for 14,972 of the 15,075 losses in this subset. Motion paths account for 4,550, gap decorations for 2,394, float shapes for 2,093, and masking for 1,664. These are comparisons with main. The later targeted shape run in the first table used a different revision.

The HTML fixture logged a thread creation failure while serving 9,009 native URLs. The returned URLs included all 9,001 browser entries in the manifest. A fresh run of the 30 pages with losses, using the identical executable and timeouts, recovered 20 of their 99 lost results. The other 79 repeated, including 48 interest-delay transition expectations and nine tentative ARIA reflections. No additional previously passing results fell on those pages during the replay. The table retains the original measurement.

## Rich editing under validation

A subsequent working-tree build changed how edits preserve visible whitespace. In 727 editing pages it passed 108,879 of 116,354 named subtests, compared with 107,898 of 116,349. The named comparison found 1,025 gains and 44 losses. Of the losses, 39 involve deletion near inline or text-node boundaries, four involve inserting a space before a placeholder break in a plaintext editor, and one involves typing after a protected trailing space. Follow-up code and tests address those cases. The new gains have not been added to the last complete global result.

The outstanding branch gates are the complete API comparison, all unit and GC-stress checks, pixel comparison, applicable test262 runs and aligned Speedometer. [Earlier gate results and conditions](wpt-apis-2026-11-23-4a3abc07.md)
