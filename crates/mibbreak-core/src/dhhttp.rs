//! DH-HTTP: the HTTP-like request/response protocol the Easy4ip cloud speaks over UDP.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use rand::Rng;
use sha1::{Digest, Sha1};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::{Error, Result};

pub const EASY4IP_SERVER: &str = "www.easy4ipcloud.com:8800";
/// Client credentials shared by every Dahua client (recovered from SmartPSS Lite).
const USERNAME: &str = "cba1b29e32cb17aa46b8ff9e73c7f40b";
const USERKEY: &str = "996103384cdf19179e19243e959bbf8b";
/// IPv4 + UDP headers: what a metered link bills on top of the payload.
const UDP_OVERHEAD: u64 = 28;

#[derive(Debug, Clone)]
pub struct DhResponse {
    pub code: u16,
    pub status: String,
    pub headers: HashMap<String, String>,
    /// Flattened XML body: `<body><US>1.2.3.4:8803</US></body>` → `{"body/US": "1.2.3.4:8803"}`
    pub body: HashMap<String, String>,
}

impl DhResponse {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.body.get(key).map(String::as_str)
    }

    fn cseq(&self) -> Option<u32> {
        self.headers.get("CSeq").and_then(|v| v.parse().ok())
    }
}

/// Minimal flat XML reader: nested tags joined by "/", text nodes only.
pub fn parse_xml_flat(xml: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut stack: Vec<&str> = Vec::new();
    let mut rest = xml;
    while !rest.is_empty() {
        if let Some(tag_body) = rest.strip_prefix('<') {
            let Some(end) = tag_body.find('>') else { break };
            let tag = &tag_body[..end];
            rest = &tag_body[end + 1..];
            if let Some(_closing) = tag.strip_prefix('/') {
                stack.pop();
            } else if !(tag.starts_with('?') || tag.starts_with('!') || tag.ends_with('/')) {
                stack.push(tag.split_whitespace().next().unwrap_or(""));
            }
        } else {
            let end = rest.find('<').unwrap_or(rest.len());
            let text = rest[..end].trim();
            if !text.is_empty() && !stack.is_empty() {
                out.insert(stack.join("/"), text.to_string());
            }
            rest = &rest[end..];
        }
    }
    out
}

pub fn parse_dh_response(raw: &str) -> Option<DhResponse> {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    let mut lines = head.split("\r\n");
    let mut status_line = lines.next()?.splitn(3, ' ');
    status_line.next()?;
    let code = status_line.next()?.parse().ok()?;
    let status = status_line.next().unwrap_or("").to_string();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    Some(DhResponse { code, status, headers, body: parse_xml_flat(body) })
}

fn wsse_header() -> String {
    let nonce: u32 = rand::thread_rng().gen_range(0..(1 << 31));
    let created = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    let digest = Sha1::digest(format!("{nonce}{created}DHP2P:{USERNAME}:{USERKEY}"));
    let digest = base64::engine::general_purpose::STANDARD.encode(digest);
    format!(
        "Authorization: WSSE profile=\"UsernameToken\"\r\n\
         X-WSSE: UsernameToken Username=\"{USERNAME}\", PasswordDigest=\"{digest}\", Nonce=\"{nonce}\", Created=\"{created}\"\r\n"
    )
}

pub fn build_dh_request(path: &str, cseq: u32, body: Option<&str>) -> Vec<u8> {
    let method = if body.is_some() { "DHPOST" } else { "DHGET" };
    format!("{method} {path} HTTP/1.1\r\nCSeq: {cseq}\r\n{}\r\n{}", wsse_header(), body.unwrap_or("")).into_bytes()
}

pub async fn resolve(addr: &str) -> Result<SocketAddr> {
    tokio::net::lookup_host(addr)
        .await?
        .find(SocketAddr::is_ipv4)
        .ok_or_else(|| Error::Protocol(format!("cannot resolve {addr}")))
}

/// Datagrams that are not DH-HTTP responses (PTCP, STUN…), with their source.
pub type OtherRx = mpsc::UnboundedReceiver<(Vec<u8>, SocketAddr)>;

/// A UDP socket that speaks DH-HTTP with several peers. Responses are matched to
/// requests by source address and CSeq; everything else goes to the `other`
/// channel, so the same local port carries the handshake and the PTCP tunnel.
pub struct DhSocket {
    sock: Arc<UdpSocket>,
    cseq: AtomicU32,
    /// Responses not yet claimed (the cloud answers some requests late).
    inbox: Arc<Mutex<Vec<(SocketAddr, DhResponse)>>>,
    notify: Arc<Notify>,
    pub bytes_in: Arc<AtomicU64>,
    pub bytes_out: Arc<AtomicU64>,
    reader: JoinHandle<()>,
}

impl DhSocket {
    pub async fn bind() -> Result<(Self, OtherRx)> {
        let sock = Arc::new(UdpSocket::bind("0.0.0.0:0").await?);
        let inbox: Arc<Mutex<Vec<(SocketAddr, DhResponse)>>> = Arc::default();
        let notify = Arc::new(Notify::new());
        let bytes_in = Arc::new(AtomicU64::new(0));
        let (other_tx, other_rx) = mpsc::unbounded_channel();

        let reader = {
            let (sock, inbox, notify, bytes_in) = (sock.clone(), inbox.clone(), notify.clone(), bytes_in.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 65536];
                loop {
                    let Ok((n, from)) = sock.recv_from(&mut buf).await else { break };
                    bytes_in.fetch_add(n as u64 + UDP_OVERHEAD, Ordering::Relaxed);
                    let msg = &buf[..n];
                    if n > 9 && msg.starts_with(b"HTTP/") {
                        if let Some(res) = parse_dh_response(&String::from_utf8_lossy(msg)) {
                            log::debug!("<<< {from} {} {} {:?}", res.code, res.status, res.body);
                            let mut inbox = inbox.lock().unwrap();
                            if inbox.len() >= 64 {
                                inbox.remove(0); // late answers nobody will claim
                            }
                            inbox.push((from, res));
                            drop(inbox);
                            notify.notify_waiters();
                        }
                    } else if other_tx.send((msg.to_vec(), from)).is_err() {
                        break;
                    }
                }
            })
        };

        let me = Self { sock, cseq: AtomicU32::new(0), inbox, notify, bytes_in, bytes_out: Arc::default(), reader };
        Ok((me, other_rx))
    }

    pub fn local_port(&self) -> u16 {
        self.sock.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    pub fn socket(&self) -> Arc<UdpSocket> {
        self.sock.clone()
    }

    pub async fn send(&self, to: SocketAddr, data: &[u8]) {
        self.bytes_out.fetch_add(data.len() as u64 + UDP_OVERHEAD, Ordering::Relaxed);
        if let Err(e) = self.sock.send_to(data, to).await {
            log::debug!("udp send to {to}: {e}");
        }
    }

    /// Sends a request without waiting; returns its CSeq (answer collected later with `read`).
    pub async fn request(&self, to: SocketAddr, path: &str, body: Option<&str>) -> u32 {
        let cseq = self.cseq.fetch_add(1, Ordering::Relaxed) + 1;
        log::debug!(">>> {to} {path} {}", body.unwrap_or(""));
        self.send(to, &build_dh_request(path, cseq, body)).await;
        cseq
    }

    fn take(&self, from: SocketAddr, cseqs: Option<&[u32]>) -> Option<DhResponse> {
        let mut inbox = self.inbox.lock().unwrap();
        // Responses without a CSeq (some relay agents omit it) match any request
        let i = inbox.iter().position(|(src, res)| {
            *src == from && match (cseqs, res.cseq()) {
                (Some(want), Some(c)) => want.contains(&c),
                _ => true,
            }
        })?;
        Some(inbox.remove(i).1)
    }

    /// Next response from `from` (final or provisional), optionally for given CSeqs.
    pub async fn read(&self, from: SocketAddr, timeout: Duration, cseqs: Option<&[u32]>) -> Result<DhResponse> {
        let deadline = Instant::now() + timeout;
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(res) = self.take(from, cseqs) {
                return Ok(res);
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Err(Error::Timeout(format!("waiting for {from}")));
            }
        }
    }

    /// Waits for a final (>= 200) response, skipping provisional ones.
    pub async fn read_final(&self, from: SocketAddr, timeout: Duration, cseqs: Option<&[u32]>) -> Result<DhResponse> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
            let res = self.read(from, left, cseqs).await?;
            if res.code >= 200 {
                return Ok(res);
            }
        }
    }

    /// Request + final response, retried on timeout (UDP). Errors on >= 300.
    pub async fn call(&self, to: SocketAddr, path: &str, body: Option<&str>) -> Result<DhResponse> {
        self.call_with(to, path, body, Duration::from_millis(2500), 3).await
    }

    pub async fn call_with(
        &self,
        to: SocketAddr,
        path: &str,
        body: Option<&str>,
        timeout: Duration,
        tries: u32,
    ) -> Result<DhResponse> {
        let mut cseqs = Vec::new();
        let mut last = Error::Timeout(path.to_string());
        for _ in 0..tries {
            cseqs.push(self.request(to, path, body).await);
            match self.read_final(to, timeout, Some(&cseqs)).await {
                Ok(res) if res.code >= 300 => {
                    return Err(Error::Status { what: path.to_string(), code: res.code, status: res.status })
                }
                Ok(res) => return Ok(res),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}

impl Drop for DhSocket {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_flat() {
        let m = parse_xml_flat("<?xml version=\"1.0\"?><body><DS>1.2.3.4:8802</DS><US>1.2.3.4:8803</US><E/></body>");
        assert_eq!(m.get("body/US").map(String::as_str), Some("1.2.3.4:8803"));
        assert_eq!(m.get("body/DS").map(String::as_str), Some("1.2.3.4:8802"));
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn response() {
        let r = parse_dh_response("HTTP/1.1 200 Server Nat Info!\r\nCSeq: 6\r\n\r\n<body><PubAddr>9.9.9.9:1</PubAddr></body>").unwrap();
        assert_eq!(r.code, 200);
        assert_eq!(r.status, "Server Nat Info!");
        assert_eq!(r.cseq(), Some(6));
        assert_eq!(r.get("body/PubAddr"), Some("9.9.9.9:1"));
    }
}
