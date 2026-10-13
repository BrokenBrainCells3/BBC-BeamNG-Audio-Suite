// amp_state.rs
// Receiver for the amplifier/subwoofer/enclosure telemetry pushed by the
// BeamNG mod (AMP_SIMULATION.md). The mod sends one flat
// {"command": "amp_state", ...} JSON packet at ~10 Hz over two transports:
//
//   1. WebSocket (ws://<app>:5001, visualizer.rs) - the mod's subwooferWsGE
//      client sends amp_state as a text frame on the already-open connection.
//   2. UDP 5001 (beamng_udp.rs media command listener) - same JSON as a
//      datagram, same channel as the play_pause/volume/eq commands.
//
// handle_amp_state() validates, merges the packet into the state map (new
// fields from future mod versions are kept automatically), emits the
// "amp_state_updated" event to the frontend and exposes the state to Rust
// consumers (DSP) via the typed getters.

use serde_json::{Map, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

static AMP_STATE: OnceLock<Mutex<Map<String, Value>>> = OnceLock::new();
static PACKETS: AtomicU64 = AtomicU64::new(0);
static LAST_PACKET_MS: AtomicU64 = AtomicU64::new(0);
static FIRST_PACKET_LOGGED: AtomicBool = AtomicBool::new(false);

fn state_cell() -> &'static Mutex<Map<String, Value>> {
    AMP_STATE.get_or_init(|| Mutex::new(Map::new()))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Store one amp_state packet and notify the frontend. Safe to call from the
/// WebSocket read loop and the UDP listener thread. Returns true when the
/// packet was recognized and stored.
pub(crate) fn handle_amp_state(app: &AppHandle, text: &str) -> bool {
    let parsed: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let obj = match parsed.as_object() {
        Some(o) => o,
        None => return false,
    };
    if obj.get("command").and_then(|c| c.as_str()) != Some("amp_state") {
        return false;
    }

    {
        let mut st = state_cell().lock().unwrap_or_else(|e| e.into_inner());
        for (k, v) in obj {
            st.insert(k.clone(), v.clone());
        }
    }
    PACKETS.fetch_add(1, Ordering::Relaxed);
    LAST_PACKET_MS.store(now_ms(), Ordering::Relaxed);

    // One-time confirmation in the in-app log panel so a silent receive path
    // is visible; then a heartbeat every 300 packets (~30 s at 10 Hz).
    let n = PACKETS.load(Ordering::Relaxed);
    if !FIRST_PACKET_LOGGED.swap(true, Ordering::Relaxed) {
        let msg = format!("[AmpState] first amp_state packet received ({} fields)", obj.len());
        eprintln!("{}", msg);
        let _ = app.emit("rust_log", msg);
    } else if n % 300 == 0 {
        let _ = app.emit("rust_log", format!("[AmpState] {} packets received", n));
    }

    // Snapshot for the frontend event (~10 Hz from the mod, fine over IPC)
    let snapshot: Map<String, Value> =
        state_cell().lock().unwrap_or_else(|e| e.into_inner()).clone();
    let _ = app.emit("amp_state_updated", Value::Object(snapshot));
    true
}

// ---------------------------------------------------------------------------
// Typed getters for Rust consumers (DSP / power / thermal simulation)
// ---------------------------------------------------------------------------

pub(crate) fn get_f64(key: &str, default: f64) -> f64 {
    let st = state_cell().lock().unwrap_or_else(|e| e.into_inner());
    st.get(key).and_then(|v| v.as_f64()).unwrap_or(default)
}

pub(crate) fn get_bool(key: &str, default: bool) -> bool {
    let st = state_cell().lock().unwrap_or_else(|e| e.into_inner());
    st.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}

pub(crate) fn get_str(key: &str, default: &str) -> String {
    let st = state_cell().lock().unwrap_or_else(|e| e.into_inner());
    st.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or(default)
        .to_string()
}

/// True while amp_state packets are arriving (<3 s since the last one).
/// Audio hooks use this to decide whether the mod or the UI controls the
/// sub crossover / levels.
pub(crate) fn is_receiving() -> bool {
    let last = LAST_PACKET_MS.load(Ordering::Relaxed);
    last > 0 && now_ms().saturating_sub(last) < 3000
}

// ---------------------------------------------------------------------------
// App-side fallback power/thermal model
// ---------------------------------------------------------------------------
// The mod's ampSimGE computes amp/coil temps from the sub demand it receives
// back from this app (the electrics loop). When that loop is dead (WS push
// off, no subwoofer controller on the vehicle), demand arrives as 0 and the
// dynamic sim never develops. The app already knows the true sub demand - its
// own smoothed sub band - so we run the same model locally (same constants as
// ampSimGE) and let the audio hooks use whichever side reports the HOTTER
// state. Falls out of the way automatically when the mod's model is alive.

static FB_AMP_TEMP: AtomicU64 = AtomicU64::new(25.0f64.to_bits());
static FB_COIL_TEMP: AtomicU64 = AtomicU64::new(25.0f64.to_bits());

const FB_AMBIENT_C: f64 = 25.0;
const FB_HEAT_GAIN: f64 = 0.004;
const FB_COOL_K: f64 = 0.012;
const FB_COIL_HEAT_GAIN: f64 = 0.02;
const FB_COIL_COOL_K: f64 = 0.008;
const FB_PROTECT_C: f64 = 95.0;
const FB_CU_TEMP_COEF: f64 = 0.00393;

/// Called once per audio block from the capture callback with the app's own
/// sub band as demand (0..1), the mod-reported supply voltage and the block
/// duration in seconds.
pub(crate) fn tick_power_model(demand: f64, supply_volts: f64, dt: f64) {
    if dt <= 0.0 || dt > 0.5 {
        return;
    }
    let demand_s = demand.clamp(0.0, 1.0);
    let supply = supply_volts.clamp(8.0, 18.0);
    let rms = get_f64_eff("rms_power_w", 1500.0).max(50.0);
    let eff = get_f64_eff("efficiency", 0.8).clamp(0.1, 1.0);
    let subs = get_f64_eff("sub_count", 1.0).max(1.0);
    let rpm = get_f64_eff("rpm", 0.0).max(0.0);
    let airflow = 1.0 + rpm / 3000.0;

    // Voice coil construction: CCA wire handles less heat, aluminum formers
    // dissipate better than fiberglass/kapton.
    let wire = get_str_eff("vc_wire", "cu").to_lowercase();
    let former = get_str_eff("vc_former", "alu").to_lowercase();
    let coil_heat_mult = if wire.contains("cca") { 1.3 } else { 1.0 };
    let coil_cool_mult = if former.contains("alu") { 1.2 } else { 1.0 };

    let headroom = ((supply / 14.4) * (supply / 14.4)).clamp(0.25, 1.6);
    let p_out = rms * demand_s * headroom;

    // Amp thermal: lost power heats, airflow cools
    let loss = p_out * (1.0 / eff - 1.0);
    let amp = f64::from_bits(FB_AMP_TEMP.load(Ordering::Relaxed));
    let amp_next =
        (amp + (loss * FB_HEAT_GAIN - (amp - FB_AMBIENT_C) * FB_COOL_K * airflow) * dt)
            .max(FB_AMBIENT_C);
    FB_AMP_TEMP.store(amp_next.to_bits(), Ordering::Relaxed);

    // Voice coil thermal per sub
    let p_per_sub = p_out / subs;
    LAST_P_PER_SUB.store(p_per_sub.to_bits(), Ordering::Relaxed);
    let coil = f64::from_bits(FB_COIL_TEMP.load(Ordering::Relaxed));
    let coil_next = (coil
        + (p_per_sub * FB_COIL_HEAT_GAIN * coil_heat_mult
            - (coil - FB_AMBIENT_C) * FB_COIL_COOL_K * coil_cool_mult * airflow)
            * dt)
        .max(FB_AMBIENT_C);
    FB_COIL_TEMP.store(coil_next.to_bits(), Ordering::Relaxed);
}

/// Hotter of the mod's amp temp and the app-side fallback model.
pub(crate) fn get_effective_amp_temp() -> f64 {
    let fb = f64::from_bits(FB_AMP_TEMP.load(Ordering::Relaxed));
    if is_receiving() {
        get_f64("amp_temp_c", FB_AMBIENT_C).max(fb)
    } else {
        fb
    }
}

/// Hotter of the mod's coil temp and the app-side fallback model.
pub(crate) fn get_effective_coil_temp() -> f64 {
    let fb = f64::from_bits(FB_COIL_TEMP.load(Ordering::Relaxed));
    if is_receiving() {
        get_f64("coil_temp_c", FB_AMBIENT_C).max(fb)
    } else {
        fb
    }
}

/// Stronger (more negative) of the two power compression models.
pub(crate) fn get_power_compression_db() -> f64 {
    let packet = if is_receiving() {
        get_f64("power_compression_db", 0.0)
    } else {
        0.0
    };
    let coil = get_effective_coil_temp();
    let fb = -10.0 * (1.0 + FB_CU_TEMP_COEF * (coil - 20.0)).log10();
    packet.min(fb)
}

/// Thermal protect from either model.
pub(crate) fn get_thermal_protect() -> bool {
    get_bool("thermal_protect", false) || get_effective_amp_temp() >= FB_PROTECT_C
}

/// Last computed per-sub power from tick_power_model (W) - feeds the
/// Xmax/RMS overdrive distortion model.
static LAST_P_PER_SUB: AtomicU64 = AtomicU64::new(0.0f64.to_bits());
pub(crate) fn get_last_p_per_sub() -> f64 {
    f64::from_bits(LAST_P_PER_SUB.load(Ordering::Relaxed))
}

// ---------------------------------------------------------------------------
// Manual overrides (app sliders win over game packets, for testing)
// ---------------------------------------------------------------------------

static OVERRIDES: OnceLock<Mutex<Map<String, Value>>> = OnceLock::new();

fn overrides_cell() -> &'static Mutex<Map<String, Value>> {
    OVERRIDES.get_or_init(|| Mutex::new(Map::new()))
}

/// True when at least one slider/toggle override is set. Audio hooks treat
/// this like an active connection so tests work without the game running.
pub(crate) fn overrides_active() -> bool {
    !overrides_cell().lock().unwrap_or_else(|e| e.into_inner()).is_empty()
}

fn override_get(key: &str) -> Option<Value> {
    overrides_cell().lock().unwrap_or_else(|e| e.into_inner()).get(key).cloned()
}

pub(crate) fn get_f64_eff(key: &str, default: f64) -> f64 {
    if let Some(v) = override_get(key) {
        if let Some(n) = v.as_f64() {
            return n;
        }
    }
    get_f64(key, default)
}

pub(crate) fn get_bool_eff(key: &str, default: bool) -> bool {
    if let Some(v) = override_get(key) {
        if let Some(b) = v.as_bool() {
            return b;
        }
    }
    get_bool(key, default)
}

pub(crate) fn get_str_eff(key: &str, default: &str) -> String {
    if let Some(v) = override_get(key) {
        if let Some(s) = v.as_str() {
            return s.to_string();
        }
    }
    get_str(key, default)
}

#[tauri::command]
pub fn set_amp_override(key: String, value: serde_json::Value) -> bool {
    let mut ov = overrides_cell().lock().unwrap_or_else(|e| e.into_inner());
    let was_null = value.is_null();
    let value_str = serde_json::to_string(&value).unwrap_or_default();
    if was_null {
        ov.remove(&key);
    } else {
        ov.insert(key.clone(), value);
    }
    eprintln!("[AmpOverride] {} -> {} ({} active)", key, value_str, ov.len());
    true
}

#[tauri::command]
pub fn clear_amp_override(key: String) -> bool {
    overrides_cell().lock().unwrap_or_else(|e| e.into_inner()).remove(&key).is_some()
}

#[tauri::command]
pub fn clear_amp_overrides() -> bool {
    let n = overrides_cell().lock().unwrap_or_else(|e| e.into_inner()).len();
    overrides_cell().lock().unwrap_or_else(|e| e.into_inner()).clear();
    eprintln!("[AmpOverride] cleared {} overrides", n);
    true
}

#[tauri::command]
pub fn get_amp_overrides() -> Value {
    let ov = overrides_cell().lock().unwrap_or_else(|e| e.into_inner());
    Value::Object(ov.clone())
}

// ---------------------------------------------------------------------------
// Tauri commands (frontend)
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_amp_state() -> Value {
    let st = state_cell().lock().unwrap_or_else(|e| e.into_inner());
    Value::Object(st.clone())
}

#[tauri::command]
pub fn get_amp_state_status() -> Value {
    let packets = PACKETS.load(Ordering::Relaxed);
    let last = LAST_PACKET_MS.load(Ordering::Relaxed);
    let age_ms = if last > 0 {
        now_ms().saturating_sub(last)
    } else {
        u64::MAX
    };
    serde_json::json!({
        "packets": packets,
        "last_packet_age_ms": age_ms,
        "receiving": age_ms < 3000,
        "keys": state_cell().lock().unwrap_or_else(|e| e.into_inner()).len(),
    })
}