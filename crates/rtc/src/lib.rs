//! Nonblocking WebRTC transport. Callers choose the interfaces available
//! to an origin and drive ICE, DTLS and SCTP from the browser event loop.
mod certificate;
pub use certificate::Certificate;

use std::collections::HashMap;
use std::io::{self, ErrorKind};
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::channel::ChannelId;
pub use str0m::channel::{ChannelConfig, Reliability};
use str0m::net::{Protocol, Receive};
pub use str0m::IceConnectionState;
use str0m::{Candidate, Event, Input, Output, Rtc};

#[derive(Debug)]
pub enum PeerEvent {
    Closed,
    SelectedPair { local: SocketAddr, remote: SocketAddr },
    Ice(IceConnectionState),
    Connected,
    Open {
        handle: u32,
        stream: u16,
        config: ChannelConfig,
    },
    Message {
        handle: u32,
        binary: bool,
        data: Vec<u8>,
    },
    Close(u32),
    BufferedLow(u32),
}

/// A full application message must fit both the send buffer and the negotiated limit.
pub const MESSAGE_CAPACITY: u32 = 256 * 1024;

pub struct Peer {
    rtc: Rtc,
    sockets: Vec<(SocketAddr, UdpSocket)>,
    channels: HashMap<u32, ChannelId>,
    queued: Vec<(u32, ChannelConfig)>,
    next_handle: u32,
    pending: Option<SdpPendingOffer>,
    offer: Option<String>,
    application: bool,
    selected_pair: Option<(SocketAddr, SocketAddr)>,
}

impl Peer {
    pub fn bind(addresses: &[IpAddr]) -> io::Result<Self> {
        Self::bind_with_certificate(addresses, None)
    }

    pub fn bind_with_certificate(addresses: &[IpAddr], cert: Option<&Certificate>) -> io::Result<Self> {
        let mut config = str0m::RtcConfig::new();
        if let Some(cert) = cert {
            if cert.expired() { return Err(io::Error::other("The DTLS certificate has expired")); }
            config = config.set_dtls_cert(cert.identity.clone());
        }
        let mut rtc = config.set_sctp_max_message_size(MESSAGE_CAPACITY)
            .set_sctp_max_buffered_amount(MESSAGE_CAPACITY as usize)
            .set_stats_interval(Some(Duration::from_millis(100)))
            .build(Instant::now());
        let mut sockets = Vec::new();
        for &ip in addresses {
            let socket = UdpSocket::bind((ip, 0))?;
            socket.set_nonblocking(true)?;
            let addr = socket.local_addr()?;
            let candidate = Candidate::host(addr, "udp").map_err(io::Error::other)?;
            if rtc.add_local_candidate(candidate).is_some() {
                sockets.push((addr, socket));
            }
        }
        Ok(Self {
            rtc,
            sockets,
            channels: HashMap::new(),
            queued: Vec::new(),
            next_handle: 0,
            pending: None,
            offer: None,
            application: false,
            selected_pair: None,
        })
    }

    fn allocate_handle(&mut self) -> io::Result<u32> {
        self.next_handle = self
            .next_handle
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Too many data channels"))?;
        Ok(self.next_handle)
    }

    pub fn add_channel(&mut self, config: ChannelConfig) -> io::Result<u32> {
        let handle = self.allocate_handle()?;
        if self.application {
            let id = self.rtc.direct_api().create_data_channel(config);
            self.channels.insert(handle, id);
        } else {
            self.queued.push((handle, config));
        }
        Ok(handle)
    }

    pub fn create_offer(&mut self) -> io::Result<String> {
        if !self.queued.is_empty() && self.pending.is_none() {
            let mut changes = self.rtc.sdp_api();
            for (handle, config) in &self.queued {
                let id = changes.add_channel_with_config(config.clone());
                self.channels.insert(*handle, id);
            }
            if let Some((offer, pending)) = changes.apply() {
                self.offer = Some(offer.to_sdp_string());
                self.pending = Some(pending);
            }
        }
        self.offer
            .clone()
            .ok_or_else(|| io::Error::other("No negotiated media or data channels"))
    }

    pub fn answer_offer(&mut self, sdp: &str) -> io::Result<String> {
        let offer = SdpOffer::from_sdp_string(sdp).map_err(io::Error::other)?;
        let answer = self
            .rtc
            .sdp_api()
            .accept_offer(offer)
            .map_err(io::Error::other)?
            .to_sdp_string();
        self.pending = None;
        self.application = answer.lines().any(|line| {
            line.starts_with("m=application ") && !line.starts_with("m=application 0 ")
        });
        if self.application {
            for (handle, config) in self.queued.drain(..) {
                let id = self.rtc.direct_api().create_data_channel(config);
                self.channels.insert(handle, id);
            }
        }
        Ok(answer)
    }

    pub fn accept_answer(&mut self, sdp: &str) -> io::Result<()> {
        let answer = SdpAnswer::from_sdp_string(sdp).map_err(io::Error::other)?;
        let pending = self
            .pending
            .take()
            .ok_or_else(|| io::Error::other("No pending offer"))?;
        self.rtc
            .sdp_api()
            .accept_answer(pending, answer)
            .map_err(io::Error::other)?;
        self.application = sdp.lines().any(|line| {
            line.starts_with("m=application ") && !line.starts_with("m=application 0 ")
        });
        if self.application {
            for (handle, config) in self.queued.drain(..) {
                self.channels
                    .entry(handle)
                    .or_insert_with(|| self.rtc.direct_api().create_data_channel(config));
            }
        }
        Ok(())
    }

    pub fn add_candidate(&mut self, sdp: &str) -> io::Result<()> {
        self.rtc
            .add_remote_candidate(Candidate::from_sdp_string(sdp).map_err(io::Error::other)?);
        Ok(())
    }

    pub fn send(&mut self, handle: u32, binary: bool, bytes: &[u8]) -> io::Result<bool> {
        let mut channel = self
            .channels
            .get(&handle)
            .and_then(|id| self.rtc.channel(*id))
            .ok_or_else(|| io::Error::other("Data channel is closed"))?;
        channel.write(binary, bytes).map_err(io::Error::other)
    }

    pub fn close(&mut self) -> io::Result<()> {
        self.rtc.close().map_err(io::Error::other)?;
        // Drain pending close_notify datagrams before dropping the UDP socket.
        self.poll().map(|_| ())
    }

    pub fn buffered_amount(&mut self, handle: u32) -> usize {
        self.channels.get(&handle).and_then(|id| self.rtc.channel(*id))
            .map_or(0, |mut channel| channel.buffered_amount())
    }

    pub fn close_channel(&mut self, handle: u32) {
        self.queued.retain(|(h, _)| *h != handle);
        if let Some(&id) = self.channels.get(&handle) {
            self.rtc.direct_api().close_data_channel(id);
        }
    }

    fn handle(&self, id: ChannelId) -> Option<u32> {
        self.channels
            .iter()
            .find_map(|(&handle, &value)| (value == id).then_some(handle))
    }

    fn event(&mut self, event: Event) -> io::Result<Option<PeerEvent>> {
        Ok(match event {
            Event::Closed => Some(PeerEvent::Closed),
            Event::PeerStats(stats) => {
                let pair = stats.selected_candidate_pair.map(|p| (p.local.addr, p.remote.addr));
                if self.selected_pair == pair { return Ok(None); }
                self.selected_pair = pair;
                pair.map(|(local, remote)| PeerEvent::SelectedPair { local, remote })
            }
            Event::Connected => Some(PeerEvent::Connected),
            Event::IceConnectionStateChange(state) => Some(PeerEvent::Ice(state)),
            Event::ChannelOpen(id, label) => {
                let handle = match self.handle(id) {
                    Some(handle) => handle,
                    None => {
                        let handle = self.allocate_handle()?;
                        self.channels.insert(handle, id);
                        handle
                    }
                };
                let stream = self
                    .rtc
                    .direct_api()
                    .sctp_stream_id_by_channel_id(id)
                    .ok_or_else(|| io::Error::other("Missing SCTP stream"))?;
                let config = self
                    .rtc
                    .channel(id)
                    .and_then(|c| c.config().cloned())
                    .unwrap_or(ChannelConfig {
                        label,
                        negotiated: Some(stream),
                        ..Default::default()
                    });
                Some(PeerEvent::Open {
                    handle,
                    stream,
                    config,
                })
            }
            Event::ChannelData(d) => self.handle(d.id).map(|handle| PeerEvent::Message {
                handle,
                binary: d.binary,
                data: d.data,
            }),
            Event::ChannelClose(id) => self.handle(id).map(|handle| {
                self.channels.remove(&handle);
                PeerEvent::Close(handle)
            }),
            Event::ChannelBufferedAmountLow(id) => self.handle(id).map(PeerEvent::BufferedLow),
            _ => None,
        })
    }

    /// Limit per-turn datagram work so a busy peer cannot starve other tasks.
    pub fn poll(&mut self) -> io::Result<Vec<PeerEvent>> {
        let mut events = Vec::new();
        let mut packet = [0; 2048];
        for _ in 0..128 {
            match self.rtc.poll_output().map_err(io::Error::other)? {
                Output::Event(event) => {
                    if let Some(item) = self.event(event)? {
                        events.push(item);
                    }
                }
                Output::Transmit(t) => {
                    if let Some((_, socket)) =
                        self.sockets.iter().find(|(addr, _)| *addr == t.source)
                    {
                        match socket.send_to(&t.contents, t.destination) {
                            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                            res => {
                                res?;
                            }
                        }
                    }
                }
                Output::Timeout(deadline) => {
                    let now = Instant::now();
                    let mut input = None;
                    for (local, socket) in &self.sockets {
                        match socket.recv_from(&mut packet) {
                            Ok((len, source)) => {
                                if let Ok(contents) = (&packet[..len]).try_into() {
                                    input = Some(Input::Receive(
                                        now,
                                        Receive {
                                            proto: Protocol::Udp,
                                            source,
                                            destination: *local,
                                            contents,
                                        },
                                    ));
                                }
                                break;
                            }
                            Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                            Err(e) => return Err(e),
                        }
                    }
                    if let Some(input) = input {
                        self.rtc.handle_input(input).map_err(io::Error::other)?;
                    } else if now >= deadline {
                        self.rtc
                            .handle_input(Input::Timeout(now))
                            .map_err(io::Error::other)?;
                    } else {
                        break;
                    }
                }
            }
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::time::Duration;

    #[test]
    fn peers_exchange_binary_data_over_dtls_sctp() {
        let interfaces = [IpAddr::V4(Ipv4Addr::LOCALHOST)];
        let mut a = Peer::bind(&interfaces).unwrap();
        let mut b = Peer::bind(&interfaces).unwrap();
        let channel = a
            .add_channel(ChannelConfig {
                label: "chat".into(),
                ..Default::default()
            })
            .unwrap();
        let offer = a.create_offer().unwrap();
        assert!(offer.contains("a=fingerprint:"));
        let answer = b.answer_offer(&offer).unwrap();
        a.accept_answer(&answer).unwrap();
        let start = Instant::now();
        loop {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "timed out waiting for SCTP"
            );
            for event in a.poll().unwrap() {
                if let PeerEvent::Open { handle, .. } = event {
                    assert_eq!(handle, channel);
                    assert!(a.send(handle, true, &[1, 0, 255]).unwrap());
                }
            }
            for event in b.poll().unwrap() {
                if let PeerEvent::Message { binary, data, .. } = event {
                    assert!(binary);
                    assert_eq!(data, [1, 0, 255]);
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
