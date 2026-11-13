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
