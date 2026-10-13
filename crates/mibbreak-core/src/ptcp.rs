//! PTCP ("PhonyTCP"): Dahua's reliable byte stream over UDP.
//!
//! Every datagram carries a 24-byte header:
//!   "PTCP" | sent | recv | pid | lmid | rmid      (all u32 big-endian)
//! `sent` is the byte offset of this datagram's body in the sender's stream and
//! `recv` is the cumulative acknowledgement of the other direction — the same
//! roles as TCP's seq/ack. Every non-empty body consumes offset space; an empty
//! body is a pure ACK.
//!
//! Through the Easy4ip relay datagrams get lost and reordered, so the receiver
//! only advances `recv` over contiguous bytes, parks early datagrams until the gap
//! is filled and never delivers a byte twice. Our own bodies are retransmitted
//! until the peer acknowledges them. (Design from ludufre/dahua-p2p.)
//!
//! This module is sans-IO: the caller feeds datagrams and time, and sends what
//! comes out.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use rand::Rng;

pub const HEADER_SIZE: usize = 24;
/// Largest data chunk per datagram (what the devices themselves use).
pub const MAX_DATA: usize = 1280;

pub mod kind {
    pub const SYNC: u8 = 0x00;
    pub const DATA: u8 = 0x10;
    pub const BIND: u8 = 0x11;
    pub const STATUS: u8 = 0x12;
    pub const HEARTBEAT: u8 = 0x13;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub sent: u32,
    pub recv: u32,
    pub pid: u32,
    pub lmid: u32,
    pub rmid: u32,
    pub body: Vec<u8>,
}

pub fn encode(p: &Packet) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_SIZE + p.body.len());
    out.extend_from_slice(b"PTCP");
    for v in [p.sent, p.recv, p.pid, p.lmid, p.rmid] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(&p.body);
    out
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// None when the datagram is not PTCP (relay agents also send HTTP-like text).
pub fn decode(buf: &[u8]) -> Option<Packet> {
    if buf.len() < HEADER_SIZE || &buf[..4] != b"PTCP" {
        return None;
    }
    Some(Packet {
        sent: be32(buf, 4),
        recv: be32(buf, 8),
        pid: be32(buf, 12),
        lmid: be32(buf, 16),
        rmid: be32(buf, 20),
        body: buf[HEADER_SIZE..].to_vec(),
    })
}

// ------------------------------------------------------------------ bodies

fn type_header(kind: u8, len: usize, realm: u32) -> Vec<u8> {
    let mut b = Vec::with_capacity(12 + len);
    b.extend_from_slice(&(((kind as u32) << 24) | (len as u32 & 0xff_ffff)).to_be_bytes());
    b.extend_from_slice(&realm.to_be_bytes());
    b.extend_from_slice(&[0; 4]);
    b
}

pub mod bodies {
    use super::*;

    pub fn sync() -> Vec<u8> {
        vec![0x00, 0x03, 0x01, 0x00]
    }
    pub fn heartbeat() -> Vec<u8> {
        type_header(kind::HEARTBEAT, 0, 0)
    }
    pub fn data(realm: u32, data: &[u8]) -> Vec<u8> {
        let mut b = type_header(kind::DATA, data.len(), realm);
        b.extend_from_slice(data);
        b
    }
    pub fn bind(realm: u32, port: u16) -> Vec<u8> {
        let mut b = type_header(kind::BIND, 0, realm);
        b.extend_from_slice(&(port as u32).to_be_bytes());
        b.extend_from_slice(&0x7f00_0001u32.to_be_bytes()); // 127.0.0.1 on the device side
        b
    }
    pub fn status(realm: u32, status: &str) -> Vec<u8> {
        let mut b = type_header(kind::STATUS, 4, realm);
        b.extend_from_slice(status.as_bytes());
        b
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Sync,
    Data { realm: u32, data: Vec<u8> },
    Bind { realm: u32, port: u32 },
    Status { realm: u32, status: String },
    Heartbeat,
    Command { code: u8, payload: Vec<u8> },
}

pub fn parse_body(b: &[u8]) -> Body {
    let code = b[0];
    if code == kind::SYNC {
        return Body::Sync;
    }
    if b.len() < 12 {
        return Body::Command { code, payload: b[1..].to_vec() };
    }
    let realm = be32(b, 4);
    match code {
        kind::DATA => {
            let len = (be32(b, 0) & 0xff_ffff) as usize;
            Body::Data { realm, data: b[12..(12 + len).min(b.len())].to_vec() }
        }
        kind::BIND => Body::Bind { realm, port: if b.len() >= 16 { be32(b, 12) } else { 0 } },
        kind::STATUS => {
            let s = String::from_utf8_lossy(&b[12..]).trim_end_matches('\0').to_string();
            Body::Status { realm, status: s }
        }
        kind::HEARTBEAT => Body::Heartbeat,
        _ => Body::Command { code, payload: b[12..].to_vec() },
    }
}

/// `a` comes after `b` in 32-bit wrap-around space.
pub fn seq_after(a: u32, b: u32) -> bool {
    let d = a.wrapping_sub(b);
    d != 0 && d < 0x8000_0000
}

// ------------------------------------------------------------------ session

#[derive(Debug, Clone)]
pub struct Config {
    /// Coalesces ACKs for this long.
    pub ack_delay: Duration,
    pub heartbeat: Duration,
    /// First retransmission timeout; doubles per retry.
    pub rto: Duration,
    pub max_retries: u32,
    /// Closes after this long without hearing from the peer.
    pub idle_timeout: Duration,
    /// Closes when the peer stops acknowledging our bytes for this long.
    pub stall_timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ack_delay: Duration::from_millis(20),
            heartbeat: Duration::from_secs(5),
            rto: Duration::from_millis(500),
            max_retries: 8,
            idle_timeout: Duration::from_secs(60),
            stall_timeout: Duration::from_secs(20),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub datagrams_in: u64,
    pub datagrams_out: u64,
    pub retransmits_out: u64,
    pub duplicates_in: u64,
    pub out_of_order_in: u64,
    /// Gaps later filled by the peer (i.e. it retransmits).
    pub gaps_filled: u64,
}

struct Outstanding {
    offset: u32,
    body: Vec<u8>,
    pid: u32,
    last_sent: Instant,
    tries: u32,
}

/// Reliable, ordered PTCP stream with one peer.
pub struct Session {
    cfg: Config,
    snd_next: u32,
    snd_una: u32,
    count: u32,
    lmid: u32,
    outstanding: VecDeque<Outstanding>,
    rcv_next: u32,
    early: HashMap<u32, Vec<u8>>,
    last_peer_lmid: u32,
    ack_due: Option<Instant>,
    last_heard: Instant,
    last_heartbeat: Instant,
    last_ack_progress: Instant,
    pub heartbeats: bool,
    pub stats: Stats,
    /// Datagrams to put on the wire (drain after every call).
    pub out: Vec<Vec<u8>>,
}

impl Session {
    pub fn new(cfg: Config, now: Instant) -> Self {
        Self {
            cfg,
            snd_next: 0,
            snd_una: 0,
            count: 0,
            lmid: rand::thread_rng().gen_range(1..0x7fff_ffff),
            outstanding: VecDeque::new(),
            rcv_next: 0,
            early: HashMap::new(),
            last_peer_lmid: 0,
            ack_due: None,
            last_heard: now,
            last_heartbeat: now,
            last_ack_progress: now,
            heartbeats: true,
            stats: Stats::default(),
            out: Vec::new(),
        }
    }

    /// Our bytes still waiting for an ACK.
    pub fn unacked(&self) -> u32 {
        self.snd_next.wrapping_sub(self.snd_una)
    }

    /// Queues a body for reliable delivery.
    pub fn send(&mut self, body: Vec<u8>, now: Instant) {
        let is_sync = body.len() == 4 && body[0] == kind::SYNC;
        let pid = if is_sync { 0x0002_ffff } else { 0xffff - (self.count & 0xffff) };
        if !is_sync {
            self.count = self.count.wrapping_add(1);
        }
        if self.outstanding.is_empty() {
            self.last_ack_progress = now;
        }
        let offset = self.snd_next;
        self.snd_next = self.snd_next.wrapping_add(body.len() as u32);
        self.transmit(offset, pid, &body);
        self.outstanding.push_back(Outstanding { offset, body, pid, last_sent: now, tries: 0 });
    }

    pub fn send_data(&mut self, realm: u32, data: &[u8], now: Instant) {
        for chunk in data.chunks(MAX_DATA) {
            self.send(bodies::data(realm, chunk), now);
        }
    }

    fn transmit(&mut self, offset: u32, pid: u32, body: &[u8]) {
        self.ack_due = None; // every datagram carries our current ACK
        self.stats.datagrams_out += 1;
        let lmid = self.next_lmid();
        self.out.push(encode(&Packet {
            sent: offset,
            recv: self.rcv_next,
            pid,
            lmid,
            rmid: self.last_peer_lmid,
            body: body.to_vec(),
        }));
    }

    fn next_lmid(&mut self) -> u32 {
        let v = self.lmid;
        self.lmid = self.lmid.wrapping_add(1);
        v
    }

    fn send_ack(&mut self) {
        self.ack_due = None;
        self.stats.datagrams_out += 1;
        let lmid = self.next_lmid();
        self.out.push(encode(&Packet {
            sent: self.snd_next,
            recv: self.rcv_next,
            pid: 0xffff - (self.count & 0xffff),
            lmid,
            rmid: self.last_peer_lmid,
            body: Vec::new(),
        }));
    }

    fn schedule_ack(&mut self, immediate: bool, now: Instant) {
        if immediate {
            self.send_ack();
        } else if self.ack_due.is_none() {
            self.ack_due = Some(now + self.cfg.ack_delay);
        }
    }

    /// Feeds one datagram from the peer; returns the bodies delivered in order.
    /// Returns None when it was not PTCP.
    pub fn input(&mut self, datagram: &[u8], now: Instant) -> Option<Vec<Body>> {
        let p = decode(datagram)?;
        let mut delivered = Vec::new();
        self.stats.datagrams_in += 1;
        self.last_heard = now;
        self.last_peer_lmid = p.lmid;
        self.on_peer_ack(p.recv, now);
        if p.body.is_empty() {
            return Some(delivered);
        }

        if p.sent == self.rcv_next {
            self.accept(&p.body, &mut delivered);
            while let Some(next) = self.early.remove(&self.rcv_next) {
                self.stats.gaps_filled += 1;
                self.accept(&next, &mut delivered);
            }
            let gap = !self.early.is_empty();
            self.schedule_ack(gap, now);
        } else if seq_after(p.sent, self.rcv_next) {
            // A gap: keep it for later and tell the peer right away where we are
            if self.early.contains_key(&p.sent) {
                self.stats.duplicates_in += 1;
            } else {
                self.stats.out_of_order_in += 1;
                if self.early.len() < 4096 {
                    self.early.insert(p.sent, p.body);
                }
            }
            self.schedule_ack(true, now);
        } else {
            let end = p.sent.wrapping_add(p.body.len() as u32);
            if seq_after(end, self.rcv_next) {
                // Overlaps what we have: account for the new tail only
                let skip = self.rcv_next.wrapping_sub(p.sent) as usize;
                self.rcv_next = self.rcv_next.wrapping_add((p.body.len() - skip) as u32);
            } else {
                self.stats.duplicates_in += 1;
            }
            self.schedule_ack(true, now);
        }
        Some(delivered)
    }

    fn accept(&mut self, body: &[u8], delivered: &mut Vec<Body>) {
        self.rcv_next = self.rcv_next.wrapping_add(body.len() as u32);
        delivered.push(parse_body(body));
    }

    fn on_peer_ack(&mut self, ack: u32, now: Instant) {
        if !seq_after(ack, self.snd_una) || seq_after(ack, self.snd_next) {
            return; // old, or acks bytes we never sent
        }
        self.snd_una = ack;
        self.last_ack_progress = now;
        while let Some(o) = self.outstanding.front() {
            if seq_after(o.offset.wrapping_add(o.body.len() as u32), ack) {
                break;
            }
            self.outstanding.pop_front();
        }
    }

    /// Timers: delayed ACK, retransmissions, heartbeat, liveness. Returns a close
    /// reason when the session is dead.
    pub fn poll(&mut self, now: Instant) -> Option<&'static str> {
        if self.ack_due.is_some_and(|t| now >= t) {
            self.send_ack();
        }

        let mut resend = Vec::new();
        for o in self.outstanding.iter_mut() {
            let rto = self.cfg.rto * 2u32.pow(o.tries.min(16));
            if now.duration_since(o.last_sent) < rto {
                break; // later ones were sent after this one
            }
            if o.tries >= self.cfg.max_retries {
                return Some("peer stopped acknowledging");
            }
            o.tries += 1;
            o.last_sent = now;
            resend.push((o.offset, o.pid, o.body.clone()));
        }
        for (offset, pid, body) in resend {
            self.stats.retransmits_out += 1;
            log::debug!("ptcp: retransmit offset {offset} ({} B)", body.len());
            self.transmit(offset, pid, &body);
        }

        if !self.outstanding.is_empty() && now.duration_since(self.last_ack_progress) > self.cfg.stall_timeout {
            return Some("stalled");
        }
        if now.duration_since(self.last_heard) > self.cfg.idle_timeout {
            return Some("peer silent");
        }
        if self.heartbeats && now.duration_since(self.last_heartbeat) >= self.cfg.heartbeat {
            self.last_heartbeat = now;
            self.send(bodies::heartbeat(), now);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkt(sent: u32, body: Vec<u8>) -> Vec<u8> {
        encode(&Packet { sent, recv: 0, pid: 0, lmid: 1, rmid: 0, body })
    }

    #[test]
    fn roundtrip_header() {
        let p = Packet { sent: 1, recv: 2, pid: 3, lmid: 4, rmid: 5, body: vec![9, 9] };
        assert_eq!(decode(&encode(&p)), Some(p));
        assert_eq!(decode(b"HTTP/1.1 200 OK"), None);
    }

    #[test]
    fn reorders_and_drops_duplicates() {
        let now = Instant::now();
        let mut s = Session::new(Config::default(), now);
        let a = bodies::data(7, b"hello ");
        let b = bodies::data(7, b"world");
        let off_b = a.len() as u32;

        // b arrives first: parked, nothing delivered, immediate ACK of 0
        assert!(s.input(&pkt(off_b, b.clone()), now).unwrap().is_empty());
        assert_eq!(s.stats.out_of_order_in, 1);
        // a fills the gap: both delivered in order
        let got = s.input(&pkt(0, a.clone()), now).unwrap();
        assert_eq!(
            got,
            vec![Body::Data { realm: 7, data: b"hello ".to_vec() }, Body::Data { realm: 7, data: b"world".to_vec() }]
        );
        assert_eq!(s.stats.gaps_filled, 1);
        // a again: duplicate, not delivered
        assert!(s.input(&pkt(0, a), now).unwrap().is_empty());
        assert_eq!(s.stats.duplicates_in, 1);
        // the last datagram out acknowledges everything
        let last = decode(s.out.last().unwrap()).unwrap();
        assert_eq!(last.recv, off_b + b.len() as u32);
    }

    #[test]
    fn retransmits_until_acked() {
        let t0 = Instant::now();
        let mut s = Session::new(Config::default(), t0);
        s.heartbeats = false;
        s.send(bodies::status(1, "CONN"), t0);
        s.out.clear();
        assert!(s.poll(t0 + Duration::from_millis(100)).is_none());
        assert!(s.out.is_empty());
        assert!(s.poll(t0 + Duration::from_millis(600)).is_none());
        assert_eq!(s.stats.retransmits_out, 1);
        // peer acks: nothing outstanding any more
        let ack = encode(&Packet { sent: 0, recv: 16, pid: 0, lmid: 9, rmid: 0, body: vec![] });
        s.input(&ack, t0 + Duration::from_millis(700));
        assert_eq!(s.unacked(), 0);
        s.out.clear();
        assert!(s.poll(t0 + Duration::from_secs(5)).is_none());
        assert!(s.out.is_empty());
    }

    #[test]
    fn wraparound() {
        assert!(seq_after(5, u32::MAX - 5));
        assert!(!seq_after(u32::MAX - 5, 5));
    }
}
