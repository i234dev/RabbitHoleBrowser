# Speedometer 3.1: 0.1.35 against 0.1.34 (19 November 2026)

Paired runs on 19 November 2026 (05:21–05:31 machine time), Speedometer 3.1
(`speedometer-3.1-1386415be8fe`, served locally), 3 iterations per run, all
suites, JIT on (default), one core (`taskset -c 2`), Intel Core i7-12700H under
WSL2. Both binaries are the `shot` example built with the same
alignment-normalized flags (`rh-build3`): A is 0.1.34 code, B is source
`aa05c2d` (0.1.35 code before the version bump). The order of A and B rotates
every repetition.

Repetitions 1–3 overlapped another benchmark pinned to the same core and are
excluded. Repetitions 4–6 (clean):

| Rep | A geomean (ms) | B geomean (ms) | A/B |
|---|---:|---:|---:|
| 4 | 324.70 | 319.74 | 1.016 |
| 5 | 328.66 | 318.72 | 1.031 |
| 6 | 324.46 | 319.25 | 1.016 |

Median **1.016×** (B faster). NewsSite is slower:

| Rep | NewsSite-Next A / B (ms) | ratio | NewsSite-Nuxt A / B (ms) | ratio |
|---|---|---:|---|---:|
| 4 | 922.44 / 933.94 | 0.988 | 861.80 / 890.02 | 0.968 |
| 5 | 919.45 / 937.12 | 0.981 | 863.10 / 881.19 | 0.979 |
| 6 | 919.93 / 946.73 | 0.972 | 850.42 / 868.94 | 0.979 |

The NewsSite cost comes from the flexbox/sizing work (stretched items with
definite heights, alignment modifiers, aspect-ratio transfer). Merged as first
written it was 9 % (A/B on NewsSite only, 5 reps: 0.911×). Two follow-up
commits (`1f1f96c`, `aa05c2d`) recovered about two thirds of it. The rest is
being worked on.
