//! Cameras, layouts and settings, saved as JSON in the app config directory.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// LAN when the camera answers there, cloud otherwise.
    #[default]
    Auto,
    Lan,
    Cloud,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Camera {
    pub id: String,
    pub name: String,
    pub serial: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "default_user")]
    pub user: String,
    pub password: String,
    #[serde(default = "default_channel")]
    pub channel: u32,
    #[serde(default)]
    pub mode: Mode,
    /// Fixed LAN address set by the user (ip or ip:port), tried first.
    #[serde(default)]
    pub host: Option<String>,
    /// LAN IPs the device reported through the cloud (learned automatically).
    #[serde(default)]
    pub lan_hosts: Vec<String>,
}

fn default_user() -> String {
    "admin".into()
}
fn default_channel() -> u32 {
    1
}

/// What the UI sees: everything but the password.
#[derive(Debug, Clone, Serialize)]
pub struct CameraView {
    pub id: String,
    pub name: String,
    pub serial: String,
    pub model: Option<String>,
    pub user: String,
    pub channel: u32,
    pub mode: Mode,
    pub host: Option<String>,
    pub lan_hosts: Vec<String>,
}

impl From<&Camera> for CameraView {
    fn from(c: &Camera) -> Self {
        Self {
            id: c.id.clone(),
            name: c.name.clone(),
            serial: c.serial.clone(),
            model: c.model.clone(),
            user: c.user.clone(),
            channel: c.channel,
            mode: c.mode,
            host: c.host.clone(),
            lan_hosts: c.lan_hosts.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Always request the main (full resolution) stream.
    pub always_hd: bool,
    /// Switch a tile to the main stream while the mouse is over it.
    pub hover_hd: bool,
    /// Release a camera's cloud session this long after nobody watches it, so
    /// the vendor phone app can reach it again.
    pub release_after_secs: u64,
    pub lang: String,
    /// auto | list | mosaic (auto: list on phones, mosaic on bigger screens)
    pub view: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self { always_hd: false, hover_hd: false, release_after_secs: 30, lang: "pt".into(), view: "auto".into() }
    }
}

pub struct Store {
    dir: PathBuf,
    cameras: Mutex<Vec<Camera>>,
    settings: Mutex<Settings>,
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(v) => Some(v),
        Err(e) => {
            log::error!("{}: {e}", path.display());
            None
        }
    }
}

fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(tmp, path)
}

impl Store {
    pub fn open(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        let cameras = read_json(&dir.join("cameras.json")).unwrap_or_default();
        let settings = read_json(&dir.join("settings.json")).unwrap_or_default();
        Self { dir, cameras: Mutex::new(cameras), settings: Mutex::new(settings) }
    }

    fn save_cameras(&self, cams: &[Camera]) {
        let json = serde_json::to_vec_pretty(cams).unwrap();
        if let Err(e) = write_private(&self.dir.join("cameras.json"), &json) {
            log::error!("saving cameras: {e}");
        }
    }

    pub fn cameras(&self) -> Vec<Camera> {
        self.cameras.lock().unwrap().clone()
    }

    pub fn camera(&self, id: &str) -> Option<Camera> {
        self.cameras.lock().unwrap().iter().find(|c| c.id == id).cloned()
    }

    pub fn add(&self, cam: Camera) -> Result<(), String> {
        let mut cams = self.cameras.lock().unwrap();
        if cams.iter().any(|c| c.serial.eq_ignore_ascii_case(&cam.serial) && c.channel == cam.channel) {
            return Err("duplicate".into());
        }
        cams.push(cam);
        self.save_cameras(&cams);
        Ok(())
    }

    pub fn update(&self, id: &str, f: impl FnOnce(&mut Camera)) -> Option<Camera> {
        let mut cams = self.cameras.lock().unwrap();
        let cam = cams.iter_mut().find(|c| c.id == id)?;
        f(cam);
        let out = cam.clone();
        self.save_cameras(&cams);
        Some(out)
    }

    pub fn remove(&self, id: &str) -> bool {
        let mut cams = self.cameras.lock().unwrap();
        let before = cams.len();
        cams.retain(|c| c.id != id);
        let removed = cams.len() != before;
        if removed {
            self.save_cameras(&cams);
        }
        removed
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    pub fn set_settings(&self, s: Settings) {
        let json = serde_json::to_vec_pretty(&s).unwrap();
        *self.settings.lock().unwrap() = s;
        if let Err(e) = write_private(&self.dir.join("settings.json"), &json) {
            log::error!("saving settings: {e}");
        }
    }

    /// Layouts belong to the UI: stored as given.
    pub fn layouts(&self) -> serde_json::Value {
        read_json(&self.dir.join("layouts.json")).unwrap_or(serde_json::json!({}))
    }

    pub fn set_layouts(&self, v: &serde_json::Value) {
        let json = serde_json::to_vec_pretty(v).unwrap();
        if let Err(e) = write_private(&self.dir.join("layouts.json"), &json) {
            log::error!("saving layouts: {e}");
        }
    }
}
