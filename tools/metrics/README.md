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

The latest accepted records are the [complete test262 run](../../docs/benchmarks/test262-2026-11-14-v0.1.11.json)
and [Speedometer measurement](../../docs/benchmarks/speedometer-2026-11-14-v0.1.11.md)
on source `baca2a7`, plus the [functional framework checks](../../docs/benchmarks/frameworks-2026-11-14-v0.1.11.json)
on source `b88c8ff`. Each record retains its measured source and conditions;
a later release build does not imply that those measurements were repeated.
CSS, DOM and other WPT totals retain their previous measured baselines.

Historical records remain available: [test262 on cc90](../../docs/benchmarks/test262-2026-11-14.json),
[Speedometer on 0.1.7](../../docs/benchmarks/speedometer-2026-11-14.md), and
[framework checks on 0.1.10](../../docs/benchmarks/frameworks-2026-11-14-v0.1.10.json).
The performance records do not establish a causal speed change between versions
or a same-machine comparison with Chrome/V8.
