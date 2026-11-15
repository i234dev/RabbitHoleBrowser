//! The delivered CLI must select native execution without a Realm test override.
#![cfg(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos", windows)))]

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PROBE: &str = "__rabbithole_cli_native_probe_2a6";
const SOURCE: &str = "function __rabbithole_cli_native_probe_2a6(n) { var sum = 0; for (var i = 0; i < n; i++) sum += i; return sum; } __rabbithole_cli_native_probe_2a6(4096)";
const EXPECTED: &str = "8386560";

struct PrivateProfile(PathBuf);

impl PrivateProfile {
    fn new() -> Self {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("rh-jit-cli-{}-{nonce}", std::process::id()));
        fs::create_dir(&path).expect("create isolated CLI test directory");
        Self(path)
    }
}

impl Drop for PrivateProfile {
    fn drop(&mut self) {
        // Also run on assertion failure, after Running has killed/reaped any child.
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() { let _ = self.0.kill(); }
        let _ = self.0.wait();
    }
}

fn run_cli(root: &Path, selector: &str) -> Output {
    let stdout = root.join(format!("{selector}.stdout"));
    let stderr = root.join(format!("{selector}.stderr"));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rabbithole"));
    cmd.args(["js", "about:blank", SOURCE, "--wait", "0"])
        .arg(format!("--jit={selector}"))
        .arg("--jit-threshold=0");
    // Filters, caps, dumps and entry thresholds from the caller must not change
    // this probe. The opposing ambient selector/threshold prove CLI precedence.
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().to_ascii_uppercase().starts_with("RABBITHOLE_JIT") {
            cmd.env_remove(key);
        }
    }
    cmd.env("RABBITHOLE_PROFILE", root.join(format!("profile-{selector}")))
        .env("RABBITHOLE_JIT", if selector == "on" { "off" } else { "on" })
        .env("RABBITHOLE_JIT_THRESHOLD", u32::MAX.to_string())
        .env("RABBITHOLE_JIT_TRACE", "1")
        .env("RABBITHOLE_JIT_STATS", "1")
        .env("RABBITHOLE_JIT_ONLY", PROBE)
        .stdin(Stdio::null())
        // Files avoid an undrained stdout/stderr pipe blocking the child.
        .stdout(Stdio::from(File::create(&stdout).unwrap()))
        .stderr(Stdio::from(File::create(&stderr).unwrap()));
    let mut child = Running(cmd.spawn().expect("spawn actual rabbithole CLI"));
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.0.try_wait().expect("poll CLI process") { break status; }
        assert!(Instant::now() < deadline, "CLI --jit={selector} exceeded 30 seconds; stderr: {}",
            fs::read_to_string(&stderr).unwrap_or_default());
        std::thread::sleep(Duration::from_millis(10));
    };
    Output { status, stdout: fs::read(stdout).unwrap(), stderr: fs::read(stderr).unwrap() }
}

fn entries(stderr: &str) -> Vec<u64> {
    stderr.lines().filter(|line| line.starts_with("[jit] samples ")).map(|line| {
        line.split(" | ").find_map(|field| field.strip_prefix("entries "))
            .expect("stats row contains native entry count")
            .parse().expect("native entry count is an integer")
    }).collect()
}

#[test]
fn cli_jit_switch_controls_real_native_execution() {
    let profile = PrivateProfile::new();
    let compiled = format!("[jit] compiled {PROBE}@");
    let mut results = Vec::new();
    for selector in ["off", "on"] {
        let output = run_cli(&profile.0, selector);
        let stdout = String::from_utf8(output.stdout).expect("CLI output is UTF-8");
        let stderr = String::from_utf8(output.stderr).expect("CLI diagnostics are UTF-8");
        assert!(output.status.success(), "--jit={selector}: {}\n{stderr}", output.status);
        assert_eq!(stdout.trim(), EXPECTED, "--jit={selector}\n{stderr}");
        let counts = entries(&stderr);
        assert!(!counts.is_empty(), "--jit={selector} emitted no stats witness:\n{stderr}");
        let named_compile = stderr.lines().any(|line| line.starts_with(&compiled));
        if selector == "on" {
            assert!(named_compile, "CLI selector did not compile the named probe:\n{stderr}");
            assert!(counts.iter().any(|&n| n > 0), "compiled probe never entered native code:\n{stderr}");
        } else {
            assert!(!stderr.lines().any(|line| line.starts_with("[jit] compiled ")), "off compiled native code:\n{stderr}");
            assert!(counts.iter().all(|&n| n == 0), "off entered native code:\n{stderr}");
        }
        eprintln!("jit-cli-canary selector={selector} named_compiled={named_compile} entries={counts:?} result={EXPECTED}");
        results.push(stdout);
    }
    assert_eq!(results[0], results[1], "VM and native CLI results differ");
    fs::remove_dir_all(&profile.0).expect("remove private CLI profiles and capture files");
}
