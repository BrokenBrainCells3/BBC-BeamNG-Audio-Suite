/**
 * Session Stats Module - Track and persist fun session statistics
 * Handles telemetry-based stats and session logging
 */

use serde::{Deserialize, Serialize};
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};
use chrono::{DateTime, Local};

// ============================================
// Data Structures
// ============================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetrySnapshot {
    pub position_x: f32,
    pub position_y: f32,
    pub position_z: f32,
    pub speed: f32,
    pub rpm: f32,
    pub gear: i32,
    pub timestamp: u64,

    // Performance data
    pub turbo: f32,
    pub engine_temp: f32,
    pub fuel: f32,
    pub oil_temp: f32,
    pub throttle: f32,
    pub brake: f32,
    pub clutch: f32,

    // Speaker positions (24 floats: 8 speakers × 3 axes)
    pub speaker_positions: [f32; 24],
    pub speaker_valid_flags: u32,

    // Light states
    pub dash_lights: u32,
    pub show_lights: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashEvent {
    pub timestamp: u64,
    pub speed_at_impact: f32,
    pub speed_drop: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GearChangeEvent {
    pub timestamp: u64,
    pub from_gear: i32,
    pub to_gear: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStats {
    // Telemetry-based stats
    pub distance_traveled_km: f64,
    pub top_speed_this_session: f32,
    pub top_speed_timestamp: Option<u64>,
    pub highest_rpm_this_session: f32,
    pub highest_rpm_timestamp: Option<u64>,
    pub time_spent_driving_seconds: u64,
    pub crashes_detected: Vec<CrashEvent>,
    pub time_at_high_rpm_seconds: u64,
    pub gear_changes: Vec<GearChangeEvent>,

    // Performance stats
    pub highest_turbo_this_session: f32,
    pub highest_engine_temp_this_session: f32,
    pub highest_oil_temp_this_session: f32,
    pub fuel_consumed_this_session: f32,
    pub time_wot_this_session: u64,
    pub hard_braking_events_this_session: u32,
    pub time_clutch_this_session: u64,
    
    // Light usage stats
    pub time_highbeam_this_session: u64,
    pub signal_activations_this_session: u32,
    pub handbrake_pulls_this_session: u32,
    pub abs_activations_this_session: u32,

    // App-based stats
    pub session_start_time: u64,

    // Runtime state (not persisted)
    #[serde(skip)]
    pub last_telemetry: Option<TelemetrySnapshot>,
    #[serde(skip)]
    pub high_rpm_start_time: Option<u64>,
    
    // Light state tracking
    #[serde(skip)]
    pub last_signal_state: u32,
    #[serde(skip)]
    pub last_handbrake_state: bool,
    #[serde(skip)]
    pub last_abs_state: bool,
    #[serde(skip)]
    pub last_highbeam_state: bool,

    // Crash detection - cumulative deceleration tracking
    #[serde(skip)]
    pub recent_deceleration: f32,  // Total speed loss in last ~1 second
    #[serde(skip)]
    pub decel_window_start: Option<u64>,  // When current decel window started
    #[serde(skip)]
    pub last_crash_time: Option<u64>,  // Timestamp of last detected crash
}

impl SessionStats {
    pub fn new() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Self {
            distance_traveled_km: 0.0,
            top_speed_this_session: 0.0,
            top_speed_timestamp: None,
            highest_rpm_this_session: 0.0,
            highest_rpm_timestamp: None,
            time_spent_driving_seconds: 0,
            crashes_detected: Vec::new(),
            time_at_high_rpm_seconds: 0,
            gear_changes: Vec::new(),
            // Performance stats
            highest_turbo_this_session: 0.0,
            highest_engine_temp_this_session: 0.0,
            highest_oil_temp_this_session: 0.0,
            fuel_consumed_this_session: 0.0,
            time_wot_this_session: 0,
            hard_braking_events_this_session: 0,
            time_clutch_this_session: 0,
            // Light stats
            time_highbeam_this_session: 0,
            signal_activations_this_session: 0,
            handbrake_pulls_this_session: 0,
            abs_activations_this_session: 0,
            // App stats
            session_start_time: now,
            // Runtime state
            last_telemetry: None,
            high_rpm_start_time: None,
            last_signal_state: 0,
            last_handbrake_state: false,
            last_abs_state: false,
            last_highbeam_state: false,
            recent_deceleration: 0.0,
            decel_window_start: None,
            last_crash_time: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LifetimeStats {
    // Telemetry-based lifetime stats
    pub total_distance_km: f64,
    pub all_time_top_speed: f32,
    pub all_time_top_speed_timestamp: Option<u64>,
    pub all_time_highest_rpm: f32,
    pub all_time_highest_rpm_timestamp: Option<u64>,
    pub total_drive_time_seconds: u64,
    pub total_crashes: u32,
    pub total_gear_changes: u32,
    pub total_time_at_high_rpm_seconds: u64,

    // Performance lifetime stats
    pub highest_turbo_pressure: f32,
    pub highest_engine_temp: f32,
    pub highest_oil_temp: f32,
    pub total_fuel_consumed: f32,
    pub time_wot_seconds: u64,  // Wide Open Throttle (>90%)
    pub hard_braking_events: u32,  // Brake >80%
    pub time_clutch_depressed: u64,  // Clutch >50%
    
    // Light usage stats
    pub time_highbeam_seconds: u64,
    pub signal_activations: u32,
    pub handbrake_pulls: u32,
    pub abs_activations: u32,
    
    // Speaker tracking stats
    pub total_speaker_adjustments: u32,
    pub avg_speaker_distance: f32,

    // App-based lifetime stats
    pub total_app_time_seconds: u64,
}

impl LifetimeStats {
    pub fn new() -> Self {
        Self::default()
    }
    
    pub fn load_from_file(path: &PathBuf) -> Self {
        if path.exists() {
            if let Ok(content) = fs::read_to_string(path) {
                if let Ok(stats) = serde_json::from_str(&content) {
                    return stats;
                }
            }
        }
        Self::new()
    }
    
    pub fn save_to_file(&self, path: &PathBuf) -> Result<(), String> {
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize lifetime stats: {}", e))?;
        fs::write(path, content)
            .map_err(|e| format!("Failed to write lifetime stats file: {}", e))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_start: u64,
    pub session_end: u64,
    pub total_distance_km: f64,
    pub drive_time_seconds: u64,
    pub crashes: u32,
    pub top_speed: f32,
    pub app_time_added_seconds: u64,
}

// ============================================
// Session Stats State
// ============================================

pub struct SessionStatsState {
    pub current_session: Arc<RwLock<SessionStats>>,
    pub baseline_lifetime_stats: Arc<RwLock<LifetimeStats>>, // Snapshot at session start
    pub stats_dir: PathBuf,
    pub log_file: Option<PathBuf>,
    pub redline_rpm: u32,
    pub app_handle: Arc<RwLock<Option<AppHandle>>>,
}

impl SessionStatsState {
    pub fn new(app_handle: &AppHandle) -> Self {
        let app_data = app_handle
            .path()
            .app_data_dir()
            .unwrap_or_else(|_| PathBuf::from("."));

        let stats_dir = app_data.join("brokenbrain_cache").join("session_stats");

        // Create directory if it doesn't exist
        let _ = fs::create_dir_all(&stats_dir);

        // Load lifetime stats (baseline from previous sessions)
        let lifetime_path = stats_dir.join("lifetime_stats.json");
        let lifetime_stats = LifetimeStats::load_from_file(&lifetime_path);

        // Clean up old log files (keep last 30)
        Self::cleanup_old_logs(&stats_dir);

        // Create new session log file
        let now = Local::now();
        let log_filename = format!("session_{}.log", now.format("%Y-%m-%d_%H-%M-%S"));
        let log_path = stats_dir.join(&log_filename);

        // Write session start entry
        let session = SessionStats::new();
        let start_time = session.session_start_time;
        let start_datetime = DateTime::from_timestamp(start_time as i64, 0)
            .unwrap_or_else(|| Local::now().into())
            .with_timezone(&Local);

        let start_entry = format!(
            "[{}] [SESSION_START] {}\n",
            start_datetime.format("%H:%M:%S"),
            serde_json::json!({
                "session_id": log_filename.replace("session_", "").replace(".log", ""),
                "start_time": start_time
            })
        );

        let _ = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .and_then(|mut f| f.write_all(start_entry.as_bytes()));

        // Store baseline lifetime stats (snapshot at session start)
        let baseline_lifetime_stats = Arc::new(RwLock::new(lifetime_stats));
        let state_app_handle = Arc::new(RwLock::new(Some(app_handle.clone())));

        Self {
            current_session: Arc::new(RwLock::new(session)),
            baseline_lifetime_stats,
            stats_dir,
            log_file: Some(log_path),
            redline_rpm: 7000, // Default redline
            app_handle: state_app_handle,
        }
    }
    
    fn cleanup_old_logs(stats_dir: &PathBuf) {
        if let Ok(entries) = fs::read_dir(stats_dir) {
            let mut log_files: Vec<_> = entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with("session_") && n.ends_with(".log"))
                        .unwrap_or(false)
                })
                .filter_map(|e| {
                    e.metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .map(|t| (e.path(), t))
                })
                .collect();
            
            // Sort by modification time, newest first
            log_files.sort_by(|a, b| b.1.cmp(&a.1));
            
            // Delete all but the 30 most recent
            for (path, _) in log_files.iter().skip(30) {
                let _ = fs::remove_file(path);
            }
        }
    }
    
    fn write_log_entry(&self, event_type: &str, data: serde_json::Value) {
        if let Some(log_path) = &self.log_file {
            let now = Local::now();
            let entry = format!(
                "[{}] [{}] {}\n",
                now.format("%H:%M:%S"),
                event_type,
                data
            );
            
            if let Ok(mut file) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_path)
            {
                let _ = file.write_all(entry.as_bytes());
            }
        }
    }
    
    fn write_session_end(&self, session: &SessionStats) {
        if let Some(log_path) = &self.log_file {
            let now = Local::now();
            let session_duration = now.timestamp() as u64 - session.session_start_time;
            
            let summary = SessionSummary {
                session_start: session.session_start_time,
                session_end: now.timestamp() as u64,
                total_distance_km: session.distance_traveled_km,
                drive_time_seconds: session.time_spent_driving_seconds,
                crashes: session.crashes_detected.len() as u32,
                top_speed: session.top_speed_this_session,
                app_time_added_seconds: session_duration,
            };
            
            let entry = format!(
                "[{}] [SESSION_END] {}\n",
                now.format("%H:%M:%S"),
                serde_json::to_string(&summary).unwrap_or_default()
            );
            
            if let Ok(mut file) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_path)
            {
                let _ = file.write_all(entry.as_bytes());
            }
        }
    }
}

// ============================================
// Tauri Commands
// ============================================

#[tauri::command]
pub fn get_session_stats(state: State<SessionStatsState>) -> Result<SessionStats, String> {
    let session = state.current_session.read().map_err(|e| e.to_string())?;
    eprintln!("[SessionStats] get_session_stats called - crashes: {}, distance: {:.2} km, top_speed: {:.1} km/h",
        session.crashes_detected.len(), session.distance_traveled_km, session.top_speed_this_session);
    Ok(session.clone())
}

#[tauri::command]
pub fn get_lifetime_stats(state: State<SessionStatsState>) -> Result<LifetimeStats, String> {
    let session = state.current_session.read().map_err(|e| e.to_string())?;
    let baseline = state.baseline_lifetime_stats.read().map_err(|e| e.to_string())?;
    
    // Calculate combined stats on-demand (baseline + current session)
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let session_duration = now - session.session_start_time;
    
    let mut combined = baseline.clone();
    combined.total_distance_km += session.distance_traveled_km;
    combined.total_drive_time_seconds += session.time_spent_driving_seconds;
    combined.total_crashes += session.crashes_detected.len() as u32;
    combined.total_gear_changes += session.gear_changes.len() as u32;
    combined.total_time_at_high_rpm_seconds += session.time_at_high_rpm_seconds;
    combined.total_app_time_seconds += session_duration;
    
    // Update top speed/RPM if session has new records
    if session.top_speed_this_session > combined.all_time_top_speed {
        combined.all_time_top_speed = session.top_speed_this_session;
        combined.all_time_top_speed_timestamp = session.top_speed_timestamp;
    }
    if session.highest_rpm_this_session > combined.all_time_highest_rpm {
        combined.all_time_highest_rpm = session.highest_rpm_this_session;
        combined.all_time_highest_rpm_timestamp = session.highest_rpm_timestamp;
    }
    
    // Update performance stats
    if session.highest_turbo_this_session > combined.highest_turbo_pressure {
        combined.highest_turbo_pressure = session.highest_turbo_this_session;
    }
    if session.highest_engine_temp_this_session > combined.highest_engine_temp {
        combined.highest_engine_temp = session.highest_engine_temp_this_session;
    }
    if session.highest_oil_temp_this_session > combined.highest_oil_temp {
        combined.highest_oil_temp = session.highest_oil_temp_this_session;
    }
    combined.total_fuel_consumed += session.fuel_consumed_this_session;
    combined.time_wot_seconds += session.time_wot_this_session;
    combined.hard_braking_events += session.hard_braking_events_this_session;
    combined.time_clutch_depressed += session.time_clutch_this_session;
    
    // Update light usage stats
    combined.time_highbeam_seconds += session.time_highbeam_this_session;
    combined.signal_activations += session.signal_activations_this_session;
    combined.handbrake_pulls += session.handbrake_pulls_this_session;
    combined.abs_activations += session.abs_activations_this_session;
    
    Ok(combined)
}

#[tauri::command]
pub fn reset_lifetime_stats(state: State<SessionStatsState>) -> Result<(), String> {
    let mut lifetime = state.baseline_lifetime_stats.write().map_err(|e| e.to_string())?;
    *lifetime = LifetimeStats::new();

    let lifetime_path = state.stats_dir.join("lifetime_stats.json");
    lifetime.save_to_file(&lifetime_path)
}

#[tauri::command]
pub fn get_session_log_files(state: State<SessionStatsState>) -> Result<Vec<String>, String> {
    let mut log_files: Vec<_> = fs::read_dir(&state.stats_dir)
        .map_err(|e| format!("Failed to read stats directory: {}", e))?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("session_") && n.ends_with(".log"))
                .unwrap_or(false)
        })
        .filter_map(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .map(String::from)
        })
        .collect();
    
    // Sort by name (which includes timestamp), newest first
    log_files.sort_by(|a, b| b.cmp(a));
    
    Ok(log_files)
}

#[tauri::command]
pub fn read_session_log(filename: String, state: State<SessionStatsState>) -> Result<String, String> {
    let log_path = state.stats_dir.join(&filename);
    
    if !log_path.exists() {
        return Err(format!("Log file not found: {}", filename));
    }
    
    fs::read_to_string(&log_path)
        .map_err(|e| format!("Failed to read log file: {}", e))
}

#[tauri::command]
pub fn export_session_log(filename: String, state: State<SessionStatsState>) -> Result<Vec<u8>, String> {
    let log_path = state.stats_dir.join(&filename);
    
    if !log_path.exists() {
        return Err(format!("Log file not found: {}", filename));
    }
    
    fs::read(&log_path)
        .map_err(|e| format!("Failed to read log file: {}", e))
}

// ============================================
// Telemetry Processing
// ============================================

pub fn process_telemetry(
    state: &SessionStatsState,
    telemetry: TelemetrySnapshot,
) {
    let mut session = match state.current_session.write() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[SessionStats] Failed to acquire session lock: {}", e);
            return;
        }
    };

    let now = telemetry.timestamp;
    let redline_rpm = state.redline_rpm;

    // Clone last telemetry to avoid borrow issues
    let last_telemetry = session.last_telemetry.clone();

    // Calculate distance traveled and other metrics based on last telemetry
    if let Some(last) = &last_telemetry {
        let dx = telemetry.position_x - last.position_x;
        let dy = telemetry.position_y - last.position_y;
        let dz = telemetry.position_z - last.position_z;
        let distance_m = ((dx * dx + dy * dy + dz * dz) as f64).sqrt();
        session.distance_traveled_km += distance_m / 1000.0;

        // Detect crashes using cumulative deceleration tracking
        // This catches crashes that happen over multiple telemetry ticks
        let speed_drop = last.speed - telemetry.speed;
        let speed_ratio = if last.speed > 0.1 { telemetry.speed / last.speed } else { 1.0 };
        
        // Track cumulative deceleration over a 0.5 second window
        let time_delta = now - last.timestamp;
        if time_delta <= 1 {  // Within ~1 second window (telemetry is ~10-60 Hz)
            session.recent_deceleration += speed_drop.max(0.0);  // Only add positive drops
            if session.decel_window_start.is_none() {
                session.decel_window_start = Some(last.timestamp);
            }
        } else {
            // Reset window if too much time passed
            session.recent_deceleration = speed_drop.max(0.0);
            session.decel_window_start = Some(now);
        }
        
        // Log all significant speed changes for debugging
        if speed_drop.abs() > 2.0 {
            if let Ok(app_handle_guard) = state.app_handle.read() {
                if let Some(ref app_handle) = *app_handle_guard {
                    let _ = app_handle.emit("rust_log", format!(
                        "📉 Speed: {:.1} → {:.1} km/h (drop: {:.1}, cumulative: {:.1})",
                        last.speed, telemetry.speed, speed_drop, session.recent_deceleration
                    ));
                }
            }
        }
        
        // Crash detection - multiple conditions:
        // 1. Single large drop (>10 km/h) with >50% speed loss
        // 2. Very large single drop (>25 km/h)
        // 3. High speed to near-stop (>40 km/h → <5 km/h)
        // 4. Complete stop from speed (>30 km/h → <1 km/h)
        // 5. CUMULATIVE: Lost >50 km/h AND came to near-stop (actual crash, not just braking)
        
        // Add 5-second cooldown after each crash to prevent multiple detections
        let time_since_last_crash = session.last_crash_time
            .map(|t| now.saturating_sub(t))
            .unwrap_or(10);  // Default to 10s if no previous crash
        
        let is_crash = time_since_last_crash >= 5 && (
            (speed_drop > 10.0 && speed_ratio < 0.5) ||
            (speed_drop > 25.0) ||
            (last.speed > 40.0 && telemetry.speed < 5.0) ||
            (last.speed > 30.0 && telemetry.speed < 1.0) ||
            (session.recent_deceleration > 50.0 && telemetry.speed < 10.0)  // Must be near-stop
        );
        
        if is_crash {
            let crash = CrashEvent {
                timestamp: now,
                speed_at_impact: last.speed,
                speed_drop,
            };
            session.crashes_detected.push(crash.clone());
            session.last_crash_time = Some(now);  // Set cooldown timer
            state.write_log_entry("CRASH", serde_json::json!({
                "speed_at_impact": last.speed,
                "speed_drop": speed_drop,
                "cumulative_decel": session.recent_deceleration,
                "timestamp": now
            }));
            
            // Emit crash notification to frontend
            if let Ok(app_handle_guard) = state.app_handle.read() {
                if let Some(ref app_handle) = *app_handle_guard {
                    let crash_count = session.crashes_detected.len();
                    let _ = app_handle.emit("rust_log", format!(
                        "💥 CRASH #{}! {:.1} → {:.1} km/h (cumulative loss: {:.1} km/h)",
                        crash_count, last.speed, telemetry.speed, session.recent_deceleration
                    ));
                }
            }
            
            // Reset deceleration tracking after crash
            session.recent_deceleration = 0.0;
            session.decel_window_start = None;
        }

        // Count gear changes
        if telemetry.gear != last.gear {
            let gear_change = GearChangeEvent {
                timestamp: now,
                from_gear: last.gear,
                to_gear: telemetry.gear,
            };
            session.gear_changes.push(gear_change);
        }

        // Track time spent driving (speed > 5 km/h)
        if last.speed > 5.0 {
            let delta_time = now - last.timestamp;
            session.time_spent_driving_seconds += delta_time;
        }

        // Track time at high RPM (> 80% of redline)
        let high_rpm_threshold = (redline_rpm as f32) * 0.8;
        if last.rpm > high_rpm_threshold {
            if session.high_rpm_start_time.is_none() {
                session.high_rpm_start_time = Some(last.timestamp);
            }
        } else {
            if let Some(start) = session.high_rpm_start_time {
                session.time_at_high_rpm_seconds += now - start;
                session.high_rpm_start_time = None;
            }
        }
    }

    // Track top speed
    if telemetry.speed > session.top_speed_this_session {
        session.top_speed_this_session = telemetry.speed;
        session.top_speed_timestamp = Some(now);
        state.write_log_entry("TOP_SPEED", serde_json::json!({
            "speed": telemetry.speed,
            "timestamp": now,
            "is_session_record": true
        }));
    }

    // Track highest RPM
    if telemetry.rpm > session.highest_rpm_this_session {
        session.highest_rpm_this_session = telemetry.rpm;
        session.highest_rpm_timestamp = Some(now);
        state.write_log_entry("HIGHEST_RPM", serde_json::json!({
            "rpm": telemetry.rpm,
            "timestamp": now,
            "is_session_record": true
        }));
    }

    // Track performance stats
    if telemetry.turbo > session.highest_turbo_this_session {
        session.highest_turbo_this_session = telemetry.turbo;
    }
    if telemetry.engine_temp > session.highest_engine_temp_this_session {
        session.highest_engine_temp_this_session = telemetry.engine_temp;
    }
    if telemetry.oil_temp > session.highest_oil_temp_this_session {
        session.highest_oil_temp_this_session = telemetry.oil_temp;
    }
    
    // Track fuel consumed (difference from last reading)
    if let Some(last) = &last_telemetry {
        if telemetry.fuel < last.fuel {
            session.fuel_consumed_this_session += last.fuel - telemetry.fuel;
        }
    }

    // Track light usage and pedal inputs
    if let Some(last) = &last_telemetry {
        let time_delta = now - last.timestamp;
        
        // Wide Open Throttle (>90%)
        if telemetry.throttle > 0.9 {
            session.time_wot_this_session += time_delta;
        }
        
        // Hard braking (>80%)
        if telemetry.brake > 0.8 {
            session.hard_braking_events_this_session += 1;
        }
        
        // Clutch depressed (>50%)
        if telemetry.clutch > 0.5 {
            session.time_clutch_this_session += time_delta;
        }
        
        // High beam tracking
        let highbeam_on = (telemetry.show_lights & 0x02) != 0;  // DL_FULLBEAM
        if highbeam_on {
            session.time_highbeam_this_session += time_delta;
        }
        if highbeam_on != session.last_highbeam_state && highbeam_on {
            // Just turned on highbeam
        }
        session.last_highbeam_state = highbeam_on;
        
        // Signal activations (count transitions)
        let signal_l = (telemetry.show_lights & 0x20) != 0;  // DL_SIGNAL_L
        let signal_r = (telemetry.show_lights & 0x40) != 0;  // DL_SIGNAL_R
        let last_signal_l = (session.last_signal_state & 0x01) != 0;
        let last_signal_r = (session.last_signal_state & 0x02) != 0;
        if (signal_l && !last_signal_l) || (signal_r && !last_signal_r) {
            session.signal_activations_this_session += 1;
        }
        session.last_signal_state = (if signal_l { 0x01 } else { 0 }) | (if signal_r { 0x02 } else { 0 });
        
        // Handbrake pulls
        let handbrake_on = (telemetry.show_lights & 0x04) != 0;  // DL_HANDBRAKE
        if handbrake_on && !session.last_handbrake_state {
            session.handbrake_pulls_this_session += 1;
        }
        session.last_handbrake_state = handbrake_on;
        
        // ABS activations
        let abs_on = (telemetry.show_lights & 0x400) != 0;  // DL_ABS
        if abs_on && !session.last_abs_state {
            session.abs_activations_this_session += 1;
        }
        session.last_abs_state = abs_on;
    }

    session.last_telemetry = Some(telemetry);
    
    // Emit stats update every 100 packets
    static STATS_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let pkt_num = STATS_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if pkt_num % 100 == 0 && pkt_num > 0 {
        if let Ok(app_handle_guard) = state.app_handle.read() {
            if let Some(ref app_handle) = *app_handle_guard {
                let _ = app_handle.emit("rust_log", format!(
                    "📊 Session Stats: crashes={}, distance={:.2} km, top_speed={:.1} km/h, rpm={:.0}",
                    session.crashes_detected.len(), session.distance_traveled_km,
                    session.top_speed_this_session, session.highest_rpm_this_session
                ));
            }
        }
    }
}

// ============================================
// Session Lifecycle
// ============================================

pub fn end_session(state: &SessionStatsState) {
    let session = match state.current_session.read() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[SessionStats] Failed to acquire session lock: {}", e);
            return;
        }
    };

    // Write session end log
    state.write_session_end(&session);

    // Finalize tracking
    drop(session);
    finalize_high_rpm_tracking(&state);

    // Get finalized session data
    let session = match state.current_session.read() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[SessionStats] Failed to acquire session lock after finalize: {}", e);
            return;
        }
    };

    // Calculate final lifetime stats (baseline + session)
    let baseline = match state.baseline_lifetime_stats.read() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[SessionStats] Failed to acquire baseline lock: {}", e);
            return;
        }
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let session_duration = now - session.session_start_time;

    let mut final_lifetime = baseline.clone();
    final_lifetime.total_distance_km += session.distance_traveled_km;
    final_lifetime.total_drive_time_seconds += session.time_spent_driving_seconds;
    final_lifetime.total_crashes += session.crashes_detected.len() as u32;
    final_lifetime.total_gear_changes += session.gear_changes.len() as u32;
    final_lifetime.total_time_at_high_rpm_seconds += session.time_at_high_rpm_seconds;
    final_lifetime.total_app_time_seconds += session_duration;

    if session.top_speed_this_session > final_lifetime.all_time_top_speed {
        final_lifetime.all_time_top_speed = session.top_speed_this_session;
        final_lifetime.all_time_top_speed_timestamp = session.top_speed_timestamp;
    }
    if session.highest_rpm_this_session > final_lifetime.all_time_highest_rpm {
        final_lifetime.all_time_highest_rpm = session.highest_rpm_this_session;
        final_lifetime.all_time_highest_rpm_timestamp = session.highest_rpm_timestamp;
    }

    // Save final lifetime stats
    let lifetime_path = state.stats_dir.join("lifetime_stats.json");
    if let Err(e) = final_lifetime.save_to_file(&lifetime_path) {
        eprintln!("[SessionStats] Failed to save lifetime stats: {}", e);
    }

    drop(session);
    drop(baseline);
    drop(final_lifetime);

    // Clear current session for next launch
    if let Ok(mut new_session) = state.current_session.write() {
        *new_session = SessionStats::new();
    }
}

// ============================================
// Helper Functions
// ============================================

pub fn finalize_high_rpm_tracking(state: &SessionStatsState) {
    let mut session = match state.current_session.write() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[SessionStats] Failed to acquire session lock: {}", e);
            return;
        }
    };

    // If still at high RPM, count until now
    if let Some(start) = session.high_rpm_start_time {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        session.time_at_high_rpm_seconds += now - start;
        session.high_rpm_start_time = None;
    }
}

