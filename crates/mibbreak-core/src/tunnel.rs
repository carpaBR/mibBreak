//! A PTCP tunnel to a Dahua device: TCP connections to the device's own ports
//! (RTSP 554, HTTP 80, 37777…) carried as "realms" over one PTCP session.
//!
//! `connect` returns a `DuplexStream`, so RTSP code runs the same over the cloud
//! tunnel and over a plain LAN `TcpStream`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rand::Rng;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot, watch, Semaphore};

use crate::dhhttp::{DhSocket, OtherRx, EASY4IP_SERVER};
use crate::handshake::{relay_handshake, DeviceAddrs};
use crate::ptcp::{self, bodies, Body};
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct TunnelOptions {
    pub serial: String,
    /// Cloud entry point (Easy4ip serves Dahua, Intelbras Mibo, Imou).
    pub cloud: String,
    pub timeout: Duration,
    /// Keep retrying while the device is busy (it holds a closed relay session
    /// for ~15–20 s) for up to this long. Zero = single attempt.
    pub busy_retry: Duration,
    /// Realms (TCP connections) open at the same time. Dahua devices are most
    /// reliable with one; the next connection waits for the previous to close.
    pub max_realms: usize,
}

impl TunnelOptions {
    pub fn new(serial: impl Into<String>) -> Self {
        Self {
            serial: serial.into(),
            cloud: EASY4IP_SERVER.to_string(),
            timeout: Duration::from_secs(8),
            busy_retry: Duration::from_secs(35),
            max_realms: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TunnelStats {
    /// Bytes on the wire, IPv4/UDP headers included (what a metered link bills).
    pub udp_bytes_in: u64,
    pub udp_bytes_out: u64,
    pub ptcp: ptcp::Stats,
    pub handshake_ms: u64,
}

const BIND_TIMEOUT: Duration = Duration::from_secs(8);
const BUSY_RETRY_WAIT: Duration = Duration::from_secs(4);
const DISC_TIMEOUT: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_millis(10);
const REALM_BUFFER: usize = 1 << 20;

enum Cmd {
    Bind { realm: u32, port: u16, data_tx: mpsc::UnboundedSender<Vec<u8>>, reply: oneshot::Sender<bool> },
    Data { realm: u32, data: Vec<u8> },
    Disc { realm: u32, done: oneshot::Sender<()> },
    Close,
}

pub struct Tunnel {
    cmd: mpsc::UnboundedSender<Cmd>,
    closed: watch::Receiver<Option<String>>,
    realms: Arc<Semaphore>,
    stats: Arc<Mutex<ptcp::Stats>>,
    bytes_in: Arc<AtomicU64>,
    bytes_out: Arc<AtomicU64>,
    handshake_ms: u64,
    pub device: DeviceAddrs,
}

impl Tunnel {
    pub async fn open(opts: &TunnelOptions) -> Result<Tunnel> {
        let deadline = Instant::now() + opts.busy_retry;
        let mut attempt = 1;
        loop {
            match Self::open_once(opts).await {
                Err(Error::DeviceBusy) if Instant::now() + BUSY_RETRY_WAIT < deadline => {
                    log::info!("{}: device busy (attempt {attempt}), retrying in {}s", opts.serial, BUSY_RETRY_WAIT.as_secs());
                    attempt += 1;
                    tokio::time::sleep(BUSY_RETRY_WAIT).await;
                }
                other => return other,
            }
        }
    }

    async fn open_once(opts: &TunnelOptions) -> Result<Tunnel> {
        let (sock, other) = DhSocket::bind().await?;
        let relay = relay_handshake(&sock, &opts.serial, &opts.cloud, opts.timeout).await?;

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (closed_tx, closed_rx) = watch::channel(None);
        let (synced_tx, synced_rx) = oneshot::channel();
        let stats = Arc::new(Mutex::new(ptcp::Stats::default()));
        let (bytes_in, bytes_out) = (sock.bytes_in.clone(), sock.bytes_out.clone());

        let driver = Driver {
            sock,
            udp: None,
            agent: relay.agent,
            session: ptcp::Session::new(ptcp::Config::default(), Instant::now()),
            realms: HashMap::new(),
            pending_binds: HashMap::new(),
            pending_discs: HashMap::new(),
            synced: Some(synced_tx),
            stats: stats.clone(),
        };
        tokio::spawn(driver.run(other, cmd_rx, closed_tx));

        match tokio::time::timeout(opts.timeout, synced_rx).await {
            Ok(Ok(())) => {}
            _ => {
                let _ = cmd_tx.send(Cmd::Close);
                return Err(Error::Timeout("device did not answer PTCP SYNC through the relay".into()));
            }
        }
        let handshake_ms = relay.elapsed.as_millis() as u64;
        log::info!("{}: tunnel ready in {handshake_ms} ms via agent {}", opts.serial, relay.agent);
        Ok(Tunnel {
            cmd: cmd_tx,
            closed: closed_rx,
            realms: Arc::new(Semaphore::new(opts.max_realms.max(1))),
            stats,
            bytes_in,
            bytes_out,
            handshake_ms,
            device: relay.device,
        })
    }

    pub fn closed_reason(&self) -> Option<String> {
        self.closed.borrow().clone()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.borrow().is_some()
    }

    /// Resolves when the tunnel dies, with the reason.
    pub async fn wait_closed(&self) -> String {
        let mut rx = self.closed.clone();
        loop {
            if let Some(r) = rx.borrow().clone() {
                return r;
            }
            if rx.changed().await.is_err() {
                return "driver gone".into();
            }
        }
    }

    pub fn stats(&self) -> TunnelStats {
        TunnelStats {
            udp_bytes_in: self.bytes_in.load(Ordering::Relaxed),
            udp_bytes_out: self.bytes_out.load(Ordering::Relaxed),
            ptcp: *self.stats.lock().unwrap(),
            handshake_ms: self.handshake_ms,
        }
    }

    /// Opens a TCP connection to `port` on the device. Waits while all realm slots
    /// are taken (by default one: the previous connection must close first).
    pub async fn connect(&self, port: u16) -> Result<DuplexStream> {
        if let Some(r) = self.closed_reason() {
            return Err(Error::Closed(r));
        }
        let permit = self.realms.clone().acquire_owned().await.map_err(|_| Error::Closed("shutdown".into()))?;
        let realm: u32 = rand::thread_rng().gen_range(1..0x7fff_ffff);
        let (data_tx, mut data_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd
            .send(Cmd::Bind { realm, port, data_tx, reply: reply_tx })
            .map_err(|_| Error::Closed("driver gone".into()))?;
        let ok = matches!(tokio::time::timeout(BIND_TIMEOUT, reply_rx).await, Ok(Ok(true)));
        if !ok {
            return Err(Error::Protocol(format!("device refused or ignored the connection to port {port}")));
        }
        log::debug!("realm {realm:x} connected to device port {port}");

        let (client, ours) = tokio::io::duplex(REALM_BUFFER);
        let (mut rd, mut wr) = tokio::io::split(ours);
        let cmd = self.cmd.clone();
        tokio::spawn(async move {
            // device → client
            let writer = tokio::spawn(async move {
                while let Some(chunk) = data_rx.recv().await {
                    if wr.write_all(&chunk).await.is_err() {
                        break;
                    }
                }
                let _ = wr.shutdown().await;
            });
            // client → device
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                match rd.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if cmd.send(Cmd::Data { realm, data: buf[..n].to_vec() }).is_err() {
                            break;
                        }
                    }
                }
            }
            let (done_tx, done_rx) = oneshot::channel();
            if cmd.send(Cmd::Disc { realm, done: done_tx }).is_ok() {
                // Keep the slot briefly so the device can release the realm
                let _ = tokio::time::timeout(DISC_TIMEOUT, done_rx).await;
            }
            writer.abort();
            drop(permit);
        });
        Ok(client)
    }

    /// Exposes `remote_port` on the device as a local TCP port (for ffplay, VLC…).
    pub async fn listen(self: &Arc<Self>, local: SocketAddr, remote_port: u16) -> Result<()> {
        let server = tokio::net::TcpListener::bind(local).await?;
        loop {
            tokio::select! {
                accepted = server.accept() => {
                    let (mut client, from) = accepted?;
                    let me = self.clone();
                    tokio::spawn(async move {
                        match me.connect(remote_port).await {
                            Ok(mut realm) => {
                                let _ = tokio::io::copy_bidirectional(&mut client, &mut realm).await;
                            }
                            Err(e) => log::warn!("listen: {from}: {e}"),
                        }
                    });
                }
                reason = self.wait_closed() => return Err(Error::Closed(reason)),
            }
        }
    }

    pub fn close(&self) {
        let _ = self.cmd.send(Cmd::Close);
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.close();
    }
}

/// Owns the PTCP session; everything goes through its loop.
struct Driver {
    sock: DhSocket,
    udp: Option<Arc<UdpSocket>>,
    agent: SocketAddr,
    session: ptcp::Session,
    realms: HashMap<u32, mpsc::UnboundedSender<Vec<u8>>>,
    pending_binds: HashMap<u32, oneshot::Sender<bool>>,
    pending_discs: HashMap<u32, oneshot::Sender<()>>,
    synced: Option<oneshot::Sender<()>>,
    stats: Arc<Mutex<ptcp::Stats>>,
}

impl Driver {
    async fn run(mut self, mut other: OtherRx, mut cmds: mpsc::UnboundedReceiver<Cmd>, closed: watch::Sender<Option<String>>) {
        self.udp = Some(self.sock.socket());
        self.session.send(bodies::sync(), Instant::now());
        self.flush().await;
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        let reason: String = loop {
            tokio::select! {
                msg = other.recv() => {
                    let Some((dgram, from)) = msg else { break "socket closed".into() };
                    if from != self.agent { continue; }
                    if let Some(bodies) = self.session.input(&dgram, Instant::now()) {
                        for b in bodies { self.on_body(b); }
                    }
                }
                cmd = cmds.recv() => {
                    let now = Instant::now();
                    match cmd {
                        None | Some(Cmd::Close) => break "closed".into(),
                        Some(Cmd::Bind { realm, port, data_tx, reply }) => {
                            self.realms.insert(realm, data_tx);
                            self.pending_binds.insert(realm, reply);
                            self.session.send(bodies::bind(realm, port), now);
                        }
                        Some(Cmd::Data { realm, data }) => {
                            if self.realms.contains_key(&realm) { self.session.send_data(realm, &data, now); }
                        }
                        Some(Cmd::Disc { realm, done }) => {
                            if self.realms.remove(&realm).is_some() {
                                self.session.send(bodies::status(realm, "DISC"), now);
                                self.pending_discs.insert(realm, done);
                            } else {
                                let _ = done.send(());
                            }
                        }
                    }
                }
                _ = tick.tick() => {
                    if let Some(r) = self.session.poll(Instant::now()) { break r.to_string(); }
                    *self.stats.lock().unwrap() = self.session.stats;
                }
            }
            self.flush().await;
        };

        log::info!("tunnel closed: {reason}");
        // Best effort: tell the device every realm is gone
        let now = Instant::now();
        for realm in self.realms.keys().copied().collect::<Vec<_>>() {
            self.session.send(bodies::status(realm, "DISC"), now);
        }
        self.flush().await;
        self.realms.clear();
        for (_, p) in self.pending_binds.drain() {
            let _ = p.send(false);
        }
        let _ = closed.send(Some(reason));
    }

    fn on_body(&mut self, b: Body) {
        match b {
            Body::Sync => {
                if let Some(tx) = self.synced.take() {
                    let _ = tx.send(());
                }
            }
            Body::Data { realm, data } => {
                if let Some(tx) = self.realms.get(&realm) {
                    let _ = tx.send(data);
                }
            }
            Body::Status { realm, status } if status == "CONN" => {
                if let Some(p) = self.pending_binds.remove(&realm) {
                    let _ = p.send(true);
                }
            }
            Body::Status { realm, status } if status == "DISC" => {
                if let Some(p) = self.pending_binds.remove(&realm) {
                    let _ = p.send(false);
                }
                if let Some(p) = self.pending_discs.remove(&realm) {
                    let _ = p.send(());
                }
                // Dropping the sender ends the client's read side
                self.realms.remove(&realm);
            }
            Body::Command { code, payload } => log::debug!("ptcp command 0x{code:x} ({} B)", payload.len()),
            _ => {}
        }
    }

    async fn flush(&mut self) {
        let out = std::mem::take(&mut self.session.out);
        for d in out {
            self.sock.send(self.agent, &d).await;
        }
    }
}
