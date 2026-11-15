//! Explicit waits through the public dispatcher, using only loopback fixtures.
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rabbithole_agent::{call, Agent, Config, Out};
use serde_json::{json, Value};

const PAGE: &str = r#"<!doctype html><title>Explicit waits</title><main>
<p id="state">Initial state</p><div id="result"></div>
<button id="insert" onclick="document.getElementById('state').textContent='Immediate change'; setTimeout(function(){var p=document.createElement('p');p.id='late';p.textContent='Late pingüino 🐇';document.getElementById('result').appendChild(p)},2000)">Insert</button>
<button id="remove" onclick="setTimeout(function(){document.getElementById('remove').remove()},50)">Remove me</button>
<button id="request" onclick="fetch('/slow').then(function(r){return r.text()}).then(function(t){document.getElementById('result').textContent=t})">Request</button>
<input id="typed" aria-label="Typed" oninput="setTimeout(function(){document.getElementById('result').textContent='Typed completion'},70)">
<label>Filled<input id="filled" oninput="setTimeout(function(){var p=document.createElement('p');p.id='filled-done';p.textContent='Filled completion';document.getElementById('result').appendChild(p)},70)"></label>
<button id="microtask" onclick="Promise.resolve().then(function(){document.getElementById('result').textContent='Microtask completion'})">Microtask</button>
<button id="count" onclick="document.getElementById('state').textContent='MUTATED'">Count</button>
</main>"#;

struct Server { address: String, stop: Arc<AtomicBool>, thread: Option<JoinHandle<()>> }
impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut workers = Vec::new();
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                if stopping.load(Ordering::Acquire) { break; }
                workers.push(std::thread::spawn(move || {
                    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 { return; }
                    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 { return; }
                        if line == "\r\n" || line == "\n" { break; }
                    }
                    let body = match path.as_str() {
                        "/slow" => { std::thread::sleep(Duration::from_millis(1200)); "Request completion" }
                        "/frames" => "<!doctype html><title>Frames</title><iframe src='/child' width='500' height='250'></iframe>",
                        "/child" => "<!doctype html><button id='frame-go' onclick=\"setTimeout(function(){document.body.insertAdjacentHTML('beforeend','<p>Frame completion</p>'); alert('wait dialog');window.open('/popup')},50)\">Frame action</button>",
                        "/next" => "<!doctype html><title>Next</title><p id='next'>After navigation</p><script>setTimeout(function(){document.getElementById('next').textContent='Navigation completion'},600)</script>",
                        _ => PAGE,
                    };
                    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = stream.write_all(response.as_bytes());
                }));
            }
            for worker in workers { let _ = worker.join(); }
        });
        Self { address, stop, thread: Some(thread) }
    }
    fn url(&self) -> String { format!("http://{}/", self.address) }
    fn agent(&self) -> Agent {
        let mut agent = Agent::new(Config { settle_cap: Duration::from_secs(3), ..Config::default() });
        run(&mut agent, "open", json!({"url":self.url()}));
        agent
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(&self.address);
        if let Some(thread) = self.thread.take() { thread.join().unwrap(); }
    }
}
fn run(agent: &mut Agent, name: &str, args: Value) -> Out {
    call(agent, name, &args).unwrap_or_else(|error| panic!("{name} {args}: {error}"))
}
fn element(agent: &mut Agent, selector: &str) -> String {
    let output = run(agent, "find", json!({"selector":selector})).text;
    output.lines().find_map(|line| line.split_whitespace().next().filter(|s| s.starts_with("@e"))).unwrap_or_else(|| panic!("{output}")).to_string()
}

#[test]
fn explicit_find_waits_two_seconds_and_timeout_preserves_pending_timer() {
    let server = Server::start();
    let mut agent = server.agent();
    run(&mut agent, "debug", json!({"eval":"setTimeout(function(){document.getElementById('result').textContent='Long timer completion 🐇'},2000)"}));
    let start = Instant::now();
    let timeout = run(&mut agent, "find", json!({"text":"Long timer completion 🐇","wait":300}));
    assert!(timeout.failed && timeout.text.contains("Wait timed out") && timeout.text.contains("Pending: timer in"), "{}", timeout.text);
    assert!(start.elapsed() >= Duration::from_millis(250) && start.elapsed() < Duration::from_millis(1500), "{:?}", start.elapsed());
    let completed = run(&mut agent, "find", json!({"text":"Long timer completion 🐇","wait":3000}));
    assert!(!completed.failed && completed.text.contains("Wait satisfied") && completed.text.contains("Long timer completion 🐇"), "{}", completed.text);
    assert!(start.elapsed() >= Duration::from_millis(1700), "must actually wait for the 2 s timer");
}

#[test]
fn explicit_click_wait_returns_the_complete_action_to_condition_delta() {
    let server = Server::start();
    let mut agent = server.agent();
    let button = element(&mut agent, "#insert");
    let start = Instant::now();
    let output = run(&mut agent, "click", json!({"ref":button,"until":{"text":"Late pingüino 🐇"},"timeout":3000}));
    assert!(!output.failed && output.text.contains("Wait satisfied"), "{}", output.text);
    assert!(output.text.contains("Initial state") && output.text.contains("Immediate change") && output.text.contains("Late pingüino 🐇"), "both immediate and late deltas: {}", output.text);
    assert!(start.elapsed() >= Duration::from_millis(1700), "must actually wait for the 2 s insertion");
    let selected = run(&mut agent, "find", json!({"selector":"#late"}));
    assert!(selected.text.contains("@e") && selected.text.contains("Late pingüino 🐇"), "{}", selected.text);
}

#[test]
fn explicit_find_gone_uses_css_or_role_name_presence_and_zero_is_immediate() {
    let server = Server::start();
    let mut agent = server.agent();
    let button = element(&mut agent, "#remove");
    run(&mut agent, "click", json!({"ref":button,"until":{"selector":"#state"},"timeout":0}));
    let gone = run(&mut agent, "find", json!({"role":"button","name":"Remove me","gone":true,"wait":1000}));
    assert!(!gone.failed && gone.text.contains("Wait satisfied (gone)"), "{}", gone.text);
    let absent = run(&mut agent, "find", json!({"selector":"#remove","gone":true,"wait":0}));
    assert!(!absent.failed, "{}", absent.text);
    let present = run(&mut agent, "find", json!({"selector":"#state","gone":true,"wait":0}));
    assert!(present.failed && present.text.contains("Pending: no pending timers or requests"), "{}", present.text);
}

#[test]
fn explicit_wait_timeout_reports_pending_fetch_then_delivers_its_promise() {
    let server = Server::start();
    let mut agent = server.agent();
    let button = element(&mut agent, "#request");
    let start = Instant::now();
    let timeout = run(&mut agent, "click", json!({"ref":button,"until":{"text":"Request completion"},"timeout":300}));
    assert!(timeout.failed && timeout.text.contains("Wait timed out") && timeout.text.contains("Pending: GET /slow"), "{}", timeout.text);
    assert!(start.elapsed() < Duration::from_millis(1000), "automatic settle must not consume the 1.2 s request first: {:?}", start.elapsed());
    let done = run(&mut agent, "find", json!({"text":"Request completion","wait":3000}));
    assert!(!done.failed && done.text.contains("Wait satisfied"), "{}", done.text);
}

#[test]
fn explicit_type_fill_and_microtask_conditions_use_the_same_observer() {
    let server = Server::start();
    let mut agent = server.agent();
    let input = element(&mut agent, "#typed");
    let typed = run(&mut agent, "type", json!({"ref":input,"text":"x","until":{"text":"Typed completion"},"timeout":1000}));
    assert!(!typed.failed && typed.text.contains("Typed completion"), "{}", typed.text);
    let input = element(&mut agent, "#filled");
    let filled = run(&mut agent, "fill", json!({"ref":input,"value":"y","until":{"selector":"#filled-done"},"timeout":1000}));
    assert!(!filled.failed && filled.text.contains("Filled completion"), "{}", filled.text);
    let button = element(&mut agent, "#microtask");
    let microtask = run(&mut agent, "click", json!({"ref":button,"until":{"text":"Microtask completion"},"timeout":1000}));
    assert!(!microtask.failed && microtask.text.contains("Microtask completion"), "{}", microtask.text);
}

#[test]
fn explicit_find_follows_timer_navigation_and_keeps_the_original_deadline() {
    let server = Server::start();
    let mut agent = server.agent();
    run(&mut agent, "debug", json!({"eval":"setTimeout(function(){location.href='/next'},100)"}));
    let timeout = run(&mut agent, "find", json!({"text":"Navigation completion","wait":300}));
    assert!(timeout.failed && timeout.text.contains("Wait timed out") && timeout.text.contains("timer in"), "{}", timeout.text);
    let page = agent.tabs[agent.active.unwrap()].page().unwrap();
    assert_eq!(page.url.path, "/next");
    let done = run(&mut agent, "find", json!({"text":"Navigation completion","wait":1500}));
    assert!(!done.failed && done.text.contains("Wait satisfied"), "{}", done.text);
}

#[test]
fn explicit_wait_validation_precedes_action_and_errors_restore_auto_wait() {
    let server = Server::start();
    let mut agent = server.agent();
    let button = element(&mut agent, "#count");
    for options in [json!({"until":"bad"}), json!({"until":{"text":"x","selector":"p"}}), json!({"until":{"text":" "}}), json!({"until":{"selector":"["}}), json!({"until":{"text":"x"},"timeout":30001}), json!({"until":{"text":"x"},"timeout":-1}), json!({"until":{"text":"x"},"timeout":0.5}), json!({"timeout":300})] {
        let mut args = options;
        args["ref"] = json!(button);
        assert!(call(&mut agent, "click", &args).is_err(), "{args}");
    }
    for options in [json!({"wait":30001}), json!({"wait":-1}), json!({"wait":0.5}), json!({"gone":"true"})] {
        let mut args = options;
        args["selector"] = json!("#state");
        assert!(call(&mut agent, "find", &args).is_err(), "{args}");
    }
    let page = agent.tabs[agent.active.unwrap()].page().unwrap();
    assert_eq!(page.doc.text_content(page.doc.element_by_id("state").unwrap()), "Initial state");
    // This error occurs inside the scoped defer flag, after wait validation.
    assert!(call(&mut agent, "click", &json!({"ref":"@e999999","until":{"text":"x"},"timeout":0})).is_err());
    let input = element(&mut agent, "#typed");
    let ordinary = run(&mut agent, "type", json!({"ref":input,"text":"x"}));
    assert!(ordinary.text.contains("Typed completion"), "normal automatic settle restored after error: {}", ordinary.text);
}

#[test]
fn explicit_wait_pumps_child_frame_and_delivers_dialog_and_popup_once() {
    let server = Server::start();
    let mut agent = server.agent();
    run(&mut agent, "open", json!({"url":format!("{}frames",server.url())}));
    let button = element(&mut agent, "#frame-go");
    let output = run(&mut agent, "click", json!({"ref":button,"until":{"text":"Frame completion"},"timeout":1000}));
    assert!(!output.failed && output.text.contains("Frame completion"), "{}", output.text);
    assert_eq!(output.text.matches("Dialog alert(").count(), 1, "{}", output.text);
    assert_eq!(output.text.matches("New tab t").count(), 1, "{}", output.text);
    assert_eq!(agent.tabs.len(), 2);
    assert_eq!(agent.active, Some(0));
    let next = run(&mut agent, "find", json!({"text":"Frame completion","wait":0}));
    assert!(!next.failed && !next.text.contains("New tab t") && !next.text.contains("Dialog alert("), "{}", next.text);
}
