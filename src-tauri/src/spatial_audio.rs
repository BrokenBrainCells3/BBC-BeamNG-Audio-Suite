use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SpeakerOffsets {
    pub fl: [f32; 3],
    pub fr: [f32; 3],
    pub rl: [f32; 3],
    pub rr: [f32; 3],
    pub sub_l: [f32; 3],
    pub sub_r: [f32; 3],
}

impl Default for SpeakerOffsets {
    fn default() -> Self {
        let half_length = 2.0;
        let half_width = 0.9;
        let height = 0.5;
        let sub_height = 0.3;
        Self {
            fl: [-half_width, -half_length, height],
            fr: [half_width, -half_length, height],
            rl: [-half_width, half_length, height],
            rr: [half_width, half_length, height],
            sub_l: [-0.3, half_length - 0.5, sub_height],
            sub_r: [0.3, half_length - 0.5, sub_height],
        }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct SpeakerLevels {
    pub front_left: f32,
    pub front_right: f32,
    pub rear_left: f32,
    pub rear_right: f32,
    pub subwoofer_left: f32,
    pub subwoofer_right: f32,
    pub tweeter_left: f32,
    pub tweeter_right: f32,
}

impl Default for SpeakerLevels {
    fn default() -> Self {
        Self {
            front_left: 0.25,
            front_right: 0.25,
            rear_left: 0.25,
            rear_right: 0.25,
            subwoofer_left: 0.5,
            subwoofer_right: 0.5,
            tweeter_left: 0.25,
            tweeter_right: 0.25,
        }
    }
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
pub struct TelemetryData {
    pub vehicle_speed: f32,
    pub engine_rpm: f32,
    pub throttle: f32,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
pub struct VehicleState {
    pub pos: [f32; 3],
    pub quat: [f32; 4],
}

#[derive(Clone, Copy, Default)]
pub struct CameraState {
    pub pos: [f32; 3],
    pub forward: [f32; 3],
}

// ============================================================================
// DOUBLE BUFFERING FOR SPATIAL AUDIO - Ensures consistent position reads
// ============================================================================

/// Snapshot of all spatial audio positions at a single point in time
/// This is atomically swapped to prevent tearing between camera and speaker updates
#[derive(Clone, Copy)]
struct SpatialSnapshot {
    camera_pos: [f32; 3],
    camera_forward: [f32; 3],
    vehicle_pos: [f32; 3],
    vehicle_quat: [f32; 4],
    speaker_positions: [[f32; 3]; 6],  // FL, FR, RL, RR, SubL, SubR
}

impl Default for SpatialSnapshot {
    fn default() -> Self {
        Self {
            camera_pos: [0.0; 3],
            camera_forward: [0.0, 0.0, 1.0],  // Facing -Y by default
            vehicle_pos: [0.0; 3],
            vehicle_quat: [0.0, 0.0, 0.0, 1.0],  // Identity quaternion
            speaker_positions: [[0.0; 3]; 6],
        }
    }
}

#[derive(Clone)]
pub struct SpatialAudioState {
    pub(crate) enabled: Arc<AtomicBool>,
    pub(crate) udp_audio_enabled: Arc<AtomicBool>,
    pub(crate) udp_running: Arc<AtomicBool>,
    pub(crate) speaker_levels: Arc<Mutex<SpeakerLevels>>,
    pub(crate) telemetry: Arc<Mutex<TelemetryData>>,
    snapshots: [Arc<Mutex<SpatialSnapshot>>; 2],
    active_snapshot_index: Arc<AtomicUsize>,
    pub(crate) vehicle: Arc<Mutex<VehicleState>>,
    pub camera: Arc<Mutex<CameraState>>,
    pub(crate) speaker_offsets: Arc<Mutex<SpeakerOffsets>>,
    pub(crate) udp_speaker_positions: Arc<Mutex<UdpSpeakerPositions>>,
    pub(crate) speaker_mutes: Arc<Mutex<SpeakerMutes>>,
    pub(crate) door_states: Arc<Mutex<[f32; 4]>>,
    pub(crate) bbccam_mode: Arc<Mutex<String>>,
    muffle_current: Arc<AtomicU32>,
    current_volume_db: Arc<AtomicU32>,
    pub(crate) itd_enabled: Arc<AtomicBool>,
    pub(crate) head_shadow_enabled: Arc<AtomicBool>,
    pub(crate) sub_crossover_enabled: Arc<AtomicBool>,
    pub(crate) rear_voicing_enabled: Arc<AtomicBool>,
    pub(crate) rattle_volume: Arc<AtomicU32>,
    pub(crate) rattle_threshold: Arc<AtomicU32>,
    pub(crate) tweeter_low_freq: Arc<AtomicU32>,
    pub(crate) tweeter_high_freq: Arc<AtomicU32>,
    pub(crate) sub_low_freq: Arc<AtomicU32>,
    pub(crate) sub_high_freq: Arc<AtomicU32>,
    // Amplifier telemetry override (amp_state from the BeamNG mod): the
    // SMOOTHED combined sub-speaker multiplier (f32 bits). calculate_speaker_
    // gains glides it toward the remote_level / fuse_blown target each block.
    amp_sub_gain: Arc<AtomicU32>,
}

#[derive(Clone, Default)]
pub struct UdpSpeakerPositions {
    pub driver_main: [f32; 3],
    pub driver_rear: [f32; 3],
    pub driver_rear2: [f32; 3],
    pub driver_sub: [f32; 3],
    pub passenger_main: [f32; 3],
    pub passenger_rear: [f32; 3],
    pub passenger_rear2: [f32; 3],
    pub passenger_sub: [f32; 3],
    pub driver_twt: [f32; 3],
    pub passenger_twt: [f32; 3],
    pub valid_flags: u32,
    pub blocked_flags: u32,
    pub static_blocked_flags: u32,
}

#[derive(Clone, Copy, Default)]
pub struct SpeakerMutes {
    pub fl_muted: bool,
    pub fr_muted: bool,
    pub rl_muted: bool,
    pub rr_muted: bool,
    pub sub_l_muted: bool,
    pub sub_r_muted: bool,
}

impl SpatialAudioState {
    pub fn new() -> Self {
        let snapshot1 = Arc::new(Mutex::new(SpatialSnapshot::default()));
        let snapshot2 = Arc::new(Mutex::new(SpatialSnapshot::default()));
        
        Self {
            enabled: Arc::new(AtomicBool::new(false)),
            udp_audio_enabled: Arc::new(AtomicBool::new(false)),
            udp_running: Arc::new(AtomicBool::new(false)),
            speaker_levels: Arc::new(Mutex::new(SpeakerLevels::default())),
            telemetry: Arc::new(Mutex::new(TelemetryData::default())),
            snapshots: [snapshot1, snapshot2],
            active_snapshot_index: Arc::new(AtomicUsize::new(0)),
            vehicle: Arc::new(Mutex::new(VehicleState::default())),
            camera: Arc::new(Mutex::new(CameraState::default())),
            speaker_offsets: Arc::new(Mutex::new(SpeakerOffsets::default())),
            udp_speaker_positions: Arc::new(Mutex::new(UdpSpeakerPositions::default())),
            speaker_mutes: Arc::new(Mutex::new(SpeakerMutes::default())),
            door_states: Arc::new(Mutex::new([0.0f32; 4])),
            bbccam_mode: Arc::new(Mutex::new(String::new())),
            muffle_current: Arc::new(AtomicU32::new(0.0f32.to_bits())),
            current_volume_db: Arc::new(AtomicU32::new(0.0f32.to_bits())),
            itd_enabled: Arc::new(AtomicBool::new(true)),
            head_shadow_enabled: Arc::new(AtomicBool::new(true)),
            sub_crossover_enabled: Arc::new(AtomicBool::new(true)),
            rear_voicing_enabled: Arc::new(AtomicBool::new(true)),
            rattle_volume: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            rattle_threshold: Arc::new(AtomicU32::new(0.2f32.to_bits())),
            tweeter_low_freq: Arc::new(AtomicU32::new(2500.0f32.to_bits())),
            tweeter_high_freq: Arc::new(AtomicU32::new(20000.0f32.to_bits())),
            sub_low_freq: Arc::new(AtomicU32::new(20.0f32.to_bits())),
            sub_high_freq: Arc::new(AtomicU32::new(90.0f32.to_bits())),
            amp_sub_gain: Arc::new(AtomicU32::new(1.0f32.to_bits())),
        }
    }

    /// Amplifier telemetry override for the sub speakers: the remote bass
    /// knob level, supply-voltage power headroom ((V/14.4)²), voice coil
    /// power compression, thermal protect cutback, or 0 when the mod reports
    /// a blown fuse. One-pole smoothed per call so knob moves, voltage sag
    /// and fuse events glide instead of zipper/click. When amp_state
    /// telemetry stops, the target returns to unity so the UI sliders are
    /// back in full control.
    pub(crate) fn amp_sub_gain_factor(&self) -> f32 {
        let current = f32::from_bits(self.amp_sub_gain.load(Ordering::Relaxed));
        let target = if crate::amp_state::is_receiving()
            || crate::amp_state::overrides_active()
        {
            let blown = crate::amp_state::get_bool_eff("fuse_blown", false);
            if blown {
                0.0
            } else {
                // Remote bass knob scales only the sub channel
                let remote = crate::amp_state::get_f64_eff("remote_level", 1.0).clamp(0.0, 1.0);
                // Power supply headroom: delivered power scales with the
                // square of the supply voltage (voltageSim sags on hits)
                let supply = crate::amp_state::get_f64_eff("supply_volts", 14.4).clamp(8.0, 18.0);
                let headroom = ((supply / 14.4) * (supply / 14.4)).clamp(0.3, 1.6);
                // Voice coil power compression (stronger of both models)
                let compression = 10f64.powf(
                    crate::amp_state::get_power_compression_db().max(-6.0) / 20.0,
                );
                // Thermal protect from either model
                let protect = if crate::amp_state::get_thermal_protect() { 0.3 } else { 1.0 };
                // More subs at the same amp power = +3 dB per doubling
                let count = crate::amp_state::get_f64_eff("sub_count", 1.0).clamp(1.0, 8.0);
                let count_gain = 10f64.powf(3.0 * count.log2() / 20.0);
                // Driver sensitivity relative to the 88 dB reference
                let sens =
                    crate::amp_state::get_f64_eff("sub_sensitivity_db", 88.0).clamp(70.0, 100.0);
                let sens_gain = 10f64.powf((sens - 88.0) / 20.0);
                // Piston area relative to a 12" (480 cm2) reference: doubled
                // area = +3 dB. This is what cone size actually buys.
                let sd = crate::amp_state::get_f64_eff("sd_cm2", 480.0).clamp(100.0, 1500.0);
                let area_gain = 10.0 * (sd / 480.0).log10();
                let area_gain = 10f64.powf(area_gain / 20.0);
                // Motor strength (BL product) relative to a 22 T*m reference:
                // more motor = more output per watt near resonance.
                let bl = crate::amp_state::get_f64_eff("bl_tm", 22.0).clamp(5.0, 40.0);
                let motor_gain = (bl / 22.0).powf(0.5).clamp(0.5, 1.4);
                // Bridged mono operation doubles the voltage swing (+3 dB)
                let bridged =
                    if crate::amp_state::get_bool_eff("bridged", false) { 2.0f64.sqrt() } else { 1.0 };

                (remote * headroom * compression * protect * count_gain * sens_gain * area_gain
                    * motor_gain * bridged)
                    .clamp(0.0, 4.0) as f32
            }
        } else {
            1.0
        };
        let next = current + (target - current) * 0.1;
        self.amp_sub_gain.store(next.to_bits(), Ordering::Relaxed);
        next
    }
    
    fn get_active_snapshot(&self) -> SpatialSnapshot {
        let active_idx = self.active_snapshot_index.load(Ordering::SeqCst);
        let snapshot = self.snapshots[active_idx].lock().unwrap_or_else(|e| e.into_inner());
        *snapshot
    }
    
    pub fn update_snapshot(&self, camera_pos: [f32; 3], camera_forward: [f32; 3], 
                       vehicle_pos: [f32; 3], vehicle_quat: [f32; 4],
                       speaker_positions: [[f32; 3]; 6]) {
        let active_idx = self.active_snapshot_index.load(Ordering::SeqCst);
        let inactive_idx = 1 - active_idx;
        
        let mut snapshot = self.snapshots[inactive_idx].lock().unwrap_or_else(|e| e.into_inner());
        snapshot.camera_pos = camera_pos;
        snapshot.camera_forward = camera_forward;
        snapshot.vehicle_pos = vehicle_pos;
        snapshot.vehicle_quat = vehicle_quat;
        snapshot.speaker_positions = speaker_positions;
        
        drop(snapshot);
        self.active_snapshot_index.store(inactive_idx, Ordering::SeqCst);
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        if !enabled {
            self.udp_running.store(false, Ordering::Relaxed);
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_itd_enabled(&self, enabled: bool) {
        self.itd_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn is_itd_enabled(&self) -> bool {
        self.itd_enabled.load(Ordering::Relaxed)
    }

    pub fn set_head_shadow_enabled(&self, enabled: bool) {
        self.head_shadow_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn is_head_shadow_enabled(&self) -> bool {
        self.head_shadow_enabled.load(Ordering::Relaxed)
    }

    pub fn set_sub_crossover_enabled(&self, enabled: bool) {
        self.sub_crossover_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn is_sub_crossover_enabled(&self) -> bool {
        self.sub_crossover_enabled.load(Ordering::Relaxed)
    }

    pub fn set_rear_voicing_enabled(&self, enabled: bool) {
        self.rear_voicing_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn is_rear_voicing_enabled(&self) -> bool {
        self.rear_voicing_enabled.load(Ordering::Relaxed)
    }

    pub fn set_udp_audio_enabled(&self, enabled: bool) {
        self.udp_audio_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn is_udp_audio_enabled(&self) -> bool {
        self.udp_audio_enabled.load(Ordering::Relaxed)
    }

    pub fn update_udp_speaker_positions(&self, positions: UdpSpeakerPositions) {
        let mut pos = self.udp_speaker_positions.lock().unwrap_or_else(|e| e.into_inner());
        *pos = positions;
    }

    pub fn update_vehicle(&self, pos: [f32; 3], quat: [f32; 4]) {
        let mut v = self.vehicle.lock().unwrap_or_else(|e| e.into_inner());
        v.pos = pos;
        v.quat = quat;
        drop(v);
        
        let camera = self.camera.lock().unwrap_or_else(|e| e.into_inner());
        let cam_pos = camera.pos;
        let cam_forward = camera.forward;
        drop(camera);
        
        let offsets = *self.speaker_offsets.lock().unwrap_or_else(|e| e.into_inner());
        let offsets_arr = [offsets.fl, offsets.fr, offsets.rl, offsets.rr, offsets.sub_l, offsets.sub_r];
        let mut speaker_positions = [[0.0f32; 3]; 6];
        for (i, &offset) in offsets_arr.iter().enumerate() {
            let rotated = rotate_vector_by_quat(offset, quat);
            speaker_positions[i] = [
                pos[0] + rotated[0],
                pos[1] + rotated[1],
                pos[2] + rotated[2],
            ];
        }
        
        self.update_snapshot(cam_pos, cam_forward, pos, quat, speaker_positions);
    }

    pub fn update_camera(&self, pos: [f32; 3], forward: [f32; 3]) {
        let mut c = self.camera.lock().unwrap_or_else(|e| e.into_inner());
        c.pos = pos;
        c.forward = forward;
        drop(c);
        
        let vehicle = self.vehicle.lock().unwrap_or_else(|e| e.into_inner());
        let veh_pos = vehicle.pos;
        let veh_quat = vehicle.quat;
        drop(vehicle);
        
        let offsets = *self.speaker_offsets.lock().unwrap_or_else(|e| e.into_inner());
        let offsets_arr = [offsets.fl, offsets.fr, offsets.rl, offsets.rr, offsets.sub_l, offsets.sub_r];
        let mut speaker_positions = [[0.0f32; 3]; 6];
        for (i, &offset) in offsets_arr.iter().enumerate() {
            let rotated = rotate_vector_by_quat(offset, veh_quat);
            speaker_positions[i] = [
                veh_pos[0] + rotated[0],
                veh_pos[1] + rotated[1],
                veh_pos[2] + rotated[2],
            ];
        }
        
        self.update_snapshot(pos, forward, veh_pos, veh_quat, speaker_positions);
    }

    pub fn update_speaker_offsets(&self, offsets: SpeakerOffsets) {
        *self.speaker_offsets.lock().unwrap_or_else(|e| e.into_inner()) = offsets;
        
        let vehicle = self.vehicle.lock().unwrap_or_else(|e| e.into_inner());
        let veh_pos = vehicle.pos;
        let veh_quat = vehicle.quat;
        drop(vehicle);
        
        let camera = self.camera.lock().unwrap_or_else(|e| e.into_inner());
        let cam_pos = camera.pos;
        let cam_forward = camera.forward;
        drop(camera);
        
        let offsets_arr = [offsets.fl, offsets.fr, offsets.rl, offsets.rr, offsets.sub_l, offsets.sub_r];
        let mut speaker_positions = [[0.0f32; 3]; 6];
        for (i, &offset) in offsets_arr.iter().enumerate() {
            let rotated = rotate_vector_by_quat(offset, veh_quat);
            speaker_positions[i] = [
                veh_pos[0] + rotated[0],
                veh_pos[1] + rotated[1],
                veh_pos[2] + rotated[2],
            ];
        }
        
        self.update_snapshot(cam_pos, cam_forward, veh_pos, veh_quat, speaker_positions);
    }

    pub fn get_speaker_positions(&self) -> [[f64; 3]; 6] {
        let snapshot = self.get_active_snapshot();
        [
            [snapshot.speaker_positions[0][0] as f64, snapshot.speaker_positions[0][1] as f64, snapshot.speaker_positions[0][2] as f64],
            [snapshot.speaker_positions[1][0] as f64, snapshot.speaker_positions[1][1] as f64, snapshot.speaker_positions[1][2] as f64],
            [snapshot.speaker_positions[2][0] as f64, snapshot.speaker_positions[2][1] as f64, snapshot.speaker_positions[2][2] as f64],
            [snapshot.speaker_positions[3][0] as f64, snapshot.speaker_positions[3][1] as f64, snapshot.speaker_positions[3][2] as f64],
            [snapshot.speaker_positions[4][0] as f64, snapshot.speaker_positions[4][1] as f64, snapshot.speaker_positions[4][2] as f64],
            [snapshot.speaker_positions[5][0] as f64, snapshot.speaker_positions[5][1] as f64, snapshot.speaker_positions[5][2] as f64],
        ]
    }

    pub fn get_speaker_distances(&self) -> [f64; 6] {
        let snapshot = self.get_active_snapshot();
        let cam_pos = snapshot.camera_pos;
        let mut distances = [0.0; 6];
        for (i, &p) in snapshot.speaker_positions.iter().enumerate() {
            let dx = p[0] as f64 - cam_pos[0] as f64;
            let dy = p[1] as f64 - cam_pos[1] as f64;
            let dz = p[2] as f64 - cam_pos[2] as f64;
            distances[i] = (dx * dx + dy * dy + dz * dz).sqrt();
        }
        distances
    }

    pub fn calculate_speaker_gains(&self) -> [(f32, f32); 6] {
        // Read speaker level sliders and convert to linear multipliers.
        // Default SpeakerLevels has front/rear at 0.25, subs at 0.5.
        // Scale so that the default maps to 1.0 (unity gain) and the maximum
        // slider position (1.0) maps to the ceiling multiplier.
        let levels = self.speaker_levels.try_lock().map(|g| *g).unwrap_or_default();
        // NOTE: the amp telemetry factor (remote knob, thermal, fuse, driver
        // gains...) is deliberately NOT baked in here — it's applied after
        // the headroom normalization in the processor, otherwise the
        // normalizer compensates level changes away and they'd be inaudible.
        let speaker_level_mult = [
            levels.front_left   * 4.0, // default 0.25 → 1.0
            levels.front_right  * 4.0,
            levels.rear_left    * 4.0,
            levels.rear_right   * 4.0,
            levels.subwoofer_left  * 2.0, // default 0.5 → 1.0
            levels.subwoofer_right * 2.0,
        ];

        let snapshot = self.get_active_snapshot();
        let cam_pos = snapshot.camera_pos;
        let mut distances = [0.0; 6];
        for (i, &p) in snapshot.speaker_positions.iter().enumerate() {
            let dx = p[0] as f64 - cam_pos[0] as f64;
            let dy = p[1] as f64 - cam_pos[1] as f64;
            let dz = p[2] as f64 - cam_pos[2] as f64;
            distances[i] = (dx * dx + dy * dy + dz * dz).sqrt();
        }
        
        let cam_forward = snapshot.camera_forward;

        let world_up = [0.0f32, 0.0, 1.0];
        let cam_right = normalize(cross(cam_forward, world_up));

        let speaker_positions = snapshot.speaker_positions;
        let mutes = self.speaker_mutes.try_lock().map(|g| *g).unwrap_or_default();
        let mut gains = [(0.0f32, 0.0f32); 6];

        for (i, &speaker_pos) in speaker_positions.iter().enumerate() {
            let is_muted = match i {
                0 => mutes.fl_muted,
                1 => mutes.fr_muted,
                2 => mutes.rl_muted,
                3 => mutes.rr_muted,
                4 => mutes.sub_l_muted,
                5 => mutes.sub_r_muted,
                _ => false,
            };
            if is_muted { continue; }

            // A speaker whose node wasn't found in the vehicle sits at exactly
            // (0, 0, 0). Treat that as "no speaker here" and exclude it from the
            // mix entirely, so a car with fewer speaker boxes (e.g. 2 mains, no
            // rears, no subs) mixes only the speakers that actually exist.
            if speaker_pos[0] == 0.0 && speaker_pos[1] == 0.0 && speaker_pos[2] == 0.0 {
                gains[i] = (0.0f32, 0.0f32);
                continue;
            }

            let dist = distances[i] as f32;

            // ===============================
            // DISTANCE ATTENUATION
            // ===============================
            // Tight full-volume radius so even tiny camera movements
            // (~0.01-0.02 m) produce audible gain changes.
            let full_volume_radius = 0.10;

            let is_interior = self.is_interior_camera();
            // Subwoofers (i >= 4) are omnidirectional low-frequency drivers that
            // physically carry much farther than the direct-radiating mains. Give
            // them a gentler falloff curve AND a floor so the bass stays audible
            // even from chase/orbit cams far away.
            let is_sub = i >= 4;
            let distance_attenuation = if dist <= full_volume_radius {
                1.0
            } else if is_sub {
                // Sub: gentle falloff with a -12 dB floor in ALL camera modes:
                //   0.3m → 0.85,  0.5m → 0.76,  1.0m → 0.60,  1.8m → 0.43,
                //   5m → 0.25,  10m → 0.25,  20m → 0.25
                let adjusted_dist = dist - full_volume_radius;
                let base = 1.0 / (1.0 + adjusted_dist * 0.4);
                (base * base).clamp(0.25, 1.0)
            } else if is_interior {
                // Interior (driver cam): gentle inverse-square falloff with a
                // -26 dB floor so the cabin audio stays clearly audible even with
                // small camera movements:
                //   0.3m → 0.69,  0.5m → 0.51,  1.0m → 0.28,  1.8m → 0.14,  5m+ → 0.05
                let adjusted_dist = dist - full_volume_radius;
                let base = 1.0 / (1.0 + adjusted_dist * 1.0);
                (base * base).clamp(0.05, 1.0)
            } else {
                // Exterior cams (hood, chase, orbit, etc.): attenuation scales with
                // distance — inverse-square with NO floor, so the farther the camera,
                // the quieter the car sounds:
                //   0.3m → 0.69,  0.5m → 0.51,  1.0m → 0.28,  1.8m → 0.14,
                //   5m → 0.04,  10m → 0.008,  20m → 0.002
                let adjusted_dist = dist - full_volume_radius;
                let base = 1.0 / (1.0 + adjusted_dist * 1.0);
                (base * base).clamp(0.0, 1.0)
            };

            let to_speaker = [
                speaker_pos[0] - cam_pos[0],
                speaker_pos[1] - cam_pos[1],
                speaker_pos[2] - cam_pos[2],
            ];
            let len = (to_speaker[0].powi(2) + to_speaker[1].powi(2) + to_speaker[2].powi(2)).sqrt();

            if len < 0.001 {
                // Camera is right on the speaker — full volume, equal L/R panning
                let g = distance_attenuation * speaker_level_mult[i];
                gains[i] = (g * 0.5, g * 0.5);
                continue;
            }

            let norm = [to_speaker[0]/len, to_speaker[1]/len, to_speaker[2]/len];

            let _forward_dot = cam_forward[0]*norm[0] + cam_forward[1]*norm[1] + cam_forward[2]*norm[2];
            // Unified: all speakers play at full level regardless of camera mode.
            // No directional penalty — same behavior inside and outside.
            let angle_gain = 1.0;

            let pan = (cam_right[0]*norm[0] + cam_right[1]*norm[1] + cam_right[2]*norm[2])
                .clamp(-1.0, 1.0);

            // Unified near-field panning for all modes: push pan values toward
            // ±1.0 so speakers feel clearly left/right rather than blended.
            let pan = pan.signum() * pan.abs().powf(0.4);

            let pan_angle = (pan + 1.0) * 0.5 * std::f32::consts::FRAC_PI_2;
            let left_gain  = pan_angle.cos();
            let right_gain = pan_angle.sin();

            // No door-open volume boost — gain is identical for all states.
            // Door state only affects EQ muffling (handled in update_gains_and_eq).
            let smoothed_boost = 1.0f32;

            let total =
                distance_attenuation *
                angle_gain *
                smoothed_boost *
                speaker_level_mult[i];
            gains[i] = (left_gain * total, right_gain * total);
        }

        gains
    }

    /// Tweeter overlay gains for the optional driver/passenger tweeters.
    /// Returns a 2×[(left, right)] pair. A tweeter whose node isn't found sits
    /// at (0,0,0) and is zeroed (skipped), exactly like the mains.
    pub fn calculate_tweeter_gains(&self) -> [(f32, f32); 2] {
        let levels = self.speaker_levels.try_lock().map(|g| *g).unwrap_or_default();
        // Tailored level is high-band only; scale like the mains so default 0.25 → 1.0.
        let tweeter_level_mult = [
            levels.tweeter_left * 4.0,
            levels.tweeter_right * 4.0,
        ];

        let snapshot = self.get_active_snapshot();
        let cam_pos = snapshot.camera_pos;
        let cam_forward = snapshot.camera_forward;
        let world_up = [0.0f32, 0.0, 1.0];
        let cam_right = normalize(cross(cam_forward, world_up));

        let udp = self.udp_speaker_positions.try_lock().map(|g| g.clone()).unwrap_or_default();
        let twt_pos = [udp.driver_twt, udp.passenger_twt];
        // Body-behind or static-occluded tweeter = can't see it = can't hear it.
        // Bits 8/9 match the mod's drivertwt/passengertwt blocked flags.
        let twt_blocked = udp.blocked_flags | udp.static_blocked_flags;
        let mut gains = [(0.0f32, 0.0f32); 2];

        for i in 0..2 {
            let pos = twt_pos[i];
            // Missing node / not present -> exact (0,0,0) -> skip.
            if pos[0] == 0.0 && pos[1] == 0.0 && pos[2] == 0.0 {
                gains[i] = (0.0f32, 0.0f32);
                continue;
            }
            // Occluded by the car body or a static object -> completely silent.
            if (twt_blocked & (256 << i)) != 0 {
                gains[i] = (0.0f32, 0.0f32);
                continue;
            }

            let dx = pos[0] as f64 - cam_pos[0] as f64;
            let dy = pos[1] as f64 - cam_pos[1] as f64;
            let dz = pos[2] as f64 - cam_pos[2] as f64;
            let dist = ((dx * dx + dy * dy + dz * dz).sqrt()) as f32;

            // Tweeters broadcast treble, which is fairly directional but shortranged.
            let full_volume_radius = 0.10;
            let distance_attenuation = if dist <= full_volume_radius {
                1.0
            } else {
                let adjusted = dist - full_volume_radius;
                let base = 1.0 / (1.0 + adjusted * 0.8);
                (base * base).clamp(0.05, 1.0)
            };

            let to_speaker = [
                pos[0] - cam_pos[0],
                pos[1] - cam_pos[1],
                pos[2] - cam_pos[2],
            ];
            let len = (to_speaker[0].powi(2) + to_speaker[1].powi(2) + to_speaker[2].powi(2)).sqrt();
            if len < 0.001 {
                let g = distance_attenuation * tweeter_level_mult[i];
                gains[i] = (g * 0.5, g * 0.5);
                continue;
            }
            let norm = [to_speaker[0] / len, to_speaker[1] / len, to_speaker[2] / len];
            let pan = (cam_right[0] * norm[0] + cam_right[1] * norm[1] + cam_right[2] * norm[2])
                .clamp(-1.0, 1.0);
            let pan = pan.signum() * pan.abs().powf(0.4);
            let pan_angle = (pan + 1.0) * 0.5 * std::f32::consts::FRAC_PI_2;
            let left_gain = pan_angle.cos();
            let right_gain = pan_angle.sin();
            let total = distance_attenuation * tweeter_level_mult[i];
            gains[i] = (left_gain * total, right_gain * total);
        }

        gains
    }

    pub fn get_speaker_levels(&self) -> SpeakerLevels {
        *self.speaker_levels.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn update_speaker_levels(&self, levels: SpeakerLevels) {
        *self.speaker_levels.lock().unwrap_or_else(|e| e.into_inner()) = levels;
    }

    pub fn get_rattle_volume(&self) -> f32 {
        f32::from_bits(self.rattle_volume.load(Ordering::Relaxed))
    }

    pub fn set_rattle_volume(&self, v: f32) {
        self.rattle_volume
            .store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn get_rattle_threshold(&self) -> f32 {
        f32::from_bits(self.rattle_threshold.load(Ordering::Relaxed))
    }

    pub fn set_rattle_threshold(&self, v: f32) {
        self.rattle_threshold
            .store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn get_tweeter_low_freq(&self) -> f32 {
        f32::from_bits(self.tweeter_low_freq.load(Ordering::Relaxed))
    }

    pub fn set_tweeter_low_freq(&self, v: f32) {
        self.tweeter_low_freq
            .store(v.clamp(500.0, 20000.0).to_bits(), Ordering::Relaxed);
    }

    pub fn get_tweeter_high_freq(&self) -> f32 {
        f32::from_bits(self.tweeter_high_freq.load(Ordering::Relaxed))
    }

    pub fn set_tweeter_high_freq(&self, v: f32) {
        self.tweeter_high_freq
            .store(v.clamp(500.0, 20000.0).to_bits(), Ordering::Relaxed);
    }

    pub fn get_sub_low_freq(&self) -> f32 {
        f32::from_bits(self.sub_low_freq.load(Ordering::Relaxed))
    }

    pub fn set_sub_low_freq(&self, v: f32) {
        self.sub_low_freq
            .store(v.clamp(10.0, 250.0).to_bits(), Ordering::Relaxed);
    }

    pub fn get_sub_high_freq(&self) -> f32 {
        f32::from_bits(self.sub_high_freq.load(Ordering::Relaxed))
    }

    pub fn set_sub_high_freq(&self, v: f32) {
        self.sub_high_freq
            .store(v.clamp(10.0, 250.0).to_bits(), Ordering::Relaxed);
    }

    pub fn get_speaker_mutes(&self) -> SpeakerMutes {
        *self.speaker_mutes.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn update_speaker_mutes(&self, mutes: SpeakerMutes) {
        *self.speaker_mutes.lock().unwrap_or_else(|e| e.into_inner()) = mutes;
    }

    pub fn update_door_states(&self, fl: f32, fr: f32, rl: f32, rr: f32) {
        *self.door_states.lock().unwrap_or_else(|e| e.into_inner()) = [fl, fr, rl, rr];
    }

    pub fn update_bbccam_mode(&self, mode: String) {
        *self.bbccam_mode.lock().unwrap_or_else(|e| e.into_inner()) = mode;
    }

    pub(crate) fn is_interior_camera(&self) -> bool {
        let mode = self.bbccam_mode.try_lock().map(|g| g.clone()).unwrap_or_default();
        mode.to_lowercase() == "driver"
    }

    pub(crate) fn is_muffled(&self) -> bool {
        if self.is_interior_camera() {
            return false;
        }
        let doors = self.door_states.try_lock().map(|g| *g).unwrap_or([0.0; 4]);
        doors.iter().all(|&v| v == 0.0)
    }

    pub fn update_volume(&self, db: f32) {
        self.current_volume_db.store(db.to_bits(), Ordering::Relaxed);
    }

    pub fn get_smoothed_muffle_factor(&self) -> f32 {
        let target = if self.is_muffled() { 1.0f32 } else { 0.0f32 };
        self.muffle_current.store(target.to_bits(), Ordering::Relaxed);
        target
    }

    pub fn get_telemetry(&self) -> TelemetryData {
        *self.telemetry.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn update_telemetry(&self, speed: f32, rpm: f32, throttle: f32) {
        let mut t = self.telemetry.lock().unwrap_or_else(|e| e.into_inner());
        t.vehicle_speed = speed;
        t.engine_rpm = rpm;
        t.throttle = throttle;
    }

    pub fn get_speaker_positions_and_distances(&self) -> ([[f64; 3]; 6], [f64; 6]) {
        let snapshot = self.get_active_snapshot();
        
        let positions = [
            [snapshot.speaker_positions[0][0] as f64, snapshot.speaker_positions[0][1] as f64, snapshot.speaker_positions[0][2] as f64],
            [snapshot.speaker_positions[1][0] as f64, snapshot.speaker_positions[1][1] as f64, snapshot.speaker_positions[1][2] as f64],
            [snapshot.speaker_positions[2][0] as f64, snapshot.speaker_positions[2][1] as f64, snapshot.speaker_positions[2][2] as f64],
            [snapshot.speaker_positions[3][0] as f64, snapshot.speaker_positions[3][1] as f64, snapshot.speaker_positions[3][2] as f64],
            [snapshot.speaker_positions[4][0] as f64, snapshot.speaker_positions[4][1] as f64, snapshot.speaker_positions[4][2] as f64],
            [snapshot.speaker_positions[5][0] as f64, snapshot.speaker_positions[5][1] as f64, snapshot.speaker_positions[5][2] as f64],
        ];
        
        let cam_pos = snapshot.camera_pos;
        let mut distances = [0.0f64; 6];
        for (i, &pos) in positions.iter().enumerate() {
            let dx = pos[0] - cam_pos[0] as f64;
            let dy = pos[1] - cam_pos[1] as f64;
            let dz = pos[2] - cam_pos[2] as f64;
            distances[i] = (dx * dx + dy * dy + dz * dz).sqrt();
        }
        (positions, distances)
    }
}

pub(crate) fn rotate_vector_by_quat(v: [f32; 3], q: [f32; 4]) -> [f32; 3] {
    let (x, y, z) = (v[0], v[1], v[2]);
    let (qx, qy, qz, qw) = (q[0], q[1], q[2], q[3]);

    let ix = qw * x + qy * z - qz * y;
    let iy = qw * y + qz * x - qx * z;
    let iz = qw * z + qx * y - qy * x;
    let iw = -qx * x - qy * y - qz * z;

    [
        ix * qw + iw * (-qx) + iy * qz - iz * qy,
        iy * qw + iw * (-qy) + iz * qx - ix * qz,
        iz * qw + iw * (-qz) + ix * qy - iy * qx,
    ]
}

pub(crate) fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1]*b[2] - a[2]*b[1],
        a[2]*b[0] - a[0]*b[2],
        a[0]*b[1] - a[1]*b[0],
    ]
}

pub(crate) fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0]*v[0] + v[1]*v[1] + v[2]*v[2]).sqrt();
    if len < 0.0001 { return [1.0, 0.0, 0.0]; }
    [v[0]/len, v[1]/len, v[2]/len]
}