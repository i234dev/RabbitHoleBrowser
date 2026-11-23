# Speedometer 3.1: 0.2.45 against 0.1.44 (23 November 2026)

Paired runs on 23 November 2026 (machine time, Europe/Madrid), same method as
the [0.1.44 record](speedometer-2026-11-21-v0.1.44.md): alignment-normalized
release builds of the `shot` example, one core (CPU 8), rotated order, three
Speedometer iterations per run, every block started below load 2 with the
shared build lock held. A is the released 0.1.44 source `a43c8e0` (sha256
`c54d0b3a…`). B is the 0.2.45 engine source `de718d24` (sha256 `b1689fa3…`);
later commits of the delivery change only scripts and documentation.

B was measured in four blocks, each of which also contained bisection points
of the Web API branch (not shown):

| Block | Rep | A geomean (ms) | B geomean (ms) | A/B |
|---|---:|---:|---:|---:|
| 21:07–21:16 | 1 | 219.82 | 223.28 | 0.985 |
| | 2 | 220.61 | 220.96 | 0.998 |
| | 3 | 220.91 | 222.07 | 0.995 |
| | 4 | 218.90 | 221.20 | 0.990 |
| | 5 | 221.06 | 219.10 | 1.009 |
| | 6 | 219.61 | 221.03 | 0.994 |
| 21:28–21:40 | 1 | 222.62 | 221.24 | 1.006 |
| | 2 | 219.07 | 222.09 | 0.986 |
| | 3 | 223.79 | 221.85 | 1.009 |
| | 4 | 220.87 | 223.84 | 0.987 |
| | 5 | 221.13 | 222.16 | 0.995 |
| 21:55–22:09 | 1 | 222.92 | 224.55 | 0.993 |
| | 2 | 224.95 | 225.66 | 0.997 |
| | 3 | 222.26 | 223.82 | 0.993 |
| | 4 | 224.27 | 229.29 | 0.978 |
| | 5 | 223.66 | 223.48 | 1.001 |
| 22:19–22:36 | 1 | 219.33 | 221.07 | 0.992 |
| | 2 | 219.55 | 222.32 | 0.988 |
| | 3 | 220.43 | 222.61 | 0.990 |
| | 4 | 221.35 | 220.74 | 1.003 |
| | 5 | 224.76 | 227.73 | 0.987 |
| | 6 | 220.04 | 204.37 | 1.077 |

Median of the 22 pairs **0.993×** (B faster in 6 of 22): 0.2.45 is about
0.7 % slower than 0.1.44. The block medians were 0.994, 0.995, 0.993 and
0.991. An earlier block of 8 pairs (20:24–20:31) measured the integration
before the same-origin frame assignment fix (sha256 `7b3d2736…`) at 0.991
(1 of 8).

## Bisection

The Web API branch adds 158 commits. Points measured against the same A
build, as median pair ratios (5–6 pairs each):

| Point | Ratio |
|---|---:|
| `367874b4` (end of the first 49 commits) | 1.001 |
| `72df8e5b`, `0a6108f8`, `42a9a2b3` (commits 28, 55, 82 of the remaining 109) | 1.010, 1.006, 1.004 |
| `0e97c9dd` (90) | 1.002 |
| `d81bc1ac` (101) | 1.006 and 0.993 in two blocks |
| `f71fa1bd` (102) | 0.998 |
| `29704af2` (103) | 0.999 and 0.991 |
| `436b2975` (104) | 0.996 |
| `ecc95044` (109, branch tip) | 1.002 |

Most of the branch is neutral. The loss appears in its last commits, but
repeated points move by up to 1.3 % between blocks, so no single commit is
identified. The release was published with this cost recorded, as with
0.1.44.
