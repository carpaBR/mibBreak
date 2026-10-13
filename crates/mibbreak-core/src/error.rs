use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("device {0} is not online in the cloud")]
    NotOnline(String),

    /// The relay agent never confirmed that the device joined. A device takes one
    /// relay session at a time and keeps a closed one for ~15–20 s, so this is what
    /// a new tunnel sees while another client (e.g. the vendor phone app) holds it.
    #[error("device is busy with another cloud session (vendor app open on a phone, or another viewer)")]
    DeviceBusy,

    #[error("device requires channel authentication (not supported)")]
    DeviceAuthRequired,

    #[error("RTSP login refused (wrong user or password)")]
    Unauthorized,

    #[error("{what}: {code} {status}")]
    Status { what: String, code: u16, status: String },

    #[error("timeout: {0}")]
    Timeout(String),

    #[error("tunnel closed: {0}")]
    Closed(String),

    #[error("protocol: {0}")]
    Protocol(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
