use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};

use crate::audio_buffers::STREAM_COUNT;
use crate::spatial_audio::SpatialAudioState;

#[derive(Clone)]
pub struct AudioState {
    pub running: Arc<AtomicBool>,
    pub eq_gains: Arc<Mutex<[f32; 16]>>,
    pub channel_gains: Arc<Mutex<(f32, f32)>>,
}

impl AudioState {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            eq_gains: Arc::new(Mutex::new([0.0; 16])),
            channel_gains: Arc::new(Mutex::new((0.0, 0.0))),
        }
    }

    pub fn stop_current_stream(&self) {
        eprintln!("Stopping current audio stream...");
        self.running.store(false, Ordering::SeqCst);
        let mut wait_count = 0;
        while STREAM_COUNT.load(Ordering::Acquire) > 0 && wait_count < 100 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            wait_count += 1;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        eprintln!("Audio stream stopped (waited {}ms)", wait_count * 50 + 200);
    }
}

#[tauri::command]
pub fn get_audio_input_devices() -> Vec<String> {
    use cpal::traits::HostTrait;
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(devs) => {
            use cpal::traits::DeviceTrait;
            devs.filter_map(|d| d.name().ok()).collect()
        }
        Err(_) => Vec::new(),
    }
}

#[tauri::command]
pub fn get_audio_output_devices() -> Vec<String> {
    use cpal::traits::HostTrait;
    let host = cpal::default_host();
    match host.output_devices() {
        Ok(devs) => {
            use cpal::traits::DeviceTrait;
            devs.filter_map(|d| d.name().ok()).collect()
        }
        Err(_) => Vec::new(),
    }
}

#[tauri::command]
pub fn start_audio_capture(
    input_device_name: String,
    output_device_name: String,
    state: State<AudioState>,
    state_3d: State<SpatialAudioState>,
    app_handle: AppHandle,
) -> Result<(), String> {
    use cpal::traits::{DeviceTrait, HostTrait};

    let _ = app_handle.emit("rust_log", format!("Start capture: input={} output={}", input_device_name, output_device_name));
    eprintln!("Start capture: input={} output={}", input_device_name, output_device_name);

    state.stop_current_stream();

    let host = cpal::default_host();

    let input_device = host
        .input_devices()
        .ok()
        .and_then(|devs| {
            devs.into_iter()
                .find(|d| d.name().map(|n| n == input_device_name).unwrap_or(false))
        })
        .or_else(|| host.default_input_device())
        .ok_or_else(|| "No audio input device found".to_string())?;

    let output_device = host
        .output_devices()
        .ok()
        .and_then(|devs| {
            devs.into_iter()
                .find(|d| d.name().map(|n| n == output_device_name).unwrap_or(false))
        })
        .or_else(|| host.default_output_device());

    state.running.store(true, Ordering::SeqCst);

    let state_arc: Arc<AudioState> = Arc::new((*state.inner()).clone());
    let state_3d_arc: Arc<SpatialAudioState> = Arc::new((*state_3d.inner()).clone());

    crate::spawn_audio_thread(app_handle, state_arc, state_3d_arc, input_device, output_device);

    Ok(())
}

#[tauri::command]
pub fn stop_audio_capture(
    state: State<AudioState>,
) -> Result<(), String> {
    eprintln!("Stop audio capture requested");
    state.stop_current_stream();
    state.running.store(false, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
pub fn set_eq_bands(gains: Vec<f32>, state: State<AudioState>) -> Result<(), String> {
    if gains.len() != 16 {
        return Err(format!("Expected 16 EQ bands, got {}", gains.len()));
    }
    eprintln!("[set_eq_bands] Received gains: {:?}", gains);
    *state.eq_gains.lock().unwrap() = gains.try_into().unwrap();
    Ok(())
}

#[tauri::command]
pub fn set_channel_gains(left_gain: f32, right_gain: f32, state: State<AudioState>) -> Result<(), String> {
    *state.channel_gains.lock().unwrap() = (left_gain, right_gain);
    Ok(())
}
