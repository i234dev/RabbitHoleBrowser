# Built-in measurements

`about:home` gets its full version from `CARGO_PKG_VERSION`. The engine build
generates its small measurement table from these committed sources:

- JavaScript counts: `crates/js/tests/test262_baseline.txt`.
- CSS pixel-test counts: `crates/engine/tests/wpt-css-baseline.txt`.
- WPT web-platform counts and the `dom` / `dom/*` subset: `tools/wpt/summary.txt`.
- Dates, scope and measurements without a machine-readable baseline:
  `tools/metrics/measurements.tsv`.

Updating a baseline updates the displayed counts on the next build. Each
computed row records its last measured pass/total counts alongside its date.
If a baseline's counts change without updating that date record, the page shows
the new result and labels its date as unrecorded. Do not assign a new date to
an old measurement just because a new browser version is built.

After a measured run, update the relevant TSV record with its counts, date and
scope. Performance records must identify the workload and machine conditions;
historical numbers remain explicitly historical until replaced by a measured
run. Additional measured workloads can be added as `recorded` rows without
changing the page or its generator.

The build uses Rust's standard library and embeds only the resulting table,
not the complete baseline files. It requires no Python or network requests.

The Speedometer row comes from the [0.1.28 record](../../docs/benchmarks/speedometer-2026-11-18-v0.1.28.md)
on source `fb184d8`, measured on 2026-11-18: eight runs per JIT mode of the
release build, mean scores 3.141 with the JIT (the default since 0.1.21) and
3.103 without. The earlier [0.1.18 record](../../docs/benchmarks/performance-2026-11-15-v0.1.18.md)
(2.102 / 2.082) stays as measured. The microbenchmark row keeps its 0.1.18
context: all 27 cases are reported individually, including `array_map` at
0.990x; no overall micro speedup is aggregated. The directed CSS row is only 33 selected controls, run on bootstrap
source `37cb93d` with CSS code identical to final 0.1.18; it is not a new global
CSS/WPT result. These rows retain their measured 0.1.18 context in later builds.

The [complete test262 run](../../docs/benchmarks/test262-2026-11-18-v0.1.34.md)
was measured on source `fe3d71e` (0.1.34 JavaScript code) on 2026-11-18; the
[0.1.11 run](../../docs/benchmarks/test262-2026-11-14-v0.1.11.json) on `baca2a7`
stays as measured. The [functional framework checks](../../docs/benchmarks/frameworks-2026-11-14-v0.1.11.json)
on source `b88c8ff`. Each retains its original date and conditions; a later build
does not imply that those suites were repeated. The [CSS reftest run](../../docs/benchmarks/wpt-css-2026-11-21-v0.1.40.md) was
measured on 2026-11-21 on source `9b4e7cc9`, with the corrected multiple-reference
rule; the [0.1.39 run](../../docs/benchmarks/wpt-css-2026-11-20-v0.1.39.md), the [0.1.38 run](../../docs/benchmarks/wpt-css-2026-11-20-v0.1.38.md), the [0.1.37 run](../../docs/benchmarks/wpt-css-2026-11-19-v0.1.37.md), the [0.1.36 run](../../docs/benchmarks/wpt-css-2026-11-19-v0.1.36.md) on `89d437d` and the earlier runs on `1cf6c03` ([0.1.35](../../docs/benchmarks/wpt-css-2026-11-19.md)) and
`68a3dd7` ([18 November](../../docs/benchmarks/wpt-css-2026-11-18.md)) stay as measured. DOM and other WPT totals retain
their previous measured baselines (2026-11-13).

Historical records remain unchanged: [test262 on cc90](../../docs/benchmarks/test262-2026-11-14.json),
[Speedometer on 0.1.11](../../docs/benchmarks/speedometer-2026-11-14-v0.1.11.md),
[Speedometer on 0.1.7](../../docs/benchmarks/speedometer-2026-11-14.md), and
[framework checks on 0.1.10](../../docs/benchmarks/frameworks-2026-11-14-v0.1.10.json).
The performance records do not establish a causal speed change between versions
or a same-machine comparison with Chrome/V8.
