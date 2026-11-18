# Speedometer 3.1 on RabbitHole 0.1.28

Measured on **18 November 2026** (07:00–07:14 UTC) from source
`fb184d8866193af5d4a512d54e9fab5af84201b1` (version **0.1.28**). The
[JSON record](speedometer-2026-11-18-v0.1.28.json) holds every pair and the
per-suite times.

| Statistic | Result |
|---|---:|
| Mean score, JIT on (default) | **3.141** (8 runs, 3.11–3.19) |
| Mean score, JIT off | 3.103 (8 runs) |
| Median paired ratio, off geomean / on geomean | 1.013× (7 of 8 blocks faster with the JIT) |

The previous published measurement, on 0.1.18
([record](performance-2026-11-15-v0.1.18.md)), was 2.102 with the JIT and
2.082 without it, in the same harness on the same machine. The changes in
between are listed in the [README](../../README.md) with their own paired
measurements.

## Conditions

- Speedometer 3.1 at suite commit `1386415be8fe`, all 20 default suites,
  three iterations per run, served from a local copy.
- Headless `shot` built from that source with the normal release profile
  (thin LTO, stripped) and the `jit` feature, Rust 1.99.1, SHA-256
  `9be66c08…732c`.
- Linux x86-64 under WSL2 on an Intel Core i7-12700H, pinned to CPU 0, one run
  at a time; 8 blocks with the JIT-on/off order alternated per block; default
  threshold and normal GC.

These scores come from this harness and are not comparable with other
browsers; Windows and macOS were not measured.
