#!/usr/bin/env python3
"""Prepare or check one patch release against an up-to-date published Git ref.

Uses only Python's standard library and Cargo metadata; never builds, fetches,
commits, pushes, or upgrades dependencies. Requires Python 3.11 or newer.
"""
import argparse
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import tomllib


class ReleaseError(Exception):
    pass


def command(repo, args):
    result = subprocess.run(args, cwd=repo, text=True, capture_output=True)
    if result.returncode:
        raise ReleaseError(result.stderr.strip() or result.stdout.strip() or f"{args[0]} failed")
    return result.stdout


def version(value):
    if not isinstance(value, str) or not re.fullmatch(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)", value):
        raise ReleaseError(f"Expected a major.minor.patch release version, got {value!r}")
    return tuple(map(int, value.split(".")))


def replace_version(text, old, new):
    pattern = re.compile(r'''(?m)^(\s*version\s*=\s*)(["'])([^"'\r\n]+)(\2)''')
    matches = list(pattern.finditer(text))
    if len(matches) != 1 or matches[0][3] != old:
        raise ReleaseError("Expected exactly one matching version assignment")
    return pattern.sub(lambda m: m[1] + m[2] + new + m[4], text, count=1)


def inherited_packages(repo, workspace):
    excluded = {p.resolve() for pattern in workspace.get("exclude", []) for p in repo.glob(pattern)}
    manifests = {repo / "Cargo.toml"}
    for pattern in workspace.get("members", []):
        for member in repo.glob(pattern):
            if member.resolve() not in excluded:
                manifests.add(member / "Cargo.toml")
    names = set()
    for manifest in manifests:
        data = tomllib.loads(manifest.read_text())
        package = data.get("package", {})
        if package.get("version") == {"workspace": True}:
            names.add(package["name"])
    if not names:
        raise ReleaseError("No workspace packages inherit workspace.package.version")
    return names


def lock_versions(text, names, allowed, target):
    found = set()
    pattern = re.compile(r"(?ms)^\[\[package\]\]\r?\n.*?(?=^\[\[package\]\]|\Z)")

    def update(match):
        block = match[0]
        package = tomllib.loads(block)["package"][0]
        name = package["name"]
        if name not in names or "source" in package:
            return block
        if name in found or package["version"] not in allowed:
            raise ReleaseError(f"Unexpected locked workspace package: {name}")
        found.add(name)
        return replace_version(block, package["version"], target)

    result = pattern.sub(update, text)
    if found != names:
        raise ReleaseError("Workspace packages missing from Cargo.lock: " + ", ".join(sorted(names - found)))
    return result


def write_atomic(path, content):
    mode = stat.S_IMODE(path.stat().st_mode)
    fd, temporary = tempfile.mkstemp(prefix=path.name + ".", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(content)
        os.chmod(temporary, mode)
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def release(repo, published_ref, action, cargo):
    manifest = repo / "Cargo.toml"
    lock = repo / "Cargo.lock"
    original = {p: p.read_bytes() for p in (manifest, lock)}
    source = original[manifest].decode()
    data = tomllib.loads(source)
    current = data["workspace"]["package"]["version"]
    commit = command(repo, ["git", "rev-parse", "--verify", "--end-of-options", published_ref + "^{commit}"]).strip()
    published = tomllib.loads(command(repo, ["git", "show", commit + ":Cargo.toml"]))["workspace"]["package"]["version"]
    major, minor, patch = version(published)
    version(current)
    target = f"{major}.{minor}.{patch + 1}"
    if current not in (published, target):
        raise ReleaseError(f"Version {current} must match published {published} or prepared {target}")
    if action == "check" and current != target:
        raise ReleaseError(f"Next delivery must use {target}; run prepare first")
    names = inherited_packages(repo, data["workspace"])
    section = re.compile(r"(?ms)^\[workspace\.package\]\r?\n.*?(?=^\[|\Z)")
    if len(list(section.finditer(source))) != 1:
        raise ReleaseError("Expected a [workspace.package] table")
    updated_manifest = section.sub(lambda m: replace_version(m[0], current, target), source)
    updated_lock = lock_versions(original[lock].decode(), names, {published, target}, target)
    updates = {manifest: updated_manifest.encode(), lock: updated_lock.encode()}
    readme = repo / "README.md"
    if readme.exists():
        original[readme] = readme.read_bytes()
        text = original[readme].decode()
        status = re.compile(r"(?m)^(## Status \(v)([^)\r\n]+)(\))")
        matches = list(status.finditer(text))
        if len(matches) > 1 or (matches and matches[0][2] not in {published, target}):
            raise ReleaseError("README status version does not match this delivery")
        updates[readme] = status.sub(lambda m: m[1] + target + m[3], text).encode()
    if action == "check" and any(original[p] != value for p, value in updates.items()):
        raise ReleaseError("Cargo.lock or README version is stale; run prepare first")
    changed = []
    try:
        for path, content in updates.items():
            if content != original[path]:
                write_atomic(path, content)
                changed.append(path)
        metadata = json.loads(command(repo, [cargo, "metadata", "--offline", "--locked", "--no-deps", "--format-version", "1"]))
        members = set(metadata["workspace_members"])
        actual = {p["name"]: p["version"] for p in metadata["packages"] if p["id"] in members and p["name"] in names}
        if actual != dict.fromkeys(names, target):
            raise ReleaseError("Cargo metadata does not match the prepared workspace versions")
    except (OSError, ReleaseError, ValueError):
        for path in reversed(changed):
            write_atomic(path, original[path])
        raise
    return {"published_ref": published_ref, "published": published, "version": target,
            "changed": [str(p.relative_to(repo)) for p in changed], "workspace_packages": len(names)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "check"))
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--published-ref", default="origin/main", help="already-fetched reference of the previous delivery")
    parser.add_argument("--cargo", default=os.environ.get("CARGO", "cargo"))
    args = parser.parse_args()
    try:
        result = release(args.repo.resolve(), args.published_ref, args.action, args.cargo)
    except (OSError, KeyError, ReleaseError, ValueError) as error:
        parser.exit(1, f"release-version: {error}\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
