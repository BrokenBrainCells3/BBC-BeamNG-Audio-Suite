use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Device, SampleFormat};
use rustfft::FftPlanner;
use rubato::Resampler;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use thread_priority::*;
use tauri::{AppHandle, Emitter, Manager};

mod discord_webhook;
mod session_stats;
mod config;
mod event_queue;
mod visualizer;
mod audio_processing;
mod audio_buffers;
mod spatial_audio;
mod media_info;
mod electrics;
mod beamng_udp;
mod amp_state;
pub mod audio_capture;
mod commands;
mod cdp;
mod embed;
mod mcp;
mod stream;

use crate::media_info::MediaInfo;

// Re-exports for config module (used by discord_webhook.rs)
use config::{ConfigState, load_config, save_config};

// Re-exports for backward compat
pub use audio_buffers::RingBuffer;
pub use spatial_audio::SpatialAudioState;
pub use spatial_audio::UdpSpeakerPositions;
pub use spatial_audio::SpeakerMutes;
pub use spatial_audio::CameraState;
pub use spatial_audio::VehicleState;

// Re-exports used internally by spawn_audio_thread
use audio_processing::*;
use audio_buffers::{STREAM_COUNT, create_passthrough_buffer};
use event_queue::{should_log, log_throttled, get_event_queue, emit_band_event};
use visualizer::get_visualizer_bands;
use audio_capture::AudioState;

/// Spawn the audio capture + passthrough thread.
///
/// KEY CHANGE from original: the output callback no longer emits per-sample
/// Tauri events.  Instead it pushes into the passthrough buffer which is
/// drained by the background resampler thread.
fn spawn_audio_thread(
    app_handle: AppHandle,
    state: Arc<AudioState>,
    state_3d: Arc<SpatialAudioState>,
    device: Device,
    output_device: Option<Device>,
) {
    let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());
    log_throttled(&format!("=== Audio Thread Starting for: {} ===", device_name));

    thread::spawn(move || {
        let _ = set_current_thread_priority(ThreadPriority::Max);

        let config = match device.default_input_config() {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("Failed to get input config: {}", e);
                log_throttled(&msg);
                return;
            }
        };

        let sample_rate = config.sample_rate().0 as f32;
        let channels = config.channels() as usize;
        let device_sample_rate_u32 = config.sample_rate().0;

        let fft_size = 800;  // 60Hz at 48kHz (48000/800 = 60)
        let mut fft_planner = FftPlanner::new();
        let fft = Arc::new(fft_planner.plan_fft_forward(fft_size));
        let fft_buffer = Arc::new(Mutex::new(FftBuffer::new(fft_size)));
        let max_magnitude = Arc::new(Mutex::new(1000.0f32));
        let mut fft_workspace = FftWorkspace::new(fft_size);

        let passthrough_buffer = create_passthrough_buffer(16384); // ~341ms at 48kHz
        // Pre-fill the buffer with 2400 silent samples (50ms) to provide a latency cushion
        // This prevents the output callback from starving and clicking when running concurrently
        passthrough_buffer.extend(std::iter::repeat((0.0, 0.0)).take(2400));
        let passthrough_buffer_clone = Arc::clone(&passthrough_buffer);

        let mut spatial_processor = SpatialAudioProcessor::new(sample_rate, 50.0);
        // DISABLED: the sub rattle clip caused audible crackle/popping artifacts,
        // especially during heavy bass. It is no longer loaded or played.
        // if let Some((rattle_samples, rattle_rate)) = decode_rattle_mp3(RATTLE_MP3) {
        //     let dur = rattle_samples.len() as f32 / rattle_rate as f32;
        //     let _ = app_handle.emit("debug_log", format!(
        //         "Rattle clip: {} samples @ {} Hz ({:.2}s)",
        //         rattle_samples.len(), rattle_rate, dur
        //     ));
        //     spatial_processor.set_rattle(rattle_samples, rattle_rate);
        // } else {
        //     let _ = app_handle.emit("debug_log", "Rattle clip: DECODE FAILED");
        // }

        let err_fn = |err| eprintln!("Audio stream error: {}", err);

        // output_device is already resolved, use it directly

        match config.sample_format() {
            SampleFormat::F32 => {
                let state_clone = Arc::clone(&state);
                let state_3d_clone = Arc::clone(&state_3d);
                let app_handle_clone = app_handle.clone();
                let fft_buffer_clone = Arc::clone(&fft_buffer);
                let max_magnitude_clone = Arc::clone(&max_magnitude);
                let channel_gains = Arc::clone(&state.channel_gains);
                let eq_gains_clone = Arc::clone(&state.eq_gains);
                let passthrough_buffer = Arc::clone(&passthrough_buffer_clone);
                let fft_clone = Arc::clone(&fft);
                let mut eq_bank = EqBank::new(sample_rate);
                let mut event_throttle = EventThrottle::new(60);  // 60Hz for smoother visualizer
                let mut last_eq_gains: [f32; 16] = [f32::NEG_INFINITY; 16];
                let mut peak_limiter = PeakLimiter::new(sample_rate, 0.9);

                let _ = app_handle.emit("debug_log", "Building F32 input stream...");

                match device.build_input_stream(
                    &config.into(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        static CALLBACK_FIRED: AtomicBool = AtomicBool::new(false);
                        if !CALLBACK_FIRED.swap(true, Ordering::Relaxed) {
                            let _ = app_handle_clone.emit("debug_log", "Audio callback started!");
                        }

                        if !state_clone.running.load(Ordering::Relaxed) {
                            return;
                        }

                        // Fix #6: Use try_lock with fallback to avoid blocking
                        let (left_gain, right_gain) = channel_gains
                            .try_lock()
                            .map(|g| *g)
                            .unwrap_or((0.0, 0.0));
                        
                        let eq_gains = eq_gains_clone
                            .try_lock()
                            .map(|g| *g)
                            .unwrap_or([0.0; 16]);
                        
                        let is_muted = eq_gains[15] <= -12.0;

                        // Input trim: -3 dB headroom to compensate for db_to_linear recalibration.
                        // This also sets the base volume level for 3D audio mode.
                        // Increase to -6 dB if you need more headroom, decrease to 0 dB for louder output.
                        const INPUT_TRIM: f32 = 0.707_945_8; // 10^(-3/20) = -3 dB

                        // Amplifier input stage (amp_state telemetry): gain_db
                        // (relative to the 24 dB reference) and preout_v (relative to
                        // 4 V) scale the drive into the limiter — a hotter amp input
                        // means a louder average with more limiter work, which is
                        // exactly what a real gain knob does before clipping. Clamped
                        // so a wild config can't fry the output.
                        let amp_active = amp_state::is_receiving() || amp_state::overrides_active();
                        let amp_drive: f32 = if amp_active {
                            let gain_db = amp_state::get_f64_eff("gain_db", 24.0);
                            let preout_v = amp_state::get_f64_eff("preout_v", 4.0).max(0.5);
                            let drive_db =
                                (gain_db - 24.0) + 20.0 * (preout_v / 4.0).log10();
                            10f32.powf(drive_db as f32 / 20.0).clamp(0.25, 2.5)
                        } else {
                            1.0
                        };

                        let left_mult = db_to_linear(left_gain) * INPUT_TRIM * amp_drive;
                        let right_mult = db_to_linear(right_gain) * INPUT_TRIM * amp_drive;
                        let volume_mult = if is_muted { 0.0 } else { db_to_linear(eq_gains[15]) };

                        if eq_gains != last_eq_gains {
                            eq_bank.update_gains(&eq_gains, sample_rate);
                            last_eq_gains = eq_gains;
                        }

                        // Amplifier bass boost (amp_state telemetry): applies in
                        // both the flat-EQ path (inside process_sample) and the 3D
                        // path (process_boost). Smoothed internally; 0 dB when the
                        // mod is not connected.
                        if amp_state::is_receiving() || amp_state::overrides_active() {
                            let boost_db =
                                amp_state::get_f64_eff("bass_boost_db", 0.0).clamp(0.0, 18.0) as f32;
                            let boost_freq = amp_state::get_f64_eff("bass_boost_freq_hz", 45.0)
                                .clamp(25.0, 80.0) as f32;
                            eq_bank.update_amp_boost(boost_db, boost_freq, sample_rate);
                        } else {
                            eq_bank.update_amp_boost(0.0, 45.0, sample_rate);
                        }

                        let is_3d_audio = state_3d_clone.is_enabled() || state_3d_clone.is_udp_audio_enabled();
                        if is_3d_audio {
                            let (_, distances) = state_3d_clone.get_speaker_positions_and_distances();
                            let distances_f32: [f32; 6] = [
                                distances[0] as f32, distances[1] as f32, distances[2] as f32,
                                distances[3] as f32, distances[4] as f32, distances[5] as f32,
                            ];
                            state_3d_clone.update_volume(eq_gains[15]);
                            spatial_processor.update_delays(&distances_f32, sample_rate, state_3d_clone.is_itd_enabled());
                            let muffle_factor = state_3d_clone.get_smoothed_muffle_factor();
                            if should_log() {
                                let _ = app_handle.emit("rust_log", format!(
                                    "[Muffle] factor={:.3} muffled={} interior={} bbccam='{}'",
                                    muffle_factor,
                                    state_3d_clone.is_muffled(),
                                    state_3d_clone.is_interior_camera(),
                                    state_3d_clone.bbccam_mode.try_lock().map(|g| g.clone()).unwrap_or_default()
                                ));
                            }
                            spatial_processor.update_gains_and_eq(&state_3d_clone, &distances_f32, &eq_gains, sample_rate, muffle_factor);
                            spatial_processor.begin_block(data.len() / channels);
                        }

                                                static CALLBACK_COUNT: AtomicU32 = AtomicU32::new(0);
                        let count = CALLBACK_COUNT.fetch_add(1, Ordering::Relaxed);
                        if should_log() && count % 100 == 0 {
                            log_throttled(&format!(
                                "Audio callback #{} - running={} 3d={}",
                                count, state_clone.running.load(Ordering::Relaxed), is_3d_audio
                            ));
                        }

                        // App-side power/thermal fallback model: our own sub
                        // band is the demand source (the mod's electrics loop
                        // may be dead when WS push is off), so amp/coil heat
                        // and power compression develop regardless.
                        let sub_demand = get_visualizer_bands().get_sub().clamp(0.0, 1.0) as f64;
                        let supply_for_model = amp_state::get_f64_eff("supply_volts", 14.4);
                        let block_dt = data.len() as f64 / channels as f64 / sample_rate as f64;
                        amp_state::tick_power_model(sub_demand, supply_for_model, block_dt);

                        // Fix #6: Use try_lock for FFT buffer
                        let mut fft_buf = match fft_buffer_clone.try_lock() {
                            Ok(guard) => guard,
                            Err(_) => return,  // If lock is poisoned, skip this frame
                        };
                        let mut max_sample = 0.0f32;

                        let mut local_passthrough = Vec::with_capacity(data.len() / channels);

                        for i in (0..data.len()).step_by(channels) {
                            let mut left = data[i] * left_mult;
                            let mut right = if channels > 1 && i + 1 < data.len() {
                                data[i + 1] * right_mult
                            } else {
                                left
                            };

                            if !is_muted {
                                // In 3D mode, skip the main EQ bank — each speaker's individual EQ handles
                                // frequency shaping instead. Applying both causes compounding gain at high settings.
                                let (eq_l, eq_r) = if is_3d_audio {
                                    (left, right) // Pass through unmodified; speaker_eq handles it
                                } else {
                                    eq_bank.process_sample(left, right)
                                };
                                left = eq_l;
                                right = eq_r;
                            }

                            // Capture pre-3D audio for FFT/visualizer (so visualizer doesn't change with 3D mode)
                            let pre_3d_left = left * volume_mult;
                            let pre_3d_right = right * volume_mult;

                            // IMPORTANT: For the 3D path, feed the spatial processor the raw
                            // (pre-volume) signal and apply volume_mult AFTER.  If volume is
                            // applied first, a loud signal saturates the summed speaker outputs
                            // through soft_clip/hard_limit, making all speakers sound identical
                            // regardless of head position.  Applying volume last preserves the
                            // relative per-speaker gain differences at any volume level.
                            // Speaker summation is now bounded inside
                            // SpatialAudioProcessor::process (dynamic headroom
                            // normalization), so no extra boost is needed here.
                            const _3D_AUDIO_OUTPUT_GAIN: f32 = 1.0;

                            let (gain_applied_left, gain_applied_right) = if is_3d_audio && !is_muted {
                                let (sp_l, sp_r) = spatial_processor.process(left, right, &state_3d_clone);
                                (sp_l * _3D_AUDIO_OUTPUT_GAIN * volume_mult,
                                 sp_r * _3D_AUDIO_OUTPUT_GAIN * volume_mult)
                            } else {
                                (left * volume_mult, right * volume_mult)
                            };

                            // Apply smooth peak limiter (gain reduction, no waveform clipping) to ALL paths including 3D audio
                            let (final_left, final_right) = if is_muted {
                                (0.0_f32, 0.0_f32)
                            } else {
                                peak_limiter.process(gain_applied_left, gain_applied_right)
                            };

                            max_sample = max_sample.max(final_left.abs()).max(final_right.abs());

                            local_passthrough.push((final_left, final_right));

                            // FFT analyzes pre-3D audio (consistent visualizer regardless of 3D mode)
                            let idx = fft_buf.idx;
                            fft_buf.data[idx] = if !is_muted { (pre_3d_left + pre_3d_right) * 0.5 } else { 0.0 };
                            fft_buf.idx = idx + 1;

                            if fft_buf.idx >= fft_size {
                                // Diagnostic: report the post-limiter peak every ~2s (only when
                                // the app's logging is enabled). Values pinned near ~0.85-0.9
                                // mean the peak limiter is engaging heavily — the pumping/zipper
                                // source at high volume.
                                let cb_max = max_sample;
                                static LOUD_LOG: AtomicU32 = AtomicU32::new(0);
                                let lc = LOUD_LOG.fetch_add(1, Ordering::Relaxed);
                                if should_log() && lc % 100 == 0 {
                                    let _ = app_handle.emit(
                                        "rust_log",
                                        format!("[Loud] post-limiter peak={:.3}", cb_max),
                                    );
                                }
                                max_sample = 0.0;

                                if !is_muted {
                                    fft_workspace.apply_window(&fft_buf.data);
                                    fft_clone.process(&mut fft_workspace.windowed);

                                    let sub = calculate_magnitude_band(&fft_workspace.windowed, sample_rate, fft_size, 20.0, 60.0);
                                    let bass = calculate_magnitude_band(&fft_workspace.windowed, sample_rate, fft_size, 60.0, 250.0);
                                    let mid = calculate_magnitude_band(&fft_workspace.windowed, sample_rate, fft_size, 250.0, 4000.0);
                                    let treble = calculate_magnitude_band(&fft_workspace.windowed, sample_rate, fft_size, 4000.0, 20000.0);
                                    let current_max = sub.max(bass).max(mid).max(treble);

                                    {
                                        // Fix #6: Use try_lock for max_magnitude
                                        if let Ok(mut max_mag) = max_magnitude_clone.try_lock() {
                                            if current_max > *max_mag {
                                                *max_mag = current_max * 1.2;
                                            }
                                            if current_max < *max_mag * 0.5 {
                                                *max_mag = (*max_mag * 0.99).max(10.0);
                                            }
                                        }
                                    }

                                    let waveform_sample = fft_buf.data[fft_size - 1];
                                    // Fix #6: Use try_lock for reading max_mag
                                    let max_mag = max_magnitude_clone
                                        .try_lock()
                                        .map(|g| *g)
                                        .unwrap_or(1000.0);

                                    // Fix #4: Queue events instead of blocking emit
                                    // Log first event and then every 3 seconds to confirm emission is working
                                    static EVENT_LOG_COUNTER: AtomicU32 = AtomicU32::new(0);
                                    static LAST_LOG_TIME: AtomicU64 = AtomicU64::new(0);
                                    let event_count = EVENT_LOG_COUNTER.fetch_add(1, Ordering::Relaxed);
                                    if event_throttle.should_emit() {
                                        // Log to browser console every 3 seconds (3000ms)
                                        let now = std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_millis() as u64;
                                        let last = LAST_LOG_TIME.load(Ordering::Relaxed);
                                        let should_log = event_count < 5 || now - last >= 3000;
                                        
                                        if should_log && LAST_LOG_TIME.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
                                            let log_msg = format!("Band events #{} sub={:.3} bass={:.3} mids={:.3} treble={:.3}",
                                                event_count, sub, bass, mid, treble);
                                            let _ = app_handle.emit("rust_log", &log_msg);
                                        }
                                        let eq = get_event_queue();
                                        
                                        // Calculate normalized values for UI and HTTP server
                                        // These match what the visualizer UI shows (-1 to 1 range with waveform)
                                        let sub_norm = waveform_sample * (sub / max_mag).min(1.0);
                                        let bass_norm = waveform_sample * (bass / max_mag).min(1.0);
                                        let mid_norm = waveform_sample * (mid / max_mag).min(1.0);
                                        let treble_norm = waveform_sample * (treble / max_mag).min(1.0);
                                        
                                        emit_band_event(&eq, "band_subwoofer", sub_norm);
                                        emit_band_event(&eq, "band_bass", bass_norm);
                                        emit_band_event(&eq, "band_mids", mid_norm);
                                        emit_band_event(&eq, "band_treble", treble_norm);

                                        // Update HTTP visualizer bands for BeamNG Lua script
                                        // Send the same normalized values that the UI uses
                                        // Smoothing factor: 1.0 = no smoothing (instant response), 0.3 = smooth
                                        let bands = get_visualizer_bands();
                                        bands.set(sub_norm, bass_norm, mid_norm, treble_norm, 1.0);
                                    }
                                }

                                fft_buf.idx = 0;
                            }
                        }
                        passthrough_buffer.extend(local_passthrough);
                    },
                    err_fn,
                    None,
                ) {
                    Ok(stream) => {
                        log_throttled("=== Audio Stream Started Successfully ===");
                        STREAM_COUNT.fetch_add(1, Ordering::Relaxed);

                        if stream.play().is_err() {
                            log_throttled("Failed to play input stream");
                        }

                        let _input_stream_guard = stream;

                        // Build output passthrough stream
                        let output_stream_guard = if let Some(out_device) = output_device {
                            if let Ok(out_cfg) = out_device.default_output_config() {
                                match out_cfg.sample_format() {
                                    cpal::SampleFormat::F32 => {
                                        let passthrough_buffer_out = Arc::clone(&passthrough_buffer_clone);
                                        let out_channels = out_cfg.channels() as usize;
                                        let out_sample_rate = out_cfg.sample_rate().0;

                                        // FIX: If input and output devices run at different sample rates,
                                        // audio pushed into passthrough_buffer at input_rate would be consumed
                                        // at out_sample_rate â€” causing pitch shift and buffer starvation (scratchy audio).
                                        // Resample stereo passthrough from device_sample_rate -> out_sample_rate.
                                        let passthrough_resampler: Option<Arc<Mutex<rubato::SincFixedIn<f32>>>> =
                                            if device_sample_rate_u32 != out_sample_rate {
                                                let ratio = out_sample_rate as f64 / device_sample_rate_u32 as f64;
                                                let params = rubato::SincInterpolationParameters {
                                                    sinc_len: 128,
                                                    f_cutoff: 0.95,
                                                    interpolation: rubato::SincInterpolationType::Cubic,
                                                    oversampling_factor: 128,
                                                    window: rubato::WindowFunction::BlackmanHarris2,
                                                };
                                                let frame_size = (device_sample_rate_u32 as f64 * 0.01).ceil() as usize;
                                                match rubato::SincFixedIn::<f32>::new(ratio, 2.0, params, frame_size, 2) {
                                                    Ok(r) => {
                                                        eprintln!("[Passthrough] Resampling {}Hz -> {}Hz for output device", device_sample_rate_u32, out_sample_rate);
                                                        Some(Arc::new(Mutex::new(r)))
                                                    }
                                                    Err(e) => {
                                                        eprintln!("[Passthrough] Failed to create resampler: {} â€” audio may be pitched/scratchy", e);
                                                        None
                                                    }
                                                }
                                            } else {
                                                eprintln!("[Passthrough] Input/output sample rates match ({}Hz), no resampling needed", device_sample_rate_u32);
                                                None
                                            };

                                        // Accumulator holds raw stereo frames drained from passthrough_buffer,
                                        // waiting to be fed into the resampler in exact frame_size chunks.
                                        let passthrough_acc: Arc<Mutex<(Vec<f32>, Vec<f32>)>> =
                                            Arc::new(Mutex::new((Vec::new(), Vec::new())));
                                        // Resampled output ready to be written to the output callback.
                                        let passthrough_resampled: Arc<Mutex<(Vec<f32>, Vec<f32>)>> =
                                            Arc::new(Mutex::new((Vec::new(), Vec::new())));

                                        // Background thread: drain passthrough_buffer -> resample -> passthrough_resampled
                                        // Runs at ~1ms intervals so the output callback always finds fresh samples.
                                        if let Some(ref rs) = passthrough_resampler {
                                            let rs_bg = Arc::clone(rs);
                                            let acc_bg = Arc::clone(&passthrough_acc);
                                            let out_bg = Arc::clone(&passthrough_resampled);
                                            let pb_bg = Arc::clone(&passthrough_buffer_out);
                                            let in_rate = device_sample_rate_u32;
                                            std::thread::spawn(move || {
                                                let _ = set_current_thread_priority(ThreadPriority::Max);
                                                let frame_size = (in_rate as f64 * 0.01).ceil() as usize;
                                                while STREAM_COUNT.load(Ordering::Relaxed) > 0 {
                                                    // Drain available frames from ring buffer into accumulator
                                                    {
                                                        let mut acc = acc_bg.lock().unwrap();
                                                        for (l, r) in pb_bg.drain() {
                                                            acc.0.push(l);
                                                            acc.1.push(r);
                                                        }
                                                        if acc.0.len() > 16384 {
                                                            let excess = acc.0.len() - 16384;
                                                            acc.0.drain(..excess);
                                                            acc.1.drain(..excess);
                                                        }
                                                    }
                                                    // Resample full chunks
                                                    loop {
                                                        let chunk: Option<(Vec<f32>, Vec<f32>)> = {
                                                            let mut acc = acc_bg.lock().unwrap();
                                                            if acc.0.len() >= frame_size {
                                                                Some((
                                                                    acc.0.drain(..frame_size).collect(),
                                                                    acc.1.drain(..frame_size).collect(),
                                                                ))
                                                            } else {
                                                                None
                                                            }
                                                        };
                                                        match chunk {
                                                            None => break,
                                                            Some((lch, rch)) => {
                                                                if let Ok(mut r) = rs_bg.lock() {
                                                                    if let Ok(vecs) = r.process(&[lch, rch], None) {
                                                                        let mut out = out_bg.lock().unwrap();
                                                                        if let Some(lout) = vecs.get(0) { out.0.extend(lout); }
                                                                        if let Some(rout) = vecs.get(1) { out.1.extend(rout); }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                    std::thread::sleep(std::time::Duration::from_micros(500));
                                                }
                                            });
                                        }

                                        let passthrough_resampler_cb = passthrough_resampler.clone();
                                        let _passthrough_acc_cb = Arc::clone(&passthrough_acc);
                                        let passthrough_resampled_cb = Arc::clone(&passthrough_resampled);

                                        // Output declicker: shared, per-channel previous sample.
                                        let declick_out = Arc::new(Mutex::new(Declicker::new()));
                                        let declick_out_cb = Arc::clone(&declick_out);

                                        match out_device.build_output_stream(
                                            &out_cfg.into(),
                                            move |out_data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                                                // Step 1: Zero the output buffer first
                                                for sample in out_data.iter_mut() {
                                                    *sample = 0.0;
                                                }

                                                // Step 2: Add passthrough (car/music audio)
                                                // If resampling is needed, pull from the pre-resampled buffer;
                                                // otherwise pop directly from passthrough_buffer_out.
                                                let frames = out_data.len() / out_channels;
                                                {
                                                    let mut dc = declick_out_cb.try_lock().ok();
                                                    if passthrough_resampler_cb.is_some() {
                                                        let mut held_l: f32 = 0.0;
                                                        let mut held_r: f32 = 0.0;
                                                        if let Ok(mut rs_out) = passthrough_resampled_cb.try_lock() {
                                                            // Sample-hold underrun: when the resampler thread hasn't produced enough
                                                            // yet (output clock ahead of the input clock), hold the last real sample
                                                            // instead of writing zeros. A zero gap is a hard discontinuity (zipper
                                                            // click); holding the adjacent sample is a much smaller discontinuity
                                                            // and is inaudible for the tiny gaps clock drift causes.
                                                            if !rs_out.0.is_empty() { held_l = rs_out.0[rs_out.0.len()-1]; }
                                                            if !rs_out.1.is_empty() { held_r = rs_out.1[rs_out.1.len()-1]; }
                                                            for i in 0..frames {
                                                                let left  = if i < rs_out.0.len() { rs_out.0[i] } else { held_l };
                                                                let right = if i < rs_out.1.len() { rs_out.1[i] } else { held_r };
                                                                if i < rs_out.0.len() { held_l = left; }
                                                                if i < rs_out.1.len() { held_r = right; }
                                                                // Slew-limit so no discontinuity reaches the driver.
                                                                let (dl, dr) = if let Some(ref mut d) = dc {
                                                                    (d.step_l(left), d.step_r(right))
                                                                } else {
                                                                    (left, right)
                                                                };
                                                                let base = i * out_channels;
                                                                out_data[base] = soft_limit(dl, 0.85, 0.95);
                                                                if out_channels > 1 { out_data[base + 1] = soft_limit(dr, 0.85, 0.95); }
                                                            }
                                                            let consumed = frames.min(rs_out.0.len());
                                                            rs_out.0.drain(..consumed);
                                                            let consumed_r = consumed.min(rs_out.1.len());
                                                            rs_out.1.drain(..consumed_r);
                                                            if rs_out.0.len() > 16384 {
                                                                let excess = rs_out.0.len() - 16384;
                                                                rs_out.0.drain(..excess);
                                                                rs_out.1.drain(..excess);
                                                            }
                                                        }
                                                    } else {
                                                        let frames = out_data.len() / out_channels;
                                                        let mut popped = passthrough_buffer_out.pop_many(frames);
                                                        // Sample-hold the last real sample when the buffer underruns (input and
                                                        // output device clocks drift apart). Zeros would hard-jump to silence and
                                                        // click; holding the adjacent sample is far less audible for short gaps.
                                                        let hold = popped.last().copied().unwrap_or((0.0_f32, 0.0_f32));
                                                        popped.resize(frames, hold);

                                                        for (i, frame) in out_data.chunks_mut(out_channels).enumerate() {
                                                            let (left, right) = popped[i];
                                                            let (dl, dr) = if let Some(ref mut d) = dc {
                                                                (d.step_l(left), d.step_r(right))
                                                            } else {
                                                                (left, right)
                                                            };
                                                            frame[0] = soft_limit(frame[0] + dl, 0.85, 0.95);
                                                            if out_channels > 1 {
                                                                frame[1] = soft_limit(frame[1] + dr, 0.85, 0.95);
                                                            }
                                                        }
                                                    }
                                                }
                                            },
                                            err_fn,
                                            None,
                                        ) {
                                            Ok(s) => {
                                                if s.play().is_ok() {
                                                    Some(s)
                                                } else {
                                                    None
                                                }
                                            }
                                            Err(e) => {
                                                eprintln!("Failed to build output stream: {}", e);
                                                None
                                            }
                                        }
                                    }
                                    _ => {
                                        eprintln!("Unsupported output sample format");
                                        None
                                    }
                                }
                            } else {
                                None
                            }
                        } else {
                            eprintln!("No output device available");
                            None
                        };

                        let _output_guard = output_stream_guard;

                        while state.running.load(Ordering::Relaxed) {
                            thread::sleep(std::time::Duration::from_millis(100));
                        }

                        STREAM_COUNT.fetch_sub(1, Ordering::Relaxed);
                    }
                    Err(e) => {
                        eprintln!("Failed to build audio stream: {}", e);
                    }
                }
            }

            SampleFormat::I16 => {
                let state_clone = Arc::clone(&state);
                let _app_handle_clone = app_handle.clone();  // Kept for future use
                let fft_buffer_clone = Arc::clone(&fft_buffer);
                let max_magnitude_clone = Arc::clone(&max_magnitude);
                let channel_gains = Arc::clone(&state.channel_gains);
                let fft_clone = Arc::clone(&fft);
                let mut fft_workspace = FftWorkspace::new(fft_size);
                let mut event_throttle = EventThrottle::new(60);  // 60Hz for smoother visualizer

                match device.build_input_stream(
                    &config.into(),
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        if !state_clone.running.load(Ordering::Relaxed) { return; }
                        
                        // Fix #6: Use try_lock to avoid blocking in I16 callback
                        let (left_gain, right_gain) = channel_gains
                            .try_lock()
                            .map(|g| *g)
                            .unwrap_or((0.0, 0.0));
                        let left_mult = db_to_linear(left_gain);
                        let right_mult = db_to_linear(right_gain);
                        
                        let mut fft_buf = match fft_buffer_clone.try_lock() {
                            Ok(guard) => guard,
                            Err(_) => return,
                        };
                        
                        let mut max_mag = match max_magnitude_clone.try_lock() {
                            Ok(guard) => guard,
                            Err(_) => return,
                        };

                        for i in (0..data.len()).step_by(channels) {
                            let left = (data[i] as f32 / i16::MAX as f32) * left_mult;
                            let right = if channels > 1 && i + 1 < data.len() {
                                (data[i + 1] as f32 / i16::MAX as f32) * right_mult
                            } else {
                                left
                            };
                            let idx = fft_buf.idx;
                            fft_buf.data[idx] = (left + right) * 0.5;
                            fft_buf.idx = idx + 1;

                            if fft_buf.idx >= fft_size {
                                fft_workspace.apply_window(&fft_buf.data);
                                fft_clone.process(&mut fft_workspace.windowed);
                                let sub = calculate_magnitude_band(&fft_workspace.windowed, sample_rate, fft_size, 20.0, 60.0);
                                let bass = calculate_magnitude_band(&fft_workspace.windowed, sample_rate, fft_size, 60.0, 250.0);
                                let mid = calculate_magnitude_band(&fft_workspace.windowed, sample_rate, fft_size, 250.0, 4000.0);
                                let treble = calculate_magnitude_band(&fft_workspace.windowed, sample_rate, fft_size, 4000.0, 20000.0);
                                let current_max = sub.max(bass).max(mid).max(treble);
                                if current_max > *max_mag { *max_mag = current_max * 1.2; }
                                if current_max < *max_mag * 0.5 { *max_mag = (*max_mag * 0.99).max(10.0); }
                                let waveform_sample = fft_buf.data[fft_size - 1];
                                // Fix #4: Queue events instead of blocking emit
                                if event_throttle.should_emit() {
                                    let eq = get_event_queue();
                                    emit_band_event(&eq, "band_subwoofer", waveform_sample * (sub / *max_mag).min(1.0));
                                    emit_band_event(&eq, "band_bass", waveform_sample * (bass / *max_mag).min(1.0));
                                    emit_band_event(&eq, "band_mids", waveform_sample * (mid / *max_mag).min(1.0));
                                    emit_band_event(&eq, "band_treble", waveform_sample * (treble / *max_mag).min(1.0));
                                }
                                fft_buf.idx = 0;
                            }
                        }
                    },
                    err_fn,
                    None,
                ) {
                    Ok(stream) => {
                        STREAM_COUNT.fetch_add(1, Ordering::Relaxed);
                        if stream.play().is_err() { eprintln!("Failed to play I16 stream"); }
                        let _stream_guard = stream;
                        while state.running.load(Ordering::Relaxed) {
                            thread::sleep(std::time::Duration::from_millis(100));
                        }
                        STREAM_COUNT.fetch_sub(1, Ordering::Relaxed);
                    }
                    Err(e) => eprintln!("Failed to build I16 stream: {}", e),
                }
            }

            _ => eprintln!("Unsupported sample format"),
        }
    });
}

// ---------------------------------------------------------------------------
// Output-stage "declicker": a sample-domain slew limiter that is completely
// transparent for real audio but cannot reproduce discontinuities.
//
// Rationale: at 48 kHz, continuous audio changes by only a few hundredths of a
// full-scale step per sample (a 0.9-amplitude 500 Hz sine moves ~0.06/sample,
// bass far less). Clicks/pops/zippers, whatever their source (a DSP parameter
// jump, a filter rebuild, comb sweep, a buffer gap), are near-instant jumps —
// usually an order of magnitude larger between two adjacent samples. Clamping
// each sample's delta to MAX_SLEW lets normal music pass through essentially
// sample-identical, while any step discontinuity is forced to glide across a
// handful of samples (<<1 ms), removing it from audibility.
// ---------------------------------------------------------------------------
struct Declicker {
    prev_l: f32,
    prev_r: f32,
    // Previous absolute slope per channel: a continuous waveform has mostly
    // consistent sample-to-sample deltas, while a pop/click is a sudden step
    // far larger than its neighbors. Tracking the slope lets the slew cap be
    // transparent for fast (treble) continuous audio but catch discontinuities.
    slope_l: f32,
    slope_r: f32,
}

impl Declicker {
    // Minimum per-sample step (never lower): catches isolated jumps.
    const MAX_SLEW_BASE: f32 = 0.03;

    fn new() -> Self {
        Self {
            prev_l: 0.0,
            prev_r: 0.0,
            slope_l: 0.0,
            slope_r: 0.0,
        }
    }

    #[inline]
    fn step_l(&mut self, s: f32) -> f32 {
        let raw = s - self.prev_l;
        // Allow any step up to 3x the recent slope, so genuine musical
        // transients (kick attacks, cymbal hits) whose slope is climbing
        // naturally pass through UNTOUCHED. Only an isolated discontinuity far
        // above the running slope (a pop/click, which has no preceding ramp)
        // gets clamped to the slope, spreading it over a couple of samples.
        let allowed = Self::MAX_SLEW_BASE.max(self.slope_l * 3.0);
        let step = raw.clamp(-allowed, allowed);
        self.prev_l += step;
        // Smooth the slope estimate so single-sample jitters can't ratchet it up.
        self.slope_l = self.slope_l * 0.85 + step.abs() * 0.15;
        self.prev_l
    }

    #[inline]
    fn step_r(&mut self, s: f32) -> f32 {
        let raw = s - self.prev_r;
        let allowed = Self::MAX_SLEW_BASE.max(self.slope_r * 3.0);
        let step = raw.clamp(-allowed, allowed);
        self.prev_r += step;
        self.slope_r = self.slope_r * 0.85 + step.abs() * 0.15;
        self.prev_r
    }
}

// ---------------------------------------------------------------------------
// App entry point
// ---------------------------------------------------------------------------

use crate::media_info::{get_media_info, get_media_info_fresh};

/// Refuse a second app instance: a duplicate would spawn a second Opera,
/// fight over the DevTools port, and look like phantom windows plus flaky
/// input. Brings the running instance forward instead. Windows-only.
#[cfg(windows)]
fn ensure_single_instance() {
    use ::windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use ::windows::Win32::System::Threading::CreateMutexW;
    use ::windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow};
    use ::windows::core::PCWSTR;
    unsafe {
        let name: Vec<u16> = "Local\\BrokenBrainCellsBeamNGDSP\0"
            .encode_utf16()
            .collect();
        // Never closed on purpose: the leaked handle keeps the mutex alive
        // for the process lifetime, which is exactly the guard we want
        // (windows HANDLE has no Drop that would close it early).
        let _ = CreateMutexW(None, false, PCWSTR(name.as_ptr()));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let title: Vec<u16> = "BrokenBrainCells BeamNG DSP And Audio Suite\0"
                .encode_utf16()
                .collect();
            if let Ok(other) = FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) {
                let _ = SetForegroundWindow(other);
            }
            std::process::exit(0);
        }
    }
}

#[cfg(not(windows))]
fn ensure_single_instance() {}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    ensure_single_instance();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AudioState::new())
        .manage(SpatialAudioState::new())
        .manage(electrics::ElectricsState::new())
        .manage(embed::EmbedState(Mutex::new(embed::EmbedStateInner::default())))
        .manage(stream::StreamState::new())
        .invoke_handler(tauri::generate_handler![
            audio_capture::get_audio_input_devices,
            audio_capture::get_audio_output_devices,
            audio_capture::start_audio_capture,
            audio_capture::stop_audio_capture,
            audio_capture::set_eq_bands,
            audio_capture::set_channel_gains,
            media_info::get_media_info,
            media_info::get_media_info_fresh,
            media_info::media_play_pause,
            media_info::media_next,
            media_info::media_previous,
            beamng_udp::get_latest_camera_data_command,
            beamng_udp::get_udp_status,
            // Electrics UDP commands
            electrics::get_electrics_udp_status,
            electrics::start_electrics_udp_listener,
            electrics::stop_electrics_udp_listener,
            electrics::is_electrics_udp_enabled,
            // Amplifier/subwoofer telemetry from the mod
            amp_state::get_amp_state,
            amp_state::get_amp_state_status,
            amp_state::set_amp_override,
            amp_state::clear_amp_override,
            amp_state::clear_amp_overrides,
            amp_state::get_amp_overrides,
            electrics::get_electrics_values,
            commands::save_app_config,
            commands::load_app_config,
            commands::save_eq_preset,
            commands::load_eq_preset,
            commands::delete_eq_preset,
            commands::get_preset_names,
            commands::set_3d_audio_enabled,
            commands::is_3d_audio_enabled,
            commands::set_3d_audio_udp_enabled,
            commands::is_3d_audio_udp_enabled,
            commands::get_3d_audio_state,
            commands::get_3d_audio_enhancements,
            commands::set_3d_audio_enhancements,
            commands::set_subwoofer_volume,
            commands::set_tweeter_volume,
            commands::set_tweeter_freq_range,
            commands::set_sub_freq_range,
            commands::set_rattle_volume,
            commands::set_rattle_threshold,
            commands::set_speaker_positions,
            commands::get_speaker_positions,
            commands::reset_speaker_positions,
            commands::set_speaker_mutes,
            commands::get_speaker_mutes,
            // BeamNG mod commands
            commands::set_beamng_mods_folder,
            commands::get_beamng_mods_folder,
            commands::check_beamng_mod_status,
            commands::open_folder_picker,
            // BeamNG mod command aliases (for JS compatibility)
            commands::browse_beamng_mods,
            commands::save_beamng_mods_folder,
            commands::check_mod_status,
            // Discord webhook commands
            discord_webhook::set_discord_webhook_url,
            discord_webhook::get_discord_webhook_url,
            discord_webhook::upload_preset_to_discord,
            // Session Stats commands
            session_stats::get_session_stats,
            session_stats::get_lifetime_stats,
            session_stats::reset_lifetime_stats,
            session_stats::get_session_log_files,
            session_stats::read_session_log,
            session_stats::export_session_log,
            // Embedded Opera (Win32 SetParent) commands
            commands::get_exe_icon,
            embed::find_opera_exe,
            embed::launch_opera_embed,
            embed::launch_embedded_app,
            embed::embed_launched_app,
            embed::resize_embedded_app,
            embed::close_embedded_app,
            embed::focus_embedded_app,
            // Embedded Opera streamer (local:// disk snapshots, Opera box only)
            stream::start_opera_stream,
            stream::stop_opera_stream,
            stream::stream_status,
            stream::preview_frame_b64,
            // MCP (forward inputs to another BeamNG instance)
            mcp::mcp_configure,
            mcp::mcp_inject_input,
            mcp::mcp_status,
            mcp::mcp_test_connection,
            // Input-path health snapshot (packets, CDP vs fallback, drops)
            commands::input_status,
            // Visualizer transport toggle (HTTP polling vs WebSocket push)
            visualizer::visualizer_set_ws_mode,
        ])
        .setup(|app| {
            eprintln!("=== Audio Visualization App Started ===");

            // Start HTTP visualizer server for BeamNG Lua script
            visualizer::start_visualizer_http_server();

            // Start WebSocket visualizer server (push mode, toggleable from UI)
            visualizer::start_visualizer_ws_server(app.app_handle().clone());

            // Start media info polling thread to update HTTP server with song metadata
            let app_handle = app.app_handle().clone();
            let bands = visualizer::get_visualizer_bands().clone();
            std::thread::spawn(move || {
                eprintln!("[MediaInfo] Polling thread started");
                // One runtime for the thread lifetime, not one per poll.
                let rt = tokio::runtime::Runtime::new().unwrap();
                let mut last_poll = std::time::Instant::now();
                let mut cached_info: Option<MediaInfo> = None;
                let mut last_known_time = 0u64;
                let mut last_known_playing = false;
                let mut last_update = std::time::Instant::now();
                
                loop {
                    // Poll Windows SMTC every 200ms for metadata changes (bypass cache for real-time detection)
                    let now = std::time::Instant::now();
                    if now.duration_since(last_poll) >= std::time::Duration::from_millis(200) {
                        last_poll = now;
                        if let Ok(info) = rt.block_on(get_media_info_fresh()) {
                            // Detect play/pause/seek changes
                            let is_playing = info.is_playing;
                            let current_time = info.current_time;
                            
                            // If playing, track elapsed time locally for smooth progress
                            if is_playing && last_known_playing && cached_info.is_some() {
                                let elapsed = last_update.elapsed().as_secs();
                                let interpolated_time = last_known_time.saturating_add(elapsed).min(info.total_time);
                                let interpolated_time = interpolated_time;
                                if interpolated_time != current_time {
                                    let mut updated = info.clone();
                                    updated.current_time = interpolated_time;
                                    bands.update_media_info(updated);
                                    last_known_time = interpolated_time;
                                    last_update = std::time::Instant::now();
                                    continue;
                                }
                            }
                            
                            // New metadata received - update cache
                            if info.title != cached_info.as_ref().map(|c| c.title.clone()).unwrap_or_default()
                                || info.artist != cached_info.as_ref().map(|c| c.artist.clone()).unwrap_or_default()
                                || info.total_time != cached_info.as_ref().map(|c| c.total_time).unwrap_or(0)
                                || info.current_time != cached_info.as_ref().map(|c| c.current_time).unwrap_or(0)
                                || is_playing != last_known_playing
                            {
                                bands.update_media_info(info.clone());
                                
                                // Emit to frontend for real-time now-playing updates
                                let _ = app_handle.emit("media_info", &info);
                                
                                cached_info = Some(info.clone());
                                last_known_time = info.current_time;
                                last_known_playing = is_playing;
                                last_update = std::time::Instant::now();
                            }
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            });

            // Media command processor thread (handles play/pause, next, previous from BeamNG)
            let app_handle_for_media = app.app_handle().clone();
            std::thread::spawn(move || {
                eprintln!("[MediaCommand] Processor thread started");
                // One runtime for the thread lifetime, not one per command.
                let rt = tokio::runtime::Runtime::new().unwrap();
                loop {
                    if let Some(cmd) = visualizer::try_recv_media_command() {
                        let is_skip = matches!(cmd, visualizer::MediaCommand::Next | visualizer::MediaCommand::Previous);
                        match cmd {
                            visualizer::MediaCommand::PlayPause => {
                                let _ = rt.block_on(media_info::media_play_pause());
                            }
                            visualizer::MediaCommand::Next => {
                                let _ = rt.block_on(media_info::media_next());
                            }
                            visualizer::MediaCommand::Previous => {
                                let _ = rt.block_on(media_info::media_previous());
                            }
                        }
                        // Force immediate media info refresh after skip commands
                        if is_skip {
                            std::thread::sleep(std::time::Duration::from_millis(300)); // Wait for SMTC to update
                            if let Ok(info) = rt.block_on(media_info::get_media_info_fresh()) {
                                let _ = app_handle_for_media.emit("media_info", &info);
                            }
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            });

            // Initialize session stats state
            let session_stats_state = session_stats::SessionStatsState::new(app.app_handle());
            app.manage(session_stats_state);

            // Fix #4: Start event queue processor thread
            let app_handle = app.app_handle().clone();
            std::thread::spawn(move || {
                let _ = app_handle.emit("rust_log", "Event queue drain thread started");
                eprintln!("Event queue drain thread started");
                let mut event_counter = 0u32;
                let mut last_log_time = 0u64;
                loop {
                    let events = get_event_queue().drain();
                    if !events.is_empty() {
                        event_counter += events.len() as u32;
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        if event_counter <= 10 || now - last_log_time >= 3000 {
                            let log_msg = format!("Drained {} events (total={})", events.len(), event_counter);
                            let _ = app_handle.emit("rust_log", &log_msg);
                            eprintln!("{}", log_msg);
                            last_log_time = now;
                        }
                        for (event_name, value) in events {
                            let _ = app_handle.emit(&event_name, &value);
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(16));
                }
            });

            let initial_config = load_config(app.app_handle());
            app.manage(ConfigState::new(initial_config));

            // Start UDP listener in setup so it runs before window loads
            eprintln!("=== Setup: Starting UDP listener ===");
            let _ = app.emit("rust_log", "=== App starting up - UDP listener starting ===");
            beamng_udp::start_beamng_camera_udp_listener(app.app_handle().clone());

            // Start media command UDP listener (port 5001)
            beamng_udp::start_media_command_udp_listener(app.app_handle().clone());

            // CDP delivery worker: scroll jobs run here, never on the input
            // fast path (a wedged DevTools endpoint must not stall input).
            cdp::start_worker(app.app_handle().clone());

            // Popup warden (stray Opera windows) + foreground journal (what
            // opened over the game?) + 30s input health summary.
            embed::start_watchdog(app.app_handle().clone());

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                if let Some(state) = window.try_state::<session_stats::SessionStatsState>() {
                    session_stats::finalize_high_rpm_tracking(&state);
                    session_stats::end_session(&state);
                }
            }
        })
        .on_page_load(|window, _payload| {
            eprintln!("=== on_page_load triggered ===");
            let _ = window.emit("rust_log", "Page loaded - UDP listener should be running");
            
            let window_clone = window.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let _ = window_clone.emit("rust_log", "UDP listener status check - if you see this, events are working");
            });
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}