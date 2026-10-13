//! One actor per camera: keeps at most one live stream (main or sub, whichever
//! the viewers need), fans its frames out to every viewer, reuses the cloud
//! tunnel across quality switches and releases it when nobody watches.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use mibbreak_core::{AudioCodec, AudioFrame, AudioTrack, Codec, Error, Media, RtspPlayer, Tunnel, TunnelOptions, VideoFrame};

use crate::store::{Camera, Mode, Store};

pub type FrameSink = Channel<InvokeResponseBody>;

#[derive(Debug, Clone, Serialize, Default)]
pub struct Status {
    pub id: String,
    /// idle | connecting | live | retrying
    pub state: String,
    /// lan | cloud
    pub via: Option<String>,
    pub hd: bool,
    /// busy | auth | offline | timeout | network | other
    pub error: Option<String>,
    pub detail: Option<String>,
    pub viewers: usize,
}

fn error_code(e: &Error) -> &'static str {
    match e {
        Error::DeviceBusy => "busy",
        Error::Unauthorized => "auth",
        Error::NotOnline(_) => "offline",
        Error::Timeout(_) => "timeout",
        Error::Io(_) => "network",
        _ => "other",
    }
}

enum Cmd {
    Subscribe { sub: u64, hd: bool, sink: FrameSink },
    SetHd { sub: u64, hd: bool },
    SetAudio { sub: u64, on: bool },
    Unsubscribe { sub: u64 },
    Restart,
    Shutdown,
}

enum Ev {
    Live { via: &'static str, audio: Option<AudioTrack> },
    Frame(VideoFrame),
    Audio(AudioFrame),
    Failed(Error),
    Ended(String),
}

struct Viewer {
    hd: bool,
    sink: FrameSink,
    configured: bool,
    /// Wants sound (muted by default).
    audio: bool,
    audio_configured: bool,
}

pub struct Hub {
    app: AppHandle,
    store: Arc<Store>,
    cams: Mutex<HashMap<String, mpsc::UnboundedSender<Cmd>>>,
    /// sub id → camera id
    subs: Mutex<HashMap<u64, String>>,
    statuses: Arc<Mutex<HashMap<String, Status>>>,
    next_sub: AtomicU64,
    /// Cloud handshakes in flight at once: a burst of them times out and leaves
    /// half-open relay sessions that make the next attempt see a busy device.
    handshakes: Arc<Semaphore>,
}

impl Hub {
    pub fn new(app: AppHandle, store: Arc<Store>) -> Self {
        Self {
            app,
            store,
            cams: Mutex::default(),
            subs: Mutex::default(),
            statuses: Arc::default(),
            next_sub: AtomicU64::new(1),
            handshakes: Arc::new(Semaphore::new(3)),
        }
    }

    fn actor(&self, cam_id: &str) -> Option<mpsc::UnboundedSender<Cmd>> {
        let mut cams = self.cams.lock().unwrap();
        if let Some(tx) = cams.get(cam_id).filter(|tx| !tx.is_closed()) {
            return Some(tx.clone());
        }
        self.store.camera(cam_id)?;
        let (tx, rx) = mpsc::unbounded_channel();
        let actor = Actor {
            cam_id: cam_id.to_string(),
            app: self.app.clone(),
            store: self.store.clone(),
            statuses: self.statuses.clone(),
            viewers: HashMap::new(),
            runner: None,
            running_hd: None,
            tunnel: Arc::default(),
            handshakes: self.handshakes.clone(),
            retry_at: None,
            backoff: Duration::from_secs(2),
            release_at: None,
            status: Status { id: cam_id.to_string(), state: "idle".into(), ..Default::default() },
            frames: 0,
            frames_logged_at: Instant::now(),
            audio_track: None,
        };
        tauri::async_runtime::spawn(actor.run(rx));
        cams.insert(cam_id.to_string(), tx.clone());
        Some(tx)
    }

    pub fn subscribe(&self, cam_id: &str, hd: bool, sink: FrameSink) -> Result<u64, String> {
        let tx = self.actor(cam_id).ok_or("unknown camera")?;
        let sub = self.next_sub.fetch_add(1, Ordering::Relaxed);
        self.subs.lock().unwrap().insert(sub, cam_id.to_string());
        tx.send(Cmd::Subscribe { sub, hd, sink }).map_err(|_| "camera stopped")?;
        Ok(sub)
    }

    fn to_sub(&self, sub: u64, cmd: Cmd) {
        let cam = self.subs.lock().unwrap().get(&sub).cloned();
        if let Some(tx) = cam.and_then(|c| self.cams.lock().unwrap().get(&c).cloned()) {
            let _ = tx.send(cmd);
        }
    }

    pub fn set_hd(&self, sub: u64, hd: bool) {
        self.to_sub(sub, Cmd::SetHd { sub, hd });
    }

    pub fn set_audio(&self, sub: u64, on: bool) {
        self.to_sub(sub, Cmd::SetAudio { sub, on });
    }

    pub fn unsubscribe(&self, sub: u64) {
        self.to_sub(sub, Cmd::Unsubscribe { sub });
        self.subs.lock().unwrap().remove(&sub);
    }

    /// Camera settings changed: reconnect with the new ones.
    pub fn restart(&self, cam_id: &str) {
        if let Some(tx) = self.cams.lock().unwrap().get(cam_id) {
            let _ = tx.send(Cmd::Restart);
        }
    }

    pub fn remove(&self, cam_id: &str) {
        if let Some(tx) = self.cams.lock().unwrap().remove(cam_id) {
            let _ = tx.send(Cmd::Shutdown);
        }
        self.statuses.lock().unwrap().remove(cam_id);
    }

    pub fn statuses(&self) -> Vec<Status> {
        self.statuses.lock().unwrap().values().cloned().collect()
    }
}

struct Actor {
    cam_id: String,
    app: AppHandle,
    store: Arc<Store>,
    statuses: Arc<Mutex<HashMap<String, Status>>>,
    viewers: HashMap<u64, Viewer>,
    runner: Option<(JoinHandle<()>, mpsc::UnboundedReceiver<Ev>)>,
    running_hd: Option<bool>,
    tunnel: Arc<tokio::sync::Mutex<Option<Arc<Tunnel>>>>,
    handshakes: Arc<Semaphore>,
    retry_at: Option<Instant>,
    backoff: Duration,
    release_at: Option<Instant>,
    status: Status,
    frames: u64,
    frames_logged_at: Instant,
    audio_track: Option<AudioTrack>,
}

async fn sleep_until_opt(t: Option<Instant>) {
    match t {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending().await,
    }
}

async fn recv_opt(r: &mut Option<(JoinHandle<()>, mpsc::UnboundedReceiver<Ev>)>) -> Option<Ev> {
    match r {
        Some((_, rx)) => rx.recv().await,
        None => std::future::pending().await,
    }
}

impl Actor {
    async fn run(mut self, mut cmds: mpsc::UnboundedReceiver<Cmd>) {
        self.publish();
        loop {
            tokio::select! {
                cmd = cmds.recv() => match cmd {
                    None | Some(Cmd::Shutdown) => break,
                    Some(Cmd::Subscribe { sub, hd, sink }) => {
                        self.viewers.insert(sub, Viewer { hd, sink, configured: false, audio: false, audio_configured: false });
                    }
                    Some(Cmd::SetHd { sub, hd }) => {
                        if let Some(v) = self.viewers.get_mut(&sub) { v.hd = hd; }
                    }
                    Some(Cmd::SetAudio { sub, on }) => {
                        if let Some(v) = self.viewers.get_mut(&sub) {
                            v.audio = on;
                            v.audio_configured = false;
                        }
                    }
                    Some(Cmd::Unsubscribe { sub }) => { self.viewers.remove(&sub); }
                    Some(Cmd::Restart) => {
                        self.stop_runner();
                        *self.tunnel.lock().await = None;
                        self.retry_at = None;
                        self.backoff = Duration::from_secs(2);
                    }
                },
                ev = recv_opt(&mut self.runner) => self.on_event(ev),
                _ = sleep_until_opt(self.retry_at) => { self.retry_at = None; }
                _ = sleep_until_opt(self.release_at) => {
                    self.release_at = None;
                    if self.viewers.is_empty() && self.tunnel.lock().await.take().is_some() {
                        log::info!("{}: cloud session released", self.cam_id);
                    }
                }
            }
            self.reconcile();
        }
        self.stop_runner();
        *self.tunnel.lock().await = None;
    }

    fn wanted_hd(&self) -> Option<bool> {
        if self.viewers.is_empty() {
            return None;
        }
        let always = self.store.settings().always_hd;
        Some(always || self.viewers.values().any(|v| v.hd))
    }

    fn reconcile(&mut self) {
        let want = self.wanted_hd();
        if want != self.running_hd {
            self.stop_runner();
            if want.is_none() {
                self.status.state = "idle".into();
                self.status.via = None;
                self.status.error = None;
                let secs = self.store.settings().release_after_secs;
                self.release_at = Some(Instant::now() + Duration::from_secs(secs));
            }
        }
        if let Some(hd) = want {
            self.release_at = None;
            if self.runner.is_none() && self.retry_at.is_none() {
                self.start_runner(hd);
            }
        }
        self.status.viewers = self.viewers.len();
        self.publish();
    }

    fn start_runner(&mut self, hd: bool) {
        let Some(cam) = self.store.camera(&self.cam_id) else { return };
        let (tx, rx) = mpsc::unbounded_channel();
        // Actors run on Tauri's tokio runtime, so a plain tokio task is abortable here
        let task = tokio::spawn(run_stream(cam, hd, self.tunnel.clone(), self.handshakes.clone(), self.store.clone(), tx));
        self.runner = Some((task, rx));
        self.running_hd = Some(hd);
        self.status.state = "connecting".into();
        self.status.hd = hd;
        for v in self.viewers.values_mut() {
            v.configured = false;
            v.audio_configured = false;
        }
        self.audio_track = None;
    }

    fn stop_runner(&mut self) {
        if let Some((task, _)) = self.runner.take() {
            task.abort();
        }
        self.running_hd = None;
    }

    fn on_event(&mut self, ev: Option<Ev>) {
        match ev {
            Some(Ev::Live { via, audio }) => {
                self.audio_track = audio;
                self.status.state = "live".into();
                self.status.via = Some(via.into());
                self.status.error = None;
                self.status.detail = None;
                self.backoff = Duration::from_secs(2);
            }
            Some(Ev::Frame(f)) => {
                self.frames += 1;
                let since = self.frames_logged_at.elapsed();
                if since >= Duration::from_secs(30) {
                    log::info!("{}: {:.1} fps over {}s ({} viewers)", self.cam_id, self.frames as f64 / since.as_secs_f64(), since.as_secs(), self.viewers.len());
                    self.frames = 0;
                    self.frames_logged_at = Instant::now();
                }
                self.fan_out(&f)
            }
            Some(Ev::Audio(a)) => self.fan_out_audio(&a),
            Some(Ev::Failed(e)) => self.retry(Some(error_code(&e)), e.to_string()),
            Some(Ev::Ended(reason)) => self.retry(Some("network"), reason),
            None => self.retry(Some("other"), "stream task ended".into()),
        }
    }

    fn retry(&mut self, code: Option<&str>, detail: String) {
        log::warn!("{}: {detail}", self.cam_id);
        self.stop_runner();
        self.status.state = "retrying".into();
        self.status.error = code.map(String::from);
        self.status.detail = Some(detail);
        self.retry_at = Some(Instant::now() + self.backoff);
        self.backoff = (self.backoff * 2).min(Duration::from_secs(30));
    }

    fn fan_out(&mut self, f: &VideoFrame) {
        let mut frame_msg: Option<Vec<u8>> = None;
        let mut dead = Vec::new();
        for (id, v) in self.viewers.iter_mut() {
            if !v.configured {
                if !f.keyframe {
                    continue;
                }
                let cfg = serde_json::json!({
                    "codec": codec_string(f),
                    "hd": self.running_hd == Some(true),
                    "via": self.status.via,
                });
                let mut msg = vec![1u8];
                msg.extend_from_slice(cfg.to_string().as_bytes());
                if v.sink.send(InvokeResponseBody::Raw(msg)).is_err() {
                    dead.push(*id);
                    continue;
                }
                v.configured = true;
            }
            let msg = frame_msg.get_or_insert_with(|| {
                let mut m = Vec::with_capacity(6 + f.data.len());
                m.push(2u8);
                m.push(f.keyframe as u8);
                m.extend_from_slice(&f.timestamp.to_be_bytes());
                m.extend_from_slice(&f.data);
                m
            });
            if v.sink.send(InvokeResponseBody::Raw(msg.clone())).is_err() {
                dead.push(*id);
            }
        }
        for id in dead {
            self.viewers.remove(&id);
        }
    }

    fn fan_out_audio(&mut self, a: &AudioFrame) {
        let Some(track) = &self.audio_track else { return };
        let mut msg: Option<Vec<u8>> = None;
        let mut dead = Vec::new();
        for (id, v) in self.viewers.iter_mut() {
            if !v.audio {
                continue;
            }
            if !v.audio_configured {
                let (codec, description) = match &track.codec {
                    AudioCodec::Aac { config, .. } => {
                        let aot = config.first().map(|b| b >> 3).filter(|&o| o > 0).unwrap_or(2);
                        (format!("mp4a.40.{aot}"), config.iter().map(|b| format!("{b:02x}")).collect::<String>())
                    }
                    AudioCodec::Pcma => ("alaw".into(), String::new()),
                    AudioCodec::Pcmu => ("ulaw".into(), String::new()),
                };
                let cfg = serde_json::json!({
                    "codec": codec,
                    "sampleRate": track.clock_rate,
                    "channels": track.channels.max(1),
                    "description": description,
                });
                let mut m = vec![3u8];
                m.extend_from_slice(cfg.to_string().as_bytes());
                if v.sink.send(InvokeResponseBody::Raw(m)).is_err() {
                    dead.push(*id);
                    continue;
                }
                v.audio_configured = true;
            }
            let m = msg.get_or_insert_with(|| {
                let mut m = Vec::with_capacity(5 + a.data.len());
                m.push(4u8);
                m.extend_from_slice(&a.timestamp.to_be_bytes());
                m.extend_from_slice(&a.data);
                m
            });
            if v.sink.send(InvokeResponseBody::Raw(m.clone())).is_err() {
                dead.push(*id);
            }
        }
        for id in dead {
            self.viewers.remove(&id);
        }
    }

    fn publish(&self) {
        self.statuses.lock().unwrap().insert(self.cam_id.clone(), self.status.clone());
        let _ = self.app.emit("camera-status", &self.status);
    }
}

/// WebCodecs codec string from the parameter sets of a keyframe.
fn codec_string(f: &VideoFrame) -> String {
    match f.codec {
        Codec::H264 => {
            let sps = f
                .data
                .windows(4)
                .enumerate()
                .filter(|(_, w)| *w == [0, 0, 0, 1])
                .map(|(i, _)| &f.data[i + 4..])
                .find(|n| !n.is_empty() && n[0] & 0x1f == 7);
            match sps {
                Some(s) if s.len() >= 4 => format!("avc1.{:02x}{:02x}{:02x}", s[1], s[2], s[3]),
                _ => "avc1.4d002a".into(),
            }
        }
        Codec::H265 => "hvc1.1.6.L120.90".into(),
    }
}

fn rtsp_url(host: &str, cam: &Camera, hd: bool) -> String {
    format!("rtsp://{host}/cam/realmonitor?channel={}&subtype={}", cam.channel, if hd { 0 } else { 1 })
}

/// (rtsp "ip:port", set by the user?) — user-set first, then learned ones.
fn lan_candidates(cam: &Camera) -> Vec<(String, bool)> {
    let with_port = |h: &str| if h.contains(':') { h.to_string() } else { format!("{h}:554") };
    let mut out: Vec<(String, bool)> = cam.host.iter().map(|h| (with_port(h), true)).collect();
    out.extend(cam.lan_hosts.iter().map(|h| (with_port(h), false)));
    out
}

/// Is the device at `rtsp_host` really this camera? Learned addresses must prove
/// it; a user-set address is trusted when the device has no HTTP API.
async fn same_device(cam: &Camera, rtsp_host: &str, user_set: bool) -> bool {
    let ip = rtsp_host.rsplit_once(':').map(|(ip, _)| ip).unwrap_or(rtsp_host);
    match mibbreak_core::devinfo::serial(&format!("{ip}:80"), &cam.user, &cam.password, Duration::from_secs(4)).await {
        Ok(sn) if sn.eq_ignore_ascii_case(&cam.serial) => true,
        Ok(sn) => {
            log::warn!("{}: {rtsp_host} is another device ({sn}), skipped", cam.serial);
            false
        }
        Err(e) => {
            log::info!("{}: cannot confirm {rtsp_host}: {e}", cam.serial);
            user_set
        }
    }
}

const RTSP_TIMEOUT: Duration = Duration::from_secs(10);

async fn pump<S>(mut p: RtspPlayer<S>, ev: &mpsc::UnboundedSender<Ev>) -> String
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static,
{
    let mut last_video = Instant::now();
    loop {
        match tokio::time::timeout(Duration::from_secs(8), p.next_media()).await {
            Ok(Some(Media::Video(f))) => {
                last_video = Instant::now();
                if ev.send(Ev::Frame(f)).is_err() {
                    return "viewer gone".into();
                }
            }
            Ok(Some(Media::Audio(a))) => {
                if last_video.elapsed() > Duration::from_secs(8) {
                    return "no video for 8 s".into();
                }
                if ev.send(Ev::Audio(a)).is_err() {
                    return "viewer gone".into();
                }
            }
            Ok(None) => return "stream ended".into(),
            Err(_) => return "no video for 8 s".into(),
        }
    }
}

/// Connects (LAN first in auto mode, then the cloud) and pumps frames until it fails.
async fn run_stream(
    cam: Camera,
    hd: bool,
    tunnel_slot: Arc<tokio::sync::Mutex<Option<Arc<Tunnel>>>>,
    handshakes: Arc<Semaphore>,
    store: Arc<Store>,
    ev: mpsc::UnboundedSender<Ev>,
) {
    let pass = Some(cam.password.as_str());
    let user = Some(cam.user.as_str());

    if cam.mode != Mode::Cloud {
        for (host, user_set) in lan_candidates(&cam) {
            let Ok(Ok(tcp)) = tokio::time::timeout(Duration::from_millis(900), tokio::net::TcpStream::connect(&host)).await
            else {
                continue;
            };
            if !same_device(&cam, &host, user_set).await {
                continue;
            }
            let _ = tcp.set_nodelay(true);
            match RtspPlayer::start(tcp, &rtsp_url(&host, &cam, hd), user, pass, RTSP_TIMEOUT).await {
                Ok(p) => {
                    let _ = ev.send(Ev::Live { via: "lan", audio: p.audio.clone() });
                    let reason = pump(p, &ev).await;
                    let _ = ev.send(Ev::Ended(reason));
                    return;
                }
                Err(Error::Unauthorized) => {
                    let _ = ev.send(Ev::Failed(Error::Unauthorized));
                    return;
                }
                Err(e) => log::info!("{}: LAN {host} failed: {e}", cam.serial),
            }
        }
        if cam.mode == Mode::Lan {
            let _ = ev.send(Ev::Failed(Error::Protocol("camera not reachable on the LAN".into())));
            return;
        }
    }

    let tunnel = {
        let mut slot = tunnel_slot.lock().await;
        match slot.as_ref().filter(|t| !t.is_closed()) {
            Some(t) => t.clone(),
            None => {
                let mut opts = TunnelOptions::new(&cam.serial);
                opts.busy_retry = Duration::from_secs(8);
                let _turn = handshakes.acquire().await;
                match Tunnel::open(&opts).await {
                    Ok(t) => {
                        let t = Arc::new(t);
                        // Remember the LAN IPs so the next connection can go direct
                        let ips: Vec<String> =
                            t.device.local.iter().filter_map(|a| a.rsplit_once(':').map(|(ip, _)| ip.to_string())).collect();
                        if !ips.is_empty() && ips != cam.lan_hosts {
                            store.update(&cam.id, |c| c.lan_hosts = ips);
                        }
                        *slot = Some(t.clone());
                        t
                    }
                    Err(e) => {
                        let _ = ev.send(Ev::Failed(e));
                        return;
                    }
                }
            }
        }
    };
    let realm = match tunnel.connect(554).await {
        Ok(r) => r,
        Err(e) => {
            // The device still holds a stale connection in this session: start a fresh one
            let mut slot = tunnel_slot.lock().await;
            if slot.as_ref().is_some_and(|t| Arc::ptr_eq(t, &tunnel)) {
                *slot = None;
            }
            let _ = ev.send(Ev::Failed(e));
            return;
        }
    };
    match RtspPlayer::start(realm, &rtsp_url("127.0.0.1", &cam, hd), user, pass, RTSP_TIMEOUT).await {
        Ok(p) => {
            let _ = ev.send(Ev::Live { via: "cloud", audio: p.audio.clone() });
            let reason = pump(p, &ev).await;
            let _ = ev.send(Ev::Ended(reason));
        }
        Err(e) => {
            let _ = ev.send(Ev::Failed(e));
        }
    }
}
