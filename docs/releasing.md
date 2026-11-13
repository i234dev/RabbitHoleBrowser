# Versioned deliveries

Each new delivery on `main` increments `workspace.package.version` by one patch:
`0.1.5`, `0.1.6`, …, `0.1.9`, `0.1.10`. A local implementation commit does not
by itself create a new delivery.

Before preparing a delivery, update the reference of the previous published one:

```sh
git fetch origin main
python3 scripts/release-version.py prepare
```

The helper uses the fetched `origin/main` as its base. It updates the workspace
version, inherited workspace package versions in `Cargo.lock`, and the README
status heading. Repeating the command while that delivery remains unpublished
keeps the prepared version. It rejects an unexpected version rather than skip
or lower a patch number. It does not fetch, commit, push, or publish artifacts.

Python 3.11+ and Cargo are required. Verification uses only
`cargo metadata --offline --locked --no-deps`; it never compiles or upgrades
dependencies. If verification fails, the helper restores the files it changed.
An existing lockfile and locally available dependency metadata are required.

Run relevant regression and integration checks, then check the final version:

```sh
python3 scripts/release-version.py check
git diff --check
```

Build the delivered executables from that revision. Confirm their `--version`
and the full version on `about:home`; include the revision and checksums in the
artifact manifest. Update measured compatibility and performance records with
their date, corpus and conditions. Publishing a new version does not constitute
a fresh run of a historical benchmark.

Review and commit the prepared changes, push, and publish the corresponding
artifacts. Retain the previous artifacts until the new downloads and checksums
have been verified. Do not reuse an earlier executable for the new version.

After publishing, `origin/main` may already point at the new delivery. To audit
that delivery without preparing the next one, pass the previous published
revision explicitly:

```sh
python3 scripts/release-version.py check --published-ref PREVIOUS_COMMIT
```

The helper changes only version fields; it does not verify API compatibility,
test results, benchmark freshness, or artifact contents on behalf of the release
checks above.

Its transaction and version-sequencing regressions use temporary Cargo/Git
workspaces and metadata only:

```sh
python3 -B -m unittest discover -s scripts/tests -p 'test_release_version.py' -v
```
