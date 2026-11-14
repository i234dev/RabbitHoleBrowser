#!/usr/bin/env python3
"""CI-only cross compilation. Never runs an Android executable or installs an NDK."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile

TARGET = "aarch64-linux-android"
API = 21
ROOT = Path(__file__).resolve().parents[2]
INPUTS = ROOT / "scripts/android-js"
OUTPUT = Path(os.environ.get("ANDROID_JS_OUTPUT", "/tmp/android-js-artifacts"))
BUNDLE = OUTPUT / "bundle"
META = BUNDLE / "metadata"


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def write(name, value):
    path = META / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(value, encoding="utf-8")


def run(arguments, *, env=None, check=True):
    arguments = [str(value) for value in arguments]
    result = subprocess.run(arguments, cwd=ROOT, env=env, text=True, capture_output=True)
    with (OUTPUT / "commands.log").open("a", encoding="utf-8") as log:
        log.write(json.dumps(arguments) + "\n")
        log.write(result.stdout)
        log.write(result.stderr)
        log.write(f"\nexit={result.returncode}\n")
    print(json.dumps(arguments), flush=True)
    if check and result.returncode:
        raise RuntimeError(f"Command failed ({result.returncode}); see commands.log: {arguments[0]}")
    return result


def discover_ndk():
    candidates = []
    for name in ("ANDROID_NDK_HOME", "ANDROID_NDK_ROOT", "ANDROID_NDK", "ANDROID_NDK_LATEST_HOME"):
        if os.environ.get(name):
            candidates.append(Path(os.environ[name]))
    for name in ("ANDROID_HOME", "ANDROID_SDK_ROOT"):
        if os.environ.get(name):
            directory = Path(os.environ[name]) / "ndk"
            if directory.is_dir():
                candidates.extend(sorted(directory.iterdir(), reverse=True))
    checked = []
    seen = set()
    selected = None
    for path in candidates:
        path = path.resolve()
        if path in seen:
            continue
        seen.add(path)
        item = {"path": str(path)}
        try:
            properties = (path / "source.properties").read_text()
            revision = re.search(r"^Pkg.Revision\s*=\s*(.+)$", properties, re.MULTILINE)
            if not revision:
                raise ValueError("Missing NDK revision")
            platforms = json.loads((path / "meta/platforms.json").read_text())
            if not int(platforms["min"]) <= API <= int(platforms["max"]):
                raise ValueError(f"API{API} outside this NDK's supported platform range")
            toolchain = path / "toolchains/llvm/prebuilt/linux-x86_64"
            for name in ("clang", "llvm-readelf", "llvm-ar"):
                if not os.access(toolchain / "bin" / name, os.X_OK):
                    raise ValueError(f"Missing executable {name}")
            if not (toolchain / "sysroot/usr/lib" / TARGET / str(API)).is_dir():
                raise ValueError("Missing API21 arm64 sysroot libraries")
            item.update(revision=revision.group(1).strip(), api_range=platforms, usable=True)
            if selected is None:
                selected = (path, toolchain, item["revision"])
        except (OSError, ValueError, KeyError) as error:
            item.update(usable=False, reason=str(error))
        checked.append(item)
    write("ndk-candidates.json", json.dumps(checked, indent=2) + "\n")
    if selected is None:
        raise RuntimeError("No preinstalled NDK supports API21 arm64; no installation attempted")
    return selected


def main():
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_OS") != "Linux" or os.environ.get("RUNNER_ARCH") != "X64":
        raise RuntimeError("This script is restricted to a Linux X64 GitHub Actions runner")
    if not os.environ.get("GITHUB_REF", "").startswith("refs/heads/android/"):
        raise RuntimeError("An android/** branch is required")
    OUTPUT.mkdir(parents=True, exist_ok=True)
    META.mkdir(parents=True, exist_ok=True)
    source = run(["git", "rev-parse", "HEAD"]).stdout.strip()
    if source != os.environ.get("GITHUB_SHA"):
        raise RuntimeError("Checkout does not match the workflow source SHA")
    run(["git", "diff", "--exit-code"])
    run(["git", "diff", "--cached", "--exit-code"])
    rust_before = run(["rustc", "-vV"]).stdout
    write("rustc-vV.txt", rust_before)
    write("cargo-version.txt", run(["cargo", "-V"]).stdout)
    active = run(["rustup", "show", "active-toolchain"]).stdout.strip()
    write("rust-toolchain.txt", active + "\n")
    toolchain_name = active.split()[0]
    ndk, tools, ndk_revision = discover_ndk()
    shutil.copy2(ndk / "source.properties", META / "ndk-source.properties")
    shutil.copy2(ndk / "meta/platforms.json", META / "ndk-platforms.json")
    installed = run(["rustup", "target", "list", "--installed", "--toolchain", toolchain_name]).stdout
    write("rust-targets-before.txt", installed)
    added_std = TARGET not in installed.splitlines()
    if added_std:
        # Only this small std component is added to the already-installed Rust.
        run(["rustup", "target", "add", "--toolchain", toolchain_name, TARGET])
    write("rust-targets-after.txt", run(["rustup", "target", "list", "--installed", "--toolchain", toolchain_name]).stdout)
    if run(["rustc", "-vV"]).stdout != rust_before:
        raise RuntimeError("The existing Rust compiler changed during target setup")
    linker = tools / "bin/clang"
    write("linker-version.txt", run([linker, "--version"]).stdout)
    env = os.environ.copy()
    for key in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"):
        env.pop(key, None)
    env.update({
        "CARGO_BUILD_JOBS": "2",
        "RUSTUP_TOOLCHAIN": toolchain_name,
        "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER": str(linker),
        "CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS": "-C link-arg=--target=aarch64-linux-android21",
        "CARGO_TARGET_DIR": str(OUTPUT / "target"),
    })
    write("target-configuration.json", json.dumps({key: env[key] for key in (
        "RUSTUP_TOOLCHAIN", "CARGO_BUILD_JOBS", "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER",
        "CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS",
    )}, indent=2) + "\n")
    tree = run(["cargo", "tree", "--locked", "--target", TARGET, "-p", "rabbithole-js", "--no-default-features", "--edges", "normal"], env=env).stdout
    write("normal-dependencies.txt", tree)
    if "rabbithole-jit" in tree:
        raise RuntimeError("The diagnostic unexpectedly includes the JIT crate")
    run(["cargo", "build", "--locked", "--release", "--target", TARGET, "-p", "rabbithole-js", "--no-default-features", "--example", "rhjs", "-j", "2"], env=env)
    binary = OUTPUT / "target" / TARGET / "release/examples/rhjs"
    readelf = tools / "bin/llvm-readelf"
    elf = run([readelf, "-h", "-l", "-d", "-n", "-V", "--dyn-syms", binary]).stdout
    write("rhjs-elf.txt", elf)
    if not re.search(r"Class:\s+ELF64", elf) or not re.search(r"Machine:\s+AArch64", elf) or not re.search(r"Type:\s+DYN", elf):
        raise RuntimeError("Expected a 64-bit AArch64 position-independent ELF")
    interpreter = re.search(r"Requesting program interpreter:\s*([^\]\s]+)", elf)
    if not interpreter or interpreter.group(1) != "/system/bin/linker64":
        raise RuntimeError("Unexpected ELF program interpreter")
    needed = re.findall(r"\(NEEDED\).*?\[([^\]]+)\]", elf)
    if "libc.so" not in needed or set(needed) - {"libc.so", "libdl.so", "libm.so", "liblog.so"}:
        raise RuntimeError(f"Unexpected runtime libraries: {needed}")
    if "(RPATH)" in elf or "(RUNPATH)" in elf:
        raise RuntimeError("Unexpected ELF library search path")
    shutil.copy2(binary, BUNDLE / "rhjs")
    (BUNDLE / "rhjs").chmod(0o755)
    for name in ("smoke.js", "negative-uncaught.js", "negative-rejection.js", "expected.stdout", "expected-results.json", "run-device.sh", "README.md", "oracle.cjs"):
        shutil.copy2(INPUTS / name, BUNDLE / name)
    node_version = run(["node", "--version"]).stdout
    write("oracle-node-version.txt", node_version)
    expected = (INPUTS / "expected.stdout").read_text()
    for name, status, marker in (("smoke.js", 0, ""), ("negative-uncaught.js", 1, "ANDROID_JS_NEGATIVE_CONTROL"), ("negative-rejection.js", 1, "ANDROID_JS_REJECTION_CONTROL")):
        oracle = run(["node", "--unhandled-rejections=strict", INPUTS / "oracle.cjs", name], check=False)
        write(f"oracle-{name}.stdout", oracle.stdout)
        write(f"oracle-{name}.stderr", oracle.stderr)
        if oracle.returncode != status or (status == 0 and oracle.stdout != expected) or (marker and marker not in oracle.stderr):
            raise RuntimeError(f"Independent host oracle failed: {name}")
    run(["git", "archive", "--format=tar.gz", f"--output={BUNDLE / 'source.tar.gz'}", source])
    manifest = {
        "source": source, "tree": run(["git", "rev-parse", "HEAD^{tree}"]).stdout.strip(),
        "target": TARGET, "android_abi": "arm64-v8a", "link_api": API,
        "ndk_revision": ndk_revision, "ndk_path": str(ndk),
        "linker": str(linker), "linker_sha256": digest(linker),
        "rust_std_component_added": added_std, "rust_toolchain": toolchain_name,
        "runner_image": os.environ.get("ImageOS"), "runner_image_version": os.environ.get("ImageVersion"),
        "github_run_id": os.environ.get("GITHUB_RUN_ID"), "github_run_attempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
        "needed": needed, "interpreter": interpreter.group(1), "binary_sha256": digest(BUNDLE / "rhjs"),
        "status": "cross-compiled; Android binary NOT executed",
        "jit": "not compiled", "performance_comparison": "none",
        "runtime_api21_verified": False,
    }
    (BUNDLE / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    sums = [f"{digest(path)}  {path.relative_to(BUNDLE)}" for path in sorted(BUNDLE.rglob("*")) if path.is_file()]
    (BUNDLE / "SHA256SUMS").write_text("\n".join(sums) + "\n")
    archive = OUTPUT / "android-js-aarch64-api21.tar.gz"
    with tarfile.open(archive, "w:gz") as handle:
        handle.add(BUNDLE, arcname="android-js")
    (OUTPUT / "archive.sha256").write_text(f"{digest(archive)}  {archive.name}\n")
    # Keep the uploaded artifact bounded: no object files or NDK/toolchain copy.
    shutil.rmtree(OUTPUT / "target")
    print(f"Packaged {archive.name}; ARM execution remains pending", flush=True)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        OUTPUT.mkdir(parents=True, exist_ok=True)
        (OUTPUT / "error.txt").write_text(str(error) + "\n")
        print(str(error), file=sys.stderr)
        sys.exit(1)
