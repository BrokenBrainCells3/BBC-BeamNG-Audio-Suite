//! Discord Webhook Integration for EQ Preset Sharing
//!
//! This module handles uploading EQ presets to a Discord webhook.
//! The webhook URL is stored in the main app config.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

/// EQ Preset data for Discord embed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordPresetData {
    pub name: String,
    pub gains: [f32; 16],
    pub left_gain: f32,
    pub right_gain: f32,
    pub timestamp: u64,
}

/// Discord webhook payload
#[derive(Debug, Clone, Serialize)]
struct DiscordPayload {
    username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    avatar_url: Option<String>,
    embeds: Vec<DiscordEmbed>,
}

/// Discord embed for rich message formatting
#[derive(Debug, Clone, Serialize)]
struct DiscordEmbed {
    title: String,
    description: String,
    color: u32,
    fields: Vec<EmbedField>,
    footer: Option<EmbedFooter>,
    timestamp: String,
}

/// Individual field in an embed
#[derive(Debug, Clone, Serialize)]
struct EmbedField {
    name: String,
    value: String,
    inline: bool,
}

/// Footer for embed
#[derive(Debug, Clone, Serialize)]
struct EmbedFooter {
    text: String,
}

/// Set the Discord webhook URL (stored in main config)
#[tauri::command]
pub fn set_discord_webhook_url(
    app_handle: AppHandle,
    url: Option<String>,
) -> Result<(), String> {
    // Use lib.rs functions via crate:: prefix
    use crate::{ConfigState, save_config};

    let config_state = app_handle.state::<ConfigState>();
    let mut config = config_state.config.lock().unwrap().clone();
    config.discord_webhook_url = url;
    save_config(&app_handle, &config)
}

/// Get the Discord webhook URL (from main config)
#[tauri::command]
pub fn get_discord_webhook_url(app_handle: AppHandle) -> Option<String> {
    use crate::ConfigState;
    app_handle.state::<ConfigState>().config.lock().unwrap().discord_webhook_url.clone()
}

/// Upload an EQ preset to Discord
#[tauri::command]
pub async fn upload_preset_to_discord(
    app_handle: AppHandle,
    preset_name: String,
    gains: Vec<f32>,
    left_gain: f32,
    right_gain: f32,
    user_name: String,
) -> Result<String, String> {
    use crate::ConfigState;

    // Get webhook URL from main config
    // Extract data from the lock guard BEFORE any .await to ensure the Future is Send
    let webhook_url = {
        let config_state = app_handle.state::<ConfigState>();
        let config = config_state.config.lock().unwrap();
        config.discord_webhook_url.clone().ok_or_else(|| {
            "Discord webhook URL not configured. Please set it in settings.".to_string()
        })?
    }; // guard dropped here, before any .await

    // Validate gains length
    if gains.len() != 16 {
        return Err("Invalid gains array length (expected 16)".to_string());
    }

    // Create preset data
    let preset_data = DiscordPresetData {
        name: preset_name.clone(),
        gains: gains.try_into().map_err(|_| "Failed to convert gains")?,
        left_gain,
        right_gain,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };

    // Create Discord embed with user's name
    let embed = create_preset_embed(&preset_data, &user_name);

    // Create payload with user's name as username
    let payload = DiscordPayload {
        username: user_name.clone(),
        avatar_url: None,
        embeds: vec![embed],
    };

    // Send to Discord webhook
    let client = reqwest::Client::new();
    let response = client
        .post(&webhook_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("Failed to send to Discord: {}", e))?;

    if response.status().is_success() {
        Ok(format!("Preset '{}' uploaded to Discord successfully!", preset_name))
    } else {
        Err(format!(
            "Discord webhook returned status: {}",
            response.status()
        ))
    }
}

/// Create a Discord embed for an EQ preset
fn create_preset_embed(preset: &DiscordPresetData, user_name: &str) -> DiscordEmbed {
    // Format EQ bands as a visual representation
    let eq_bands = [
        "60Hz", "150Hz", "300Hz", "600Hz", "1kHz", "2.4kHz",
        "4kHz", "6kHz", "8kHz", "10kHz", "12kHz", "14kHz",
        "16kHz", "18kHz", "20kHz", "Vol"
    ];
    
    // Create fields for EQ bands (grouped for readability)
    let mut fields = Vec::new();
    
    // Low frequencies
    let low_eq: String = preset.gains[0..5]
        .iter()
        .zip(eq_bands[0..5].iter())
        .map(|(&gain, &band)| format!("{}: {:+.1}dB", band, gain))
        .collect::<Vec<_>>()
        .join("\n");
    
    fields.push(EmbedField {
        name: "🔊 Low Frequencies".to_string(),
        value: low_eq,
        inline: true,
    });
    
    // Mid frequencies
    let mid_eq: String = preset.gains[5..10]
        .iter()
        .zip(eq_bands[5..10].iter())
        .map(|(&gain, &band)| format!("{}: {:+.1}dB", band, gain))
        .collect::<Vec<_>>()
        .join("\n");
    
    fields.push(EmbedField {
        name: "🎵 Mid Frequencies".to_string(),
        value: mid_eq,
        inline: true,
    });
    
    // High frequencies
    let high_eq: String = preset.gains[10..16]
        .iter()
        .zip(eq_bands[10..16].iter())
        .map(|(&gain, &band)| format!("{}: {:+.1}dB", band, gain))
        .collect::<Vec<_>>()
        .join("\n");
    
    fields.push(EmbedField {
        name: "✨ High Frequencies".to_string(),
        value: high_eq,
        inline: true,
    });
    
    // Channel gains
    fields.push(EmbedField {
        name: "📢 Channel Gains".to_string(),
        value: format!("Left: {:+.1}dB\nRight: {:+.1}dB", preset.left_gain, preset.right_gain),
        inline: false,
    });
    
    // Create ISO 8601 timestamp for Discord
    let timestamp = chrono::DateTime::from_timestamp(preset.timestamp as i64, 0)
        .unwrap_or_default()
        .to_rfc3339();
    
    DiscordEmbed {
        title: format!("🎚️ EQ Preset: {}", preset.name),
        description: format!("Custom equalizer preset shared by {}", user_name),
        color: 0x58c2ff, // Blue color matching app theme
        fields,
        footer: Some(EmbedFooter {
            text: format!("Shared by {}", user_name),
        }),
        timestamp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embed_creation() {
        let preset = DiscordPresetData {
            name: "Test Preset".to_string(),
            gains: [0.0; 16],
            left_gain: 0.0,
            right_gain: 0.0,
            timestamp: 1234567890,
        };

        let embed = create_preset_embed(&preset, "Test User");
        assert_eq!(embed.title, "🎚️ EQ Preset: Test Preset");
        assert_eq!(embed.fields.len(), 4); // 3 frequency bands + channel gains
    }
}
