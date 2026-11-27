# Style matching, Speedometer and web platform checks: 0.2.51 (27 November 2026)

Measured on 27 November 2026 (Europe/Madrid) on TheOtherSide (Intel
i7-12700H, WSL2 Linux x86-64), profile ci, other build lanes quiet.

## Selector class filter

Selector matching during style recalculation keeps a 63-bit filter of each
element's class names, and each compound selector caches the bits of the
classes it requires: a compound whose classes are not all in the element's
filter is rejected without scanning the class attribute. Only the style walk
uses the filter; script queries (`querySelector`, `matches`, …) scan the class
attribute as before.

A news front page (3 loads per binary, CPU 8, 6 s script wait, first round
discarded as cache warm-up):

| Binary | User CPU, round 2 | User CPU, round 3 |
|---|---:|---:|
| 0.2.50 (`ea1b94b4`) | 7.34 s | 7.30 s |
| 0.2.51 integration (`6f256aa1`) | 6.80 s | 6.70 s |
| 0.2.51 without the class filter | 7.37 s | 7.36 s |

About 8 % less CPU on that page. A first version that allocated a filter
table the size of the document in every matcher, including script queries,
made Speedometer 0.8 % slower (TodoMVC vanilla workloads 3–6 %); that version
was not delivered.

## Speedometer 3.1

Paired runs, 8 repetitions, CPU 8, same method as the
[0.2.48 record](speedometer-2026-11-26-v0.2.48.md): A = 0.2.50 (sha256
`308ca348…`), B = 0.2.51 integration `6f256aa1` (sha256 `d2168c25…`).

| Rep | A geomean (ms) | B geomean (ms) | A/B |
|---|---:|---:|---:|
| 1 | 182.23 | 182.61 | 0.998 |
| 2 | 182.35 | 184.11 | 0.990 |
| 3 | 179.95 | 182.32 | 0.987 |
| 4 | 183.33 | 181.20 | 1.012 |
| 5 | 181.80 | 180.75 | 1.006 |
| 6 | 184.93 | 182.25 | 1.015 |
| 7 | 181.63 | 181.61 | 1.000 |
| 8 | 181.72 | 181.41 | 1.002 |

Median **1.001×** (5 of 8 pairs): no measurable change.

## Web platform tests

Full WPT testharness run of the 38,226 browser pages (WPT 831baf28,
`rh-wpt -j 8 --timeout 5000`, isolated loopback HTTP) on the 0.2.51
integration before the final class filter change (`9a02606f`):
1,943,673 / 2,118,802 subtests, against 1,943,677 for 0.2.50 on the same
pages; 14 subtests lost and 10 gained, paired by URL. The lost ones were
repeated separately:

- `subresource-integrity/signatures/tentative/authority.https.window.html`
  (6): subtest names contain signatures generated on each run, so they are
  counted as lost and gained every time.
- `paint-timing/idlharness.window.html` (7): 17 or 10 of 18 subtests pass
  depending on the run, for both 0.2.50 and 0.2.51 (4 runs each).
- `ElementInternals-setFormValue.html`, "Single value - name is missing" (1):
  depends on when a form submission to a frame finishes; it passed in 2 of
  4 runs for 0.2.50 and 2 of 7 for 0.2.51.

After the final change, `css/selectors` (4,857), `dom` (62,903) and
`css/cssom` (3,745) were run again with the same results, and CSS reftests
stay at 16,578 / 19,627 with no differences. `tools/wpt/baseline.txt` keeps
the 0.2.50 run.

## Video (Windows)

Measured by the video work on 27 November 2026 on the same laptop under
Windows 11, native GUI, anonymous profile per run, audio output disabled,
public video page, ABBA order between `ea1b94b4` and `8d0b9dbf`. Video frames
with fractional bounds are composited on the content raster grid instead of
falling back to page repaints: in the 10–40 s range of the video, one control
run presented 11.6 fps with no frames on the video layer and the candidate
runs 22.3–22.5 fps, all on the layer. Startup (the first 10 s, including the
consent dialog) varied between 1.6 and 17.5 fps across runs of both
versions and is not counted as an improvement.
