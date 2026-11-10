//! Live view: a tiny local HTTP server so a human can watch the agent's
//! tab (the latest frame is re-rendered after every tool call).

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Shared {
    png: Vec<u8>,
    caption: String,
    version: u64,
}

/// A running live-view server.
pub struct Live {
    shared: Arc<Mutex<Shared>>,
    pub url: String,
}

const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>RabbitHole agent — live</title>
<style>body{margin:0;background:#1e1e1e;color:#ddd;font:13px system-ui,sans-serif}#c{padding:6px 10px;white-space:pre-wrap}img{display:block;max-width:100%;border-top:1px solid #444}</style></head>
<body><div id="c">waiting for the agent…</div><img id="f" alt="">
<script>
let v=-1;
async function poll(){try{const r=await fetch('/status');const t=await r.text();const i=t.indexOf('\n');const nv=parseInt(t.slice(0,i));if(nv!==v){v=nv;document.getElementById('c').textContent=t.slice(i+1);document.getElementById('f').src='/frame.png?v='+nv;}}catch(e){}setTimeout(poll,500);}
poll();
</script></body></html>"#;

impl Live {
    /// Start listening on 127.0.0.1 (`port` 0 = any free port).
    pub fn start(port: u16) -> Result<Live, String> {
        let l = TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("cannot open live view port: {e}"))?;
        let addr = l.local_addr().map_err(|e| e.to_string())?;
        let shared = Arc::new(Mutex::new(Shared::default()));
        let s2 = shared.clone();
        std::thread::spawn(move || {
            for stream in l.incoming() {
                let Ok(mut stream) = stream else { continue };
                let s3 = s2.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(match stream.try_clone() {
                        Ok(s) => s,
                        Err(_) => return,
                    });
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        return;
                    }
                    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                    loop {
                        let mut h = String::new();
                        if reader.read_line(&mut h).map(|n| n == 0).unwrap_or(true) || h.trim().is_empty() {
                            break;
                        }
                    }
                    let (ct, body): (&str, Vec<u8>) = if path.starts_with("/frame.png") {
                        ("image/png", s3.lock().map(|s| s.png.clone()).unwrap_or_default())
                    } else if path.starts_with("/status") {
                        ("text/plain; charset=utf-8", s3.lock().map(|s| format!("{}\n{}", s.version, s.caption).into_bytes()).unwrap_or_default())
                    } else {
                        ("text/html; charset=utf-8", PAGE.as_bytes().to_vec())
                    };
                    let head = format!("HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", body.len());
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                });
            }
        });
        Ok(Live { shared, url: format!("http://{addr}/") })
    }

    /// Publish a new frame.
    pub fn publish(&self, png: Vec<u8>, caption: String) {
        if let Ok(mut s) = self.shared.lock() {
            s.png = png;
            s.caption = caption;
            s.version += 1;
        }
    }
}
