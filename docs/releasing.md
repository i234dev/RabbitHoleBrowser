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

Before advancing `main`, push the prepared revision to a `release/<version>`
branch. CI runs the same build, complete default test targets and headless smoke
checks on Linux, macOS and Windows for these branches. Wait for all three jobs
to succeed, then advance `main` to that exact tested revision and publish its
matching artifacts. Preparation continues to use the previous published
`origin/main` until that delivery is complete.

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

## CI build and test profiles

CI builds the workspace and runs its browser smoke test with the production
`release` profile. Tests use the experimental `ci` profile, which inherits
`release` and overrides only `lto = false` and `codegen-units = 16`. This removes
cross-crate ThinLTO from the test executables while retaining release settings
such as optimization level and assertion behavior. Cargo may still perform
local LTO between a crate's codegen units.

`cargo test --workspace --profile ci` keeps Cargo's default target selection:
library/binary unit tests, integration tests, example build/link checks and
library documentation tests. Existing ignored tests remain ignored. The
production release build and smoke test continue to validate the delivered
browser's profile. Example linking is not replaced by `cargo check`.

The expected CI time saving has not been measured. Compare complete GitHub
runs before claiming an improvement; a changed profile also creates a separate
artifact cache on its first run. See the Cargo references for
[target selection](https://doc.rust-lang.org/cargo/commands/cargo-test.html#target-selection)
and [profile settings](https://doc.rust-lang.org/cargo/reference/profiles.html).
