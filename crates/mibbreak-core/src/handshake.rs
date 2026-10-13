//! Easy4ip handshake: locate the device by serial and get a relay path to it.
//!
//! Relay mode (works behind carrier-grade NAT / 4G):
//!   main   /probe/p2psrv, /online/p2psrv/{SN} → P2P server, /online/relay → relay
//!   p2psrv /probe/device/{SN}
//!   main   /device/{SN}/p2p-channel          (answered later, with the device's addresses)
//!   relay  /relay/agent → token + agent
//!   agent  /relay/start/{token}
//!   main   /device/{SN}/relay-channel        (device is told to join the agent)
//!   agent  confirms the device joined; PTCP then flows through the agent.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use rand::RngCore;

use crate::dhhttp::{resolve, DhSocket};
use crate::{Error, Result};

#[derive(Debug, Clone, Default)]
pub struct DeviceAddrs {
    /// LAN addresses the device reports, e.g. "192.168.1.108:19196" (first of the list).
    pub local: Vec<String>,
    pub public: Option<String>,
    pub version: Option<String>,
}

pub struct Relay {
    pub agent: SocketAddr,
    pub device: DeviceAddrs,
    pub elapsed: Duration,
}

fn parse_local_addrs(s: &str) -> Vec<String> {
    // "10.0.0.5,192.168.1.108:19196": several IPs share one port
    let Some((hosts, port)) = s.rsplit_once(':') else { return Vec::new() };
    hosts.split(',').filter(|h| !h.is_empty()).map(|h| format!("{h}:{port}")).collect()
}

pub async fn relay_handshake(sock: &DhSocket, serial: &str, cloud: &str, timeout: Duration) -> Result<Relay> {
    let t0 = Instant::now();
    let main = resolve(cloud).await?;
    sock.call(main, "/probe/p2psrv", None).await?;
    let online = sock.call(main, &format!("/online/p2psrv/{serial}"), None).await?;
    let us = online.get("body/US").ok_or_else(|| Error::NotOnline(serial.to_string()))?.to_string();
    let relay_res = sock.call(main, "/online/relay", None).await?;
    let relay_addr = relay_res.get("body/Address").ok_or_else(|| Error::Protocol("no relay server".into()))?.to_string();

    let p2psrv = resolve(&us).await?;
    match sock.call(p2psrv, &format!("/probe/device/{serial}"), None).await {
        // The P2P server does not know the device right now: it is not connected
        Err(Error::Status { code: 404, .. }) => return Err(Error::NotOnline(serial.to_string())),
        other => other?,
    };

    let mut cid = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut cid);
    let ident = cid.iter().map(|b| format!("{b:x}")).collect::<Vec<_>>().join(" ");
    let channel_body = format!(
        "<body><Identify>{ident}</Identify><IpEncrpt>true</IpEncrpt><LocalAddr>127.0.0.1:{}</LocalAddr><version>5.0.0</version></body>",
        sock.local_port()
    );
    let channel_cseq = sock.request(main, &format!("/device/{serial}/p2p-channel"), Some(&channel_body)).await;

    let relay = resolve(&relay_addr).await?;
    let agent_res = sock.call(relay, "/relay/agent", None).await?;
    let (Some(token), Some(agent_addr)) = (agent_res.get("body/Token"), agent_res.get("body/Agent")) else {
        return Err(Error::Protocol("relay returned no agent".into()));
    };
    let agent_addr = agent_addr.to_string();
    let agent = resolve(&agent_addr).await?;
    sock.call(agent, &format!("/relay/start/{token}"), Some("<body><Client>:0</Client></body>")).await?;

    let channel = sock.read_final(main, timeout, Some(&[channel_cseq])).await?;
    match channel.code {
        403 => return Err(Error::DeviceAuthRequired),
        c if c >= 300 => {
            return Err(Error::Status { what: "p2p-channel".into(), code: c, status: channel.status })
        }
        _ => {}
    }
    let device = DeviceAddrs {
        local: channel.get("body/LocalAddr").map(parse_local_addrs).unwrap_or_default(),
        public: channel.get("body/PubAddr").map(String::from),
        version: channel.get("body/version").map(String::from),
    };

    sock.request(main, &format!("/device/{serial}/relay-channel"), Some(&format!("<body><agentAddr>{agent_addr}</agentAddr></body>")))
        .await;
    // The agent confirms the device joined within ~100 ms, or never (device busy)
    sock.read_final(agent, Duration::from_secs(3), None).await.map_err(|_| Error::DeviceBusy)?;

    Ok(Relay { agent, device, elapsed: t0.elapsed() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_addrs() {
        assert_eq!(
            parse_local_addrs("10.0.0.5,192.168.1.108:19196"),
            vec!["10.0.0.5:19196".to_string(), "192.168.1.108:19196".to_string()]
        );
        assert_eq!(parse_local_addrs("10.0.0.2:1"), vec!["10.0.0.2:1".to_string()]);
    }
}
