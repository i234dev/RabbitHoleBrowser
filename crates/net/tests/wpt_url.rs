//! WHATWG URL conformance data from web-platform-tests
//! (`url/resources/urltestdata.json`, `setters_tests.json`), run against
//! the parser directly. Skipped without a checkout (`RH_WPT_ROOT`);
//! `RH_URL_VERBOSE=1` prints failures.

use rabbithole_net::Url;
use serde_json::Value;

fn root() -> std::path::PathBuf {
    std::env::var_os("RH_WPT_ROOT").map(Into::into).unwrap_or_else(|| "/mnt/hdd5tb/rabbithole/suites/wpt-dom".into())
}

fn components(u: &Url) -> Vec<(&'static str, String)> {
    vec![
        ("href", u.to_string()),
        ("protocol", format!("{}:", u.scheme)),
        ("username", u.username.clone()),
        ("password", u.password.clone()),
        ("host", u.host_component()),
        ("hostname", u.host.clone()),
        ("port", u.port.map(|p| p.to_string()).unwrap_or_default()),
        ("pathname", u.pathname()),
        ("search", u.search()),
        ("hash", u.hash()),
    ]
}

fn load(name: &str) -> Option<Value> {
    let text = std::fs::read_to_string(root().join("url/resources").join(name)).ok()?;
    serde_json::from_str(&text).ok()
}

#[test]
fn urltestdata() {
    let Some(data) = load("urltestdata.json") else {
        eprintln!("skipping: no WPT checkout");
        return;
    };
    let verbose = std::env::var_os("RH_URL_VERBOSE").is_some();
    let (mut pass, mut total) = (0, 0);
    for case in data.as_array().unwrap() {
        let Some(input) = case["input"].as_str() else { continue };
        total += 1;
        let base = match case["base"].as_str() {
            Some(b) => match Url::parse_absolute(b) {
                Some(u) => Some(u),
                None => {
                    if verbose {
                        eprintln!("bad base {b:?}");
                    }
                    continue;
                }
            },
            None => None,
        };
        let got = match &base {
            Some(b) => b.join(input),
            None => Url::parse_absolute(input),
        };
        let ok = if case["failure"].as_bool() == Some(true) {
            got.is_none()
        } else {
            match &got {
                None => false,
                Some(u) => components(u).iter().all(|(k, v)| case[*k].as_str().map(|e| e == v).unwrap_or(true)),
            }
        };
        if ok {
            pass += 1;
        } else if verbose {
            eprintln!("FAIL {input:?} base {:?}: got {:?} expected {}", case["base"], got.as_ref().map(|u| components(u)), case);
        }
    }
    eprintln!("urltestdata: {pass}/{total}");
    assert!(pass * 100 >= total * 99, "URL parser regressed: {pass}/{total}");
}

#[test]
fn setters() {
    let Some(data) = load("setters_tests.json") else { return };
    let verbose = std::env::var_os("RH_URL_VERBOSE").is_some();
    let (mut pass, mut total) = (0, 0);
    for (setter, cases) in data.as_object().unwrap() {
        let Some(cases) = cases.as_array().filter(|_| setter != "comment") else { continue };
        for case in cases {
            total += 1;
            let Some(mut u) = Url::parse_absolute(case["href"].as_str().unwrap_or("")) else { continue };
            let v = case["new_value"].as_str().unwrap_or("");
            match setter.as_str() {
                "protocol" => u.set_protocol(v),
                "username" => u.set_username(v),
                "password" => u.set_password(v),
                "host" => u.set_host(v),
                "hostname" => u.set_hostname(v),
                "port" => u.set_port(v),
                "pathname" => u.set_pathname(v),
                "search" => u.set_search(v),
                "hash" => u.set_hash(v),
                "href" => {
                    if let Some(n) = Url::parse_absolute(v) {
                        u = n;
                    }
                }
                _ => continue,
            }
            let comps = components(&u);
            let expected = case["expected"].as_object().unwrap();
            let ok = expected.iter().all(|(k, e)| comps.iter().find(|(n, _)| n == k).map(|(_, v)| Some(v.as_str()) == e.as_str()).unwrap_or(true));
            if ok {
                pass += 1;
            } else if verbose {
                eprintln!("FAIL {setter} {:?} = {v:?}: got {comps:?} expected {:?}", case["href"], expected);
            }
        }
    }
    eprintln!("setters: {pass}/{total}");
    assert!(pass * 100 >= total * 99, "URL setters regressed: {pass}/{total}");
}

#[test]
fn idna_v2() {
    let Some(data) = load("IdnaTestV2.json") else { return };
    let verbose = std::env::var_os("RH_URL_VERBOSE").is_some();
    let (mut pass, mut total) = (0, 0);
    for case in data.as_array().unwrap() {
        let Some(input) = case["input"].as_str() else { continue };
        if input.is_empty() {
            continue;
        }
        total += 1;
        let got = Url::parse_absolute(&format!("https://{input}/x")).map(|u| u.host_component());
        let ok = match case["output"].as_str() {
            None => got.is_none(),
            Some(e) => got.as_deref() == Some(e),
        };
        if ok {
            pass += 1;
        } else if verbose {
            eprintln!("IDNA FAIL {input:?}: got {got:?} expected {:?}", case["output"]);
        }
    }
    eprintln!("IdnaTestV2: {pass}/{total}");
    assert!(pass * 100 >= total * 50, "IDNA regressed: {pass}/{total}");
}
