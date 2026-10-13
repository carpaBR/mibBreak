mod hub;
mod store;

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{Manager, State};
#[cfg(desktop)]
use tauri::{WebviewUrl, WebviewWindowBuilder};
use mibbreak_core::{RtspPlayer, Tunnel, TunnelOptions};

use hub::{FrameSink, Hub, Status};
use store::{Camera, CameraView, Mode, Settings, Store};

struct App {
    store: Arc<Store>,
    hub: Hub,
}

type S<'a> = State<'a, App>;

#[derive(Deserialize)]
struct NewCamera {
    name: String,
    serial: String,
    user: Option<String>,
    password: String,
    model: Option<String>,
    channel: Option<u32>,
    host: Option<String>,
    mode: Option<Mode>,
}

#[derive(Deserialize)]
struct CameraPatch {
    name: Option<String>,
    user: Option<String>,
    password: Option<String>,
    channel: Option<u32>,
    host: Option<String>,
    mode: Option<Mode>,
}

#[derive(Serialize)]
struct QrInfo {
    serial: String,
    model: Option<String>,
    safety_code: Option<String>,
}

#[derive(Serialize)]
struct TestResult {
    ok: bool,
    via: Option<String>,
    codec: Option<String>,
    /// busy | auth | offline | timeout | network | other
    error: Option<String>,
    detail: Option<String>,
}

fn clean_serial(s: &str) -> Result<String, String> {
    let s = s.trim().to_ascii_uppercase();
    if s.len() < 6 || !s.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("invalid serial".into());
    }
    Ok(s)
}

/// Lets the UI write to the app log (diagnostics).
#[tauri::command]
fn ui_log(msg: String) {
    log::info!(target: "ui", "{msg}");
}

#[tauri::command]
fn list_cameras(app: S) -> Vec<CameraView> {
    app.store.cameras().iter().map(CameraView::from).collect()
}

#[tauri::command]
fn add_camera(app: S, cam: NewCamera) -> Result<CameraView, String> {
    let serial = clean_serial(&cam.serial)?;
    let id = format!("{}-{}", serial.to_ascii_lowercase(), cam.channel.unwrap_or(1));
    let cam = Camera {
        id,
        name: if cam.name.trim().is_empty() { serial.clone() } else { cam.name.trim().to_string() },
        serial,
        model: cam.model,
        user: cam.user.filter(|u| !u.is_empty()).unwrap_or_else(|| "admin".into()),
        password: cam.password,
        channel: cam.channel.unwrap_or(1),
        mode: cam.mode.unwrap_or_default(),
        host: cam.host.filter(|h| !h.trim().is_empty()),
        lan_hosts: Vec::new(),
    };
    let view = CameraView::from(&cam);
    app.store.add(cam)?;
    Ok(view)
}

#[tauri::command]
fn update_camera(app: S, id: String, patch: CameraPatch) -> Result<CameraView, String> {
    // A new name alone does not touch the connection
    let reconnect = patch.user.is_some() || patch.password.is_some() || patch.channel.is_some() || patch.host.is_some() || patch.mode.is_some();
    let cam = app
        .store
        .update(&id, |c| {
            if let Some(v) = patch.name.filter(|v| !v.trim().is_empty()) {
                c.name = v.trim().to_string();
            }
            if let Some(v) = patch.user.filter(|v| !v.is_empty()) {
                c.user = v;
            }
            if let Some(v) = patch.password.filter(|v| !v.is_empty()) {
                c.password = v;
            }
            if let Some(v) = patch.channel {
                c.channel = v;
            }
            if let Some(v) = patch.host {
                c.host = Some(v.trim().to_string()).filter(|h| !h.is_empty());
            }
            if let Some(v) = patch.mode {
                c.mode = v;
            }
        })
        .ok_or("unknown camera")?;
    if reconnect {
        app.hub.restart(&id);
    }
    Ok(CameraView::from(&cam))
}

#[tauri::command]
fn remove_camera(app: S, id: String) -> bool {
    app.hub.remove(&id);
    app.store.remove(&id)
}

#[tauri::command]
fn parse_qr_text(text: String) -> Option<QrInfo> {
    mibbreak_core::qr::parse(&text).map(|q| QrInfo { serial: q.serial, model: q.model, safety_code: q.safety_code })
}

#[tauri::command]
fn decode_qr_image(bytes: Vec<u8>) -> Result<Option<QrInfo>, String> {
    let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?.to_luma8();
    let mut prepared = rqrr::PreparedImage::prepare(img);
    for grid in prepared.detect_grids() {
        if let Ok((_, text)) = grid.decode() {
            if let Some(info) = parse_qr_text(text) {
                return Ok(Some(info));
            }
        }
    }
    Ok(None)
}

/// Opens the camera once (cloud, or LAN when `host` is given) and waits for a keyframe.
#[tauri::command]
async fn test_camera(serial: String, user: Option<String>, password: String, host: Option<String>) -> TestResult {
    let user = user.filter(|u| !u.is_empty()).unwrap_or_else(|| "admin".into());
    let fail = |e: mibbreak_core::Error| {
        let code = match &e {
            mibbreak_core::Error::DeviceBusy => "busy",
            mibbreak_core::Error::Unauthorized => "auth",
            mibbreak_core::Error::NotOnline(_) => "offline",
            mibbreak_core::Error::Timeout(_) => "timeout",
            mibbreak_core::Error::Io(_) => "network",
            _ => "other",
        };
        TestResult { ok: false, via: None, codec: None, error: Some(code.into()), detail: Some(e.to_string()) }
    };
    let Ok(serial) = clean_serial(&serial) else {
        return TestResult { ok: false, via: None, codec: None, error: Some("other".into()), detail: Some("invalid serial".into()) };
    };
    let url_for = |host: &str| format!("rtsp://{host}/cam/realmonitor?channel=1&subtype=1");
    let timeout = Duration::from_secs(10);

    async fn first_key<S2>(mut p: RtspPlayer<S2>) -> Option<String>
    where
        S2: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static,
    {
        let codec = format!("{:?}", p.track.codec);
        let got = tokio::time::timeout(Duration::from_secs(8), async {
            while let Some(f) = p.next_frame().await {
                if f.keyframe {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false);
        p.stop().await;
        got.then_some(codec)
    }

    if let Some(h) = host.filter(|h| !h.trim().is_empty()) {
        let h = if h.contains(':') { h } else { format!("{h}:554") };
        return match tokio::net::TcpStream::connect(&h).await {
            Ok(tcp) => match RtspPlayer::start(tcp, &url_for(&h), Some(&user), Some(&password), timeout).await {
                Ok(p) => match first_key(p).await {
                    Some(codec) => TestResult { ok: true, via: Some("lan".into()), codec: Some(codec), error: None, detail: None },
                    None => fail(mibbreak_core::Error::Timeout("no video".into())),
                },
                Err(e) => fail(e),
            },
            Err(e) => fail(e.into()),
        };
    }

    let mut opts = TunnelOptions::new(&serial);
    opts.busy_retry = Duration::from_secs(8);
    let tunnel = match Tunnel::open(&opts).await {
        Ok(t) => t,
        Err(e) => return fail(e),
    };
    let realm = match tunnel.connect(554).await {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let res = match RtspPlayer::start(realm, &url_for("127.0.0.1"), Some(&user), Some(&password), timeout).await {
        Ok(p) => match first_key(p).await {
            Some(codec) => TestResult { ok: true, via: Some("cloud".into()), codec: Some(codec), error: None, detail: None },
            None => fail(mibbreak_core::Error::Timeout("no video".into())),
        },
        Err(e) => fail(e),
    };
    tunnel.close();
    res
}

/// Portable camera list (import / export between devices).
#[derive(Serialize, Deserialize)]
struct ExportedCamera {
    name: String,
    serial: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    password: Option<String>,
    #[serde(default)]
    channel: Option<u32>,
    #[serde(default)]
    mode: Option<Mode>,
    #[serde(default)]
    host: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct ExportFile {
    format: String,
    version: u32,
    cameras: Vec<ExportedCamera>,
}

const EXPORT_FORMAT: &str = "mibbreak-cameras";

#[tauri::command]
fn export_cameras(app: S, include_passwords: bool) -> String {
    let cameras = app
        .store
        .cameras()
        .into_iter()
        .map(|c| ExportedCamera {
            name: c.name,
            serial: c.serial,
            model: c.model,
            user: Some(c.user),
            password: include_passwords.then_some(c.password),
            channel: Some(c.channel),
            mode: Some(c.mode),
            host: c.host,
        })
        .collect();
    serde_json::to_string_pretty(&ExportFile { format: EXPORT_FORMAT.into(), version: 1, cameras }).unwrap()
}

#[derive(Serialize)]
struct ImportResult {
    added: usize,
    updated: usize,
    skipped: usize,
}

/// Adds new cameras and updates known ones (same serial + channel).
#[tauri::command]
fn import_cameras(app: S, json: String) -> Result<ImportResult, String> {
    let file: ExportFile = serde_json::from_str(&json).map_err(|e| format!("invalid file: {e}"))?;
    // "mibobreak-cameras": files from the test builds made before the rename
    if file.format != EXPORT_FORMAT && file.format != "mibobreak-cameras" {
        return Err("not a mibBreak camera file".into());
    }
    let mut res = ImportResult { added: 0, updated: 0, skipped: 0 };
    let known = app.store.cameras();
    for c in file.cameras {
        let Ok(serial) = clean_serial(&c.serial) else {
            res.skipped += 1;
            continue;
        };
        let channel = c.channel.unwrap_or(1);
        if let Some(existing) = known.iter().find(|k| k.serial == serial && k.channel == channel) {
            app.store.update(&existing.id, |k| {
                k.name = c.name.clone();
                if let Some(u) = c.user.clone().filter(|u| !u.is_empty()) {
                    k.user = u;
                }
                if let Some(p) = c.password.clone().filter(|p| !p.is_empty()) {
                    k.password = p;
                }
                if let Some(m) = c.mode {
                    k.mode = m;
                }
                k.host = c.host.clone().filter(|h| !h.trim().is_empty());
                if c.model.is_some() {
                    k.model = c.model.clone();
                }
            });
            app.hub.restart(&existing.id);
            res.updated += 1;
        } else {
            let cam = Camera {
                id: format!("{}-{channel}", serial.to_ascii_lowercase()),
                name: c.name,
                serial,
                model: c.model,
                user: c.user.filter(|u| !u.is_empty()).unwrap_or_else(|| "admin".into()),
                password: c.password.unwrap_or_default(),
                channel,
                mode: c.mode.unwrap_or_default(),
                host: c.host.filter(|h| !h.trim().is_empty()),
                lan_hosts: Vec::new(),
            };
            match app.store.add(cam) {
                Ok(()) => res.added += 1,
                Err(_) => res.skipped += 1,
            }
        }
    }
    Ok(res)
}

/// Asks where to save and writes `text` there. Returns false when cancelled.
#[cfg(desktop)]
#[tauri::command]
async fn save_text_file(handle: tauri::AppHandle, name: String, text: String) -> Result<bool, String> {
    use tauri_plugin_dialog::DialogExt;
    let Some(path) = handle.dialog().file().set_file_name(&name).add_filter("JSON", &["json"]).blocking_save_file() else {
        return Ok(false);
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())?;
    Ok(true)
}

#[cfg(mobile)]
#[tauri::command]
async fn save_text_file(_name: String, _text: String) -> Result<bool, String> {
    Err("use copy on mobile".into())
}

#[tauri::command]
fn subscribe(app: S, id: String, hd: bool, sink: FrameSink) -> Result<u64, String> {
    app.hub.subscribe(&id, hd, sink)
}

#[tauri::command]
fn set_hd(app: S, sub: u64, hd: bool) {
    app.hub.set_hd(sub, hd);
}

#[tauri::command]
fn set_audio(app: S, sub: u64, on: bool) {
    app.hub.set_audio(sub, on);
}

#[tauri::command]
fn unsubscribe(app: S, sub: u64) {
    app.hub.unsubscribe(sub);
}

#[tauri::command]
fn statuses(app: S) -> Vec<Status> {
    app.hub.statuses()
}

#[tauri::command]
fn get_settings(app: S) -> Settings {
    app.store.settings()
}

#[tauri::command]
fn set_settings(app: S, settings: Settings) {
    app.store.set_settings(settings);
}

#[tauri::command]
fn get_layouts(app: S) -> serde_json::Value {
    app.store.layouts()
}

#[tauri::command]
fn set_layouts(app: S, layouts: serde_json::Value) {
    app.store.set_layouts(&layouts);
}

/// A small always-on-top window with one camera (desktop only).
#[cfg(desktop)]
#[tauri::command]
fn open_pip(handle: tauri::AppHandle, app: S, id: String) -> Result<(), String> {
    let cam = app.store.camera(&id).ok_or("unknown camera")?;
    let label = format!("pip-{}", id.replace(|c: char| !c.is_ascii_alphanumeric(), "_"));
    if let Some(w) = handle.get_webview_window(&label) {
        let _ = w.set_focus();
        return Ok(());
    }
    let url = format!("index.html?pip={id}");
    WebviewWindowBuilder::new(&handle, label, WebviewUrl::App(url.into()))
        .title(cam.name)
        .inner_size(480.0, 270.0)
        .min_inner_size(200.0, 112.0)
        .always_on_top(true)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(mobile)]
#[tauri::command]
fn open_pip(_id: String) -> Result<(), String> {
    Err("not available on mobile".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,mibbreak_core=info")).init();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            log::info!("config dir: {}", dir.display());
            let store = Arc::new(Store::open(dir));
            let hub = Hub::new(app.handle().clone(), store.clone());
            app.manage(App { store, hub });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ui_log,
            list_cameras,
            add_camera,
            update_camera,
            remove_camera,
            parse_qr_text,
            decode_qr_image,
            test_camera,
            subscribe,
            set_hd,
            set_audio,
            unsubscribe,
            statuses,
            get_settings,
            set_settings,
            get_layouts,
            set_layouts,
            open_pip,
            export_cameras,
            import_cameras,
            save_text_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running mibbreak");
}
