use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tauri::State;

// ============================================================================
// ELECTRICS UDP LISTENER - Controls EQ bands and volume from BeamNG electrics
// ============================================================================

#[derive(Clone)]
pub struct ElectricsState {
    pub(crate) eq_band_values: Arc<Mutex<[f32; 15]>>,
    pub(crate) volume_value: Arc<AtomicU32>,
    pub(crate) udp_running: Arc<AtomicBool>,
}

impl ElectricsState {
    pub fn new() -> Self {
        Self {
            eq_band_values: Arc::new(Mutex::new([0.0; 15])),
            volume_value: Arc::new(AtomicU32::new(0.0f32.to_bits())),
            udp_running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn update_eq_band(&self, band_index: usize, value: f32) {
        if let Ok(mut bands) = self.eq_band_values.lock() {
            if band_index < 15 {
                bands[band_index] = value.clamp(-16.0, 16.0);
            }
        }
    }

    pub fn update_volume(&self, value: f32) {
        self.volume_value.store(value.clamp(-16.0, 16.0).to_bits(), Ordering::Relaxed);
    }

    pub fn get_eq_bands(&self) -> [f32; 15] {
        *self.eq_band_values.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get_volume(&self) -> f32 {
        f32::from_bits(self.volume_value.load(Ordering::Relaxed))
    }
}

// Electrics UDP status (uses port 4444 camera stream)
pub(crate) static ELECTRICS_UDP_ENABLED: AtomicBool = AtomicBool::new(false);

#[tauri::command]
pub fn get_electrics_udp_status() -> serde_json::Value {
    use serde_json::json;
    json!({
        "enabled": ELECTRICS_UDP_ENABLED.load(Ordering::Relaxed),
    })
}

#[tauri::command]
pub fn start_electrics_udp_listener(electrics_state: State<ElectricsState>) -> Result<(), String> {
    electrics_state.udp_running.store(true, Ordering::Relaxed);
    ELECTRICS_UDP_ENABLED.store(true, Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
pub fn stop_electrics_udp_listener(electrics_state: State<ElectricsState>) -> Result<(), String> {
    electrics_state.udp_running.store(false, Ordering::Relaxed);
    ELECTRICS_UDP_ENABLED.store(false, Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
pub fn is_electrics_udp_enabled(electrics_state: State<ElectricsState>) -> bool {
    electrics_state.udp_running.load(Ordering::Relaxed)
}

#[tauri::command]
pub fn get_electrics_values(electrics_state: State<ElectricsState>) -> serde_json::Value {
    use serde_json::json;
    let eq_bands = electrics_state.get_eq_bands();
    let volume = electrics_state.get_volume();
    json!({
        "eq_bands": eq_bands,
        "volume": volume
    })
}
