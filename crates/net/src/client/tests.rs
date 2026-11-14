use super::*;
use crate::h2::{frame, hpack};
use std::io::{BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

const WAIT: Duration = Duration::from_secs(10);

struct Peer {
    addr: SocketAddr,
    accepted: Arc<AtomicUsize>,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Peer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let stopping = Arc::new(AtomicBool::new(false));
        let (count, stop) = (accepted.clone(), stopping.clone());
        let thread = thread::spawn(move || {
            let mut sockets = Vec::new();
            let mut workers = Vec::new();
            for socket in listener.incoming() {
                let Ok(socket) = socket else { break };
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let number = count.fetch_add(1, Ordering::SeqCst);
                sockets.push(socket.try_clone().unwrap());
                workers.push(thread::spawn(move || serve(socket, number)));
            }
            for socket in sockets {
                let _ = socket.shutdown(Shutdown::Both);
            }
            for worker in workers {
                let _ = worker.join();
            }
        });
        Self { addr, accepted, stopping, thread: Some(thread) }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        // Wake accept; the stop check excludes this socket from the count.
        let _ = TcpStream::connect_timeout(&self.addr, WAIT);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(socket: TcpStream, number: usize) {
    socket.set_read_timeout(Some(WAIT)).unwrap();
    socket.set_write_timeout(Some(WAIT)).unwrap();
    let mut writer = socket.try_clone().unwrap();
    let mut reader = BufReader::new(socket);
    let mut preface = [0; 24];
    if reader.read_exact(&mut preface).is_err() || &preface != frame::PREFACE {
        return;
    }
    let _ = writer.write_all(&frame::settings_frame(&[]).to_bytes());
    let mut decoder = hpack::Decoder::new(65536);
    let mut encoder = hpack::Encoder::new(4096);
    let mut held = None;
    while let Ok(f) = frame::Frame::read(&mut reader, 1 << 20) {
        match f.kind {
            frame::SETTINGS if !f.has(frame::FLAG_ACK) => {
                let _ = writer.write_all(&frame::Frame::new(frame::SETTINGS, frame::FLAG_ACK, 0, vec![]).to_bytes());
            }
            frame::HEADERS => {
                let Ok(fragment) = f.header_block_fragment() else { return };
                let Ok(headers) = decoder.decode(fragment) else { return };
                let Some((_, path)) = headers.iter().find(|(name, _)| name == ":path") else { return };
                let body = format!("GET {path} conn{number}").into_bytes();
                let headers = encoder.encode(&[(":status".into(), "200".into()), ("content-length".into(), body.len().to_string())]);
                let mut out = Vec::new();
                frame::header_frames(f.stream, &headers, false, 16384, None, &mut out);
                if writer.write_all(&out).is_err() {
                    return;
                }
                if path == "/hold" {
                    // Keep a body outstanding until another stream arrives.
                    held = Some((f.stream, body));
                } else {
                    let _ = writer.write_all(&frame::Frame::new(frame::DATA, frame::FLAG_END_STREAM, f.stream, body).to_bytes());
                    if let Some((stream, body)) = held.take() {
                        let _ = writer.write_all(&frame::Frame::new(frame::DATA, frame::FLAG_END_STREAM, stream, body).to_bytes());
                    }
                }
            }
            frame::GOAWAY => return,
            _ => {}
        }
    }
}

fn get(client: &Arc<Client>, url: &Url) -> Result<WireResponse, ClientError> {
    client.send(&WireRequest {
        method: "GET", url, headers: &[], body: None,
        priority: Priority::Medium, incremental: false,
    }, WAIT)
}

#[test]
fn stale_h2_miss_reuses_published_connection_and_preserves_its_stream() {
    let peer = Peer::start();
    let client = Arc::new(Client::new(6));
    let first_url = Url::parse_absolute(&format!("http://{}/hold", peer.addr)).unwrap();
    let raced_url = Url::parse_absolute(&format!("http://{}/raced", peer.addr)).unwrap();
    client.h2c.lock().unwrap().insert(origin_key(&first_url));
    let (miss_tx, miss_rx) = mpsc::sync_channel(1);
    let (resume_tx, resume_rx) = mpsc::sync_channel(1);
    *client.after_h2_miss.lock().unwrap() = Some(Box::new(move || {
        miss_tx.send(()).unwrap();
        resume_rx.recv_timeout(WAIT).expect("release stale-miss request");
    }));
    let raced_client = client.clone();
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        let result = get(&raced_client, &raced_url).map_err(|error| format!("{error:?}")).and_then(|mut response| {
            let mut body = Vec::new();
            response.body.read_to_end(&mut body).map_err(|error| error.to_string())?;
            Ok((response.protocol, body))
        });
        let _ = done_tx.send(result);
    });
    miss_rx.recv_timeout(WAIT).expect("pause after the fast-path miss");
    let mut first = get(&client, &first_url).expect("publish the first connection");
    assert_eq!(first.protocol, "h2");
    assert_eq!(client.live_h2(&origin_key(&first_url)).unwrap().open_streams(), 1);
    resume_tx.send(()).unwrap();
    let raced = done_rx.recv_timeout(WAIT).expect("stale-miss request completes");
    let mut first_body = Vec::new();
    let first_result = first.body.read_to_end(&mut first_body);
    client.clear();
    worker.join().unwrap();
    let connections = peer.accepted.load(Ordering::SeqCst);
    drop(peer);
    assert_eq!(raced.unwrap(), ("h2", b"GET /raced conn0".to_vec()));
    first_result.expect("the published connection's outstanding body survives");
    assert_eq!(first_body, b"GET /hold conn0");
    assert_eq!(connections, 1, "a stale miss must not open a second connection");
}
