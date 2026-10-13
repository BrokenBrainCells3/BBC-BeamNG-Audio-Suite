use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

use crate::electrics::{ElectricsState, ELECTRICS_UDP_ENABLED};
use crate::mcp::inject_input_mcp;
use crate::media_info;
use crate::spatial_audio::SpatialAudioState;
use crate::session_stats;

// Fix #4: Latest camera data cache for background polling
static LATEST_CAMERA_DATA: OnceLock<Arc<Mutex<Option<CameraData>>>> = OnceLock::new();

pub(crate) fn get_latest_camera_data() -> &'static Arc<Mutex<Option<CameraData>>> {
    LATEST_CAMERA_DATA.get_or_init(|| Arc::new(Mutex::new(None)))
}

// UDP Listener Status
pub(crate) static UDP_LISTENER_RUNNING: AtomicBool = AtomicBool::new(false);
pub(crate) static UDP_PACKETS_RECEIVED: AtomicU32 = AtomicU32::new(0);
pub(crate) static UDP_LAST_PACKET_SIZE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub(crate) static UDP_SIZE_MISMATCH: AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(crate) static UDP_EXPECTED_SIZE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(456);

// Input-path health counters (media channel :5001). Read by the
// `input_status` command and the watchdog's periodic summary.
pub(crate) static INPUT_PACKETS: AtomicU64 = AtomicU64::new(0);
pub(crate) static INPUT_UNKNOWN: AtomicU64 = AtomicU64::new(0);

// Fix #9: Added Clone derive for Emitter::emit() compatibility (Tauri API requires Serialize + Clone)
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct BeamNgTelemetry {
    #[serde(default)]
    pub speed: f32,
    #[serde(default)]
    pub rpm: f32,
    #[serde(default)]
    pub throttle: f32,
}

impl Default for BeamNgTelemetry {
    fn default() -> Self {
        Self { speed: 0.0, rpm: 0.0, throttle: 0.0 }
    }
}

// Fix #9: Added Clone derive for Emitter::emit() compatibility (Tauri API requires Serialize + Clone)
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CameraData {
    pub magic: String,
    pub time: u32,
    pub car: String,
    pub flags: u16,
    pub gear: i8,
    pub plid: i8,
    pub speed: f32,
    pub rpm: f32,
    pub turbo: f32,
    pub eng_temp: f32,
    pub fuel: f32,
    pub oil_pressure: f32,
    pub oil_temp: f32,
    pub dash_lights: u32,
    pub show_lights: u32,
    pub throttle: f32,
    pub brake: f32,
    pub clutch: f32,
    pub display1: String,
    pub display2: String,
    pub id: i32,
    pub cam_local_x: f32, pub cam_local_y: f32, pub cam_local_z: f32,
    pub cam_local_pitch: f32, pub cam_local_yaw: f32, pub cam_local_roll: f32,
    pub cam_local_quat_x: f32, pub cam_local_quat_y: f32, pub cam_local_quat_z: f32, pub cam_local_quat_w: f32,
    pub cam_local_forward_x: f32, pub cam_local_forward_y: f32, pub cam_local_forward_z: f32,
    pub cam_global_x: f32, pub cam_global_y: f32, pub cam_global_z: f32,
    pub cam_global_pitch: f32, pub cam_global_yaw: f32, pub cam_global_roll: f32,
    pub cam_global_quat_x: f32, pub cam_global_quat_y: f32, pub cam_global_quat_z: f32, pub cam_global_quat_w: f32,
    pub cam_global_forward_x: f32, pub cam_global_forward_y: f32, pub cam_global_forward_z: f32,
    pub veh_pos_x: f32, pub veh_pos_y: f32, pub veh_pos_z: f32,
    pub veh_quat_x: f32, pub veh_quat_y: f32, pub veh_quat_z: f32, pub veh_quat_w: f32,
    // Speaker Positions - Driver Side
    pub speaker_driver_main: [f32; 3],
    pub speaker_driver_rear: [f32; 3],
    pub speaker_driver_rear2: [f32; 3],
    pub speaker_driver_sub: [f32; 3],
    // Speaker Positions - Passenger Side
    pub speaker_passenger_main: [f32; 3],
    pub speaker_passenger_rear: [f32; 3],
    pub speaker_passenger_rear2: [f32; 3],
    pub speaker_passenger_sub: [f32; 3],
    // Speaker Positions - Optional Tweeters
    pub speaker_driver_twt: [f32; 3],
    pub speaker_passenger_twt: [f32; 3],
    // Speaker Validity Flags
    pub speaker_valid_flags: u32,
    // BBC Camera Mode (16-byte string from electrics.values.bbccamMode)
    pub bbccam_mode: String,
    // Door & Coupler Status (4 corners: FL, FR, RL, RR)
    pub door_fl_coupler_not_attached: f32,
    pub door_fr_coupler_not_attached: f32,
    pub door_rl_coupler_not_attached: f32,
    pub door_rr_coupler_not_attached: f32,
    pub coupler_attached_flags: u32,
    pub speaker_blocked_flags: u32,
}

pub(crate) fn start_beamng_camera_udp_listener(app_handle: AppHandle) {
    thread::spawn(move || {
        use socket2::{Domain, Protocol, Socket, Type};
        use std::mem::MaybeUninit;
        use std::net::SocketAddr;

        eprintln!("=== Starting BeamNG Camera UDP Listener ===");
        let _ = app_handle.emit("debug_log", "Starting BeamNG Camera UDP listener on port 4444");
        let _ = app_handle.emit("rust_log", "=== Starting BeamNG Camera UDP Listener on port 4444 ===");
        
        UDP_LISTENER_RUNNING.store(true, Ordering::Relaxed);

        let socket = match Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to create UDP socket: {}", e);
                let _ = app_handle.emit("rust_log", format!("Failed to create UDP socket: {}", e));
                UDP_LISTENER_RUNNING.store(false, Ordering::Relaxed);
                return;
            }
        };
        let _ = socket.set_reuse_address(true);
        let addr: SocketAddr = "0.0.0.0:4444".parse().unwrap();
        if let Err(e) = socket.bind(&addr.into()) {
            eprintln!("Failed to bind UDP socket: {}", e);
            let _ = app_handle.emit("rust_log", format!("Failed to bind UDP socket: {}", e));
            UDP_LISTENER_RUNNING.store(false, Ordering::Relaxed);
            return;
        }
        let _ = socket.set_read_timeout(Some(std::time::Duration::from_millis(100)));

        eprintln!("UDP camera listener bound to port 4444");
        let _ = app_handle.emit("rust_log", "UDP camera listener bound to port 4444");
        let mut buffer = vec![MaybeUninit::<u8>::uninit(); 600];
        let state_3d = app_handle.state::<SpatialAudioState>();

        loop {
            match socket.recv_from(&mut buffer) {
                Ok((size, _addr)) => {
                    UDP_PACKETS_RECEIVED.fetch_add(1, Ordering::Relaxed);
                    UDP_LAST_PACKET_SIZE.store(size, Ordering::Relaxed);
                    
                    let data: &[u8] = unsafe {
                        std::slice::from_raw_parts(buffer.as_ptr() as *const u8, size)
                    };

                    static UDP_PACKET_COUNT: AtomicU32 = AtomicU32::new(0);
                    let pkt_count = UDP_PACKET_COUNT.fetch_add(1, Ordering::Relaxed);

                    if data.len() != 332 && data.len() != 396 && data.len() != 412 && data.len() != 432 && data.len() != 436 && data.len() != 440 && data.len() != 464 {
                        static WRONG_SIZE_LOGGED: AtomicBool = AtomicBool::new(false);
                        if !WRONG_SIZE_LOGGED.load(Ordering::Relaxed) {
                            let msg = format!("UDP packet size mismatch: expected 332, 396, 412, 432, 436, 440, or 464 bytes, got {}. Check BeamNG mod version.", data.len());
                            eprintln!("{}", msg);
                            let _ = app_handle.emit("debug_log", &msg);
                            let _ = app_handle.emit("rust_log", &msg);
                            WRONG_SIZE_LOGGED.store(true, Ordering::Relaxed);
                            UDP_SIZE_MISMATCH.store(true, Ordering::Relaxed);
                        }
                        continue;
                    }
                    if &data[0..4] != b"BNGC" { continue; }

                    if pkt_count < 3 {
                        let msg = "Valid BNGC packet received (single precision f32)!";
                        eprintln!("{}", msg);
                        let _ = app_handle.emit("rust_log", msg);
                    }

                    macro_rules! f32_at {
                        ($i:expr) => {
                            f32::from_le_bytes([data[$i], data[$i+1], data[$i+2], data[$i+3]])
                        };
                    }
                    macro_rules! u32_at {
                        ($i:expr) => {
                            u32::from_le_bytes([data[$i], data[$i+1], data[$i+2], data[$i+3]])
                        };
                    }

                    let veh_pos_x = f32_at!(204); let veh_pos_y = f32_at!(208); let veh_pos_z = f32_at!(212);
                    let veh_quat_x = f32_at!(216); let veh_quat_y = f32_at!(220); let veh_quat_z = f32_at!(224); let veh_quat_w = f32_at!(228);
                    let cam_global_x = f32_at!(152); let cam_global_y = f32_at!(156); let cam_global_z = f32_at!(160);
                    let cam_global_forward_x = f32_at!(192); let cam_global_forward_y = f32_at!(196); let cam_global_forward_z = f32_at!(200);

                    let speaker_driver_main = [f32_at!(232), f32_at!(236), f32_at!(240)];
                    let speaker_driver_rear = [f32_at!(244), f32_at!(248), f32_at!(252)];
                    let speaker_driver_rear2 = [f32_at!(256), f32_at!(260), f32_at!(264)];
                    let speaker_driver_sub = [f32_at!(268), f32_at!(272), f32_at!(276)];
                    let speaker_passenger_main = [f32_at!(280), f32_at!(284), f32_at!(288)];
                    let speaker_passenger_rear = [f32_at!(292), f32_at!(296), f32_at!(300)];
                    let speaker_passenger_rear2 = [f32_at!(304), f32_at!(308), f32_at!(312)];
                    let speaker_passenger_sub = [f32_at!(316), f32_at!(320), f32_at!(324)];
                    let speaker_valid_flags = u32_at!(328);

                    let has_electrics = data.len() >= 396;
                    let has_bbccam_mode = data.len() >= 412;
                    let has_door_status = data.len() >= 432;
                    let electrics_enabled = ELECTRICS_UDP_ENABLED.load(Ordering::Relaxed);

                    let bbccam_mode_str = if has_bbccam_mode {
                        let bbccam_mode_bytes = &data[396..412];
                        String::from_utf8_lossy(bbccam_mode_bytes)
                            .trim_end_matches('\0')
                            .trim()
                            .to_string()
                    } else {
                        String::new()
                    };

                    let (door_fl, door_fr, door_rl, door_rr, coupler_flags) = if has_door_status {
                        (
                            f32_at!(412),
                            f32_at!(416),
                            f32_at!(420),
                            f32_at!(424),
                            u32_at!(428),
                        )
                    } else {
                        (0.0, 0.0, 0.0, 0.0, 0)
                    };

                    let has_blocked_flags = data.len() >= 436;
                    let speaker_blocked_flags = if has_blocked_flags {
                        u32_at!(432)
                    } else {
                        0
                    };

                    let has_static_blocked_flags = data.len() >= 440;
                    let speaker_static_blocked_flags = if has_static_blocked_flags {
                        u32_at!(436)
                    } else {
                        0
                    };

                    let has_tweaters = data.len() >= 464;
                    let speaker_driver_twt = if has_tweaters {
                        [f32_at!(440), f32_at!(444), f32_at!(448)]
                    } else {
                        [0.0f32, 0.0f32, 0.0f32]
                    };
                    let speaker_passenger_twt = if has_tweaters {
                        [f32_at!(452), f32_at!(456), f32_at!(460)]
                    } else {
                        [0.0f32, 0.0f32, 0.0f32]
                    };

                    if has_electrics && electrics_enabled {
                        let mut eq_band_values = [0.0f32; 15];
                        let volume_value: f32;

                        for i in 0..15 {
                            eq_band_values[i] = f32_at!(332 + i * 4);
                        }
                        volume_value = f32_at!(392);

                        if let Some(electrics_state) = app_handle.try_state::<ElectricsState>() {
                            for i in 0..15 {
                                electrics_state.update_eq_band(i, eq_band_values[i]);
                            }
                            electrics_state.update_volume(volume_value);

                            let bands = electrics_state.get_eq_bands();
                            let vol = electrics_state.get_volume();
                            let _ = app_handle.emit("electrics_values_updated", serde_json::json!({
                                "eq_bands": bands,
                                "volume": vol
                            }));
                        }
                    }

                    // Parse performance data for session stats
                    let turbo = f32_at!(24);
                    let eng_temp = f32_at!(28);
                    let fuel = f32_at!(32);
                    let oil_temp = f32_at!(40);
                    let throttle = f32_at!(52);
                    let brake = f32_at!(56);
                    let clutch = f32_at!(60);
                    let dash_lights = u32_at!(44);
                    let show_lights = u32_at!(48);

                    static TELEMETRY_COUNTER: AtomicU32 = AtomicU32::new(0);
                    let pkt_num = TELEMETRY_COUNTER.fetch_add(1, Ordering::Relaxed);
                    
                    if let Some(session_state) = app_handle.try_state::<session_stats::SessionStatsState>() {
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_secs();
                        let speed_ms = f32_at!(16);
                        let speed_kmh = speed_ms * 3.6;
                        let snapshot = session_stats::TelemetrySnapshot {
                            position_x: veh_pos_x,
                            position_y: veh_pos_y,
                            position_z: veh_pos_z,
                            speed: speed_kmh,
                            rpm: f32_at!(20),
                            gear: data[14] as i32,
                            timestamp: now,
                            turbo,
                            engine_temp: eng_temp,
                            fuel,
                            oil_temp,
                            throttle,
                            brake,
                            clutch,
                            speaker_positions: [
                                speaker_driver_main[0], speaker_driver_main[1], speaker_driver_main[2],
                                speaker_driver_rear[0], speaker_driver_rear[1], speaker_driver_rear[2],
                                speaker_driver_rear2[0], speaker_driver_rear2[1], speaker_driver_rear2[2],
                                speaker_driver_sub[0], speaker_driver_sub[1], speaker_driver_sub[2],
                                speaker_passenger_main[0], speaker_passenger_main[1], speaker_passenger_main[2],
                                speaker_passenger_rear[0], speaker_passenger_rear[1], speaker_passenger_rear[2],
                                speaker_passenger_rear2[0], speaker_passenger_rear2[1], speaker_passenger_rear2[2],
                                speaker_passenger_sub[0], speaker_passenger_sub[1], speaker_passenger_sub[2],
                            ],
                            speaker_valid_flags,
                            dash_lights,
                            show_lights,
                        };
                        session_stats::process_telemetry(&session_state, snapshot);

                        if pkt_num < 3 || pkt_num % 100 == 0 {
                            let _ = app_handle.emit("rust_log", format!(
                                "[SessionStats] Telemetry #{}: speed={:.1} km/h, rpm={:.0}, gear={}, turbo={:.2} bar, temp={}°C",
                                pkt_num, speed_kmh, f32_at!(20), data[14], turbo, eng_temp
                            ));
                        }
                    }

                    // Update legacy fields
                    {
                        let mut v = state_3d.vehicle.lock().unwrap_or_else(|e| e.into_inner());
                        v.pos = [veh_pos_x, veh_pos_y, veh_pos_z];
                        v.quat = [veh_quat_x, veh_quat_y, veh_quat_z, veh_quat_w];
                    }
                    {
                        let mut c = state_3d.camera.lock().unwrap_or_else(|e| e.into_inner());
                        c.pos = [cam_global_x, cam_global_y, cam_global_z];
                        c.forward = [cam_global_forward_x, cam_global_forward_y, cam_global_forward_z];
                    }
                    {
                        let mut udp = state_3d.udp_speaker_positions.lock().unwrap_or_else(|e| e.into_inner());
                        udp.driver_main = speaker_driver_main;
                        udp.driver_rear = speaker_driver_rear;
                        udp.driver_rear2 = speaker_driver_rear2;
                        udp.driver_sub = speaker_driver_sub;
                        udp.passenger_main = speaker_passenger_main;
                        udp.passenger_rear = speaker_passenger_rear;
                        udp.passenger_rear2 = speaker_passenger_rear2;
                        udp.passenger_sub = speaker_passenger_sub;
                        udp.valid_flags = speaker_valid_flags;
                        udp.blocked_flags = speaker_blocked_flags;
                        udp.static_blocked_flags = speaker_static_blocked_flags;
                        udp.driver_twt = speaker_driver_twt;
                        udp.passenger_twt = speaker_passenger_twt;
                    }

                    state_3d.update_door_states(door_fl, door_fr, door_rl, door_rr);
                    state_3d.update_bbccam_mode(bbccam_mode_str.clone());

                    if state_3d.is_udp_audio_enabled() {
                        state_3d.update_snapshot(
                            [cam_global_x, cam_global_y, cam_global_z],
                            [cam_global_forward_x, cam_global_forward_y, cam_global_forward_z],
                            [veh_pos_x, veh_pos_y, veh_pos_z],
                            [veh_quat_x, veh_quat_y, veh_quat_z, veh_quat_w],
                            [
                                [speaker_driver_main[0], speaker_driver_main[1], speaker_driver_main[2]],
                                [speaker_passenger_main[0], speaker_passenger_main[1], speaker_passenger_main[2]],
                                [speaker_driver_rear[0], speaker_driver_rear[1], speaker_driver_rear[2]],
                                [speaker_passenger_rear[0], speaker_passenger_rear[1], speaker_passenger_rear[2]],
                                [speaker_driver_sub[0], speaker_driver_sub[1], speaker_driver_sub[2]],
                                [speaker_passenger_sub[0], speaker_passenger_sub[1], speaker_passenger_sub[2]],
                            ]
                        );
                    } else {
                        let offsets = *state_3d.speaker_offsets.lock().unwrap_or_else(|e| e.into_inner());
                        state_3d.update_speaker_offsets(offsets);
                    }

                    let camera_data = CameraData {
                        magic: String::from_utf8_lossy(&data[0..4]).to_string(),
                        time: u32_at!(4),
                        car: String::from_utf8_lossy(&data[8..12]).trim_end_matches('\0').to_string(),
                        flags: u16::from_le_bytes([data[12], data[13]]),
                        gear: data[14] as i8, plid: data[15] as i8,
                        speed: f32_at!(16), rpm: f32_at!(20), turbo: f32_at!(24),
                        eng_temp: f32_at!(28), fuel: f32_at!(32), oil_pressure: f32_at!(36), oil_temp: f32_at!(40),
                        dash_lights: u32_at!(44), show_lights: u32_at!(48),
                        throttle: f32_at!(52), brake: f32_at!(56), clutch: f32_at!(60),
                        display1: String::from_utf8_lossy(&data[64..80]).trim_end_matches('\0').to_string(),
                        display2: String::from_utf8_lossy(&data[80..96]).trim_end_matches('\0').to_string(),
                        id: i32::from_le_bytes([data[96], data[97], data[98], data[99]]),
                        cam_local_x: f32_at!(100), cam_local_y: f32_at!(104), cam_local_z: f32_at!(108),
                        cam_local_pitch: f32_at!(112), cam_local_yaw: f32_at!(116), cam_local_roll: f32_at!(120),
                        cam_local_quat_x: f32_at!(124), cam_local_quat_y: f32_at!(128), cam_local_quat_z: f32_at!(132), cam_local_quat_w: f32_at!(136),
                        cam_local_forward_x: f32_at!(140), cam_local_forward_y: f32_at!(144), cam_local_forward_z: f32_at!(148),
                        cam_global_x, cam_global_y, cam_global_z,
                        cam_global_pitch: f32_at!(164), cam_global_yaw: f32_at!(168), cam_global_roll: f32_at!(172),
                        cam_global_quat_x: f32_at!(176), cam_global_quat_y: f32_at!(180), cam_global_quat_z: f32_at!(184), cam_global_quat_w: f32_at!(188),
                        cam_global_forward_x, cam_global_forward_y, cam_global_forward_z,
                        veh_pos_x, veh_pos_y, veh_pos_z, veh_quat_x, veh_quat_y, veh_quat_z, veh_quat_w,
                        speaker_driver_main,
                        speaker_driver_rear,
                        speaker_driver_rear2,
                        speaker_driver_sub,
                        speaker_passenger_main,
                        speaker_passenger_rear,
                        speaker_passenger_rear2,
                        speaker_passenger_sub,
                        speaker_driver_twt,
                        speaker_passenger_twt,
                        speaker_valid_flags,
                        bbccam_mode: bbccam_mode_str,
                        door_fl_coupler_not_attached: door_fl,
                        door_fr_coupler_not_attached: door_fr,
                        door_rl_coupler_not_attached: door_rl,
                        door_rr_coupler_not_attached: door_rr,
                        coupler_attached_flags: coupler_flags,
                        speaker_blocked_flags,
                    };

                    if let Ok(mut latest) = get_latest_camera_data().try_lock() {
                        *latest = Some(camera_data.clone());
                    }

                    static CAMERA_EMIT_THROTTLE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
                    let last_emit = CAMERA_EMIT_THROTTLE.load(Ordering::Relaxed);
                    if now_ms - last_emit > 33 {
                        CAMERA_EMIT_THROTTLE.store(now_ms, Ordering::Relaxed);
                        let _ = app_handle.emit("beamng_camera_data", serde_json::to_value(&camera_data).unwrap());
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(e) => eprintln!("UDP camera receive error: {}", e),
            }
        }
    });
}

pub(crate) fn start_beamng_udp_listener(app_handle: AppHandle, audio_state_3d: Arc<SpatialAudioState>) {
    thread::spawn(move || {
        use socket2::{Domain, Protocol, Socket, Type};
        use std::mem::MaybeUninit;
        use std::net::SocketAddr;

        let socket = match Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to create UDP socket: {}", e);
                return;
            }
        };
        let _ = socket.set_reuse_address(true);
        let addr: SocketAddr = "0.0.0.0:30000".parse().unwrap();
        if let Err(e) = socket.bind(&addr.into()) {
            eprintln!("Failed to bind UDP socket on 30000: {}", e);
            return;
        }
        let _ = socket.set_read_timeout(Some(std::time::Duration::from_millis(100)));
        eprintln!("UDP telemetry listener bound to port 30000");

        let mut buffer = vec![MaybeUninit::<u8>::uninit(); 4096];
        let udp_running = audio_state_3d.udp_running.clone();
        udp_running.store(true, Ordering::Relaxed);

        while udp_running.load(Ordering::Relaxed) {
            match socket.recv_from(&mut buffer) {
                Ok((size, _)) => {
                    let data: &[u8] = unsafe {
                        std::slice::from_raw_parts(buffer.as_ptr() as *const u8, size)
                    };
                    if let Ok(data_str) = std::str::from_utf8(data) {
                        if let Ok(telemetry) = serde_json::from_str::<BeamNgTelemetry>(data_str) {
                            audio_state_3d.update_telemetry(
                                telemetry.speed, telemetry.rpm, telemetry.throttle,
                            );
                            // NOTE: Previously this modified speaker levels based on
                            // speed/RPM/throttle, which caused a ~2× volume boost while
                            // driving vs stationary.  Speaker levels now stay at their
                            // configured defaults so audio sounds identical regardless
                            // of driving state.  The telemetry data above is still
                            // recorded for other features (session stats, UI display).

                            static TELEMETRY_EMIT_THROTTLE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                            let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
                            let last_emit = TELEMETRY_EMIT_THROTTLE.load(Ordering::Relaxed);
                            if now_ms - last_emit > 33 {
                                TELEMETRY_EMIT_THROTTLE.store(now_ms, Ordering::Relaxed);
                                let _ = app_handle.emit("beamng_telemetry", serde_json::json!({
                                    "speed": telemetry.speed, "rpm": telemetry.rpm, "throttle": telemetry.throttle,
                                    "front_left": 0.25, "front_right": 0.25,
                                    "rear_left": 0.25, "rear_right": 0.25,
                                    "subwoofer": 0.5,
                                }));
                            }
                        }
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(e) => eprintln!("UDP telemetry receive error: {}", e),
            }
        }
        eprintln!("UDP telemetry listener stopped");
    });
}

// Fix #4: Tauri command to poll latest camera data (for when window is in background)
#[tauri::command]
pub fn get_latest_camera_data_command() -> Option<CameraData> {
    get_latest_camera_data()
        .try_lock()
        .ok()
        .and_then(|g| g.clone())
}

#[tauri::command]
pub fn get_udp_status() -> serde_json::Value {
    use serde_json::json;
    json!({
        "running": UDP_LISTENER_RUNNING.load(Ordering::Relaxed),
        "packets_received": UDP_PACKETS_RECEIVED.load(Ordering::Relaxed),
        "last_packet_size": UDP_LAST_PACKET_SIZE.load(Ordering::Relaxed),
        "size_mismatch": UDP_SIZE_MISMATCH.load(Ordering::Relaxed),
        "expected_size": UDP_EXPECTED_SIZE.load(Ordering::Relaxed)
    })
}

// Media command UDP listener (port 5001)
#[derive(Debug, Deserialize)]
pub(crate) struct MediaCommandUdp {
    pub command: String,
    // ws_click / ws_input from the in-game stream view (normalized 0..1).
    // action: move|down|up|wheel|key, button: left|right|middle,
    // delta: wheel units, held: JS e.buttons bitmask, method: post|sendinput,
    // code/key/down: physical key code, resolved char, pressed flag.
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub method: Option<String>,
    pub action: Option<String>,
    pub button: Option<String>,
    pub delta: Option<i32>,
    pub held: Option<i32>,
    pub code: Option<String>,
    pub key: Option<String>,
    pub down: Option<i32>,
}

pub(crate) fn start_media_command_udp_listener(app_handle: AppHandle) {
    thread::spawn(move || {
        use socket2::{Domain, Protocol, Socket, Type};
        use std::mem::MaybeUninit;
        use std::net::SocketAddr;

        eprintln!("=== Starting Media Command UDP Listener ===");
        let _ = app_handle.emit("rust_log", "Starting Media Command UDP listener on port 5001");

        let socket = match Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to create media command UDP socket: {}", e);
                let _ = app_handle.emit("rust_log", format!("Failed to create media command UDP socket: {}", e));
                return;
            }
        };
        let _ = socket.set_reuse_address(true);
        let addr: SocketAddr = "0.0.0.0:5001".parse().unwrap();
        if let Err(e) = socket.bind(&addr.into()) {
            eprintln!("Failed to bind media command UDP socket: {}", e);
            let _ = app_handle.emit("rust_log", format!("Failed to bind media command UDP socket: {}", e));
            return;
        }
        // Room for bursts + the larger amp_state telemetry datagrams (~2 KB
        // of JSON at 10 Hz): small buffers truncate on Windows and the JSON
        // parse then fails silently.
        let _ = socket.set_recv_buffer_size(256 * 1024);
        let _ = socket.set_read_timeout(Some(std::time::Duration::from_millis(100)));

        eprintln!("Media command UDP listener bound to port 5001");
        let _ = app_handle.emit("rust_log", "Media command UDP listener bound to port 5001");
        let mut buffer = vec![MaybeUninit::<u8>::uninit(); 8192];

        loop {
            match socket.recv_from(&mut buffer) {
                Ok((size, _addr)) => {
                    let data: &[u8] = unsafe {
                        std::slice::from_raw_parts(buffer.as_ptr() as *const u8, size)
                    };

                    if let Ok(json_str) = std::str::from_utf8(data) {
                        if let Ok(cmd) = serde_json::from_str::<MediaCommandUdp>(json_str) {
                            INPUT_PACKETS.fetch_add(1, Ordering::Relaxed);
                            // NOTE: no Runtime::new() per packet anymore (it
                            // spawned a thread pool per datagram at 30Hz).
                            // Rare async media commands ride Tauri's runtime.
                            match cmd.command.as_str() {
                                "play_pause" => {
                                    tauri::async_runtime::spawn(async move {
                                        let _ = media_info::media_play_pause().await;
                                    });
                                    eprintln!("[MediaUDP] Received play_pause command");
                                }
                                "next" => {
                                    tauri::async_runtime::spawn(async move {
                                        let _ = media_info::media_next().await;
                                    });
                                    eprintln!("[MediaUDP] Received next command");
                                }
                                "previous" => {
                                    tauri::async_runtime::spawn(async move {
                                        let _ = media_info::media_previous().await;
                                    });
                                    eprintln!("[MediaUDP] Received previous command");
                                }
                                // Tap on the in-game stream view -> click the
                                // embedded Opera child at the same spot.
                                "ws_click" => {
                                    let nx = cmd.x.unwrap_or(0.5).clamp(0.0, 1.0);
                                    let ny = cmd.y.unwrap_or(0.5).clamp(0.0, 1.0);
                                    let method =
                                        cmd.method.clone().unwrap_or_else(|| "post".to_string());
                                    let child = app_handle
                                        .state::<crate::embed::EmbedState>()
                                        .0
                                        .lock()
                                        .ok()
                                        .and_then(|g| g.window.map(|(raw, _)| raw));
                                    match child {
                                        Some(raw) => {
                                            match crate::embed::opera_click(raw, nx, ny, &method)
                                            {
                                                Ok(msg) => {
                                                    eprintln!(
                                                        "[MediaUDP] ws_click {nx:.3},{ny:.3} ({method}) -> {msg}"
                                                    );
                                                    // eprintln is invisible in release builds;
                                                    // mirror to the in-app log panel.
                                                    let _ = app_handle.emit("rust_log", format!(
                                                        "[MediaUDP] ws_click {nx:.3},{ny:.3} -> {msg}"
                                                    ));
                                                }
                                                Err(e) => {
                                                    eprintln!(
                                                        "[MediaUDP] ws_click failed: {e}"
                                                    );
                                                    let _ = app_handle.emit("rust_log", format!(
                                                        "[MediaUDP] ws_click failed: {e}"
                                                    ));
                                                }
                                            }
                                        }
                                        None => eprintln!(
                                            "[MediaUDP] ws_click with no embedded Opera window"
                                        ),
                                    }
                                    // Forward click to MCP if enabled for the embedded app
                                    let app_handle_clone = app_handle.clone();
                                    tauri::async_runtime::spawn(async move {
                                        crate::mcp::mcp_log("[MCP] ws_click: forwarding click via MCP");
                                        let _ = inject_input_mcp(&app_handle_clone, "click", 1.0, Some(0), None).await;
                                    });
                                }
                                // Full input relay from the in-game stream
                                // view -> embedded Opera child (pointer sx
                                // and physical keys via mod input actions).
                                "ws_input" => {
                                    let action = cmd
                                        .action
                                        .clone()
                                        .unwrap_or_else(|| "move".to_string());
                                    let method = cmd
                                        .method
                                        .clone()
                                        .unwrap_or_else(|| "post".to_string());
                                    let child = app_handle
                                        .state::<crate::embed::EmbedState>()
                                        .0
                                        .lock()
                                        .ok()
                                        .and_then(|g| g.window.map(|(raw, _)| raw));
                                    let Some(raw) = child else {
                                        eprintln!(
                                            "[MediaUDP] ws_input with no embedded Opera window"
                                        );
                                        continue;
                                    };
                                    if action == "key" {
                                        let code =
                                            cmd.code.clone().unwrap_or_default();
                                        let key =
                                            cmd.key.clone().unwrap_or_default();
                                        let down = cmd.down.unwrap_or(0) != 0;
                                        match crate::embed::opera_key(
                                            raw, &code, &key, down, &method,
                                        ) {
                                            Ok(msg) => {
                                                eprintln!(
                                                    "[MediaUDP] ws_input key {code} {} -> {msg}",
                                                    if down { "down" } else { "up" }
                                                );
                                                let _ = app_handle.emit("rust_log", format!(
                                                    "[MediaUDP] ws_input key {code} {} -> {msg}",
                                                    if down { "down" } else { "up" }
                                                ));
                                            }
                                            Err(e) => {
                                                eprintln!(
                                                    "[MediaUDP] ws_input key failed: {e}"
                                                );
                                                let _ = app_handle.emit("rust_log", format!(
                                                    "[MediaUDP] ws_input key failed: {e}"
                                                ));
                                            }
                                        }
                                        // Forward key input to MCP if enabled - map to vehicle controls
                                        let app_handle_clone = app_handle.clone();
                                        let code_owned = code.clone();
                                        let down_owned = down;
                                        tauri::async_runtime::spawn(async move {
                                            // Map key codes to vehicle control events
                                            let (event, value) = match code_owned.as_str() {
                                                // Throttle: ArrowUp / KeyW
                                                "ArrowUp" | "KeyW" => ("throttle", if down_owned { 1.0 } else { 0.0 }),
                                                // Brake: ArrowDown / KeyS
                                                "ArrowDown" | "KeyS" => ("brake", if down_owned { 1.0 } else { 0.0 }),
                                                // Steering left: ArrowLeft / KeyA
                                                "ArrowLeft" | "KeyA" => ("steering", if down_owned { -1.0 } else { 0.0 }),
                                                // Steering right: ArrowRight / KeyD
                                                "ArrowRight" | "KeyD" => ("steering", if down_owned { 1.0 } else { 0.0 }),
                                                // Parking brake: Space
                                                "Space" => ("parkingbrake", if down_owned { 1.0 } else { 0.0 }),
                                                // Clutch: ShiftLeft / ShiftRight
                                                "ShiftLeft" | "ShiftRight" => ("clutch", if down_owned { 1.0 } else { 0.0 }),
                                                _ => return, // ignore unmapped keys
                                            };
                                            let _ = inject_input_mcp(&app_handle_clone, event, value, Some(2), None).await;
                                        crate::mcp::mcp_log(&format!("[MCP] ws_input key: sent event {event} = {value}"));
                                        });
                                        continue;
                                    }
                                    let button = cmd
                                        .button
                                        .clone()
                                        .unwrap_or_else(|| "left".to_string());
                                    let nx = cmd.x.unwrap_or(0.5).clamp(0.0, 1.0);
                                    let ny = cmd.y.unwrap_or(0.5).clamp(0.0, 1.0);
                                    let delta = cmd.delta.unwrap_or(0);
                                    let held = cmd.held.unwrap_or(0);
                                    match crate::embed::opera_input(
                                        raw, &action, &button, nx, ny, delta, held,
                                        &method,
                                    ) {
                                        Ok(msg) => {
                                            eprintln!(
                                                "[MediaUDP] ws_input {action} {button} {nx:.3},{ny:.3} -> {msg}"
                                            );
                                            // Skip `move` (30Hz spam); the panel
                                            // throttles anyway, but don't waste IPC.
                                            if action != "move" {
                                                let _ = app_handle.emit("rust_log", format!(
                                                    "[MediaUDP] ws_input {action} {button} {nx:.3},{ny:.3} -> {msg}"
                                                ));
                                            }
                                        }
                                        Err(e) => {
                                            eprintln!(
                                                "[MediaUDP] ws_input failed: {e}"
                                            );
                                            if action != "move" {
                                                let _ = app_handle.emit("rust_log", format!(
                                                    "[MediaUDP] ws_input {action} failed: {e}"
                                                ));
                                            }
                                        }
}
                                        // Forward pointer input to MCP if enabled
                                        let app_handle_clone = app_handle.clone();
                                        let action_owned = action.clone();
                                        let button_owned = button.clone();
                                        let delta_owned = delta;
                                        tauri::async_runtime::spawn(async move {
                                            let event = match action_owned.as_str() {
                                                "down" => format!("pointer_down_{}", button_owned),
                                                "up" => format!("pointer_up_{}", button_owned),
                                                "wheel" => "wheel".to_string(),
                                                _ => return, // skip move
                                            };
                                            let value = if action_owned == "wheel" { delta_owned as f64 } else { 1.0 };
                                            let _ = inject_input_mcp(&app_handle_clone, &event, value, Some(0), None).await;
                                            crate::mcp::mcp_log(&format!("[MCP] ws_input pointer: sent event {event} = {value}"));
                                        });
                                }
                                // Amplifier/subwoofer telemetry from the mod
                                // (ampSimGE / subwooferWsGE UDP fallback)
                                "amp_state" => {
                                    crate::amp_state::handle_amp_state(&app_handle, json_str);
                                }
                                _ => {
                                    INPUT_UNKNOWN.fetch_add(1, Ordering::Relaxed);
                                    eprintln!("[MediaUDP] Unknown command: {}", cmd.command);
                                }
                            }
                        }
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(e) => eprintln!("Media command UDP receive error: {}", e),
            }
        }
    });
}
