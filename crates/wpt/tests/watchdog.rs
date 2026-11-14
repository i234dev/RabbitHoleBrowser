//! Self-contained runner regressions, independent of a WPT checkout.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rabbithole_wpt::runner::{parse_report, TestResult};
use rabbithole_wpt::server::Server;

struct FixtureRoot(PathBuf);

impl Drop for FixtureRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run_fixture(body: &str) -> TestResult {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root = FixtureRoot(std::env::temp_dir().join(format!(
        "rabbithole-wpt-watchdog-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    std::fs::create_dir_all(&root.0).unwrap();
    std::fs::write(root.0.join("page.html"), format!(r#"<!doctype html><meta charset=utf-8>
<script>
const tests = [];
function test(name) {{ tests.push({{name, status: 0, message: null}}); }}
addEventListener('load', () => console.log('@@WPT@@' + JSON.stringify({{status: 0, message: null, tests}})));
{body}
</script>"#)).unwrap();
    let server = Server::start(&root.0).unwrap();
    let url = format!("{}/page.html", server.base());
    let mut child = Command::new(env!("CARGO_BIN_EXE_rh-wpt"))
        .args(["child", &url, "5000"])
        // Each child has its own OnceLock/environment; parallel Rust tests
        // and the product's default five-second budget are unaffected.
        .env("RH_TASK_BUDGET_MS", "250")
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("watchdog fixture child did not terminate");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let report = stdout.lines().filter(|line| line.starts_with('{')).last().expect("harness report");
    parse_report("/page.html", report, 0).expect("valid harness report")
}

#[test]
fn watchdog_aborting_test_registration_cannot_report_partial_ok() {
    for body in [
        "test('registered before loop'); while (true) {} test('unreachable');",
        "test('registered before loop'); Promise.resolve().then(() => {while (true) {} test('unreachable');});",
        "test('registered before loop'); addEventListener('load', () => {while (true) {} test('unreachable');});",
    ] {
        let result = run_fixture(body);
        assert_eq!(result.harness, "TIMEOUT", "{body}");
        assert!(result.message.as_deref().unwrap_or("").contains("watchdog"));
        assert_eq!(result.subtests.len(), 1);
        assert_eq!(result.subtests[0].name, "registered before loop");
        assert_eq!(result.subtests[0].status, "PASS");
    }
}

#[test]
fn console_error_with_watchdog_text_does_not_invalidate_completed_tests() {
    let result = run_fixture("console.error('Script execution timed out'); test('completed normally');");
    assert_eq!(result.harness, "OK");
    assert_eq!(result.message, None);
    assert_eq!(result.subtests.len(), 1);
    assert_eq!(result.subtests[0].name, "completed normally");
    assert_eq!(result.subtests[0].status, "PASS");
}
