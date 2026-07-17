use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::sync::{OnceLock, RwLock};

static APP_CONFIG: OnceLock<RwLock<AppConfig>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub config_version: u32,
    pub server_url: String,
    pub active_tab: i32,
    pub auto_launch_player: bool,
    pub torrent_port: u16,
    pub hardware_acceleration: bool,
    pub ui_style: UiStyle,
    pub theme: ThemeConfig,
    pub tidb_api_key: String,
    pub tidb_show_intro: bool,
    pub tidb_show_recap: bool,
    pub tidb_show_credits: bool,
    pub tidb_show_preview: bool,
}

/// Selects the presentation layer mounted by the Slint `MainWindow`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiStyle {
    Cinematic,
    #[default]
    #[serde(other)]
    Classic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub background: String,
    pub secondary_background: String,
    pub sidebar_background: String,
    pub modal_background: String,
    pub drawer_background: String,
    pub control_background: String,
    pub overlay: String,
    pub overlay_hover: String,
    pub overlay_pressed: String,
    pub divider: String,
    pub scrim: String,
    pub accent: String,
    pub accent_hover: String,
    pub success: String,
    pub warning: String,
    pub info: String,
    pub danger: String,
    pub focus: String,
    pub title_bar: String,
    pub card_background: String,
    pub card_border: String,
    pub text_primary: String,
    pub text_secondary: String,
    pub text_muted: String,
    pub skeleton_base: String,
    pub skeleton_shimmer: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            background: "#0c0b11".to_string(),
            secondary_background: "#1a173e".to_string(),
            sidebar_background: "#0f0d2099".to_string(),
            modal_background: "#0f0d20".to_string(),
            drawer_background: "#00000066".to_string(),
            control_background: "#ffffff0d".to_string(),
            overlay: "#ffffff0d".to_string(),
            overlay_hover: "#ffffff14".to_string(),
            overlay_pressed: "#ffffff20".to_string(),
            divider: "#ffffff14".to_string(),
            scrim: "#00000066".to_string(),
            accent: "#7b5bf5".to_string(),
            accent_hover: "#9275f7".to_string(),
            success: "#22b365".to_string(),
            warning: "#f6c700".to_string(),
            info: "#1245a6".to_string(),
            danger: "#dc2626".to_string(),
            focus: "#ffffffe6".to_string(),
            title_bar: "#15122b".to_string(),
            card_background: "#151320".to_string(),
            card_border: "#201e2f".to_string(),
            text_primary: "#ffffffe6".to_string(),
            text_secondary: "#ffffff99".to_string(),
            text_muted: "#ffffff66".to_string(),
            skeleton_base: "#1a1828".to_string(),
            skeleton_shimmer: "#2a2838".to_string(),
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            config_version: 2,
            server_url: "http://127.0.0.1:11470".to_string(),
            active_tab: 0,
            auto_launch_player: true,
            torrent_port: 11470,
            hardware_acceleration: true,
            ui_style: UiStyle::Classic,
            theme: ThemeConfig::default(),
            tidb_api_key: String::new(),
            tidb_show_intro: true,
            tidb_show_recap: true,
            tidb_show_credits: true,
            tidb_show_preview: true,
        }
    }
}

impl AppConfig {
    fn migrate(&mut self) -> bool {
        if self.config_version >= 2 {
            return false;
        }

        // Version zero was generated with a palette that predates the
        // official stremio-web tokens. Only replace it when all legacy values
        // still match, preserving any genuinely customized theme.
        let legacy_theme = self.theme.background.eq_ignore_ascii_case("#08070d")
            && self
                .theme
                .sidebar_background
                .eq_ignore_ascii_case("#13111f")
            && self.theme.accent.eq_ignore_ascii_case("#7b5bf5")
            && self.theme.card_background.eq_ignore_ascii_case("#1a1829")
            && self.theme.card_border.eq_ignore_ascii_case("#2c2842")
            && self.theme.text_primary.eq_ignore_ascii_case("#ffffff")
            && self.theme.text_secondary.eq_ignore_ascii_case("#8d8a9f");

        if legacy_theme {
            self.theme = ThemeConfig::default();
        }
        self.config_version = 2;
        true
    }
}

pub async fn init_config() {
    let mut config = AppConfig::default();

    // 1. Try to load from database settings table
    let mut loaded_from_db = false;
    let mut migrated_from_db = false;
    if let Ok(conn) = crate::db::get_conn() {
        if let Ok(mut rows) = conn
            .query("SELECT value FROM settings WHERE key = 'app_config'", ())
            .await
        {
            if let Ok(Some(row)) = rows.next().await {
                if let Ok(val_str) = row.get::<String>(0) {
                    if let Ok(mut parsed) = serde_json::from_str::<AppConfig>(&val_str) {
                        migrated_from_db = parsed.migrate();
                        config = parsed;
                        loaded_from_db = true;
                    }
                }
            }
        }
    }

    if loaded_from_db
        && migrated_from_db
        && let Ok(conn) = crate::db::get_conn()
        && let Ok(serialized) = serde_json::to_string(&config)
    {
        let _ = conn
            .execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES ('app_config', ?)",
                [serialized],
            )
            .await;
    }

    // 2. If not found in database, check for legacy config.json file
    if !loaded_from_db {
        let config_path = Path::new("config.json");
        if config_path.exists() {
            if let Ok(content) = fs::read_to_string(config_path) {
                if let Ok(mut legacy_config) = serde_json::from_str::<AppConfig>(&content) {
                    legacy_config.migrate();
                    config = legacy_config;

                    // Save legacy config to database
                    if let Ok(conn) = crate::db::get_conn() {
                        if let Ok(serialized) = serde_json::to_string(&config) {
                            let _ = conn.execute(
                                "INSERT OR REPLACE INTO settings (key, value) VALUES ('app_config', ?)",
                                [serialized],
                            )
                            .await;
                        }
                    }

                    // Rename legacy config.json to config.json.bak
                    let backup_path = Path::new("config.json.bak");
                    let _ = fs::rename(config_path, backup_path);
                }
            }
        } else {
            // First run, populate default config in database
            if let Ok(conn) = crate::db::get_conn() {
                if let Ok(serialized) = serde_json::to_string(&config) {
                    let _ = conn
                        .execute(
                            "INSERT OR REPLACE INTO settings (key, value) VALUES ('app_config', ?)",
                            [serialized],
                        )
                        .await;
                }
            }
        }
    }

    let _ = APP_CONFIG.set(RwLock::new(config));
}

pub fn load_config() -> AppConfig {
    if let Some(lock) = APP_CONFIG.get() {
        if let Ok(guard) = lock.read() {
            return guard.clone();
        }
    }
    AppConfig::default()
}

pub fn with_config<R>(read: impl FnOnce(&AppConfig) -> R) -> R {
    if let Some(lock) = APP_CONFIG.get()
        && let Ok(config) = lock.read()
    {
        return read(&config);
    }
    read(&AppConfig::default())
}

pub fn save_config(config: &AppConfig) {
    if let Some(lock) = APP_CONFIG.get() {
        if let Ok(mut guard) = lock.write() {
            *guard = config.clone();
        }
    }

    let config_cloned = config.clone();
    tokio::spawn(async move {
        if let Ok(conn) = crate::db::get_conn() {
            if let Ok(serialized) = serde_json::to_string(&config_cloned) {
                let _ = conn
                    .execute(
                        "INSERT OR REPLACE INTO settings (key, value) VALUES ('app_config', ?)",
                        [serialized],
                    )
                    .await;
            }
        }
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_config() -> AppConfig {
        let mut config = AppConfig::default();
        config.config_version = 0;
        config.theme.background = "#08070d".to_string();
        config.theme.sidebar_background = "#13111f".to_string();
        config.theme.accent = "#7B5BF5".to_string();
        config.theme.card_background = "#1a1829".to_string();
        config.theme.card_border = "#2c2842".to_string();
        config.theme.text_primary = "#ffffff".to_string();
        config.theme.text_secondary = "#8d8a9f".to_string();
        config
    }

    #[test]
    fn migrates_generated_legacy_theme_to_official_defaults() {
        let mut config = legacy_config();

        assert!(config.migrate());
        assert_eq!(config.config_version, 2);
        assert_eq!(config.theme.background, "#0c0b11");
        assert_eq!(config.theme.secondary_background, "#1a173e");
        assert_eq!(config.theme.success, "#22b365");
    }

    #[test]
    fn preserves_custom_theme_during_version_migration() {
        let mut config = legacy_config();
        config.theme.accent = "#ff3366".to_string();

        assert!(config.migrate());
        assert_eq!(config.config_version, 2);
        assert_eq!(config.theme.background, "#08070d");
        assert_eq!(config.theme.accent, "#ff3366");
    }

    #[test]
    fn fills_new_semantic_theme_slots_when_reading_old_json() {
        let config: AppConfig = serde_json::from_str(
            r##"{
                "server_url": "http://127.0.0.1:11470",
                "theme": {
                    "background": "#111111",
                    "accent": "#abcdef"
                }
            }"##,
        )
        .expect("old config should deserialize with semantic defaults");

        assert_eq!(config.theme.background, "#111111");
        assert_eq!(config.theme.accent, "#abcdef");
        assert_eq!(config.theme.drawer_background, "#00000066");
        assert_eq!(config.theme.text_muted, "#ffffff66");
        assert_eq!(config.ui_style, UiStyle::Classic);
    }

    #[test]
    fn ui_style_should_round_trip_classic() {
        let config = AppConfig::default();
        let json = serde_json::to_string(&config).expect("serialize config");
        let restored: AppConfig = serde_json::from_str(&json).expect("deserialize config");

        assert_eq!(restored.ui_style, UiStyle::Classic);
    }

    #[test]
    fn ui_style_should_round_trip_cinematic() {
        let config = AppConfig {
            ui_style: UiStyle::Cinematic,
            ..Default::default()
        };
        let json = serde_json::to_string(&config).expect("serialize config");
        assert!(json.contains(r#""ui_style":"cinematic""#));
        let restored: AppConfig = serde_json::from_str(&json).expect("deserialize config");
        assert_eq!(restored.ui_style, UiStyle::Cinematic);
    }

    #[test]
    fn ui_style_should_fall_back_to_classic_for_unknown_values() {
        let config = AppConfig {
            ui_style: UiStyle::Cinematic,
            ..Default::default()
        };
        let json = serde_json::to_string(&config).expect("serialize config");
        let unknown = json.replace("cinematic", "future-style");
        let config: AppConfig = serde_json::from_str(&unknown).expect("deserialize unknown style");
        assert_eq!(config.ui_style, UiStyle::Classic);
    }

    #[test]
    fn migration_should_preserve_cinematic_selection() {
        let mut config = AppConfig {
            config_version: 1,
            ui_style: UiStyle::Cinematic,
            ..Default::default()
        };

        config.migrate();

        assert_eq!(config.ui_style, UiStyle::Cinematic);
    }

    #[test]
    fn changing_ui_style_should_preserve_classic_theme() {
        let mut config = AppConfig::default();
        config.theme.accent = "#abcdef".to_string();
        let classic_theme = config.theme.clone();

        config.ui_style = UiStyle::Cinematic;

        assert_eq!(config.theme, classic_theme);
    }
}
