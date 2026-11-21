# Speedometer 3.1: 0.1.44 against 0.1.43 (21 November 2026)

Paired runs on 21 November 2026 (21:16–21:23 machine time), same method as
the [0.1.43 record](speedometer-2026-11-21-v0.1.43.md): A is the released
0.1.43 source `ffdaf92` (sha256 `7c546c28…`), B is source `0935148c` (sha256
`eaa0e7f2…`), 8 rotated repetitions, one core, started below load 2.

| Rep | A geomean (ms) | B geomean (ms) | A/B |
|---|---:|---:|---:|
| 1 | 224.37 | 227.08 | 0.988 |
| 2 | 223.01 | 223.84 | 0.996 |
| 3 | 223.10 | 223.15 | 1.000 |
| 4 | 223.28 | 225.73 | 0.989 |
| 5 | 224.31 | 226.61 | 0.990 |
| 6 | 223.54 | 224.02 | 0.998 |
| 7 | 223.18 | 225.72 | 0.989 |
| 8 | 223.99 | 223.40 | 1.003 |

Median **0.993×** (B faster in 1 of 8 pairs): 0.1.44 is about 0.7 % slower,
spread evenly over most suites (per-suite medians 0.970–1.011). An earlier
8-repetition A/B of the same change on another core measured 1.0009 (4 of 8).
The release was published with this cost recorded: it makes three popular
sites work (engine timers no longer taken from page-replaceable globals,
stylesheet link load events after forced layouts, frames inside shadow trees,
`:scope` in DOM queries). The cause of the slowdown has not been located yet.
