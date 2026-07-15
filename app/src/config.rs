use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub server_url: String,
    pub active_tab: i32,
    pub auto_launch_player: bool,
    pub torrent_port: u16,
    pub hardware_acceleration: bool,
    pub theme: ThemeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
    pub background: String,
    pub sidebar_background: String,
    pub accent: String,
    pub card_background: String,
    pub card_border: String,
    pub text_primary: String,
    pub text_secondary: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server_url: "http://127.0.0.1:11470".to_string(),
            active_tab: 0,
            auto_launch_player: true,
            torrent_port: 11470,
            hardware_acceleration: true,
            theme: ThemeConfig {
                background: "#08070d".to_string(),
                sidebar_background: "#13111f".to_string(),
                accent: "#7B5BF5".to_string(),
                card_background: "#1a1829".to_string(),
                card_border: "#2c2842".to_string(),
                text_primary: "#ffffff".to_string(),
                text_secondary: "#8d8a9f".to_string(),
            },
        }
    }
}

pub fn load_config() -> AppConfig {
    let config_path = Path::new("config.json");
    if config_path.exists() {
        if let Ok(content) = fs::read_to_string(config_path) {
            if let Ok(config) = serde_json::from_str::<AppConfig>(&content) {
                return config;
            }
        }
    }

    let default_config = AppConfig::default();
    if let Ok(content) = serde_json::to_string_pretty(&default_config) {
        let _ = fs::write(config_path, content);
    }
    default_config
}

pub fn save_config(config: &AppConfig) {
    let config_path = Path::new("config.json");
    if let Ok(content) = serde_json::to_string_pretty(config) {
        let _ = fs::write(config_path, content);
    }
}

pub fn parse_color(hex: &str) -> Option<slint::Color> {
    let hex = hex.trim_start_matches('#');
    if hex.len() == 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some(slint::Color::from_rgb_u8(r, g, b))
    } else if hex.len() == 8 {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
        Some(slint::Color::from_argb_u8(a, r, g, b))
    } else {
        None
    }
}
