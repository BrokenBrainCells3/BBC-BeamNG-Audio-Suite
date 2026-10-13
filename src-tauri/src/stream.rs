//! Streams ONLY the Opera box (see embed.rs) as JPEG frames to disk snapshots.
//!
//! Single streaming method: the local:// VFS file poller. Every encoded frame
//! is mirrored to drop files inside the BeamNG mods folder, which the vehicle
//! texture pages <img>-poll with a cache-buster (?v=N). The engine's CEF
//! client cancels http:// and holds ws:// forever inside texture webviews
//! (both proven broken on 0.39), so disk snapshots via local:// are the only
//! channel that reaches an HTML texture.
//!
//! Capture path: the PARENT (Tauri main window) via Windows.Graphics Capture
//! (GPU compositor surface), cropped exactly to the Opera box rect. Frames
//! are Opera-sized by construction — the rest of the app cannot leak in — and
//! compositor capture keeps working when the app is covered, occluded, or on
//! another virtual desktop (a true minimize has no DWM surface; nothing on
//! Windows can capture that, so keep the app unminimized while streaming).
//! A reparented `WS_CHILD` has no own DWM surface, which is why child-targeted
//! capture misbehaved — the parent always has one.
//!
//! Fallback path: `PrintWindow` renders the Opera child HWND itself
//! (screen-independent), used only when WGC can't start.
//!
//! While streaming, the loop also re-places the embedded child from the latest
//! frontend rect (`EmbedState.pending`) at capture fps, so the native window
//! stays glued to its HTML section mid-scroll.
//!
//! Windows-only. All commands report a friendly error elsewhere.

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};

use tauri::{AppHandle, Manager, Runtime, State};

use crate::embed::EmbedState;

#[derive(Clone, Copy)]
struct StreamCfg {
    fps: u32,
    quality: u8,
}

impl Default for StreamCfg {
    fn default() -> Self {
        Self {
            fps: 60,
            quality: 70,
        }
    }
}

/// Diagnostic snapshot: which capture path is feeding the stream and the
/// exact sizes involved. Visible via `stream_status` — if `frame_w/frame_h`
/// ever exceed the Opera `box_w/box_h`, something regressed.
#[derive(Clone, Default)]
struct StreamInfo {
    mode: String,
    fw: u32,
    fh: u32,
    box_w: u32,
    box_h: u32,
}

pub struct StreamState {
    running: Arc<AtomicBool>,
    gen: Arc<AtomicU64>,
    frames: Arc<AtomicU64>,
    cfg: Mutex<StreamCfg>,
    info: Arc<Mutex<StreamInfo>>,
}

impl StreamState {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            gen: Arc::new(AtomicU64::new(0)),
            frames: Arc::new(AtomicU64::new(0)),
            cfg: Mutex::new(StreamCfg::default()),
            info: Arc::new(Mutex::new(StreamInfo {
                mode: "idle".into(),
                ..Default::default()
            })),
        }
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Start streaming the embedded Opera window to the local:// disk snapshots.
/// No target picker by design. `port` is accepted for UI compatibility but
/// unused — there are no network servers anymore.
#[tauri::command]
pub fn start_opera_stream<R: Runtime>(
    app: AppHandle<R>,
    embed: State<'_, EmbedState>,
    stream: State<'_, StreamState>,
    port: Option<u16>, // accepted for UI compatibility; no network servers anymore
    fps: Option<u32>,
    quality: Option<u8>,
) -> Result<String, String> {
    #[cfg(not(windows))]
    {
        let _ = (&app, &embed, &stream, &port, &fps, &quality);
        return Err("Opera streaming is only supported on Windows.".into());
    }

    #[cfg(windows)]
    {
        use tauri::Emitter;
        let _ = port;

        // The ONLY capture target: our embedded child window + its section.
        let (hwnd_raw, parent_raw) = {
            let mut g = embed.0.lock().map_err(|e| e.to_string())?;
            let w = g
                .window
                .map(|(raw, _)| raw)
                .ok_or("Launch and dock Opera first — no embedded window to stream.")?;
            // Parent is published at launch; backfill for sessions embedded
            // before this field existed so no relaunch dance is needed.
            let p = match g.parent {
                Some(p) => p,
                None => {
                    let main = app
                        .get_webview_window("main")
                        .ok_or("Main window 'main' not found")?;
                    let pt = main.hwnd().map_err(|e| format!("Failed to get HWND: {e}"))?;
                    let p = pt.0 as isize;
                    g.parent = Some(p);
                    p
                }
            };
            (w, p)
        };

        if stream.running.load(Ordering::Relaxed) {
            return Ok("Already streaming (local:// disk snapshots).".into());
        }

        let fps = fps.unwrap_or(60).clamp(5, 120);
        let quality = quality.unwrap_or(70).clamp(10, 95);
        {
            let mut cfg = stream.cfg.lock().map_err(|e| e.to_string())?;
            *cfg = StreamCfg { fps, quality };
        }
        stream.running.store(true, Ordering::Relaxed);
        stream.frames.store(0, Ordering::Relaxed);
        let gen = stream.gen.fetch_add(1, Ordering::Relaxed) + 1;

        {
            let running = stream.running.clone();
            let frames = stream.frames.clone();
            let gen_ref = stream.gen.clone();
            let info = stream.info.clone();
            let app_handle = app.clone();
            std::thread::spawn(move || {
                capture_thread(
                    app_handle,
                    hwnd_raw,
                    parent_raw,
                    gen,
                    fps,
                    quality,
                    running,
                    frames,
                    gen_ref,
                    info,
                );
            });
        }

        let _ = app.emit(
            "stream-started",
            serde_json::json!({ "fps": fps, "quality": quality }),
        );
        Ok(format!(
            "Streaming embedded Opera to local:// snapshots @ {fps}fps q{quality}"
        ))
    }
}

#[tauri::command]
pub fn stop_opera_stream<R: Runtime>(
    app: AppHandle<R>,
    stream: State<'_, StreamState>,
) -> Result<String, String> {
    #[cfg(not(windows))]
    {
        let _ = (&app, &stream);
        return Err("Opera streaming is only supported on Windows.".into());
    }

    #[cfg(windows)]
    {
        use tauri::Emitter;

        stream.running.store(false, Ordering::Relaxed);
        stream.gen.fetch_add(1, Ordering::Relaxed); // retire the capture thread
        let _ = app.emit("stream-stopped", serde_json::json!({ "running": false }));
        Ok("Stream stopped. Viewers stay connected but receive no new frames.".into())
    }
}

#[tauri::command]
pub fn stream_status(stream: State<'_, StreamState>) -> serde_json::Value {
    #[cfg(not(windows))]
    {
        let _ = &stream;
        return serde_json::json!({ "running": false, "supported": false });
    }

    #[cfg(windows)]
    {
        let cfg = stream.cfg.lock().map(|c| *c).unwrap_or_default();
        let inf = stream.info.lock().map(|i| i.clone()).unwrap_or_default();
        serde_json::json!({
            "running": stream.running.load(Ordering::Relaxed),
            "supported": true,
            "fps": cfg.fps,
            "quality": cfg.quality,
            "frames": stream.frames.load(Ordering::Relaxed),
            "mode": inf.mode,
            "frame_w": inf.fw,
            "frame_h": inf.fh,
            "box_w": inf.box_w,
            "box_h": inf.box_h,
        })
    }
}

/// Latest encoded JPEG as base64, for the app's own preview <img>. The CEF
/// vehicle webviews never see this — they use the local:// disk snapshots.
#[tauri::command]
pub fn preview_frame_b64() -> Option<String> {
    #[cfg(not(windows))]
    {
        return None;
    }
    #[cfg(windows)]
    {
        let g = LATEST_JPEG.get().and_then(|c| c.lock().ok())?;
        if g.is_empty() {
            return None;
        }
        Some(base64_encode(&g))
    }
}

// ---------------------------------------------------------------------------
// Disk snapshots (local:// VFS poller — the main and only streaming method)
// ---------------------------------------------------------------------------

/// Latest encoded JPEG, kept in memory for the app's own preview command.
#[cfg(windows)]
static LATEST_JPEG: std::sync::OnceLock<Mutex<Vec<u8>>> = std::sync::OnceLock::new();

/// Disk snapshot paths for the local:// VFS poller — the MAIN streaming method
/// (the engine's CEF client blocks outbound http:// and holds ws:// forever
/// inside vehicle texture webviews, but it happily loads local:// files, so we
/// mirror every encoded frame to files the texture pages <img>-poll with a
/// cache-buster). Primary destination: the BBC_Universal_Screen mod. Secondary:
/// the bbcwstest mod. Override both with BBC_WS_TEST_FRAME_PATH.
#[cfg(windows)]
fn frame_file_paths() -> &'static Vec<String> {
    static P: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        if let Ok(p) = std::env::var("BBC_WS_TEST_FRAME_PATH") {
            return vec![p];
        }
        vec![
            r"C:\Users\BrokenBrainCells\AppData\Local\BeamNG\BeamNG.drive\current\mods\unpacked\bbc_stereo_0.6\vehicles\common\BBC_Universal_Screen\live\frame.jpg".into(),
            r"C:\Users\BrokenBrainCells\AppData\Local\BeamNG\BeamNG.drive\current\mods\unpacked\bbcwstest\vehicles\common\BBCWSTest\live\frame.jpg".into(),
        ]
    })
}

#[cfg(windows)]
fn set_latest_jpeg(jpeg: &[u8]) {
    let cell = LATEST_JPEG.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(mut g) = cell.lock() {
        g.clear();
        g.extend_from_slice(jpeg);
    }
    write_frame_file(jpeg);
}

/// Throttled (max ~60fps) atomic-ish disk write: tmp file + rename so CEF
/// never reads a half-written JPEG. Rename replaces existing on Windows.
#[cfg(windows)]
fn write_frame_file(jpeg: &[u8]) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST_MS: AtomicU64 = AtomicU64::new(0);
    static WRITE_N: AtomicU64 = AtomicU64::new(0);
    static ERR_N: AtomicU64 = AtomicU64::new(0);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let last = LAST_MS.load(Ordering::Relaxed);
    if now.saturating_sub(last) < 16 {
        return; // throttle to ~60fps to match the capture rate
    }
    if LAST_MS.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_err() {
        return;
    }

    let paths = frame_file_paths();
    let n = WRITE_N.fetch_add(1, Ordering::Relaxed);
    for path in paths.iter() {
        let target = std::path::Path::new(path);
        if let Some(parent) = target.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                let errs = ERR_N.fetch_add(1, Ordering::Relaxed);
                if errs == 0 || errs % 90 == 0 {
                    eprintln!("[opera-stream] frame snapshot dir create failed: {}", parent.display());
                }
                continue;
            }
        }
        let tmp = target.with_extension("tmp");
        match std::fs::write(&tmp, jpeg).and_then(|_| std::fs::rename(&tmp, path)) {
            Ok(_) => {
                if n == 0 {
                    eprintln!("[opera-stream] frame snapshots writing to {path}");
                }
            }
            Err(e) => {
                // Target may be briefly locked by CEF reading it — skip this frame.
                let errs = ERR_N.fetch_add(1, Ordering::Relaxed);
                if errs == 0 || errs % 90 == 0 {
                    eprintln!("[opera-stream] frame snapshot write failed ({errs}x): {e}");
                }
            }
        }
    }
}

/// Tiny base64 helper for the preview command (no external crate needed).
#[cfg(windows)]
fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

// ---------------------------------------------------------------------------
// Capture: WGC first, GDI fallback (child windows usually land on GDI)
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn capture_thread<R: Runtime>(
    app: AppHandle<R>,
    hwnd_raw: isize,
    parent_raw: isize,
    gen: u64,
    fps: u32,
    quality: u8,
    running: Arc<AtomicBool>,
    frames: Arc<AtomicU64>,
    gen_ref: Arc<AtomicU64>,
    info: Arc<Mutex<StreamInfo>>,
) {
    if run_wgc_parent_crop(
        &app, parent_raw, hwnd_raw, gen, fps, quality, &running, &frames,
        &gen_ref, &info,
    ) {
        return; // WGC ran (ended via stop/switch/close) — nothing more to do.
    }
    if running.load(Ordering::Relaxed) && gen_ref.load(Ordering::Relaxed) == gen {
        eprintln!("[opera-stream] WGC unavailable — PrintWindow(child) fallback");
        run_gdi_embedded(
            &app, hwnd_raw, parent_raw, gen, fps, quality, &running, &frames,
            &gen_ref, &info,
        );
    }
}

/// Fresh section target for this frame: parent + pending frontend rect.
/// Returns (parent_raw, x, y, w, h, scale, vw, vh) in CSS px.
#[cfg(windows)]
fn read_section_target<R: Runtime>(
    app: &AppHandle<R>,
) -> Option<(isize, i32, i32, i32, i32, f64, f64, f64)> {
    let state = app.state::<EmbedState>();
    let g = state.0.lock().ok()?;
    let p = g.parent?;
    let (x, y, w, h, s, vw, vh) = g.pending?;
    Some((p, x, y, w, h, s, vw, vh))
}



/// Steady preview height (px). Crops are scaled to this, aspect preserved, so
/// the stream view always shows the Opera box filling the frame — a 16:9 box
/// comes out exactly 960x540. 540p keeps every downstream stage (encode,
/// disk write, CEF decode) fast enough for high-fps streaming.
#[cfg(windows)]
const OUT_H: u32 = 540;

/// Primary path: WGC-capture the PARENT window, crop exactly the Opera child.
///
/// Both rectangles come straight from the OS every frame — the child's screen
/// rect via `GetWindowRect`, the frame origin via the parent's DWM extended
/// frame bounds — so no CSS coordinates, DPI factors, or titlebar guesswork
/// are involved. Output frames contain only the Opera window, scaled to
/// `OUT_W`, by construction.
///
/// Returns true when a session ran; false when WGC couldn't start (fall back).
#[cfg(windows)]
fn run_wgc_parent_crop<R: Runtime>(
    app: &AppHandle<R>,
    parent_raw: isize,
    child_raw: isize,
    gen: u64,
    fps: u32,
    quality: u8,
    running: &Arc<AtomicBool>,
    frames: &Arc<AtomicU64>,
    gen_ref: &Arc<AtomicU64>,
    info: &Arc<Mutex<StreamInfo>>,
) -> bool {
    // wgc depends on windows 0.62 while this crate uses 0.58 — transmute the
    // pointer (HWND is a transparent *mut c_void wrapper in both).
    let raw_ptr = parent_raw as *mut std::ffi::c_void;
    let hwnd = unsafe { std::mem::transmute::<*mut std::ffi::c_void, _>(raw_ptr) };

    let item = match wgc::new_item_from_hwnd(hwnd) {
        Ok(item) => item,
        Err(e) => {
            eprintln!("[opera-stream] WGC CreateForWindow(parent) failed: {e:?}");
            return false;
        }
    };

    let mut settings = wgc::WgcSettings::default();
    settings.frame_queue_length = 1; // no buffering: lowest latency
    if let Ok(true) = wgc::capabilities::is_cursor_configurable() {
        settings.capture_cursor = Some(false);
    }
    if let Ok(true) = wgc::capabilities::is_border_configurable() {
        settings.display_border = Some(false);
    }

    let wgc_session = match wgc::Wgc::new(item, settings) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("[opera-stream] WGC session failed: {e:?}");
            return false;
        }
    };
    eprintln!("[opera-stream] WGC parent capture running — cropping Opera box");

    let min_interval = std::time::Duration::from_millis((1000 / fps.max(1).min(240)) as u64);
    let mut last = std::time::Instant::now() - min_interval;
    let mut skips: u64 = 0;
    let mut last_placed: Option<(i32, i32, i32, i32)> = None;
    // Reused across frames: the two big RGBA buffers and the encode scratch
    // never realloc in steady state (only on the first frame / size change).
    let mut scale_buf: Vec<u8> = Vec::new();
    let mut enc_buf: Vec<u8> = Vec::new();
    // Change detection: identical frames skip scale+encode entirely.
    let mut last_key: u64 = 0;
    let mut last_rect: (u32, u32, u32, u32) = (0, 0, 0, 0);
    let mut last_jpeg: Option<Vec<u8>> = None;
    let mut last_out: (u32, u32) = (0, 0);
    // scale+encode timing (ms), windowed per 90-frame log line.
    let mut work_ms_sum: u128 = 0;
    let mut work_ms_max: u128 = 0;
    let mut work_frames: u32 = 0;

    for frame_res in wgc_session {
        if !running.load(Ordering::Relaxed) || gen_ref.load(Ordering::Relaxed) != gen {
            break;
        }
        // Keep the native child glued to its HTML section while scrolling
        // (placement only — the crop below is OS-measured).
        if let Some((_, x, y, w, h, s, _, _)) = read_section_target(app) {
            let s = s.clamp(1.0, 3.0);
            let px = (x as f64 * s) as i32;
            let py = (y as f64 * s) as i32;
            let pw = ((w as f64 * s) as i32).max(50);
            let ph = ((h as f64 * s) as i32).max(50);
            if last_placed != Some((px, py, pw, ph)) {
                crate::embed::place_child_raw(child_raw, px, py, pw, ph);
                last_placed = Some((px, py, pw, ph));
            }
        }
        let frame = match frame_res {
            Ok(f) => f,
            Err(_) => {
                std::thread::sleep(std::time::Duration::from_millis(33));
                continue;
            }
        };
        let size = match frame.size() {
            Ok(s) => s,
            Err(_) => continue,
        };
        if size.width == 0 || size.height == 0 || size.width > 16384 || size.height > 16384 {
            // Parent minimized (or bogus) — hold last frame, retry.
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
        }
        // Opera box in screen px (OS truth) minus the WGC frame origin in
        // screen px (OS truth) = crop origin in frame px. Intersected with
        // the frame so a scrolled/resizing window can never read OOB.
        let Some((bx0, by0, bw, bh)) = child_screen_rect(child_raw) else {
            skips += 1;
            if skips == 1 || skips % 90 == 0 {
                eprintln!("[opera-stream] embedded Opera window not measurable ({skips}x)");
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
        };
        let (fx0, fy0) = parent_frame_origin(parent_raw);
        let fw = size.width as i64;
        let fh = size.height as i64;
        let ix0 = (bx0 as i64 - fx0 as i64).max(0);
        let iy0 = (by0 as i64 - fy0 as i64).max(0);
        let ix1 = ((bx0 as i64 + bw as i64) - fx0 as i64).min(fw).max(ix0);
        let iy1 = ((by0 as i64 + bh as i64) - fy0 as i64).min(fh).max(iy0);
        let (cx, cy, cw, ch) = (ix0 as u32, iy0 as u32, (ix1 - ix0) as u32, (iy1 - iy0) as u32);
        if cw < 16 || ch < 16 {
            // Box fully off-frame (e.g. scrolled out) — hold last frame.
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
        }

        // fps gate FIRST: frame.pixels() allocates and copies the full
        // parent surface (GPU->CPU) on every call — paying that for frames
        // we're about to skip is pure bandwidth waste (huge at 144 Hz).
        if last.elapsed() < min_interval {
            continue;
        }
        last = std::time::Instant::now();

        let pixels = match frame.pixels() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if pixels.len() < size.width as usize * size.height as usize * 4 {
            continue;
        }

        // Cheap change detection: FNV-1a over sampled crop pixels plus the
        // crop rect. Identical frames skip scale + encode entirely (static
        // UI / loading screens become near-free).
        let key = sample_hash(&pixels, size.width, cx, cy, cw, ch);
        let rect = (cx, cy, cw, ch);
        if key != last_key || rect != last_rect || last_jpeg.is_none() {
            // Scale straight out of the parent frame at the crop origin —
            // no intermediate crop copy — into the reused buffer.
            let t0 = std::time::Instant::now();
            let Some((ow, oh)) = fit_height(
                &pixels, size.width, size.height, cx, cy, cw, ch, OUT_H, &mut scale_buf,
            ) else {
                continue;
            };
            let t1 = std::time::Instant::now();
            let Some(jpeg) = encode_jpeg(&scale_buf, ow, oh, quality, &mut enc_buf) else {
                continue;
            };
            let t2 = std::time::Instant::now();
            let (sm, em) = ((t1 - t0).as_millis(), (t2 - t1).as_millis());
            work_ms_max = work_ms_max.max(sm + em);
            work_ms_sum += sm + em;
            work_frames += 1;
            set_latest_jpeg(&jpeg);
            last_jpeg = Some(jpeg);
            last_out = (ow, oh);
            last_key = key;
            last_rect = rect;
        }
        let n = frames.fetch_add(1, Ordering::Relaxed) + 1;
        if n == 1 || n % 90 == 1 {
            let len = last_jpeg.as_ref().map_or(0, |j| j.len());
            let avg = if work_frames > 0 { work_ms_sum / work_frames as u128 } else { 0 };
            eprintln!(
                "[opera-stream] WGC crop frame {n}: frame={}x{} box_screen={bx0},{by0},{bw},{bh} crop={cx},{cy} {cw}x{ch} out={}.{} {len}B scale+enc avg={avg}ms max={work_ms_max}ms over {work_frames} frames",
                size.width, size.height, last_out.0, last_out.1,
            );
            work_ms_sum = 0;
            work_ms_max = 0;
            work_frames = 0;
            if let Ok(mut inf) = info.lock() {
                inf.mode = "wgc-crop".into();
                inf.fw = last_out.0;
                inf.fh = last_out.1;
                inf.box_w = cw;
                inf.box_h = ch;
            }
        }
    }
    true
}

/// FNV-1a over sampled pixels of the (ox, oy, sw, sh) sub-rect of a full
/// frame. One u32 per (6th px, 8th row) sample — ~2k reads for a 960x540
/// crop — enough to catch any UI change while costing ~nothing per frame.
#[cfg(windows)]
fn sample_hash(src: &[u8], fw: u32, ox: u32, oy: u32, sw: u32, sh: u32) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let stride = fw as usize * 4;
    let mut y = 0;
    while y < sh as usize {
        let row = (oy as usize + y) * stride + ox as usize * 4;
        let mut x = 0;
        while x < sw as usize {
            let off = row + x * 4;
            if off + 3 < src.len() {
                let px = u32::from_le_bytes([
                    src[off],
                    src[off + 1],
                    src[off + 2],
                    src[off + 3],
                ]);
                h ^= px as u64;
                h = h.wrapping_mul(0x100_0000_01b3);
            }
            x += 6;
        }
        y += 8;
    }
    h
}

/// Opera child rect in screen px, straight from the OS. The child was
/// stripped of caption/borders at embed time, so window rect == content.
#[cfg(windows)]
fn child_screen_rect(child_raw: isize) -> Option<(i32, i32, u32, u32)> {
    unsafe {
        use winapi::shared::windef::RECT;
        use winapi::um::winuser::GetWindowRect;
        let hwnd = child_raw as usize as winapi::shared::windef::HWND;
        let mut r: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut r) == 0 {
            return None;
        }
        let w = (r.right - r.left).max(0);
        let h = (r.bottom - r.top).max(0);
        if w <= 0 || h <= 0 {
            return None;
        }
        Some((r.left, r.top, w as u32, h as u32))
    }
}

/// WGC frame origin in screen px: the parent's DWM extended frame bounds
/// (window sans drop shadow — what WGC captures), falling back to the plain
/// window rect if DWM is unreachable.
#[cfg(windows)]
fn parent_frame_origin(parent_raw: isize) -> (i32, i32) {
    unsafe {
        use winapi::shared::windef::RECT;
        use winapi::um::dwmapi::DwmGetWindowAttribute;
        use winapi::um::winuser::GetWindowRect;
        let hwnd = parent_raw as usize as winapi::shared::windef::HWND;
        let mut r: RECT = std::mem::zeroed();
        // DWMWA_EXTENDED_FRAME_BOUNDS = 9
        let hr = DwmGetWindowAttribute(
            hwnd,
            9,
            &mut r as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        );
        if hr == 0 {
            return (r.left, r.top);
        }
        if GetWindowRect(hwnd, &mut r) != 0 {
            return (r.left, r.top);
        }
        (0, 0)
    }
}

/// Scale the (ox, oy, sw, sh) sub-rect of `src` to `dst_h` tall preserving
/// aspect (bilinear), writing into the reused `out` buffer. Sampling the
/// sub-rect directly (instead of copying it out first) removes one full-size
/// allocation + copy per frame.
#[cfg(windows)]
fn fit_height(
    src: &[u8],
    fw: u32,
    fh: u32,
    ox: u32,
    oy: u32,
    sw: u32,
    sh: u32,
    dst_h: u32,
    out: &mut Vec<u8>,
) -> Option<(u32, u32)> {
    if fw == 0 || fh == 0 || sw == 0 || sh == 0 || dst_h == 0 {
        return None;
    }
    if ox.checked_add(sw)? > fw || oy.checked_add(sh)? > fh {
        return None;
    }
    if fw as u64 * fh as u64 * 4 > src.len() as u64 {
        return None;
    }
    let dw = ((sw as u64 * dst_h as u64) / sh as u64).clamp(1, 4096) as u32;
    let row_len = dw as usize * 4;
    out.resize(row_len * dst_h as usize, 0);
    let stride = fw as usize * 4;
    if sw == dw && sh == dst_h {
        // 1:1 — straight row copies.
        for dy in 0..sh as usize {
            let off = (oy as usize + dy) * stride + ox as usize * 4;
            out[dy * row_len..(dy + 1) * row_len].copy_from_slice(&src[off..off + row_len]);
        }
        return Some((dw, dst_h));
    }
    let x_step = (((sw as u64) << 16) / (dw as u64)).max(1);
    let y_step = (((sh as u64) << 16) / (dst_h as u64)).max(1);
    let (x_last, y_last) = (ox + sw - 1, oy + sh - 1);
    for dy in 0..dst_h {
        let syv = oy as u64 * 65536 + dy as u64 * y_step;
        let y0c = ((syv >> 16) as u32).min(y_last);
        let y1c = (y0c + 1).min(y_last);
        let fy = (syv & 0xFFFF) as u64;
        let ify = 65536 - fy;
        for dx in 0..dw {
            let sxv = ox as u64 * 65536 + dx as u64 * x_step;
            let x0c = ((sxv >> 16) as u32).min(x_last);
            let x1c = (x0c + 1).min(x_last);
            let fx = (sxv & 0xFFFF) as u64;
            let ifx = 65536 - fx;
            let o = (dy * dw + dx) as usize * 4;
            for c in 0..4 {
                let a = src[((y0c * fw + x0c) as usize) * 4 + c] as u64;
                let b = src[((y0c * fw + x1c) as usize) * 4 + c] as u64;
                let cc = src[((y1c * fw + x0c) as usize) * 4 + c] as u64;
                let d = src[((y1c * fw + x1c) as usize) * 4 + c] as u64;
                // Weights sum to 2^32, so >>32 normalizes; max ~255*2^32 fits u64.
                out[o + c] =
                    ((a * ifx * ify + b * fx * ify + cc * ifx * fy + d * fx * fy) >> 32) as u8;
            }
        }
    }
    Some((dw, dst_h))
}

#[cfg(windows)]
fn run_gdi_embedded<R: Runtime>(
    app: &AppHandle<R>,
    hwnd_raw: isize,
    _parent_raw: isize,
    gen: u64,
    fps: u32,
    quality: u8,
    running: &Arc<AtomicBool>,
    frames: &Arc<AtomicU64>,
    gen_ref: &Arc<AtomicU64>,
    info: &Arc<Mutex<StreamInfo>>,
) {
    use winapi::shared::windef::HWND;

    let hwnd = hwnd_raw as usize as HWND;
    let interval = std::time::Duration::from_millis((1000 / fps.max(1).min(240)) as u64);
    let mut n: u64 = 0;
    // Last child position we applied (physical px) — re-place only on change.
    let mut last_placed: Option<(i32, i32, i32, i32)> = None;
    // Reused buffers + change detection, same as the WGC path.
    let mut scale_buf: Vec<u8> = Vec::new();
    let mut enc_buf: Vec<u8> = Vec::new();
    let mut last_key: u64 = 0;
    let mut last_rect: (u32, u32) = (0, 0);
    let mut last_jpeg: Option<Vec<u8>> = None;
    let mut last_out: (u32, u32) = (0, 0);

    while running.load(Ordering::Relaxed) && gen_ref.load(Ordering::Relaxed) == gen {
        let t0 = std::time::Instant::now();

        // Keep the native child glued to its HTML section while scrolling.
        // Capture itself (below) is always the WINDOW, so scroll position
        // can never corrupt the preview.
        if let Some((_, x, y, w, h, s, _, _)) = read_section_target(app) {
            let s = s.clamp(1.0, 3.0);
            let px = (x as f64 * s) as i32;
            let py = (y as f64 * s) as i32;
            let pw = ((w as f64 * s) as i32).max(50);
            let ph = ((h as f64 * s) as i32).max(50);
            if last_placed != Some((px, py, pw, ph)) {
                crate::embed::place_child_raw(hwnd_raw, px, py, pw, ph);
                last_placed = Some((px, py, pw, ph));
            }
        }

        // Native window capture: PrintWindow renders the Opera HWND itself,
        // independent of page scroll or screen position — the preview always
        // shows exactly the Opera window, never scroll regions or black.
        match gdi_capture_child(hwnd) {
            Ok((buf, w, h)) => {
                let key = sample_hash(&buf, w, 0, 0, w, h);
                let rect = (w, h);
                if key != last_key || rect != last_rect || last_jpeg.is_none() {
                    let Some((ow, oh)) = fit_height(&buf, w, h, 0, 0, w, h, OUT_H, &mut scale_buf)
                    else {
                        continue;
                    };
                    let Some(jpeg) = encode_jpeg(&scale_buf, ow, oh, quality, &mut enc_buf) else {
                        continue;
                    };
                    set_latest_jpeg(&jpeg);
                    last_jpeg = Some(jpeg);
                    last_out = (ow, oh);
                    last_key = key;
                    last_rect = rect;
                }
                n += 1;
                frames.fetch_add(1, Ordering::Relaxed);
                if n == 1 || n % 90 == 0 {
                    let len = last_jpeg.as_ref().map_or(0, |j| j.len());
                    eprintln!(
                        "[opera-stream] GDI frame {n}: child {w}x{h} out {}/{} {len} bytes",
                        last_out.0, last_out.1
                    );
                    if n == 1 {
                        if let Ok(mut inf) = info.lock() {
                            inf.mode = "printwindow".into();
                            inf.fw = last_out.0;
                            inf.fh = last_out.1;
                            inf.box_w = w;
                            inf.box_h = h;
                        }
                    }
                }
            }
            Err(e) => {
                if n == 0 || n % 90 == 0 {
                    eprintln!("[opera-stream] GDI capture: {e}");
                }
            }
        }
        let dt = t0.elapsed();
        if dt < interval {
            std::thread::sleep(interval - dt);
        }
    }
}

/// Fallback used only when no section rect is known yet: captures the embedded
/// Opera child itself via its own client rect + ClientToScreen origin — never
/// the DWM extended frame bounds (those describe the top-level parent for a
/// WS_CHILD and were the reason whole-app areas leaked into the stream).
/// PrintWindow first (occlusion-proof), screen-BitBlt of exactly the child
/// region as fallback.
#[cfg(windows)]
fn gdi_capture_child(hwnd: winapi::shared::windef::HWND) -> Result<(Vec<u8>, u32, u32), String> {
    unsafe {
        use winapi::shared::windef::{POINT, RECT};
        use winapi::um::wingdi::{
            BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits,
            SelectObject, CAPTUREBLT, SRCCOPY,
        };
        use winapi::um::wingdi::{BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS};
        use winapi::um::winuser::{
            ClientToScreen, GetClientRect, GetDC, IsWindow, PrintWindow, ReleaseDC, ShowWindow,
        };

        if IsWindow(hwnd) == 0 {
            return Err("embedded window is gone".into());
        }
        // Keep it renderable: a minimized window has no surface at all.
        // (Embedded child is rarely iconic itself, but the parent can be.)
        const SW_SHOWNA: i32 = 8;
        ShowWindow(hwnd, SW_SHOWNA);

        // Opera-section-only bounds: the child's client area mapped to screen.
        // (We stripped its caption/borders at embed time, so client == window.)
        let (sx, sy, w, h) = {
            let mut cr: RECT = std::mem::zeroed();
            if GetClientRect(hwnd, &mut cr) == 0 {
                return Err("GetClientRect failed".into());
            }
            let w = (cr.right - cr.left).max(0) as u32;
            let h = (cr.bottom - cr.top).max(0) as u32;
            if w == 0 || h == 0 {
                return Err("opera section has no area (minimized?)".into());
            }
            if w > 16384 || h > 16384 {
                return Err(format!("bad capture size {w}x{h}"));
            }
            let mut pt = POINT { x: 0, y: 0 };
            if ClientToScreen(hwnd, &mut pt) == 0 {
                return Err("ClientToScreen failed".into());
            }
            (pt.x, pt.y, w, h)
        };

        let sdc = GetDC(std::ptr::null_mut());
        if sdc.is_null() {
            return Err("GetDC failed".into());
        }
        let mdc = CreateCompatibleDC(sdc);
        let hbmp = CreateCompatibleBitmap(sdc, w as i32, h as i32);
        let old = SelectObject(mdc, hbmp as *mut _);

        // PW_RENDERFULLCONTENT asks Windows to render the HWND into our DC
        // even when occluded by sibling content.
        const PW_RENDERFULLCONTENT: u32 = 0x00000002;
        if PrintWindow(hwnd, mdc, PW_RENDERFULLCONTENT) == 0 {
            // Fallback: copy exactly the child's screen region (composited pixels).
            BitBlt(
                mdc, 0, 0, w as i32, h as i32, sdc, sx, sy,
                SRCCOPY | CAPTUREBLT,
            );
        }

        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w as i32;
        bmi.bmiHeader.biHeight = -(h as i32);
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;

        let n = (w as usize) * (h as usize) * 4;
        let mut buf = vec![0u8; n];
        let lines = GetDIBits(
            mdc, hbmp, 0, h, buf.as_mut_ptr() as *mut _, &mut bmi, DIB_RGB_COLORS,
        );

        SelectObject(mdc, old);
        DeleteObject(hbmp as *mut _);
        DeleteDC(mdc);
        ReleaseDC(std::ptr::null_mut(), sdc);

        if lines == 0 {
            return Err("GetDIBits failed".into());
        }
        for c in buf.chunks_exact_mut(4) {
            c.swap(0, 2); // BGRA -> RGBA
        }
        Ok((buf, w, h))
    }
}

#[cfg(windows)]
/// Encode RGBA into JPEG. The (reused) `scratch` absorbs the encoder output
/// without reallocating in steady state; the returned Vec is a fresh,
/// right-sized copy that goes on the wire (one small alloc per frame).
#[cfg(windows)]
fn encode_jpeg(rgba: &[u8], w: u32, h: u32, quality: u8, scratch: &mut Vec<u8>) -> Option<Vec<u8>> {
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        return None;
    }
    if w as u64 * h as u64 * 4 > rgba.len() as u64 {
        return None;
    }
    if w > u16::MAX as u32 || h > u16::MAX as u32 {
        return None;
    }
    scratch.clear();
    let enc = jpeg_encoder::Encoder::new(&mut *scratch, quality);
    enc.encode(rgba, w as u16, h as u16, jpeg_encoder::ColorType::Rgba)
        .ok()?;
    Some(scratch.clone())
}
