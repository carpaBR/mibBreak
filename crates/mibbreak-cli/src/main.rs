use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use mibbreak_core::{RtspPlayer, Tunnel, TunnelOptions};

#[derive(Parser)]
#[command(name = "mibbreak", about = "Dahua / Intelbras / Imou cameras through the Easy4ip cloud, without the vendor app")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Expose the camera's RTSP on a local TCP port (then use ffplay / VLC)
    Tunnel {
        serial: String,
        #[arg(short, long, default_value = "127.0.0.1:1554")]
        listen: SocketAddr,
        #[arg(short, long, default_value_t = 554)]
        remote: u16,
    },
    /// Save the first keyframe as raw Annex B (.h264 / .h265)
    Snapshot {
        #[command(flatten)]
        cam: CamArgs,
        #[arg(short, long)]
        out: String,
    },
    /// Live video stability test: reports frame rate and stalls
    Soak {
        #[command(flatten)]
        cam: CamArgs,
        #[arg(short, long, default_value_t = 2)]
        minutes: u64,
        /// Fail when no frame arrives for this long (seconds)
        #[arg(long, default_value_t = 5)]
        stall: u64,
    },
}

#[derive(Args)]
struct CamArgs {
    serial: String,
    #[arg(short, long, default_value = "admin")]
    user: String,
    /// File holding the device password (or set MIBBREAK_PASS)
    #[arg(long)]
    pass_file: Option<String>,
    #[arg(short, long, default_value_t = 1)]
    channel: u32,
    /// 0 = main stream (full resolution), 1 = sub stream (economy)
    #[arg(short, long, default_value_t = 1)]
    subtype: u32,
    /// Talk to the camera on the LAN instead of the cloud (ip or ip:port)
    #[arg(long)]
    direct: Option<String>,
}

impl CamArgs {
    fn password(&self) -> Result<String> {
        if let Some(f) = &self.pass_file {
            return Ok(std::fs::read_to_string(f).with_context(|| format!("reading {f}"))?.trim_end_matches(['\r', '\n']).to_string());
        }
        std::env::var("MIBBREAK_PASS").context("password needed: --pass-file or MIBBREAK_PASS")
    }

    fn url(&self) -> String {
        format!("rtsp://127.0.0.1/cam/realmonitor?channel={}&subtype={}", self.channel, self.subtype)
    }
}

/// Runs `f` on an RTSP player over the LAN or through a fresh tunnel.
async fn with_player<F, Fut>(cam: &CamArgs, f: F) -> Result<()>
where
    F: FnOnce(Player) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let pass = cam.password()?;
    let timeout = Duration::from_secs(10);
    if let Some(addr) = &cam.direct {
        let addr = if addr.contains(':') { addr.clone() } else { format!("{addr}:554") };
        let tcp = tokio::net::TcpStream::connect(&addr).await.with_context(|| format!("connecting to {addr}"))?;
        let url = cam.url().replace("127.0.0.1", &addr);
        let p = RtspPlayer::start(tcp, &url, Some(&cam.user), Some(&pass), timeout).await?;
        return f(Player::Lan(p)).await;
    }
    let t0 = Instant::now();
    let tunnel = Tunnel::open(&TunnelOptions::new(&cam.serial)).await?;
    eprintln!(
        "tunnel ready in {} ms (device LAN {:?}, firmware {})",
        t0.elapsed().as_millis(),
        tunnel.device.local,
        tunnel.device.version.as_deref().unwrap_or("?")
    );
    let realm = tunnel.connect(554).await?;
    let p = RtspPlayer::start(realm, &cam.url(), Some(&cam.user), Some(&pass), timeout).await?;
    f(Player::Cloud(p, tunnel)).await
}

enum Player {
    Lan(RtspPlayer<tokio::net::TcpStream>),
    Cloud(RtspPlayer<tokio::io::DuplexStream>, Tunnel),
}

impl Player {
    async fn next_media(&mut self) -> Option<mibbreak_core::Media> {
        match self {
            Player::Lan(p) => p.next_media().await,
            Player::Cloud(p, _) => p.next_media().await,
        }
    }
    fn audio(&self) -> Option<&mibbreak_core::AudioTrack> {
        match self {
            Player::Lan(p) => p.audio.as_ref(),
            Player::Cloud(p, _) => p.audio.as_ref(),
        }
    }
    async fn next_frame(&mut self) -> Option<mibbreak_core::VideoFrame> {
        match self {
            Player::Lan(p) => p.next_frame().await,
            Player::Cloud(p, _) => p.next_frame().await,
        }
    }
    fn track(&self) -> &mibbreak_core::VideoTrack {
        match self {
            Player::Lan(p) => &p.track,
            Player::Cloud(p, _) => &p.track,
        }
    }
    fn wire(&self) -> String {
        match self {
            Player::Lan(_) => "lan".into(),
            Player::Cloud(_, t) => {
                let s = t.stats();
                format!(
                    "udp_in={}KB ooo={} gaps_filled={} dup={} retx={}",
                    s.udp_bytes_in / 1024,
                    s.ptcp.out_of_order_in,
                    s.ptcp.gaps_filled,
                    s.ptcp.duplicates_in,
                    s.ptcp.retransmits_out
                )
            }
        }
    }
    async fn stop(self) {
        match self {
            Player::Lan(p) => p.stop().await,
            Player::Cloud(p, t) => {
                p.stop().await;
                t.close();
            }
        }
    }
}

async fn soak(mut p: Player, minutes: u64, stall: u64) -> Result<()> {
    eprintln!("codec {:?}, audio {:?}", p.track().codec, p.audio().map(|a| (&a.codec, a.clock_rate)));
    let mut audio_frames = 0u64;
    let t0 = Instant::now();
    let end = t0 + Duration::from_secs(minutes * 60);
    let (mut frames, mut keys, mut bytes) = (0u64, 0u64, 0u64);
    let mut last = Instant::now();
    let mut worst = Duration::ZERO;
    let mut report = t0 + Duration::from_secs(5);
    let mut window_bytes = 0u64;
    while Instant::now() < end {
        let f = match tokio::time::timeout(Duration::from_secs(stall), p.next_media()).await {
            Ok(Some(mibbreak_core::Media::Audio(_))) => {
                audio_frames += 1;
                continue;
            }
            Ok(Some(mibbreak_core::Media::Video(f))) => f,
            Ok(None) => bail!("stream ended after {frames} frames"),
            Err(_) => bail!("STALL: no frame for {stall}s after {frames} frames"),
        };
        let now = Instant::now();
        if frames > 0 {
            worst = worst.max(now - last);
        }
        last = now;
        frames += 1;
        keys += f.keyframe as u64;
        bytes += f.data.len() as u64;
        window_bytes += f.data.len() as u64;
        if now >= report {
            let secs = now.duration_since(t0).as_secs();
            println!(
                "{secs}s audio={audio_frames} frames={frames} fps={:.1} key={keys} worstGap={}ms kbps={} {}",
                frames as f64 / now.duration_since(t0).as_secs_f64(),
                worst.as_millis(),
                window_bytes * 8 / 5 / 1000,
                p.wire()
            );
            window_bytes = 0;
            report += Duration::from_secs(5);
        }
    }
    println!(
        "OK {}s frames={frames} key={keys} worstGap={}ms avg_kbps={} {}",
        minutes * 60,
        worst.as_millis(),
        bytes * 8 / (minutes * 60).max(1) / 1000,
        p.wire()
    );
    p.stop().await;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    match Cli::parse().cmd {
        Cmd::Tunnel { serial, listen, remote } => {
            let tunnel = Arc::new(Tunnel::open(&TunnelOptions::new(&serial)).await?);
            eprintln!("tunnel ready: {listen} → {serial}:{remote}");
            tunnel.listen(listen, remote).await?;
        }
        Cmd::Snapshot { cam, out } => {
            with_player(&cam, |mut p| async move {
                let f = loop {
                    let f = tokio::time::timeout(Duration::from_secs(10), p.next_frame())
                        .await
                        .context("no keyframe in time")?
                        .context("stream ended")?;
                    if f.keyframe {
                        break f;
                    }
                };
                std::fs::write(&out, &f.data)?;
                eprintln!("saved {} ({} bytes, {:?})", out, f.data.len(), f.codec);
                p.stop().await;
                Ok(())
            })
            .await?;
        }
        Cmd::Soak { cam, minutes, stall } => with_player(&cam, |p| soak(p, minutes, stall)).await?,
    }
    Ok(())
}
