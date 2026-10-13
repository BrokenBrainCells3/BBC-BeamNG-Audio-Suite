//! Scroll the embedded Opera via Chrome DevTools Protocol (CDP) instead of
//! Win32 messages.
//!
//! Background: Win32 `PostMessage` input injection is documented NOT to work
//! on Chromium windows — the top frame happens to forward posted clicks/keys,
//! but posted `WM_MOUSEWHEEL` is dropped while the window is unfocused, and no
//! hover/rounding/focus trickery changes that. CDP `Input.dispatchMouseEvent`
//! with `type: "mouseWheel"` goes straight into Blink's input pipeline (the
//! injector focuses the widget host itself), so it scrolls with no
//! foreground window changes, no pulse, no focus hacks.
//!
//! Requires Opera launched with `--remote-debugging-port=9333` (added to the
//! launch args in `embed.rs`). An embedded instance launched BEFORE that flag
//! existed has no DevTools endpoint: every call here fails fast and the wheel
//! arm falls back to the Win32 post path (previous behavior).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

pub const CDP_PORT: u16 = 9333;

pub fn debugging_arg() -> String {
    format!("--remote-debugging-port={CDP_PORT}")
}

static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
fn rt() -> &'static tokio::runtime::Runtime {
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("cdp runtime")
    })
}

static MSG_ID: AtomicU64 = AtomicU64::new(1);

/// Viewport calibration for the current DevTools target: browser-chrome
/// height (tab strip + toolbar, CSS px) and device pixel ratio. Cached per
/// target so the steady state is one websocket round trip per scroll flush.
#[derive(Clone)]
struct Calib {
    ws_url: String,
    chrome_px: f64,
    dpr: f64,
}
static CALIB: OnceLock<Mutex<Option<Calib>>> = OnceLock::new();
fn calib_lock() -> &'static Mutex<Option<Calib>> {
    CALIB.get_or_init(|| Mutex::new(None))
}

#[derive(Debug, serde::Deserialize)]
struct Target {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(rename = "webSocketDebuggerUrl", default)]
    ws: String,
}

/// All current page targets. Slow (~0.5s on a loaded Opera), so the worker
/// calls this only when the cache is empty/stale or delivery just failed —
/// never per scroll notch. Bounded end to end: a wedged endpoint must fail
/// fast, never hang the worker.
async fn fetch_page_targets() -> Result<Vec<String>, String> {
    let resp = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        reqwest::get(format!("http://127.0.0.1:{CDP_PORT}/json/list")),
    )
    .await
    .map_err(|_| "cdp list timeout".to_string())?
    .map_err(|e| format!("cdp list failed: {e}"))?;
    let body = tokio::time::timeout(std::time::Duration::from_secs(2), resp.text())
        .await
        .map_err(|_| "cdp list read timeout".to_string())?
        .map_err(|e| format!("cdp list read failed: {e}"))?;
    let targets: Vec<Target> =
        serde_json::from_str(&body).map_err(|e| format!("cdp list parse failed: {e}"))?;
    let urls: Vec<String> = targets
        .into_iter()
        .filter(|t| t.kind == "page" && !t.ws.is_empty())
        .map(|t| t.ws)
        .collect();
    if urls.is_empty() {
        Err("cdp: no page target (relaunch Opera from the app so it picks up --remote-debugging-port)".to_string())
    } else {
        Ok(urls)
    }
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Get-or-connect a persistent socket for a target (1.5s cap).
async fn ensure_conn(conns: &mut HashMap<String, WsStream>, url: &str) -> Result<(), String> {
    if conns.contains_key(url) {
        return Ok(());
    }
    let (sock, _) = tokio::time::timeout(
        std::time::Duration::from_millis(1500),
        tokio_tungstenite::connect_async(url),
    )
    .await
    .map_err(|_| "cdp ws connect timeout".to_string())?
    .map_err(|e| format!("cdp ws connect failed: {e}"))?;
    conns.insert(url.to_string(), sock);
    Ok(())
}

/// One CDP call over an existing socket (1.5s response cap).
async fn call_on_sock(
    sock: &mut WsStream,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    use futures::{SinkExt, StreamExt};
    let id = MSG_ID.fetch_add(1, Ordering::Relaxed);
    let msg = serde_json::json!({ "id": id, "method": method, "params": params });
    sock.send(tokio_tungstenite::tungstenite::Message::Text(
        msg.to_string().into(),
    ))
    .await
    .map_err(|e| format!("cdp send failed: {e}"))?;
    tokio::time::timeout(
        std::time::Duration::from_millis(1500),
        async {
            while let Some(m) = sock.next().await {
                let m = m.map_err(|e| format!("cdp recv failed: {e}"))?;
                let text = match m {
                    tokio_tungstenite::tungstenite::Message::Text(t) => t.to_string(),
                    tokio_tungstenite::tungstenite::Message::Close(_) => break,
                    _ => continue,
                };
                let v: serde_json::Value = serde_json::from_str(&text)
                    .map_err(|e| format!("cdp resp parse failed: {e}"))?;
                if v.get("id") == Some(&serde_json::json!(id)) {
                    if let Some(err) = v.get("error") {
                        return Err(format!("cdp {method} error: {err}"));
                    }
                    return Ok(v);
                }
            }
            Err("cdp: no response".to_string())
        },
    )
    .await
    .map_err(|_| "cdp: response timeout".to_string())?
}

async fn cdp_call(
    ws_url: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    use futures::{SinkExt, StreamExt};
    let (mut sock, _) = tokio_tungstenite::connect_async(ws_url)
        .await
        .map_err(|e| format!("cdp ws connect failed: {e}"))?;
    let id = MSG_ID.fetch_add(1, Ordering::Relaxed);
    let msg = serde_json::json!({ "id": id, "method": method, "params": params });
    sock.send(tokio_tungstenite::tungstenite::Message::Text(
        msg.to_string().into(),
    ))
    .await
    .map_err(|e| format!("cdp send failed: {e}"))?;
    let resp = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while let Some(m) = sock.next().await {
            let m = m.map_err(|e| format!("cdp recv failed: {e}"))?;
            let text = match m {
                tokio_tungstenite::tungstenite::Message::Text(t) => t.to_string(),
                tokio_tungstenite::tungstenite::Message::Close(_) => break,
                _ => continue,
            };
            let v: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| format!("cdp resp parse failed: {e}"))?;
            if v.get("id") == Some(&serde_json::json!(id)) {
                if let Some(err) = v.get("error") {
                    return Err(format!("cdp {method} error: {err}"));
                }
                return Ok(v);
            }
            // Notifications / other ids: ignore.
        }
        Err("cdp: no response".to_string())
    })
    .await
    .map_err(|_| "cdp: response timeout".to_string())??;
    let _ = sock.close(None).await;
    Ok(resp)
}

async fn calibration(ws_url: &str) -> Result<(f64, f64), String> {
    // Fast path: same target as last time.
    if let Ok(g) = calib_lock().lock() {
        if let Some(c) = g.as_ref() {
            if c.ws_url == ws_url {
                return Ok((c.chrome_px, c.dpr));
            }
        }
    }
    let v = cdp_call(
        ws_url,
        "Runtime.evaluate",
        serde_json::json!({
            "expression": "JSON.stringify({chrome: window.outerHeight - window.innerHeight, dpr: window.devicePixelRatio || 1})",
            "returnByValue": true,
        }),
    )
    .await?;
    let s = v
        .pointer("/result/result/value")
        .and_then(|x| x.as_str())
        .ok_or_else(|| "cdp: bad calibration shape".to_string())?;
    let cv: serde_json::Value =
        serde_json::from_str(s).map_err(|e| format!("cdp calib parse failed: {e}"))?;
    let chrome = cv
        .get("chrome")
        .and_then(|x| x.as_f64())
        .unwrap_or(0.0)
        .clamp(0.0, 400.0);
    let dpr = cv
        .get("dpr")
        .and_then(|x| x.as_f64())
        .filter(|x| x.is_finite() && *x > 0.0)
        .unwrap_or(1.0)
        .clamp(1.0, 4.0);
    if let Ok(mut g) = calib_lock().lock() {
        *g = Some(Calib {
            ws_url: ws_url.to_string(),
            chrome_px: chrome,
            dpr,
        });
    }
    Ok((chrome, dpr))
}

/// A wheel flush bound for the worker. Carries everything both transports
/// need so the UDP thread never blocks: normalized + physical coords for
/// CDP, plus the raw Win32 parameters for the fallback path.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WheelJob {
    pub nx: f64,
    pub ny: f64,
    pub cw: i32,
    pub ch: i32,
    pub stepped: i32,
    pub child_raw: isize,
    pub mask: usize,
    pub px: i32,
    pub py: i32,
    pub sx: i32,
    pub sy: i32,
}

// Delivery health counters, read by `input_status` + the watchdog summary.
pub(crate) static CDP_OK: AtomicU64 = AtomicU64::new(0);
pub(crate) static CDP_ERR: AtomicU64 = AtomicU64::new(0);
pub(crate) static CDP_FALLBACK: AtomicU64 = AtomicU64::new(0);
pub(crate) static QUEUE_DROPPED: AtomicU64 = AtomicU64::new(0);
static LAST_ERR: OnceLock<Mutex<String>> = OnceLock::new();
fn last_err_lock() -> &'static Mutex<String> {
    LAST_ERR.get_or_init(|| Mutex::new(String::new()))
}
pub(crate) fn last_err() -> String {
    last_err_lock().lock().map(|g| g.clone()).unwrap_or_default()
}
fn set_last_err(e: String) {
    if let Ok(mut g) = last_err_lock().lock() {
        *g = e;
    }
}

type JobTx = std::sync::mpsc::SyncSender<WheelJob>;
static TX: OnceLock<JobTx> = OnceLock::new();

/// Non-blocking enqueue from the UDP input thread. `Err` means the worker is
/// backed up: the notch is dropped (counted) rather than stalling input —
/// a dropped scroll notch beats a lagging browser.
pub(crate) fn queue_wheel(job: WheelJob) -> Result<(), ()> {
    match TX.get() {
        Some(tx) => tx.try_send(job).map_err(|_| {
            QUEUE_DROPPED.fetch_add(1, Ordering::Relaxed);
        }),
        None => Err(()),
    }
}

/// Start the delivery worker (idempotent).
///
/// Per flush: coalesce the backlog into one delivery (latest coords win),
/// fan the wheel out to EVERY open page tab (the visible one always gets
/// it; hidden tabs scroll harmlessly), over persistent per-target sockets.
/// Target list refreshes only when empty/stale/failing. Slow steps (~0.5s
/// list on a loaded Opera) never run per notch anymore. Win32 fallback only
/// when no target delivers. All off the input fast path.
pub fn start_worker(app: tauri::AppHandle) {
    if TX.get().is_some() {
        return;
    }
    let (tx, rx) = std::sync::mpsc::sync_channel::<WheelJob>(32);
    if TX.set(tx).is_err() {
        return;
    }
    std::thread::spawn(move || {
        use tauri::Emitter;
        eprintln!("[cdp] delivery worker started (persistent targets, fan-out)");
        let mut conns: HashMap<String, WsStream> = HashMap::new();
        let mut targets: Vec<String> = Vec::new();
        let mut last_list =
            std::time::Instant::now() - std::time::Duration::from_secs(3600);
        while let Ok(first) = rx.recv() {
            // Coalesce: one delivery per backlog so a burst can't go stale.
            let mut stepped = first.stepped;
            let mut latest = first;
            while let Ok(j) = rx.try_recv() {
                stepped += j.stepped;
                latest = j;
            }
            if stepped == 0 {
                continue;
            }
            // Refresh targets only when empty, stale (>10s), or failing.
            if targets.is_empty()
                || last_list.elapsed() > std::time::Duration::from_secs(10)
            {
                match rt().block_on(fetch_page_targets()) {
                    Ok(t) => {
                        targets = t;
                        last_list = std::time::Instant::now();
                        conns.retain(|u, _| targets.contains(u));
                    }
                    Err(e) => set_last_err(e),
                }
            }
            if targets.is_empty() {
                CDP_ERR.fetch_add(1, Ordering::Relaxed);
                let e = last_err();
                win32_fallback(&app, &latest, stepped, &e);
                continue;
            }
            // One calibration (same window for all its tabs); uncalibrated
            // still scrolls, just without the chrome strip on Y.
            let (chrome, dpr) = rt()
                .block_on(calibration(&targets[0]))
                .unwrap_or((0.0, 1.0));
            // Client px -> viewport CSS px (strip the browser chrome from Y).
            let x = (latest.nx * latest.cw as f64 / dpr).clamp(0.0, 16384.0);
            let y = (latest.ny * latest.ch as f64 / dpr - chrome)
                .max(0.0)
                .clamp(0.0, 16384.0);
            // Windows +120 = up; CDP +deltaY = down.
            let dy = -(stepped as f64);
            let params = serde_json::json!({
                "type": "mouseWheel",
                "x": x, "y": y,
                "deltaX": 0.0, "deltaY": dy,
            });
            let delivered = rt().block_on(async {
                let mut n = 0u32;
                for url in &targets {
                    let ok = async {
                        if ensure_conn(&mut conns, url).await.is_err() {
                            conns.remove(url);
                            return false;
                        }
                        match conns.get_mut(url) {
                            Some(sock) => {
                                match call_on_sock(
                                    sock,
                                    "Input.dispatchMouseEvent",
                                    params.clone(),
                                )
                                .await
                                {
                                    Ok(_) => true,
                                    Err(_) => {
                                        conns.remove(url);
                                        false
                                    }
                                }
                            }
                            None => false,
                        }
                    }
                    .await;
                    if ok {
                        n += 1;
                    }
                }
                n
            });
            if delivered > 0 {
                CDP_OK.fetch_add(1, Ordering::Relaxed);
                let msg =
                    format!("cdp wheel dy={dy} at {x:.0},{y:.0} x{delivered} targets");
                eprintln!("[MediaUDP] {msg}");
                let _ = app.emit("rust_log", format!("[MediaUDP] {msg}"));
            } else {
                CDP_ERR.fetch_add(1, Ordering::Relaxed);
                let e = "all page targets failed".to_string();
                set_last_err(e.clone());
                targets.clear(); // force a fresh list next flush
                win32_fallback(&app, &latest, stepped, &e);
            }
        }
    });
}

#[cfg(windows)]
fn win32_fallback(
    app: &tauri::AppHandle,
    latest: &WheelJob,
    stepped: i32,
    why: &str,
) {
    use tauri::Emitter;
    match crate::embed::post_wheel_fallback(
        latest.child_raw,
        latest.mask,
        latest.px,
        latest.py,
        latest.sx,
        latest.sy,
        stepped,
    ) {
        Ok(msg) => {
            CDP_FALLBACK.fetch_add(1, Ordering::Relaxed);
            let line = format!("{msg} (cdp unavailable: {why})");
            eprintln!("[MediaUDP] {line}");
            let _ = app.emit("rust_log", format!("[MediaUDP] {line}"));
        }
        Err(e2) => {
            let line = format!("wheel failed: cdp {why}, win32 {e2}");
            eprintln!("[MediaUDP] {line}");
            let _ = app.emit("rust_log", format!("[MediaUDP] {line}"));
        }
    }
}

#[cfg(not(windows))]
fn win32_fallback(app: &tauri::AppHandle, _latest: &WheelJob, _stepped: i32, why: &str) {
    use tauri::Emitter;
    let line = format!("wheel failed (cdp only on Windows): {why}");
    eprintln!("[MediaUDP] {line}");
    let _ = app.emit("rust_log", format!("[MediaUDP] {line}"));
}
