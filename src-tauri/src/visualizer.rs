use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock, Mutex};
use std::thread;
use std::sync::mpsc;
use tauri::AppHandle;
use crate::media_info::MediaInfo;

// ============================================================================
// HTTP VISUALIZER SERVER - Shares FFT band data with BeamNG Lua script
// ============================================================================

/// Shared state for audio band values (updated by FFT callback, read by HTTP server)
pub(crate) struct VisualizerBands {
    sub: AtomicU32,    // f32 bits
    bass: AtomicU32,   // f32 bits
    mid: AtomicU32,    // f32 bits
    treble: AtomicU32, // f32 bits
    // Smoothing state (updated by audio thread, read by HTTP)
    smoothed_sub: AtomicU32,
    smoothed_bass: AtomicU32,
    smoothed_mid: AtomicU32,
    smoothed_treble: AtomicU32,
    // Media info (updated by media thread, read by HTTP)
    media_info: Mutex<MediaInfo>,
}

impl VisualizerBands {
    fn new() -> Self {
        Self {
            sub: AtomicU32::new(0f32.to_bits()),
            bass: AtomicU32::new(0f32.to_bits()),
            mid: AtomicU32::new(0f32.to_bits()),
            treble: AtomicU32::new(0f32.to_bits()),
            smoothed_sub: AtomicU32::new(0f32.to_bits()),
            smoothed_bass: AtomicU32::new(0f32.to_bits()),
            smoothed_mid: AtomicU32::new(0f32.to_bits()),
            smoothed_treble: AtomicU32::new(0f32.to_bits()),
            media_info: Mutex::new(MediaInfo::default()),
        }
    }

    /// Set new target values with exponential smoothing
    /// smoothing: 0.0 = no change, 1.0 = instant snap (use 0.3 for smooth but responsive)
    pub fn set(&self, sub: f32, bass: f32, mid: f32, treble: f32, smoothing: f32) {
        // Store raw values
        self.sub.store(sub.to_bits(), Ordering::Relaxed);
        self.bass.store(bass.to_bits(), Ordering::Relaxed);
        self.mid.store(mid.to_bits(), Ordering::Relaxed);
        self.treble.store(treble.to_bits(), Ordering::Relaxed);
        
        // Apply exponential smoothing for HTTP output
        let current_smoothed_sub = f32::from_bits(self.smoothed_sub.load(Ordering::Relaxed));
        let current_smoothed_bass = f32::from_bits(self.smoothed_bass.load(Ordering::Relaxed));
        let current_smoothed_mid = f32::from_bits(self.smoothed_mid.load(Ordering::Relaxed));
        let current_smoothed_treble = f32::from_bits(self.smoothed_treble.load(Ordering::Relaxed));
        
        let new_sub = current_smoothed_sub + (sub - current_smoothed_sub) * smoothing;
        let new_bass = current_smoothed_bass + (bass - current_smoothed_bass) * smoothing;
        let new_mid = current_smoothed_mid + (mid - current_smoothed_mid) * smoothing;
        let new_treble = current_smoothed_treble + (treble - current_smoothed_treble) * smoothing;

        self.smoothed_sub.store(new_sub.to_bits(), Ordering::Relaxed);
        self.smoothed_bass.store(new_bass.to_bits(), Ordering::Relaxed);
        self.smoothed_mid.store(new_mid.to_bits(), Ordering::Relaxed);
        self.smoothed_treble.store(new_treble.to_bits(), Ordering::Relaxed);
    }

    /// Get smoothed values for HTTP server
    fn get_smoothed(&self) -> (f32, f32, f32, f32) {
        (
            f32::from_bits(self.smoothed_sub.load(Ordering::Relaxed)),
            f32::from_bits(self.smoothed_bass.load(Ordering::Relaxed)),
            f32::from_bits(self.smoothed_mid.load(Ordering::Relaxed)),
            f32::from_bits(self.smoothed_treble.load(Ordering::Relaxed)),
        )
    }

    /// Smoothed sub band value (the "sub number" served at /values). Used by
    /// the audio processor to scale the rattle clip volume.
    pub(crate) fn get_sub(&self) -> f32 {
        f32::from_bits(self.smoothed_sub.load(Ordering::Relaxed))
    }

    /// Update media info from the media thread
    pub fn update_media_info(&self, info: MediaInfo) {
        if let Ok(mut media) = self.media_info.lock() {
            *media = info;
        }
    }

    /// Get current media info for HTTP server
    fn get_media_info(&self) -> MediaInfo {
        self.media_info.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

static VISUALIZER_BANDS: OnceLock<Arc<VisualizerBands>> = OnceLock::new();

pub(crate) fn get_visualizer_bands() -> &'static Arc<VisualizerBands> {
    VISUALIZER_BANDS.get_or_init(|| Arc::new(VisualizerBands::new()))
}

// Media control commands sent from HTTP server
#[derive(Debug, Clone)]
pub enum MediaCommand {
    PlayPause,
    Next,
    Previous,
}

/// Channel for sending media commands from HTTP server to main thread
static MEDIA_COMMAND_TX: OnceLock<mpsc::Sender<MediaCommand>> = OnceLock::new();
static MEDIA_COMMAND_RX: OnceLock<Mutex<mpsc::Receiver<MediaCommand>>> = OnceLock::new();

fn init_media_command_channel() {
    let (tx, rx) = mpsc::channel();
    let _ = MEDIA_COMMAND_TX.set(tx);
    let _ = MEDIA_COMMAND_RX.set(Mutex::new(rx));
}

pub(crate) fn get_media_command_sender() -> mpsc::Sender<MediaCommand> {
    MEDIA_COMMAND_TX.get_or_init(|| {
        init_media_command_channel();
        // This won't be reached since init sets it, but needed for type
        mpsc::channel().0
    }).clone()
}

pub(crate) fn try_recv_media_command() -> Option<MediaCommand> {
    MEDIA_COMMAND_RX.get()
        .and_then(|rx| rx.lock().ok())
        .and_then(|rx| rx.try_recv().ok())
}

/// Start HTTP server for BeamNG Lua script
pub(crate) fn start_visualizer_http_server() {
    use tiny_http::{Server, Response, Header, StatusCode};

    // Initialize the media command channel
    init_media_command_channel();

    let bands = get_visualizer_bands().clone();
    let media_tx = get_media_command_sender();
    
    thread::spawn(move || {
        let server = match Server::http("0.0.0.0:5000") {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[VisualizerHTTP] Failed to start server on port 5000: {}", e);
                return;
            }
        };
        
        eprintln!("[VisualizerHTTP] Server started on http://0.0.0.0:5000");
        eprintln!("[VisualizerHTTP] BeamNG Lua script should GET /values");
        
        for request in server.incoming_requests() {
            let url = request.url().to_string();
            
            if url == "/values" || url == "/" {
                // In WebSocket mode the values are pushed over WS instead
                if visualizer_is_ws_mode() {
                    let json = r#"{"mode":"websocket"}"#;
                    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
                    let cors_header = Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap();
                    let response = Response::from_string(json).with_header(header).with_header(cors_header);
                    let _ = request.respond(response);
                    continue;
                }

                // Get smoothed current band values
                let json = build_values_json(&bands);

                // Build response with CORS headers
                let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
                let cors_header = Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap();

                let response = Response::from_string(json)
                    .with_header(header)
                    .with_header(cors_header);

                let _ = request.respond(response);
            } else if url == "/health" {
                let json = r#"{"status":"ok"}"#;
                let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
                let cors_header = Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap();
                
                let response = Response::from_string(json)
                    .with_header(header)
                    .with_header(cors_header);
                
                let _ = request.respond(response);
            } else if url == "/media/play_pause" {
                let _ = media_tx.send(MediaCommand::PlayPause);
                let json = r#"{"status":"ok","command":"play_pause"}"#;
                let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
                let cors_header = Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap();
                let response = Response::from_string(json).with_header(header).with_header(cors_header);
                let _ = request.respond(response);
            } else if url == "/media/next" {
                let _ = media_tx.send(MediaCommand::Next);
                let json = r#"{"status":"ok","command":"next"}"#;
                let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
                let cors_header = Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap();
                let response = Response::from_string(json).with_header(header).with_header(cors_header);
                let _ = request.respond(response);
            } else if url == "/media/previous" {
                let _ = media_tx.send(MediaCommand::Previous);
                let json = r#"{"status":"ok","command":"previous"}"#;
                let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
                let cors_header = Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap();
                let response = Response::from_string(json).with_header(header).with_header(cors_header);
                let _ = request.respond(response);
            } else {
                // 404 for other paths
                let response = Response::from_string("Not Found")
                    .with_status_code(StatusCode(404));
                let _ = request.respond(response);
            }
        }
    });
}

/// Escape JSON string for safe embedding
fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

// ============================================================================
// TRANSPORT MODE - HTTP polling (default) or WebSocket push
// ============================================================================

/// WebSocket port for pushing visualizer values to the BeamNG Lua script.
/// Change this when the WebSocket port is finalized.
pub const VISUALIZER_WS_PORT: u16 = 5001;

/// true = values are pushed over WebSocket, false = served via HTTP polling
static WS_MODE: AtomicBool = AtomicBool::new(false);

pub(crate) fn visualizer_is_ws_mode() -> bool {
    WS_MODE.load(Ordering::Relaxed)
}

/// Toggle the visualizer transport between HTTP polling and WebSocket push.
/// Called from the frontend toggle.
#[tauri::command]
pub fn visualizer_set_ws_mode(enabled: bool) -> Result<(), String> {
    WS_MODE.store(enabled, Ordering::Relaxed);
    eprintln!(
        "[VisualizerTransport] {}",
        if enabled {
            format!("WebSocket push enabled on ws://0.0.0.0:{VISUALIZER_WS_PORT}")
        } else {
            "HTTP polling enabled on http://0.0.0.0:5000".to_string()
        }
    );
    Ok(())
}

/// Build the values JSON payload shared by the HTTP and WebSocket transports
fn build_values_json(bands: &VisualizerBands) -> String {
    let (sub, bass, mid, treble) = bands.get_smoothed();
    let media = bands.get_media_info();

    format!(
        "{{\"sub\":{},\"bass\":{},\"mid\":{},\"treble\":{},\"title\":\"{}\",\"artist\":\"{}\",\"album\":\"{}\",\"current_time\":{},\"total_time\":{},\"is_playing\":{},\"thumbnail\":{}}}",
        sub,
        bass,
        mid,
        treble,
        escape_json(&media.title),
        escape_json(&media.artist),
        escape_json(&media.album),
        media.current_time,
        media.total_time,
        media.is_playing,
        media.thumbnail.as_ref().map(|t| format!("\"{}\"", escape_json(t))).unwrap_or("null".to_string())
    )
}

/// Start the WebSocket visualizer server. Pushes the same JSON payload the
/// HTTP server serves at /values to every connected client at ~30 Hz while
/// WebSocket mode is active. Clients can send "play_pause", "next" or
/// "previous" text messages back to control media playback, or JSON
/// {"command": "amp_state", ...} telemetry packets which are routed to
/// amp_state::handle_amp_state.
pub(crate) fn start_visualizer_ws_server(app_handle: AppHandle) {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    /// Push rate for WebSocket value updates (Hz)
    const WS_PUSH_RATE_HZ: f64 = 288.0;

    let bands = get_visualizer_bands().clone();
    let media_tx = get_media_command_sender();

    thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("[VisualizerWS] Failed to create tokio runtime: {}", e);
                return;
            }
        };

        rt.block_on(async move {
            use tokio::sync::broadcast;

            let addr = format!("0.0.0.0:{VISUALIZER_WS_PORT}");
            let listener = match tokio::net::TcpListener::bind(&addr).await {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("[VisualizerWS] Failed to bind {}: {}", addr, e);
                    return;
                }
            };

            eprintln!("[VisualizerWS] Server listening on ws://{}", addr);
            eprintln!("[VisualizerWS] BeamNG Lua script should connect and read pushed values (send 'play_pause'/'next'/'previous' to control media, JSON amp_state packets are accepted)");

            // Broadcast channel: ticker publishes values, one receiver per client
            let (push_tx, _) = broadcast::channel::<String>(64);

            // Push ticker: ~30 fps while WebSocket mode is active
            {
                let bands = bands.clone();
                let push_tx = push_tx.clone();
                tokio::spawn(async move {
                        let mut interval = tokio::time::interval(std::time::Duration::from_secs_f64(
                            1.0 / WS_PUSH_RATE_HZ,
                        ));
                    loop {
                        interval.tick().await;
                        if !visualizer_is_ws_mode() {
                            continue;
                        }
                        let json = build_values_json(&bands);
                        let _ = push_tx.send(json);
                    }
                });
            }

            // Accept loop
            while let Ok((stream, _peer)) = listener.accept().await {
                let push_rx = push_tx.subscribe();
                let media_tx = media_tx.clone();
                let bands = bands.clone();
                let app_handle = app_handle.clone();

                tokio::spawn(async move {
                    let ws = match tokio_tungstenite::accept_async(stream).await {
                        Ok(ws) => ws,
                        Err(e) => {
                            eprintln!("[VisualizerWS] Handshake failed: {}", e);
                            return;
                        }
                    };
                    let (mut sink, mut source) = ws.split();
                    let mut push_rx = push_rx;

                    // Send an immediate snapshot on connect
                    let initial = build_values_json(&bands);
                    if sink.send(Message::Text(initial)).await.is_err() {
                        return;
                    }

                    // Forward pushed values to this client
                    let forward = tokio::spawn(async move {
                        loop {
                            match push_rx.recv().await {
                                Ok(json) => {
                                    if sink.send(Message::Text(json)).await.is_err() {
                                        break;
                                    }
                                }
                                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                                Err(broadcast::error::RecvError::Closed) => break,
                            }
                        }
                    });

                    // Read media commands + amp telemetry from the client
                    while let Some(msg) = source.next().await {
                        match msg {
                            Ok(Message::Text(text)) => {
                                handle_client_message(
                                    &app_handle, &media_tx, &text,
                                );
                            }
                            // BeamNG's BNGWSClient may send data frames as
                            // binary - decode and parse them the same way.
                            Ok(Message::Binary(bytes)) => {
                                if let Ok(text) = String::from_utf8(bytes) {
                                    handle_client_message(
                                        &app_handle, &media_tx, &text,
                                    );
                                }
                            }
                            Ok(Message::Close(_)) | Err(_) => break,
                            _ => {}
                        }
                    }

                    forward.abort();
                });
            }
        });
    });
}

/// Parse one inbound message from the game client: amp_state telemetry or
/// media control commands (JSON or legacy plain text).
fn handle_client_message(
    app_handle: &AppHandle,
    media_tx: &mpsc::Sender<MediaCommand>,
    text: &str,
) {
    let trimmed = text.trim();
    // JSON payloads from the game mod: amp_state telemetry or JSON-wrapped
    // media commands.
    if trimmed.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
            match v.get("command").and_then(|c| c.as_str()) {
                Some("amp_state") => {
                    let _ = crate::amp_state::handle_amp_state(app_handle, trimmed);
                }
                Some("play_pause") => {
                    let _ = media_tx.send(MediaCommand::PlayPause);
                }
                Some("next") => {
                    let _ = media_tx.send(MediaCommand::Next);
                }
                Some("previous") => {
                    let _ = media_tx.send(MediaCommand::Previous);
                }
                _ => {}
            }
        }
        return;
    }
    // Plain text control messages (legacy path)
    match trimmed {
        "play_pause" => {
            let _ = media_tx.send(MediaCommand::PlayPause);
        }
        "next" => {
            let _ = media_tx.send(MediaCommand::Next);
        }
        "previous" => {
            let _ = media_tx.send(MediaCommand::Previous);
        }
        _ => {}
    }
}
