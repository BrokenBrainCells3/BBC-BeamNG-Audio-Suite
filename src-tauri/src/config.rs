use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

// ============================================================================
// CONFIG - App configuration, EQ presets, and persistence
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EqPreset {
    pub name: String,
    pub gains: [f32; 16],
    pub left_gain: f32,
    pub right_gain: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub last_input_device: Option<String>,
    pub last_output_device: Option<String>,
    pub presets: HashMap<String, EqPreset>,
    pub beamng_mods_folder: Option<String>,
    pub discord_webhook_url: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            last_input_device: None,
            last_output_device: None,
            presets: HashMap::new(),
            beamng_mods_folder: None,
            discord_webhook_url: None,
        }
    }
}

pub struct ConfigState {
    pub config: Mutex<AppConfig>,
}

impl ConfigState {
    pub fn new(config: AppConfig) -> Self {
        Self { config: Mutex::new(config) }
    }
}

/// ConfigManager helper to reduce boilerplate in Tauri commands.
/// Encapsulates the lock/clone/mutate/save pattern used across many commands.
/// Uses map_err() instead of unwrap() to prevent panic on poisoned mutex.
pub struct ConfigManager<'a> {
    app_handle: &'a AppHandle,
}

impl<'a> ConfigManager<'a> {
    pub fn new(app_handle: &'a AppHandle) -> Self {
        Self { app_handle }
    }

    pub fn update<F>(&self, f: F) -> Result<(), String>
    where
        F: FnOnce(&mut AppConfig),
    {
        let state = self.app_handle.state::<ConfigState>();
        let config = {
            let config_guard = state.config.lock()
                .map_err(|e| format!("Config mutex poisoned: {}", e))?;
            let mut c = config_guard.clone();
            f(&mut c); // Apply mutation while still holding the lock clone
            c
            // config_guard dropped here — lock released before I/O
        };
        save_config(self.app_handle, &config)
    }

    pub fn with_config<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&AppConfig) -> R,
    {
        let state = self.app_handle.state::<ConfigState>();

        // Clone config while holding lock, then release lock before calling f()
        // This avoids lifetime issues with MutexGuard
        let config = match state.config.lock() {
            Ok(config_guard) => config_guard.clone(),
            Err(e) => {
                eprintln!("Config mutex poisoned: {}", e);
                AppConfig::default()
            }
        };
        // config_guard is dropped here, before state goes out of scope

        f(&config)
    }
}

pub fn get_config_path(app_handle: &AppHandle) -> PathBuf {
    app_handle
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("config.json")
}

pub fn load_config(app_handle: &AppHandle) -> AppConfig {
    let config_path = get_config_path(app_handle);
    if config_path.exists() {
        match fs::read_to_string(&config_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => AppConfig::default(),
        }
    } else {
        AppConfig::default()
    }
}

pub fn save_config(app_handle: &AppHandle, config: &AppConfig) -> Result<(), String> {
    let config_path = get_config_path(app_handle);
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    fs::write(&config_path, content).map_err(|e| e.to_string())?;
    eprintln!("Config saved to: {:?}", config_path);
    if let Some(config_state) = app_handle.try_state::<ConfigState>() {
        *config_state.config.lock().unwrap() = config.clone();
    }
    Ok(())
}