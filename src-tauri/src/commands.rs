use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{AppHandle, State};

use crate::beamng_udp::start_beamng_udp_listener;
use crate::config::{AppConfig, ConfigManager, EqPreset};
use crate::spatial_audio::{SpeakerMutes, SpeakerOffsets, SpatialAudioState};

// ---------------------------------------------------------------------------
// 3D Audio commands
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SpeakerPositionConfig {
    fl_x: f32, fl_y: f32, fl_z: f32,
    fr_x: f32, fr_y: f32, fr_z: f32,
    rl_x: f32, rl_y: f32, rl_z: f32,
    rr_x: f32, rr_y: f32, rr_z: f32,
    sub_l_x: f32, sub_l_y: f32, sub_l_z: f32,
    sub_r_x: f32, sub_r_y: f32, sub_r_z: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SpeakerMutesConfig {
    fl_muted: bool,
    fr_muted: bool,
    rl_muted: bool,
    rr_muted: bool,
    sub_l_muted: bool,
    sub_r_muted: bool,
}

#[tauri::command]
pub fn set_3d_audio_enabled(
    enabled: bool,
    state_3d: State<SpatialAudioState>,
    app_handle: AppHandle,
) -> Result<(), String> {
    if enabled && !state_3d.is_enabled() {
        let state_3d_arc = Arc::new((*state_3d.inner()).clone());
        start_beamng_udp_listener(app_handle, state_3d_arc);
    }
    state_3d.set_enabled(enabled);
    Ok(())
}

#[tauri::command]
pub fn is_3d_audio_enabled(state_3d: State<SpatialAudioState>) -> bool {
    state_3d.is_enabled()
}

#[tauri::command]
pub fn set_3d_audio_udp_enabled(
    enabled: bool,
    state_3d: State<SpatialAudioState>,
    app_handle: AppHandle,
) -> Result<(), String> {
    if enabled && !state_3d.is_udp_audio_enabled() {
        if !state_3d.is_enabled() {
            let state_3d_arc = Arc::new((*state_3d.inner()).clone());
            start_beamng_udp_listener(app_handle, state_3d_arc);
        }
    }
    state_3d.set_udp_audio_enabled(enabled);
    Ok(())
}

#[tauri::command]
pub fn is_3d_audio_udp_enabled(state_3d: State<SpatialAudioState>) -> bool {
    state_3d.is_udp_audio_enabled()
}

#[tauri::command]
pub fn get_3d_audio_state(state_3d: State<SpatialAudioState>) -> serde_json::Value {
    let levels = state_3d.get_speaker_levels();
    let telemetry = state_3d.get_telemetry();
    serde_json::json!({
        "enabled": state_3d.is_enabled(),
        "front_left": levels.front_left, "front_right": levels.front_right,
        "rear_left": levels.rear_left, "rear_right": levels.rear_right,
        "subwoofer_left": levels.subwoofer_left, "subwoofer_right": levels.subwoofer_right,
        "tweeter_left": levels.tweeter_left, "tweeter_right": levels.tweeter_right,
        "tweeter_low_freq": state_3d.get_tweeter_low_freq(),
        "tweeter_high_freq": state_3d.get_tweeter_high_freq(),
        "sub_low_freq": state_3d.get_sub_low_freq(),
        "sub_high_freq": state_3d.get_sub_high_freq(),
        "rattle_volume": state_3d.get_rattle_volume(),
        "rattle_threshold": state_3d.get_rattle_threshold(),
        "vehicle_speed": telemetry.vehicle_speed, "engine_rpm": telemetry.engine_rpm, "throttle": telemetry.throttle,
    })
}

#[tauri::command]
pub fn get_3d_audio_enhancements(state_3d: State<SpatialAudioState>) -> serde_json::Value {
    serde_json::json!({
        "itd_enabled": state_3d.is_itd_enabled(),
        "head_shadow_enabled": state_3d.is_head_shadow_enabled(),
        "sub_crossover_enabled": state_3d.is_sub_crossover_enabled(),
        "rear_voicing_enabled": state_3d.is_rear_voicing_enabled(),
    })
}

#[tauri::command]
pub fn set_3d_audio_enhancements(
    itd_enabled: Option<bool>,
    head_shadow_enabled: Option<bool>,
    sub_crossover_enabled: Option<bool>,
    rear_voicing_enabled: Option<bool>,
    state_3d: State<SpatialAudioState>,
) -> Result<(), String> {
    if let Some(v) = itd_enabled { state_3d.set_itd_enabled(v); }
    if let Some(v) = head_shadow_enabled { state_3d.set_head_shadow_enabled(v); }
    if let Some(v) = sub_crossover_enabled { state_3d.set_sub_crossover_enabled(v); }
    if let Some(v) = rear_voicing_enabled { state_3d.set_rear_voicing_enabled(v); }
    Ok(())
}

#[tauri::command]
pub fn set_subwoofer_volume(
    volume: f32,
    state_3d: State<SpatialAudioState>,
) -> Result<(), String> {
    let mut levels = state_3d.get_speaker_levels();
    levels.subwoofer_left = volume.clamp(0.0, 1.0);
    levels.subwoofer_right = volume.clamp(0.0, 1.0);
    state_3d.update_speaker_levels(levels);
    Ok(())
}

#[tauri::command]
pub fn set_tweeter_volume(volume: f32, state_3d: State<SpatialAudioState>) -> Result<(), String> {
    let mut levels = state_3d.get_speaker_levels();
    levels.tweeter_left = volume.clamp(0.0, 1.0);
    levels.tweeter_right = volume.clamp(0.0, 1.0);
    state_3d.update_speaker_levels(levels);
    Ok(())
}

#[tauri::command]
pub fn set_tweeter_freq_range(
    low_freq: f32,
    high_freq: f32,
    state_3d: State<SpatialAudioState>,
) -> Result<(), String> {
    let low = low_freq.clamp(500.0, 20000.0);
    let mut high = high_freq.clamp(500.0, 20000.0);
    let low = low.min(high - 100.0).max(500.0);
    high = high.max(low + 100.0).min(20000.0);
    state_3d.set_tweeter_low_freq(low);
    state_3d.set_tweeter_high_freq(high);
    Ok(())
}

#[tauri::command]
pub fn set_sub_freq_range(
    low_freq: f32,
    high_freq: f32,
    state_3d: State<SpatialAudioState>,
) -> Result<(), String> {
    let low = low_freq.clamp(10.0, 250.0);
    let mut high = high_freq.clamp(10.0, 250.0);
    let low = low.min(high - 5.0).max(10.0);
    high = high.max(low + 5.0).min(250.0);
    state_3d.set_sub_low_freq(low);
    state_3d.set_sub_high_freq(high);
    Ok(())
}

#[tauri::command]
pub fn set_rattle_volume(volume: f32, state_3d: State<SpatialAudioState>) -> Result<(), String> {
    state_3d.set_rattle_volume(volume);
    Ok(())
}

#[tauri::command]
pub fn set_rattle_threshold(
    threshold: f32,
    state_3d: State<SpatialAudioState>,
) -> Result<(), String> {
    state_3d.set_rattle_threshold(threshold);
    Ok(())
}

#[tauri::command]
pub fn set_speaker_positions(config: SpeakerPositionConfig, state_3d: State<SpatialAudioState>) -> Result<(), String> {
    state_3d.update_speaker_offsets(SpeakerOffsets {
        fl: [config.fl_x, config.fl_y, config.fl_z],
        fr: [config.fr_x, config.fr_y, config.fr_z],
        rl: [config.rl_x, config.rl_y, config.rl_z],
        rr: [config.rr_x, config.rr_y, config.rr_z],
        sub_l: [config.sub_l_x, config.sub_l_y, config.sub_l_z],
        sub_r: [config.sub_r_x, config.sub_r_y, config.sub_r_z],
    });
    Ok(())
}

#[tauri::command]
pub fn get_speaker_positions(state_3d: State<SpatialAudioState>) -> SpeakerPositionConfig {
    let offsets = *state_3d.speaker_offsets.lock().unwrap();
    SpeakerPositionConfig {
        fl_x: offsets.fl[0], fl_y: offsets.fl[1], fl_z: offsets.fl[2],
        fr_x: offsets.fr[0], fr_y: offsets.fr[1], fr_z: offsets.fr[2],
        rl_x: offsets.rl[0], rl_y: offsets.rl[1], rl_z: offsets.rl[2],
        rr_x: offsets.rr[0], rr_y: offsets.rr[1], rr_z: offsets.rr[2],
        sub_l_x: offsets.sub_l[0], sub_l_y: offsets.sub_l[1], sub_l_z: offsets.sub_l[2],
        sub_r_x: offsets.sub_r[0], sub_r_y: offsets.sub_r[1], sub_r_z: offsets.sub_r[2],
    }
}

#[tauri::command]
pub fn reset_speaker_positions(state_3d: State<SpatialAudioState>) -> Result<(), String> {
    state_3d.update_speaker_offsets(SpeakerOffsets::default());
    Ok(())
}

#[tauri::command]
pub fn set_speaker_mutes(config: SpeakerMutesConfig, state_3d: State<SpatialAudioState>) -> Result<(), String> {
    state_3d.update_speaker_mutes(SpeakerMutes {
        fl_muted: config.fl_muted,
        fr_muted: config.fr_muted,
        rl_muted: config.rl_muted,
        rr_muted: config.rr_muted,
        sub_l_muted: config.sub_l_muted,
        sub_r_muted: config.sub_r_muted,
    });
    Ok(())
}

#[tauri::command]
pub fn get_speaker_mutes(state_3d: State<SpatialAudioState>) -> SpeakerMutesConfig {
    let mutes = state_3d.get_speaker_mutes();
    SpeakerMutesConfig {
        fl_muted: mutes.fl_muted,
        fr_muted: mutes.fr_muted,
        rl_muted: mutes.rl_muted,
        rr_muted: mutes.rr_muted,
        sub_l_muted: mutes.sub_l_muted,
        sub_r_muted: mutes.sub_r_muted,
    }
}

// ---------------------------------------------------------------------------
// Config & Preset commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn save_app_config(
    app_handle: AppHandle,
    input_device: Option<String>,
    output_device: Option<String>,
) -> Result<(), String> {
    let config_mgr = ConfigManager::new(&app_handle);
    config_mgr.update(|config| {
        config.last_input_device = input_device;
        config.last_output_device = output_device;
    })
}

#[tauri::command]
pub fn load_app_config(app_handle: AppHandle) -> AppConfig {
    let config_mgr = ConfigManager::new(&app_handle);
    config_mgr.with_config(|config| config.clone())
}

#[tauri::command]
pub fn save_eq_preset(
    app_handle: AppHandle,
    name: String,
    gains: Vec<f32>,
    left_gain: f32,
    right_gain: f32,
) -> Result<(), String> {
    if gains.len() != 16 {
        return Err("Invalid gains length".to_string());
    }
    let config_mgr = ConfigManager::new(&app_handle);
    config_mgr.update(|config| {
        config.presets.insert(
            name.clone(),
            EqPreset { name: name.clone(), gains: gains.try_into().unwrap(), left_gain, right_gain },
        );
    })
}

#[tauri::command]
pub fn load_eq_preset(app_handle: AppHandle, name: String) -> Result<EqPreset, String> {
    let config_mgr = ConfigManager::new(&app_handle);
    config_mgr.with_config(|config| {
        config.presets
            .get(&name)
            .cloned()
            .ok_or_else(|| format!("Preset '{}' not found", name))
    })
}

#[tauri::command]
pub fn delete_eq_preset(app_handle: AppHandle, name: String) -> Result<(), String> {
    let config_mgr = ConfigManager::new(&app_handle);
    config_mgr.update(|config| {
        config.presets.remove(&name);
    })
}

#[tauri::command]
pub fn get_preset_names(app_handle: AppHandle) -> Vec<String> {
    let config_mgr = ConfigManager::new(&app_handle);
    let mut names: Vec<String> = config_mgr.with_config(|config| {
        config.presets.keys().cloned().collect()
    });
    names.sort();
    names
}

// ---------------------------------------------------------------------------
// BeamNG mod commands
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModStatus {
    found: bool,
    is_current: bool,
    version_found: Option<String>,
    message: String,
}

#[tauri::command]
pub fn set_beamng_mods_folder(app_handle: AppHandle, folder: Option<String>) -> Result<(), String> {
    let config_mgr = ConfigManager::new(&app_handle);
    config_mgr.update(|config| {
        config.beamng_mods_folder = folder;
    })
}

#[tauri::command]
pub fn get_beamng_mods_folder(app_handle: AppHandle) -> Option<String> {
    let config_mgr = ConfigManager::new(&app_handle);
    config_mgr.with_config(|config| config.beamng_mods_folder.clone())
}

#[tauri::command]
pub fn check_beamng_mod_status(app_handle: AppHandle) -> Result<ModStatus, String> {
    let config_mgr = ConfigManager::new(&app_handle);
    let folder_path = match config_mgr.with_config(|config| config.beamng_mods_folder.clone()) {
        Some(p) => p,
        None => return Ok(ModStatus { found: false, is_current: false, version_found: None, message: "Please select your BeamNG mods folder first.".to_string() }),
    };

    let mods_root = std::path::Path::new(&folder_path);
    if !mods_root.exists() {
        return Ok(ModStatus { found: false, is_current: false, version_found: None, message: "Mods folder does not exist.".to_string() });
    }

    let found_mods: Vec<String> = walkdir::WalkDir::new(mods_root)
        .min_depth(1)
        .max_depth(10)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_prefix("bbc_stereo_"))
                .map(|v| v.to_string())
        })
        .collect();

    if found_mods.is_empty() {
        return Ok(ModStatus { found: false, is_current: false, version_found: None, message: "No bbc_stereo_ mod found.".to_string() });
    }

    let has_current = found_mods.iter().any(|v| {
        let n = v.to_lowercase();
        n == "1.1" || n.starts_with("1.1.")
    });

    if has_current {
        Ok(ModStatus { found: true, is_current: true, version_found: Some("1.1".to_string()), message: "App Updated / Mod is current.".to_string() })
    } else {
        let latest = found_mods.first().cloned().unwrap_or_default();
        Ok(ModStatus { found: true, is_current: false, version_found: Some(latest), message: "New update available.".to_string() })
    }
}

// ---------------------------------------------------------------------------
// Folder picker & aliases
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn open_folder_picker(window: tauri::Window) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = std::sync::mpsc::channel();
    window.dialog().file().pick_folder(move |folder| {
        let path = folder.and_then(|p| p.as_path().map(|p| p.to_string_lossy().to_string()));
        let _ = tx.send(path);
    });
    rx.recv().map_err(|e| e.to_string())
}

/// Alias for open_folder_picker - JS expects browse_beamng_mods
#[tauri::command]
pub async fn browse_beamng_mods(window: tauri::Window) -> Result<Option<String>, String> {
    open_folder_picker(window).await
}

/// Alias for set_beamng_mods_folder - JS expects save_beamng_mods_folder
#[tauri::command]
pub async fn save_beamng_mods_folder(window: tauri::Window, app_handle: AppHandle) -> Result<(), String> {
    if let Some(folder) = open_folder_picker(window).await? {
        set_beamng_mods_folder(app_handle, Some(folder))?;
    }
    Ok(())
}

/// Alias for check_beamng_mod_status - JS expects check_mod_status
#[tauri::command]
pub fn check_mod_status(app_handle: AppHandle) -> Result<ModStatus, String> {
    check_beamng_mod_status(app_handle)
}

/// Normalize a user-pasted exe path: trim, strip surrounding quotes, and
/// expand `%VAR%` (cmd style — the Opera box placeholder itself suggests
/// `%LOCALAPPDATA%`, so users paste that style here too).
#[cfg(windows)]
pub(crate) fn normalize_exe_path(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s = s[1..s.len() - 1].to_string();
    }
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(end) = s[i + 1..].find('%') {
                let name = &s[i + 1..i + 1 + end];
                if !name.is_empty() {
                    if let Ok(val) = std::env::var(name) {
                        out.push_str(&val);
                        i += end + 2;
                        continue;
                    }
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Extract an exe's icon as a data URL for the Extra Apps list.
///
/// Windows shell icon (large) rasterized to 32-bit BMP and base64'd.
/// Frontend caches per path and keeps the generic glyph on any failure
/// (missing file, no icon resource). BMP keeps it dependency-free; every
/// browser `<img>` renders it.
#[cfg(windows)]
#[tauri::command]
pub fn get_exe_icon(exe: String) -> Result<String, String> {
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
    use windows::Win32::UI::Shell::*;
    use windows::Win32::UI::WindowsAndMessaging::DestroyIcon;
    use windows::core::PCWSTR;

    let t = normalize_exe_path(&exe);
    if t.is_empty() {
        return Err("empty path".into());
    }
    if !std::path::Path::new(&t).exists() {
        return Err(format!("file not found: {t}"));
    }
    let wide: Vec<u16> = t.encode_utf16().chain([0]).collect();
    let mut info = SHFILEINFOW::default();
    let got = unsafe {
        SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            FILE_ATTRIBUTE_NORMAL,
            Some(&mut info as *mut _),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        )
    };
    if got == 0 {
        return Err("shell returned no icon".into());
    }
    let icon = info.hIcon;
    let bmp = unsafe { icon_to_bmp(icon) };
    unsafe {
        let _ = DestroyIcon(icon);
    }
    let bmp = bmp?;
    {
        use base64::Engine as _;
        Ok(format!(
            "data:image/bmp;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&bmp)
        ))
    }
}

#[cfg(windows)]
unsafe fn delete_icon_bitmaps(
    color: windows::Win32::Graphics::Gdi::HBITMAP,
    mask: windows::Win32::Graphics::Gdi::HBITMAP,
) {
    use windows::Win32::Graphics::Gdi::DeleteObject;
    let _ = DeleteObject(color);
    let _ = DeleteObject(mask);
}

/// Rasterize an HICON to BMP file bytes (32-bit, bottom-up). Caller destroys
/// the icon. Reads the color bitmap's real size (system large icons follow
/// DPI and are not always 32x32).
#[cfg(windows)]
unsafe fn icon_to_bmp(
    icon: windows::Win32::UI::WindowsAndMessaging::HICON,
) -> Result<Vec<u8>, String> {
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::UI::WindowsAndMessaging::{GetIconInfo, ICONINFO};

    let mut ii = ICONINFO::default();
    if GetIconInfo(icon, &mut ii as *mut _).is_err() {
        return Err("GetIconInfo failed".into());
    }
    // hbmColor can be null for monochrome icons; mask-only path is skipped —
    // shell large icons for exes are color in practice.
    if ii.hbmColor.is_invalid() {
        unsafe { delete_icon_bitmaps(ii.hbmColor, ii.hbmMask) };
        return Err("monochrome icon".into());
    }
    let mut bm = BITMAP::default();
    let got = GetObjectW(
        ii.hbmColor,
        std::mem::size_of::<BITMAP>() as i32,
        Some(&mut bm as *mut _ as *mut _),
    );
    if got == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
        unsafe { delete_icon_bitmaps(ii.hbmColor, ii.hbmMask) };
        return Err("bad bitmap size".into());
    }
    let (w, h) = (bm.bmWidth as u32, bm.bmHeight as u32);
    if w == 0 || h == 0 || w > 256 || h > 256 {
        unsafe { delete_icon_bitmaps(ii.hbmColor, ii.hbmMask) };
        return Err("bad icon size".into());
    }
    let mut bmi = BITMAPINFO::default();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w as i32;
    bmi.bmiHeader.biHeight = -(h as i32); // top-down DIB
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB.0;
    let mut px = vec![0u8; (w as usize) * (h as usize) * 4];
    let memdc = CreateCompatibleDC(HDC::default());
    if memdc.is_invalid() {
        unsafe { delete_icon_bitmaps(ii.hbmColor, ii.hbmMask) };
        return Err("memdc failed".into());
    }
    let lines = GetDIBits(
        memdc,
        ii.hbmColor,
        0,
        h,
        Some(px.as_mut_ptr() as *mut _),
        &mut bmi as *mut _,
        DIB_RGB_COLORS,
    );
    unsafe {
        let _ = DeleteDC(memdc);
        delete_icon_bitmaps(ii.hbmColor, ii.hbmMask);
    }
    if lines == 0 {
        return Err("GetDIBits failed".into());
    }
    // Assemble the BMP file (headers + bottom-up rows).
    let row = (w as usize) * 4;
    let mut file = Vec::with_capacity(54 + px.len());
    let total = (54 + px.len()) as u32;
    file.extend_from_slice(&[0x42, 0x4D]); // 'BM'
    file.extend_from_slice(&total.to_le_bytes());
    file.extend_from_slice(&[0, 0, 0, 0]);
    file.extend_from_slice(&54u32.to_le_bytes());
    file.extend_from_slice(&40u32.to_le_bytes());
    file.extend_from_slice(&(w as i32).to_le_bytes());
    file.extend_from_slice(&(h as i32).to_le_bytes()); // positive = bottom-up
    file.extend_from_slice(&1u16.to_le_bytes());
    file.extend_from_slice(&32u16.to_le_bytes());
    file.extend_from_slice(&0u32.to_le_bytes());
    file.extend_from_slice(&(px.len() as u32).to_le_bytes());
    file.extend_from_slice(&[0; 16]);
    for y in (0..h as usize).rev() {
        file.extend_from_slice(&px[y * row..(y + 1) * row]);
    }
    Ok(file)
}

#[cfg(not(windows))]
#[tauri::command]
pub fn get_exe_icon(_exe: String) -> Result<String, String> {
    Err("exe icons are only supported on Windows".into())
}

#[cfg(all(test, windows))]
mod exe_icon_tests {
    use super::get_exe_icon;

    #[test]
    fn extracts_system_icon_as_bmp_data_url() {
        let url = get_exe_icon("C:\\Windows\\explorer.exe".to_string())
            .expect("explorer.exe should have an icon");
        assert!(url.starts_with("data:image/bmp;base64,"), "unexpected prefix");
        assert!(url.len() > 100, "payload suspiciously small");
    }

    #[test]
    fn missing_file_errors() {
        assert!(get_exe_icon("C:\\definitely\\not\\here.exe".to_string()).is_err());
    }

    #[test]
    fn normalizes_quoted_and_env_paths() {
        std::env::set_var("BBC_TEST_DIR", "C:\\Windows");
        assert_eq!(
            super::normalize_exe_path("\"%BBC_TEST_DIR%\\explorer.exe\""),
            "C:\\Windows\\explorer.exe"
        );
    }
}

/// Input-path health snapshot: packet counts plus CDP delivery stats.
/// Backs the watchdog's 30s summary; invoke from devtools to inspect live.
#[tauri::command]
pub fn input_status() -> serde_json::Value {
    use std::sync::atomic::Ordering::Relaxed;
    serde_json::json!({
        "packets": crate::beamng_udp::INPUT_PACKETS.load(Relaxed),
        "unknown": crate::beamng_udp::INPUT_UNKNOWN.load(Relaxed),
        "cdp_ok": crate::cdp::CDP_OK.load(Relaxed),
        "cdp_err": crate::cdp::CDP_ERR.load(Relaxed),
        "fallback": crate::cdp::CDP_FALLBACK.load(Relaxed),
        "dropped": crate::cdp::QUEUE_DROPPED.load(Relaxed),
        "last_err": crate::cdp::last_err(),
    })
}
