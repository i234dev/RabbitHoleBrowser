# Speedometer 3.1: JIT on versus off — basis for enabling it by default

Measured on **17 November 2026** (17:53–18:17 UTC) from source
`17d662a66dbb1a7a870c8334139ed3b5f2f2b9b3` (version **0.1.20**). The
[JSON record](speedometer-2026-11-17-jit-default.json) holds every pair,
score and per-suite ratio.

## Result

| Statistic | Result |
|---|---:|
| Paired blocks with the JIT faster | **10 / 10** |
| Median paired ratio (off geomean / on geomean) | **1.020×** |
| Range of paired ratios | 1.003–1.035× |
| Mean score, JIT off / on | 2.259 / 2.307 |

Per-suite medians of the 10 pairs range from 0.978× (TodoMVC-Backbone) to
1.082× (Perf-Dashboard). The largest gains are in JavaScript-heavy suites
(Perf-Dashboard, Charts, CodeMirror, React-Redux, Angular). NewsSite-Next/Nuxt
are dominated by style and layout and stay at parity.

## Conditions

- Speedometer 3.1 at suite commit `1386415be8fe`, all 20 default suites,
  three iterations per run, served from a local copy.
- Headless `shot` release build with the `jit` feature, symbols kept and frame
  pointers forced (SHA-256 `0a5e2cb0…09b7`), Rust 1.99.1.
- Linux x86-64 under WSL2 on an Intel Core i7-12700H, child pinned to CPU 0,
  one run at a time; 10 blocks with the off/on order alternated per block.
- Normal GC and the default threshold (1000); only `RABBITHOLE_JIT` differed.

## Scope and limitations

These scores come from this harness and are not comparable with other
browsers. The runs were on Linux only; Windows x64 native execution is covered
by CI canaries, not by this timing. macOS on Apple silicon has no native JIT
backend and keeps using the interpreter. This record supersedes the 0.1.18
eight-pair run only as the basis for the default; that run stays as recorded.

An earlier exploratory run on the same day (four or two runs in parallel on
separate cores) showed the same direction with much larger variance; it is
not used for this decision. Lowering the threshold to 300 or 100 did not
improve the result.
