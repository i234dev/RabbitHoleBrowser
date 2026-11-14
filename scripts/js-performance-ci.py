#!/usr/bin/env python3
"""Build a diagnostic JS pair on CI; never run timing benchmarks here."""
from __future__ import annotations

import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tomllib

BASELINE_SHA = "673a2e9fb5f45a16d197a458e06c9ab669cd7606"
BUILD_SETTINGS = {
    "RUSTUP_TOOLCHAIN": "stable",
    "CARGO_BUILD_JOBS": "2",
    "CARGO_INCREMENTAL": "0",
    "CARGO_PROFILE_RELEASE_STRIP": "false",
    "RUSTFLAGS": "-C force-frame-pointers=yes",
}


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def capture(*args: str, cwd: Path | None = None) -> str:
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def run_logged(command: list[str], cwd: Path, env: dict[str, str], log: Path) -> dict:
    """Keep each real exit code, including after another check has failed."""
    result = {"command": command, "started_utc": utc_now(), "log": log.name}
    print(f"{cwd.name}: {' '.join(command)}", flush=True)
    with log.open("w") as output:
        try:
            proc = subprocess.Popen(command, cwd=cwd, env=env, stdout=output,
                                    stderr=subprocess.STDOUT, start_new_session=True)
        except OSError as error:
            output.write(f"Could not start command: {error}\n")
            result.update(returncode=None, start_error=str(error))
        else:
            try:
                proc.wait(timeout=900)
            except subprocess.TimeoutExpired:
                result["timed_out"] = True
                try:
                    os.killpg(proc.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(proc.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    proc.wait()
            result["returncode"] = proc.returncode
    result["finished_utc"] = utc_now()
    print(f"{cwd.name}: returncode={result['returncode']}", flush=True)
    return result


def checked(result: dict) -> bool:
    return result.get("returncode") == 0 and not result.get("timed_out", False)


def main() -> int:
    root = Path.cwd()
    artifact = root / "js-performance-artifacts"
    artifact.mkdir(exist_ok=True)
    report = {
        "kind": "diagnostic-build-pair",
        "performance_measurement": False,
        "started_utc": utc_now(),
        "baseline_revision": BASELINE_SHA,
        "candidate_revision": os.environ["CANDIDATE_SHA"],
        "build_settings": BUILD_SETTINGS,
        "test_profile": "ci",
        "example_profile": "release",
        "features": ["jit"],
        "github_run_id": os.environ.get("GITHUB_RUN_ID"),
        "github_run_attempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
        "runner_os": os.environ.get("RUNNER_OS"),
        "runner_arch": os.environ.get("RUNNER_ARCH"),
        "revisions": [],
    }
    failed = False
    try:
        for key, value in BUILD_SETTINGS.items():
            if os.environ.get(key) != value:
                raise RuntimeError(f"Expected common build setting {key}={value}")
        if os.environ.get("CARGO_ENCODED_RUSTFLAGS"):
            raise RuntimeError("CARGO_ENCODED_RUSTFLAGS would override the recorded RUSTFLAGS")
        report["rustc"] = capture("rustc", "-Vv")
        report["cargo"] = capture("cargo", "-Vv")
        report["kernel"] = capture("uname", "-srmo")
        sources = [("baseline", BASELINE_SHA), ("candidate", report["candidate_revision"])]
        release_profile = None
        cargo_config = None
        for label, revision in sources:
            source = root / "sources" / label
            if capture("git", "rev-parse", "HEAD", cwd=source) != revision:
                raise RuntimeError(f"{label} source revision mismatch")
            if capture("git", "status", "--porcelain", cwd=source):
                raise RuntimeError(f"{label} source must be clean before validation")
            manifest = tomllib.loads((source / "Cargo.toml").read_text())
            profile = manifest.get("profile", {}).get("release", {})
            if release_profile is not None and release_profile != profile:
                raise RuntimeError("Baseline and candidate release profiles differ")
            release_profile = profile
            config = {str(path): sha256(source / path) if (source / path).exists() else None
                      for path in (Path(".cargo/config"), Path(".cargo/config.toml"))}
            if cargo_config is not None and cargo_config != config:
                raise RuntimeError("Baseline and candidate Cargo configuration differs")
            cargo_config = config
        report["release_profile"] = release_profile
        report["source_cargo_config_sha256"] = cargo_config

        # Source snapshots and target directories never overlap. Finish all
        # baseline commands before starting any candidate command.
        for label, revision in sources:
            source = root / "sources" / label
            target = root / "targets" / label
            dest = artifact / f"{label}-{revision}"
            dest.mkdir(exist_ok=True)
            item = {"label": label, "revision": revision,
                    "cargo_lock_sha256": sha256(source / "Cargo.lock"),
                    "manifest_sha256": sha256(source / "Cargo.toml"), "checks": {}}
            report["revisions"].append(item)
            item["rustc"] = capture("rustc", "-Vv", cwd=source)
            item["cargo"] = capture("cargo", "-Vv", cwd=source)
            if item["rustc"] != report["rustc"] or item["cargo"] != report["cargo"]:
                raise RuntimeError(f"{label} toolchain differs from the common toolchain")
            env = os.environ.copy()
            env["CARGO_TARGET_DIR"] = str(target)
            test_command = ["cargo", "test", "--lib", "-p", "rabbithole-js", "--profile", "ci",
                            "--features", "jit", "--locked", "--no-fail-fast"]
            for mode in ("off", "on"):
                test_env = env | {"RABBITHOLE_JIT": mode, "RABBITHOLE_JIT_THRESHOLD": "0",
                                  "RH_GC_STRESS": "inc"}
                name = f"lib-{mode}"
                item["checks"][name] = run_logged(test_command, source, test_env, dest / f"{name}.log")
                item["checks"][name]["settings"] = {
                    "RABBITHOLE_JIT": mode, "RABBITHOLE_JIT_THRESHOLD": "0", "RH_GC_STRESS": "inc"}
            build_command = ["cargo", "build", "--release", "--locked", "-p", "rabbithole-js",
                             "--features", "jit", "--example", "perf", "--example", "jitdiff"]
            item["checks"]["build"] = run_logged(build_command, source, env, dest / "build.log")
            if checked(item["checks"]["build"]):
                for name in ("perf", "jitdiff"):
                    binary = target / "release" / "examples" / name
                    if binary.read_bytes()[:4] != b"\x7fELF":
                        raise RuntimeError(f"{label} {name} is not an ELF executable")
                    shutil.copy2(binary, dest / f"{name}-{revision}")
                fuzz_env = env | {"RH_GC_STRESS": "inc"}
                item["checks"]["jitdiff"] = run_logged(
                    [str(target / "release" / "examples" / "jitdiff"), "--fuzz", "10000", "--seed", "1"],
                    source, fuzz_env, dest / "jitdiff.log")
                item["checks"]["jitdiff"]["settings"] = {"RH_GC_STRESS": "inc"}
            else:
                item["checks"]["jitdiff"] = {"returncode": None, "skipped": "release build failed"}
            item["source_clean_after_checks"] = not capture("git", "status", "--porcelain", cwd=source)
            item["passed"] = item["source_clean_after_checks"] and all(checked(r) for r in item["checks"].values())
            failed |= not item["passed"]
            (dest / "metadata.json").write_text(json.dumps(item, indent=2) + "\n")
    except Exception as error:
        report["infrastructure_error"] = f"{type(error).__name__}: {error}"
        print(report["infrastructure_error"], file=sys.stderr)
        failed = True
    report["finished_utc"] = utc_now()
    report["passed"] = not failed
    (artifact / "metadata.json").write_text(json.dumps(report, indent=2) + "\n")
    readme = ("Diagnostic artifacts, never a stable delivery. No CI performance timings are measured.\n"
              "Both sources use one toolchain and identical build settings. See metadata.json and logs.\n"
              "Test failure does not prevent collecting binaries for diagnosis. Check all exit codes.\n"
              "Measure downloaded ELF pairs later under agreed quiet conditions on one machine.\n")
    (artifact / "README.txt").write_text(readme)
    files = sorted(p for p in artifact.rglob("*") if p.is_file() and p.name != "SHA256SUMS")
    (artifact / "SHA256SUMS").write_text("".join(f"{sha256(p)}  {p.relative_to(artifact)}\n" for p in files))
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
