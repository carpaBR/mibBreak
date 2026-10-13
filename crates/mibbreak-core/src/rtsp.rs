//! Minimal RTSP client over any byte stream (a tunnel realm or a LAN TcpStream).
//! Video only, RTP interleaved on the same connection, H.264 / H.265
//! depacketized into Annex B access units.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use md5::{Digest, Md5};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

use crate::audio::{self, AudioFrame, AudioTrack};
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    H264,
    H265,
}

#[derive(Debug, Clone)]
pub struct VideoFrame {
    /// Annex B access unit (start codes + NALs); keyframes carry the parameter sets.
    pub data: Vec<u8>,
    pub keyframe: bool,
    pub codec: Codec,
    /// RTP timestamp (90 kHz).
    pub timestamp: u32,
}

#[derive(Debug, Clone)]
pub struct VideoTrack {
    pub control: String,
    pub payload_type: u8,
    pub codec: Codec,
    /// Parameter sets from the SDP (SPS/PPS, + VPS for H.265), prepended to keyframes lacking them.
    pub param_sets: Vec<Vec<u8>>,
}

const START: [u8; 4] = [0, 0, 0, 1];

pub fn parse_sdp_video(sdp: &str) -> Option<VideoTrack> {
    let mut in_video = false;
    let mut control = String::new();
    let mut pt: i32 = -1;
    let mut codec = None;
    let mut fmtp = String::new();
    for line in sdp.lines() {
        let line = line.trim_end();
        if let Some(m) = line.strip_prefix("m=") {
            if in_video {
                break;
            }
            in_video = m.starts_with("video");
            if in_video {
                pt = m.split(' ').nth(3).and_then(|v| v.parse().ok()).unwrap_or(-1);
            }
            continue;
        }
        if !in_video {
            continue;
        }
        if let Some(c) = line.strip_prefix("a=control:") {
            control = c.trim().to_string();
        }
        if let Some(rest) = line.strip_prefix("a=rtpmap:") {
            let mut it = rest.splitn(2, ' ');
            let n: i32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(-2);
            let name = it.next().unwrap_or("").split('/').next().unwrap_or("").to_ascii_uppercase();
            if n == pt {
                codec = match name.as_str() {
                    "H264" => Some(Codec::H264),
                    "H265" | "HEVC" => Some(Codec::H265),
                    _ => None,
                };
            }
        }
        if let Some(rest) = line.strip_prefix(&format!("a=fmtp:{pt} ")) {
            fmtp = rest.to_string();
        }
    }
    let codec = codec?;
    let params: HashMap<String, String> = fmtp
        .split(';')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let b64 = |k: &str| -> Vec<Vec<u8>> {
        params
            .get(k)
            .map(|s| {
                s.split(',')
                    .filter(|x| !x.is_empty())
                    .filter_map(|x| base64::engine::general_purpose::STANDARD.decode(x).ok())
                    .collect()
            })
            .unwrap_or_default()
    };
    let param_sets = match codec {
        Codec::H264 => b64("sprop-parameter-sets"),
        Codec::H265 => [b64("sprop-vps"), b64("sprop-sps"), b64("sprop-pps")].concat(),
    };
    Some(VideoTrack { control, payload_type: pt as u8, codec, param_sets })
}

fn nal_type(codec: Codec, nal: &[u8]) -> u8 {
    match codec {
        Codec::H264 => nal[0] & 0x1f,
        Codec::H265 => (nal[0] >> 1) & 0x3f,
    }
}
fn is_key(codec: Codec, t: u8) -> bool {
    match codec {
        Codec::H264 => t == 5,
        Codec::H265 => (16..=21).contains(&t),
    }
}
fn is_param(codec: Codec, t: u8) -> bool {
    match codec {
        Codec::H264 => t == 7 || t == 8,
        Codec::H265 => (32..=34).contains(&t),
    }
}

/// RTP → access units. Drops a frame when a fragment of it went missing.
pub struct Depacketizer {
    track: VideoTrack,
    nals: Vec<Vec<u8>>,
    fu: Option<Vec<u8>>,
    timestamp: Option<u32>,
    last_seq: Option<u16>,
    broken: bool,
}

impl Depacketizer {
    pub fn new(track: VideoTrack) -> Self {
        Self { track, nals: Vec::new(), fu: None, timestamp: None, last_seq: None, broken: false }
    }

    pub fn push(&mut self, rtp: &[u8], out: &mut Vec<VideoFrame>) {
        if rtp.len() < 12 || rtp[0] >> 6 != 2 {
            return;
        }
        let csrc = (rtp[0] & 0x0f) as usize;
        let has_ext = rtp[0] & 0x10 != 0;
        let padding = rtp[0] & 0x20 != 0;
        let marker = rtp[1] & 0x80 != 0;
        if rtp[1] & 0x7f != self.track.payload_type {
            return;
        }
        let seq = u16::from_be_bytes([rtp[2], rtp[3]]);
        let ts = u32::from_be_bytes([rtp[4], rtp[5], rtp[6], rtp[7]]);
        let mut off = 12 + csrc * 4;
        if has_ext && rtp.len() >= off + 4 {
            off += 4 + u16::from_be_bytes([rtp[off + 2], rtp[off + 3]]) as usize * 4;
        }
        let mut end = rtp.len();
        if padding {
            end = end.saturating_sub(rtp[rtp.len() - 1] as usize);
        }
        if off >= end {
            return;
        }
        let payload = &rtp[off..end];

        if self.last_seq.is_some_and(|l| l.wrapping_add(1) != seq) {
            self.broken = true;
        }
        self.last_seq = Some(seq);
        if self.timestamp.is_some_and(|t| t != ts) {
            self.flush(out);
        }
        self.timestamp = Some(ts);

        match self.track.codec {
            Codec::H264 => self.h264(payload),
            Codec::H265 => self.h265(payload),
        }
        if marker {
            self.flush(out);
        }
    }

    fn fragment(&mut self, start: bool, end: bool, header: &[u8], data: &[u8]) {
        if start {
            self.fu = Some(header.to_vec());
        } else if self.fu.is_none() {
            self.broken = true;
            return;
        }
        let fu = self.fu.as_mut().unwrap();
        fu.extend_from_slice(data);
        if end {
            self.nals.push(self.fu.take().unwrap());
        }
    }

    fn h264(&mut self, p: &[u8]) {
        let t = p[0] & 0x1f;
        match t {
            1..=23 => self.nals.push(p.to_vec()),
            24 => self.aggregated(&p[1..]),
            28 if p.len() > 2 => {
                let fuh = p[1];
                self.fragment(fuh & 0x80 != 0, fuh & 0x40 != 0, &[(p[0] & 0xe0) | (fuh & 0x1f)], &p[2..]);
            }
            _ => {}
        }
    }

    fn h265(&mut self, p: &[u8]) {
        if p.len() < 3 {
            return;
        }
        match (p[0] >> 1) & 0x3f {
            48 => self.aggregated(&p[2..]),
            49 => {
                let fuh = p[2];
                let header = [(p[0] & 0x81) | ((fuh & 0x3f) << 1), p[1]];
                self.fragment(fuh & 0x80 != 0, fuh & 0x40 != 0, &header, &p[3..]);
            }
            _ => self.nals.push(p.to_vec()),
        }
    }

    fn aggregated(&mut self, p: &[u8]) {
        let mut i = 0;
        while i + 2 <= p.len() {
            let size = u16::from_be_bytes([p[i], p[i + 1]]) as usize;
            i += 2;
            if i + size > p.len() {
                break;
            }
            self.nals.push(p[i..i + size].to_vec());
            i += size;
        }
    }

    fn flush(&mut self, out: &mut Vec<VideoFrame>) {
        let nals = std::mem::take(&mut self.nals);
        let broken = self.broken || self.fu.is_some();
        self.fu = None;
        self.broken = false;
        if nals.is_empty() || broken {
            return;
        }
        let codec = self.track.codec;
        let types: Vec<u8> = nals.iter().filter(|n| !n.is_empty()).map(|n| nal_type(codec, n)).collect();
        let keyframe = types.iter().any(|&t| is_key(codec, t));
        let mut data = Vec::new();
        if keyframe && !types.iter().any(|&t| is_param(codec, t)) {
            for ps in &self.track.param_sets {
                data.extend_from_slice(&START);
                data.extend_from_slice(ps);
            }
        }
        for n in &nals {
            data.extend_from_slice(&START);
            data.extend_from_slice(n);
        }
        out.push(VideoFrame { data, keyframe, codec, timestamp: self.timestamp.unwrap_or(0) });
    }
}

#[derive(Debug)]
struct Response {
    code: u16,
    status: String,
    headers: HashMap<String, String>,
    body: String,
}

struct Auth {
    basic: bool,
    realm: String,
    nonce: String,
}

fn md5_hex(s: &str) -> String {
    Md5::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn header_param(h: &str, key: &str) -> String {
    let pat = format!("{key}=\"");
    h.find(&pat)
        .and_then(|i| h[i + pat.len()..].split('"').next())
        .unwrap_or("")
        .to_string()
}

/// Sends requests; shared with the keepalive task.
struct Writer<S> {
    wr: WriteHalf<S>,
    cseq: u32,
    session: String,
    auth: Option<Auth>,
    user: Option<String>,
    pass: String,
}

impl<S: AsyncRead + AsyncWrite + Send + 'static> Writer<S> {
    fn authorization(&self, method: &str, url: &str) -> Option<String> {
        let (auth, user) = (self.auth.as_ref()?, self.user.as_ref()?);
        if auth.basic {
            let raw = format!("{user}:{}", self.pass);
            return Some(format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(raw)));
        }
        let ha1 = md5_hex(&format!("{user}:{}:{}", auth.realm, self.pass));
        let ha2 = md5_hex(&format!("{method}:{url}"));
        let response = md5_hex(&format!("{ha1}:{}:{ha2}", auth.nonce));
        Some(format!(
            "Digest username=\"{user}\", realm=\"{}\", nonce=\"{}\", uri=\"{url}\", response=\"{response}\"",
            auth.realm, auth.nonce
        ))
    }

    async fn send(&mut self, method: &str, url: &str, headers: &[(&str, &str)]) -> Result<u32> {
        self.cseq += 1;
        let mut req = format!("{method} {url} RTSP/1.0\r\nCSeq: {}\r\nUser-Agent: mibbreak\r\n", self.cseq);
        if !self.session.is_empty() {
            req += &format!("Session: {}\r\n", self.session);
        }
        if let Some(a) = self.authorization(method, url) {
            req += &format!("Authorization: {a}\r\n");
        }
        for (k, v) in headers {
            req += &format!("{k}: {v}\r\n");
        }
        req += "\r\n";
        log::debug!("rtsp >>> {method} {url}");
        self.wr.write_all(req.as_bytes()).await?;
        Ok(self.cseq)
    }
}

/// Parses the incoming byte stream: RTSP responses and interleaved RTP.
async fn read_loop<S: AsyncRead + Send + 'static>(
    mut rd: ReadHalf<S>,
    responses: mpsc::UnboundedSender<Response>,
    rtp: mpsc::UnboundedSender<(u8, Vec<u8>)>,
) {
    let mut buf: Vec<u8> = Vec::with_capacity(256 * 1024);
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        match rd.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        let mut pos = 0;
        loop {
            let b = &buf[pos..];
            if b.is_empty() {
                break;
            }
            if b[0] == b'$' {
                if b.len() < 4 {
                    break;
                }
                let len = u16::from_be_bytes([b[2], b[3]]) as usize;
                if b.len() < 4 + len {
                    break;
                }
                if (b[1] == 0 || b[1] == 2) && rtp.send((b[1], b[4..4 + len].to_vec())).is_err() {
                    return;
                }
                pos += 4 + len;
                continue;
            }
            let Some(head_end) = b.windows(4).position(|w| w == b"\r\n\r\n") else {
                if b.len() > 64 * 1024 {
                    return; // garbage
                }
                break;
            };
            let head = String::from_utf8_lossy(&b[..head_end]).to_string();
            if !head.starts_with("RTSP/") {
                // Out of sync: resync on the next '$'
                match b[1..].iter().position(|&c| c == b'$') {
                    Some(i) => pos += 1 + i,
                    None => pos = buf.len(),
                }
                continue;
            }
            let mut lines = head.split("\r\n");
            let mut status = lines.next().unwrap_or("").splitn(3, ' ');
            status.next();
            let code = status.next().and_then(|c| c.parse().ok()).unwrap_or(0);
            let status = status.next().unwrap_or("").to_string();
            let headers: HashMap<String, String> = lines
                .filter_map(|l| l.split_once(':'))
                .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
                .collect();
            let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
            if b.len() < head_end + 4 + len {
                break;
            }
            let body = String::from_utf8_lossy(&b[head_end + 4..head_end + 4 + len]).to_string();
            pos += head_end + 4 + len;
            log::debug!("rtsp <<< {code} {status}");
            let _ = responses.send(Response { code, status, headers, body });
        }
        buf.drain(..pos);
    }
}

/// What the player delivers, in arrival order.
#[derive(Debug, Clone)]
pub enum Media {
    Video(VideoFrame),
    Audio(AudioFrame),
}

pub struct RtspPlayer<S> {
    writer: Arc<Mutex<Writer<S>>>,
    url: String,
    pub track: VideoTrack,
    /// Present when the camera sends audio and its codec is understood.
    pub audio: Option<AudioTrack>,
    frames: mpsc::UnboundedReceiver<Media>,
    tasks: Vec<JoinHandle<()>>,
}

impl<S: AsyncRead + AsyncWrite + Send + 'static> RtspPlayer<S> {
    /// DESCRIBE → SETUP (video, and audio when present; interleaved) → PLAY. Frames flow right after.
    pub async fn start(stream: S, url: &str, user: Option<&str>, pass: Option<&str>, timeout: Duration) -> Result<Self> {
        let (rd, wr) = tokio::io::split(stream);
        let (resp_tx, mut resp_rx) = mpsc::unbounded_channel();
        let (rtp_tx, mut rtp_rx) = mpsc::unbounded_channel();
        let reader = tokio::spawn(read_loop(rd, resp_tx, rtp_tx));
        let mut w = Writer { wr, cseq: 0, session: String::new(), auth: None, user: user.map(String::from), pass: pass.unwrap_or("").into() };

        let mut request = async |w: &mut Writer<S>, method: &str, url: &str, headers: &[(&str, &str)]| -> Result<Response> {
            for attempt in 0..2 {
                let cseq = w.send(method, url, headers).await?;
                let res = loop {
                    let r = tokio::time::timeout(timeout, resp_rx.recv())
                        .await
                        .map_err(|_| Error::Timeout(format!("RTSP {method}")))?
                        .ok_or_else(|| Error::Closed("RTSP connection closed".into()))?;
                    if r.headers.get("cseq").and_then(|c| c.parse::<u32>().ok()) == Some(cseq) {
                        break r;
                    }
                };
                if res.code == 401 && attempt == 0 && w.user.is_some() {
                    let h = res.headers.get("www-authenticate").cloned().unwrap_or_default();
                    w.auth = Some(Auth {
                        basic: h.to_ascii_lowercase().starts_with("basic"),
                        realm: header_param(&h, "realm"),
                        nonce: header_param(&h, "nonce"),
                    });
                    continue;
                }
                if res.code == 401 {
                    return Err(Error::Unauthorized);
                }
                if res.code >= 300 {
                    return Err(Error::Status { what: format!("RTSP {method}"), code: res.code, status: res.status });
                }
                return Ok(res);
            }
            Err(Error::Unauthorized)
        };

        let desc = request(&mut w, "DESCRIBE", url, &[("Accept", "application/sdp")]).await?;
        let track = parse_sdp_video(&desc.body).ok_or_else(|| Error::Protocol("no H.264/H.265 video track in the SDP".into()))?;
        let base = desc
            .headers
            .get("content-base")
            .or_else(|| desc.headers.get("content-location"))
            .cloned()
            .unwrap_or_else(|| url.to_string());
        let control_of = |control: &str| {
            if control.to_ascii_lowercase().starts_with("rtsp://") {
                control.to_string()
            } else if !control.is_empty() && control != "*" {
                format!("{}/{}", base.trim_end_matches('/'), control)
            } else {
                base.clone()
            }
        };
        let control_url = control_of(&track.control);
        let setup = request(&mut w, "SETUP", &control_url, &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")]).await?;
        let session_hdr = setup.headers.get("session").cloned().unwrap_or_default();
        w.session = session_hdr.split(';').next().unwrap_or("").to_string();
        let keepalive_secs: u64 = session_hdr
            .split(';')
            .find_map(|p| p.trim().strip_prefix("timeout="))
            .and_then(|v| v.parse().ok())
            .unwrap_or(60);
        // Audio is optional: a camera without it (or a refused SETUP) still plays video
        let mut audio_track = audio::parse_sdp_audio(&desc.body).filter(|a| !a.control.is_empty() && a.control != "*");
        if let Some(a) = &audio_track {
            let url = control_of(&a.control);
            if let Err(e) = request(&mut w, "SETUP", &url, &[("Transport", "RTP/AVP/TCP;unicast;interleaved=2-3")]).await {
                log::debug!("audio SETUP refused: {e}");
                audio_track = None;
            }
        }
        request(&mut w, "PLAY", &base, &[("Range", "npt=0.000-")]).await?;

        let (frame_tx, frames) = mpsc::unbounded_channel();
        let depack = {
            let (track, audio_track) = (track.clone(), audio_track.clone());
            tokio::spawn(async move {
                let mut d = Depacketizer::new(track);
                let mut out = Vec::new();
                let mut aout = Vec::new();
                while let Some((channel, pkt)) = rtp_rx.recv().await {
                    if channel == 2 {
                        if let Some(a) = &audio_track {
                            audio::depacketize(a, &pkt, &mut aout);
                            for f in aout.drain(..) {
                                if frame_tx.send(Media::Audio(f)).is_err() {
                                    return;
                                }
                            }
                        }
                        continue;
                    }
                    d.push(&pkt, &mut out);
                    for f in out.drain(..) {
                        if frame_tx.send(Media::Video(f)).is_err() {
                            return;
                        }
                    }
                }
            })
        };
        // Responses to keepalives are not needed
        let drain = tokio::spawn(async move { while resp_rx.recv().await.is_some() {} });

        let writer = Arc::new(Mutex::new(w));
        let keepalive = {
            let (writer, url) = (writer.clone(), url.to_string());
            tokio::spawn(async move {
                let period = Duration::from_secs((keepalive_secs / 2).max(10));
                loop {
                    tokio::time::sleep(period).await;
                    if writer.lock().await.send("GET_PARAMETER", &url, &[]).await.is_err() {
                        return;
                    }
                }
            })
        };

        Ok(Self { writer, url: url.to_string(), track, audio: audio_track, frames, tasks: vec![reader, depack, drain, keepalive] })
    }

    /// Next video access unit (audio is skipped); None when the stream ended.
    pub async fn next_frame(&mut self) -> Option<VideoFrame> {
        loop {
            match self.frames.recv().await? {
                Media::Video(f) => return Some(f),
                Media::Audio(_) => continue,
            }
        }
    }

    /// Next video or audio frame; None when the stream ended.
    pub async fn next_media(&mut self) -> Option<Media> {
        self.frames.recv().await
    }

    /// TEARDOWN (best effort) and close the connection.
    pub async fn stop(self) {
        let mut w = self.writer.lock().await;
        let _ = tokio::time::timeout(Duration::from_secs(1), w.send("TEARDOWN", &self.url, &[])).await;
        let _ = w.wr.shutdown().await;
    }
}

impl<S> Drop for RtspPlayer<S> {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SDP: &str = "v=0\r\nm=video 0 RTP/AVP 96\r\na=control:trackID=0\r\na=rtpmap:96 H264/90000\r\n\
a=fmtp:96 packetization-mode=1;profile-level-id=4D002A;sprop-parameter-sets=Z00AKpWoHgCJ+WEAAAMAAQAAAwAUhA==,aO48gA==\r\n\
m=audio 0 RTP/AVP 97\r\na=control:trackID=1\r\na=rtpmap:97 MPEG4-GENERIC/16000\r\n";

    #[test]
    fn sdp() {
        let t = parse_sdp_video(SDP).unwrap();
        assert_eq!(t.codec, Codec::H264);
        assert_eq!(t.payload_type, 96);
        assert_eq!(t.control, "trackID=0");
        assert_eq!(t.param_sets.len(), 2);
        assert_eq!(t.param_sets[0][0] & 0x1f, 7);
    }

    fn rtp(seq: u16, ts: u32, marker: bool, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![0x80, 96 | if marker { 0x80 } else { 0 }];
        p.extend_from_slice(&seq.to_be_bytes());
        p.extend_from_slice(&ts.to_be_bytes());
        p.extend_from_slice(&[0; 4]);
        p.extend_from_slice(payload);
        p
    }

    #[test]
    fn fu_a_keyframe_gets_param_sets() {
        let mut d = Depacketizer::new(parse_sdp_video(SDP).unwrap());
        let mut out = Vec::new();
        // IDR (type 5, nri 3) split in two FU-A fragments
        d.push(&rtp(1, 100, false, &[0x7c, 0x85, 1, 2]), &mut out);
        d.push(&rtp(2, 100, true, &[0x7c, 0x45, 3]), &mut out);
        assert_eq!(out.len(), 1);
        let f = &out[0];
        assert!(f.keyframe);
        // SPS, PPS, then the reassembled IDR
        assert!(f.data.ends_with(&[0, 0, 0, 1, 0x65, 1, 2, 3]));
        assert_eq!(f.data[4] & 0x1f, 7);
    }

    #[test]
    fn missing_fragment_drops_frame() {
        let mut d = Depacketizer::new(parse_sdp_video(SDP).unwrap());
        let mut out = Vec::new();
        d.push(&rtp(1, 100, false, &[0x7c, 0x85, 1]), &mut out);
        d.push(&rtp(3, 100, true, &[0x7c, 0x45, 3]), &mut out); // seq 2 lost
        assert!(out.is_empty());
        d.push(&rtp(4, 200, true, &[0x41, 9]), &mut out); // next P frame is fine
        assert_eq!(out.len(), 1);
        assert!(!out[0].keyframe);
    }
}
