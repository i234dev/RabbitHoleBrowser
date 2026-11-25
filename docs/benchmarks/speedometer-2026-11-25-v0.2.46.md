# Speedometer 3.1: 0.2.46 against 0.2.45 (25 November 2026)

Paired runs on 25 November 2026 (10:56–11:05 machine time, Europe/Madrid),
same method as the [0.2.45 record](speedometer-2026-11-23-v0.2.45.md):
alignment-normalized release builds of the `shot` example, one core (CPU 8),
rotated order, three Speedometer iterations per run, started below load 2 with
the shared build lock held. A is the released 0.2.45 source `4a87d1bd`
(sha256 `ccdb6c59…`), B is the 0.2.46 engine source `7b235dfb` (sha256
`bfc7b3ff…`).

| Rep | A geomean (ms) | B geomean (ms) | A/B |
|---|---:|---:|---:|
| 1 | 239.83 | 228.20 | 1.051 |
| 2 | 236.15 | 227.97 | 1.036 |
| 3 | 235.07 | 234.92 | 1.001 |
| 4 | 234.67 | 228.33 | 1.028 |
| 5 | 235.02 | 228.06 | 1.031 |
| 6 | 232.93 | 229.52 | 1.015 |
| 7 | 232.97 | 228.32 | 1.020 |
| 8 | 235.20 | 229.05 | 1.027 |

Median **1.027×** (B faster in 8 of 8 pairs). Charts-chartjs is 1.445×
faster and Perf-Dashboard 1.061×; the other suites range from 0.990× to
1.032×.

## Integration history

The first integrated tree (`72bcb24c`) measured 0.984× (0 of 8) on the same
day. Building each merged branch alone against the same 0.2.45 build located
the cost in the HTML DOM branch (0.996×; Charts-chartjs 0.944×,
Perf-Dashboard 0.954×): a JavaScript layer checked the receiver and the
arguments before every canvas 2D and Web IDL call. The CSS layout branch
measured 1.005× and the WebCrypto/editing branch 1.006× (6 of 6), whose
cryptography and editing code now load on first use. The fix moved those
checks into native Web IDL functions in the JavaScript engine, removed rest
parameters from path operations and caches parsed canvas colors.
