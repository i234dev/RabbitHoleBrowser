//! Web API completeness: the inventory coverage check and tests for the
//! interfaces added to make the DOM / Web API surface complete.

use crate::tests::{run, setup};

/// Floor for `webapi_coverage` (raise it as coverage improves).
const MIN_COVERAGE: f64 = 0.0;

/// Run tools/webapi/check.js against a fresh page and report coverage of
/// tools/webapi/inventory.tsv. Set `RH_WEBAPI_REPORT=<path>` to write the
/// list of missing members.
#[test]
fn webapi_coverage() {
    let (mut env, mut host) = setup("<!doctype html><html><head><title>t</title></head><body><div id=a></div></body></html>");
    let inv = include_str!("../../../tools/webapi/inventory.tsv");
    let check = include_str!("../../../tools/webapi/check.js");
    host.realm().define_global("__RH_INV", rabbithole_js::Value::str(inv));
    let src = format!(
        "{check}\n(function () {{
          const r = JSON.parse(__rhWebApiCoverage(__RH_INV, true));
          const lines = [];
          for (const [k, s] of Object.entries(r.specs))
            lines.push(k.padEnd(22) + s.tier.padEnd(9) + String(s.present).padStart(5) + '/' + String(s.total).padEnd(5) + (100 * s.present / s.total).toFixed(1) + '%');
          lines.push('TOTAL'.padEnd(31) + String(r.present).padStart(5) + '/' + r.total + ' ' + (100 * r.present / r.total).toFixed(1) + '%');
          console.log(lines.join('\\n'));
          const miss = [];
          for (const [k, s] of Object.entries(r.specs)) for (const m of s.missing) miss.push(k + '\\t' + m);
          console.log(miss.join('\\n'));
          console.log(String(r.present / r.total));
        }})();"
    );
    let out = run(&mut env, &mut host, &src);
    eprintln!("{}", out[0]);
    if let Ok(path) = std::env::var("RH_WEBAPI_REPORT") {
        std::fs::write(path, format!("{}\n\nMISSING:\n{}\n", out[0], out[1])).unwrap();
    }
    let ratio: f64 = out[2].parse().unwrap();
    assert!(ratio >= MIN_COVERAGE, "Web API coverage regressed: {ratio} < {MIN_COVERAGE}");
}
