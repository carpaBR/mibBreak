//! Asks a Dahua device on the LAN who it is (HTTP API, digest auth).
//!
//! Private addresses repeat across networks — and every Dahua device also
//! reports the factory default 192.168.1.108 — so an address learned from the
//! cloud must be confirmed by serial before video from it is trusted.

use std::time::Duration;

use md5::{Digest, Md5};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::{Error, Result};

fn md5_hex(s: &str) -> String {
    Md5::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn param(h: &str, key: &str) -> Option<String> {
    let pat = format!("{key}=\"");
    let i = h.find(&pat)? + pat.len();
    Some(h[i..].split('"').next()?.to_string())
}

async fn get(host: &str, path: &str, auth: Option<&str>) -> Result<(u16, String, String)> {
    let mut s = TcpStream::connect(host).await?;
    let mut req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    if let Some(a) = auth {
        req += &format!("Authorization: {a}\r\n");
    }
    req += "\r\n";
    s.write_all(req.as_bytes()).await?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await?;
    let text = String::from_utf8_lossy(&buf).to_string();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let code = head.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let www = head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("www-authenticate:"))
        .unwrap_or("")
        .to_string();
    Ok((code, www, body.to_string()))
}

/// Serial number of the device at `host` ("ip:port" of its HTTP API).
pub async fn serial(host: &str, user: &str, pass: &str, timeout: Duration) -> Result<String> {
    let path = "/cgi-bin/magicBox.cgi?action=getSerialNo";
    let run = async {
        let (code, www, body) = get(host, path, None).await?;
        let body = if code == 401 {
            let realm = param(&www, "realm").unwrap_or_default();
            let nonce = param(&www, "nonce").unwrap_or_default();
            let qop = www.contains("qop=");
            let ha1 = md5_hex(&format!("{user}:{realm}:{pass}"));
            let ha2 = md5_hex(&format!("GET:{path}"));
            let cnonce = format!("{:016x}", rand::random::<u64>());
            let auth = if qop {
                let resp = md5_hex(&format!("{ha1}:{nonce}:00000001:{cnonce}:auth:{ha2}"));
                format!("Digest username=\"{user}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{path}\", qop=auth, nc=00000001, cnonce=\"{cnonce}\", response=\"{resp}\"")
            } else {
                let resp = md5_hex(&format!("{ha1}:{nonce}:{ha2}"));
                format!("Digest username=\"{user}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{path}\", response=\"{resp}\"")
            };
            let (code, _, body) = get(host, path, Some(&auth)).await?;
            if code == 401 {
                return Err(Error::Unauthorized);
            }
            body
        } else {
            body
        };
        body.lines()
            .find_map(|l| l.trim().strip_prefix("sn="))
            .map(|s| s.trim().to_string())
            .ok_or_else(|| Error::Protocol("no serial in the answer".into()))
    };
    tokio::time::timeout(timeout, run).await.map_err(|_| Error::Timeout(format!("HTTP {host}")))?
}
