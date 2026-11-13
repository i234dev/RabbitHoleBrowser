"""Release transactions against real, dependency-free temporary Cargo/Git repos."""
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest


HELPER = Path(__file__).resolve().parents[1] / "release-version.py"
CARGO = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")


class ReleaseVersionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="release version ")
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        self.write("Cargo.toml", '''[workspace]
resolver = "2"
members = ["crates/*"]
exclude = ["vendor"]

[workspace.package]
version = "0.1.9"
edition = "2021"
''')
        self.write("crates/browser/Cargo.toml", '''[package]
name = "rabbithole"
version.workspace = true
edition.workspace = true
[dependencies]
rabbithole-core = { path = "../core" }
''')
        self.write("crates/core/Cargo.toml", '''[package]
name = "rabbithole-core"
version.workspace = true
edition.workspace = true
[dependencies]
fixed-dep = { path = "../../vendor" }
''')
        self.write("vendor/Cargo.toml", '''[package]
name = "fixed-dep"
version = "4.2.3"
edition = "2021"
''')
        for path in ("crates/browser/src/lib.rs", "crates/core/src/lib.rs", "vendor/src/lib.rs"):
            self.write(path, "pub fn value() -> u32 { 1 }\n")
        self.write("README.md", "# Browser\n\n## Status (v0.1.9)\n\nHistorical 0.1.9 measurement: 42/50.\n")
        self.run_command([CARGO, "generate-lockfile", "--offline"])
        self.run_command(["git", "init", "-q"])
        self.commit()
        self.run_command(["git", "update-ref", "refs/remotes/origin/main", "HEAD"])

    def write(self, name, text):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def run_command(self, args):
        return subprocess.run(args, cwd=self.repo, text=True, capture_output=True, check=True)

    def commit(self):
        self.run_command(["git", "add", "."])
        self.run_command(["git", "-c", "user.name=i234dev", "-c", "user.email=", "commit", "-qm", "Fixture delivery"])

    def helper(self, action="prepare", *extra, success=True):
        result = subprocess.run(
            [sys.executable, str(HELPER), action, "--repo", str(self.repo), "--cargo", CARGO, *extra],
            text=True, capture_output=True,
        )
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(result.stdout)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        return result

    def snapshot(self):
        return {p: (self.repo / p).read_bytes() for p in ("Cargo.toml", "Cargo.lock", "README.md")}

    def test_patch_nine_to_ten_is_idempotent_and_preserves_dependencies(self):
        original_lock = (self.repo / "Cargo.lock").read_text()
        result = self.helper()
        self.assertEqual(result["version"], "0.1.10")
        self.assertEqual(result["workspace_packages"], 2)
        self.assertIn("## Status (v0.1.10)", (self.repo / "README.md").read_text())
        self.assertIn("Historical 0.1.9 measurement: 42/50.", (self.repo / "README.md").read_text())
        self.assertEqual((self.repo / "Cargo.lock").read_text(), original_lock.replace('version = "0.1.9"', 'version = "0.1.10"'))
        prepared = self.snapshot()
        self.assertEqual(self.helper()["changed"], [])
        self.assertEqual(self.helper("check")["changed"], [])
        self.assertEqual(self.snapshot(), prepared)

    def test_next_published_delivery_advances_again(self):
        previous = self.run_command(["git", "rev-parse", "HEAD"]).stdout.strip()
        self.helper()
        self.commit()
        self.run_command(["git", "update-ref", "refs/remotes/origin/main", "HEAD"])
        self.assertEqual(self.helper("check", "--published-ref", previous)["version"], "0.1.10")
        self.assertEqual(self.helper()["version"], "0.1.11")

    def test_check_never_prepares_or_changes_files(self):
        before = self.snapshot()
        self.assertIn("run prepare first", self.helper("check", success=False).stderr)
        self.assertEqual(self.snapshot(), before)

    def test_metadata_failure_restores_all_version_files(self):
        before = self.snapshot()
        self.helper("prepare", "--cargo", str(self.repo / "missing-cargo"), success=False)
        self.assertEqual(self.snapshot(), before)
        self.assertFalse(list(self.repo.glob("Cargo.toml.*")))
        self.assertFalse(list(self.repo.glob("Cargo.lock.*")))

    def test_unexpected_version_cannot_skip_or_lower_a_delivery(self):
        for version in ("0.1.8", "0.1.11", "0.2.0"):
            with self.subTest(version=version):
                text = (self.repo / "Cargo.toml").read_text()
                data = tomllib.loads(text)["workspace"]["package"]["version"]
                self.write("Cargo.toml", text.replace(f'version = "{data}"', f'version = "{version}"'))
                before = self.snapshot()
                self.helper(success=False)
                self.assertEqual(self.snapshot(), before)

    def test_repairs_already_prepared_manifest_with_stale_lock_and_heading(self):
        text = (self.repo / "Cargo.toml").read_text().replace('version = "0.1.9"', 'version = "0.1.10"')
        self.write("Cargo.toml", text)
        self.assertEqual(self.helper()["version"], "0.1.10")
        self.helper("check")
        packages = tomllib.loads((self.repo / "Cargo.lock").read_text())["package"]
        self.assertEqual({p["name"]: p["version"] for p in packages}, {
            "rabbithole": "0.1.10", "rabbithole-core": "0.1.10", "fixed-dep": "4.2.3",
        })


if __name__ == "__main__":
    unittest.main()
