# Android JS core diagnostic

This is a cross-compilation and device smoke-test package for the existing
`crates/js/examples/rhjs.rs` CLI. It contains no GUI, APK, DOM, browser network
stack or ARM JIT. The build disables default features and does not enable `jit`.
Nothing in the VM, compiler, allocator or JIT is changed for this diagnostic.

The workflow runs on pushes to `android/**` only. Manual dispatch also requires
an `android/**` branch. The desktop workflow's push filters exclude that branch
prefix; do not open a pull request merely to start this diagnostic. No NDK,
emulator, SDK package, apt package or QEMU is installed. It uses the runner's
existing Rust compiler and NDK, adding only the Android `rust-std` component to
that existing toolchain if needed. A missing compatible NDK fails explicitly.
The build script refuses to run outside an X64 Linux GitHub Actions job on an
`android/**` branch.

## Toolchain and artifact

The target is `aarch64-linux-android` / `arm64-v8a`. NDK discovery checks existing
environment paths and SDK NDK directories, records every candidate, and requires
both an API21 platform range and the actual arm64/API21 sysroot library folder.
The selected preinstalled Clang links with
`--target=aarch64-linux-android21`. Rust uses that Clang as its target linker.
The `ubuntu-24.04` runner image is explicit, rather than the moving
`ubuntu-latest` label.

The package records the exact source/tree SHA, runner image, Rust/Cargo versions,
installed targets, NDK revision and metadata, linker path/version/SHA, build
commands, normal dependency tree, ELF headers/imports/version notes and hashes.
The ELF must be AArch64 ELF64/PIE, use `/system/bin/linker64`, and require only
Android system libc/dl/m/log libraries. It must contain no RPATH/RUNPATH. ELF
inspection uses NDK `llvm-readelf`, never `ldd` or execution of the ARM binary.

`android-js-aarch64-api21.tar.gz` preserves executable permissions and contains
`rhjs`, fixtures, independent host-oracle logs, expected results, metadata and a
full versioned `source.tar.gz`. Both archive and individual files have SHA256
records. Failure artifacts retain logs and available metadata, excluding object
files and the build target directory.

A successful CI job means **cross-compiled and inspected, not run on Android**.
An API21 linker/sysroot setting is not a runtime proof on an API21 device. A
successful run on an API28 device would establish that device/API28 result only.

## Small deterministic controls

`smoke.js` asserts closure bindings, prototype getters, numeric edge cases,
BigInt, exceptions, UTF-16/astral iteration and JSON, explicit DataView byte
order, typed-array aliases/conversions, retained cyclic objects, Maps and jobs.
It emits five fixed lines only after the corresponding assertions pass.
Preinstalled host Node independently checks those fixtures against the checked-in
stdout oracle. No ARM binary is run on the X64 runner.

The device script runs four bounded cases: smoke normally, the same smoke with
`RH_GC_STRESS=inc`, `RH_GC_INCREMENTAL=1` and GC statistics, an uncaught-error negative control, and an
unhandled-rejection negative control. The existing CLI drains jobs and exits 1
on the latter failures. GC stress uses the engine's existing environment hook;
there is no explicit `gc()` CLI API and this is not a complete collector proof.
GC statistics are retained as diagnostics, not a benchmark or a pass threshold.

## Manual device use (coordinator only)

After CI, verify the archive checksum and extract it on the host. Choose the
already-authorized Android device explicitly when using ADB. Copy only the
extracted runtime files (`rhjs`, the three JS fixtures, `expected.stdout`, and
`run-device.sh`) into a dedicated directory under `/data/local/tmp/`. The full
source archive and toolchain evidence can stay on the host. Ensure `rhjs` is
executable, then run `sh run-device.sh` from that directory.

The script requires Android API21+, `arm64-v8a`, `timeout` and `cmp`. Each process
has a 120-second wall limit. It saves four sets of stdout/stderr/exit codes and
non-identifying API/ABI/kernel-architecture metadata in `results/`; any mismatch
fails the script. Pull those results back for comparison with the source and
artifact manifests. Device serials, profiles and browser data are not collected.
This workflow never connects to a device or runs ADB.

No desktop/phone speed ratio is meaningful here. Different CPUs, operating
systems and thermal conditions make this a correctness diagnostic only.

## Primary references

- [Rust Android targets](https://doc.rust-lang.org/rustc/platform-support/android.html)
  describe cross compilation, `std`, ELF and the supported target names.
- [NDK with other build systems](https://developer.android.com/ndk/guides/other_build_systems)
  specifies the API suffix and Clang `--target` convention for ARM64/API21.
- [Cargo target linker and flags](https://doc.rust-lang.org/cargo/reference/config.html#targettriplelinker)
  documents target-specific linker/Rust flag configuration.
- [GitHub Ubuntu 24.04 image inventory](https://github.com/actions/runner-images/blob/main/images/ubuntu/Ubuntu2404-Readme.md)
  documents the preinstalled Rust/NDK tools. Actual versions are discovered and
  recorded in each run; the inventory is not treated as a permanent version pin.
