# Speedometer 3.1: 0.1.43 against 0.1.42 (21 November 2026)

Paired runs on 21 November 2026 (15:07–15:14 machine time), same method as
the [0.1.42 record](speedometer-2026-11-21-v0.1.42.md): A is the released
0.1.42 source `2f4613c` (sha256 `27b8c89c…`), B is source `03522e5f` (sha256
`c244d71a…`), 8 rotated repetitions, one core, started below load 2.

| Rep | A geomean (ms) | B geomean (ms) | A/B |
|---|---:|---:|---:|
| 1 | 245.85 | 243.87 | 1.008 |
| 2 | 243.06 | 234.53 | 1.036 |
| 3 | 235.82 | 240.26 | 0.982 |
| 4 | 243.56 | 235.89 | 1.033 |
| 5 | 237.71 | 237.25 | 1.002 |
| 6 | 236.00 | 241.97 | 0.975 |
| 7 | 244.43 | 234.39 | 1.043 |
| 8 | 241.94 | 237.84 | 1.017 |

Median **1.013×** (B faster in 6 of 8 pairs): no slowdown. This release
targets correctness (same-document navigation, content decoding, media
fallback), not speed.
