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

The latest accepted performance comparison is the [0.1.18 record](../../docs/benchmarks/performance-2026-11-15-v0.1.18.md)
on source `dae023e`, measured on 2026-11-15. It reports the eight-run mean
Speedometer score in each JIT mode separately from the median of eight paired
ratios. Five pairs improved; the consistency criterion required six, so phase 8
remains open and the JIT stays off by default. All 27 microbenchmark cases are
reported individually, including `array_map` at 0.990x; no overall micro speedup
is aggregated. The directed CSS row is only 33 selected controls, run on bootstrap
source `37cb93d` with CSS code identical to final 0.1.18; it is not a new global
CSS/WPT result. These rows retain their measured 0.1.18 context in later builds.

The [complete test262 run](../../docs/benchmarks/test262-2026-11-14-v0.1.11.json)
remains measured on source `baca2a7`, and the [functional framework checks](../../docs/benchmarks/frameworks-2026-11-14-v0.1.11.json)
on source `b88c8ff`. Each retains its original date and conditions; a later build
does not imply that those suites were repeated. CSS, DOM and other WPT totals
retain their previous measured baselines.

Historical records remain unchanged: [test262 on cc90](../../docs/benchmarks/test262-2026-11-14.json),
[Speedometer on 0.1.11](../../docs/benchmarks/speedometer-2026-11-14-v0.1.11.md),
[Speedometer on 0.1.7](../../docs/benchmarks/speedometer-2026-11-14.md), and
[framework checks on 0.1.10](../../docs/benchmarks/frameworks-2026-11-14-v0.1.10.json).
The performance records do not establish a causal speed change between versions
or a same-machine comparison with Chrome/V8.
