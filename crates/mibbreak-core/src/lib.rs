//! Reach Dahua-based cameras (Dahua, Intelbras Mibo, Imou…) through the Easy4ip
//! cloud relay, and read their video over RTSP.
//!
//! Protocol work by khoanguyen-3fc/dh-p2p (handshake, crypto) and
//! ludufre/dahua-p2p (reliable PTCP, session behaviour), both MIT.

pub mod audio;
pub mod devinfo;
pub mod dhhttp;
pub mod error;
pub mod handshake;
pub mod ptcp;
pub mod qr;
pub mod rtsp;
pub mod tunnel;

pub use error::{Error, Result};
pub use audio::{AudioCodec, AudioFrame, AudioTrack};
pub use rtsp::{Codec, Media, RtspPlayer, VideoFrame, VideoTrack};
pub use tunnel::{Tunnel, TunnelOptions, TunnelStats};
