use crate::config::AppConfig;
use crate::{AppModel, AppModelField, MainWindow};
use core_env::DesktopEnv;
use server_connector::AppServerConnector;
use settings_gui::ServerConnector;
use slint::ComponentHandle;
use std::sync::Arc;
use stremio_core::{
    models::{common::Loadable, data_export::DataExport},
    runtime::{
        Runtime, RuntimeAction,
        msg::{Action, ActionCtx, ActionLoad},
    },
    types::profile::Settings as ProfileSettings,
};

fn update_profile_settings(
    runtime: &Arc<Runtime<DesktopEnv, AppModel>>,
    update: impl FnOnce(&mut ProfileSettings),
) {
    let Ok(model) = runtime.model() else {
        return;
    };
    let mut settings = model.ctx.profile.settings.clone();
    drop(model);
    update(&mut settings);
    runtime.dispatch(RuntimeAction {
        field: Some(AppModelField::Ctx),
        action: Action::Ctx(ActionCtx::UpdateSettings(settings)),
    });
}

fn language_code(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "english" | "en" | "eng" => "eng",
        "spanish" | "es" | "spa" => "spa",
        "french" | "fr" | "fra" | "fre" => "fra",
        "german" | "de" | "deu" | "ger" => "deu",
        "italian" | "it" | "ita" => "ita",
        "portuguese" | "pt" | "por" => "por",
        "russian" | "ru" | "rus" => "rus",
        "hindi" | "hi" | "hin" => "hin",
        "japanese" | "ja" | "jpn" => "jpn",
        "korean" | "ko" | "kor" => "kor",
        "chinese" | "zh" | "zho" | "chi" => "zho",
        _ => return value.trim().to_owned(),
    }
    .to_owned()
}

fn language_display(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "eng" | "en" => "English",
        "spa" | "es" => "Spanish",
        "fra" | "fre" | "fr" => "French",
        "deu" | "ger" | "de" => "German",
        "ita" | "it" => "Italian",
        "por" | "pt" => "Portuguese",
        "rus" | "ru" => "Russian",
        "hin" | "hi" => "Hindi",
        "jpn" | "ja" => "Japanese",
        "kor" | "ko" => "Korean",
        "zho" | "chi" | "zh" => "Chinese",
        _ => return value.to_owned(),
    }
    .to_owned()
}

fn format_cache_size(bytes: f64) -> String {
    if bytes <= 0.0 {
        "No Caching".to_string()
    } else if bytes >= 1024.0 * 1024.0 * 1024.0 * 1024.0 {
        "Infinite".to_string()
    } else {
        let gb = bytes / 1024.0 / 1024.0 / 1024.0;
        format!("{:.1} GB", gb)
    }
}

pub fn setup(ui: &MainWindow, runtime: &Arc<Runtime<DesktopEnv, AppModel>>, config: &AppConfig) {
    let server_url = runtime
        .model()
        .ok()
        .map(|model| model.ctx.profile.settings.streaming_server_url.to_string())
        .unwrap_or_else(|| config.server_url.clone());
    let connector = Arc::new(AppServerConnector::new(server_url));

    // Fetch initial streaming server settings and coordinate with Turso DB
    let conn_init = connector.clone();
    let ui_weak = ui.as_weak();
    tokio::spawn(async move {
        let db_settings = crate::db::get_settings(&[
            "seeding_enabled",
            "bt_enable_dht",
            "bt_enable_pex",
            "bt_enable_lsd",
        ])
        .await
        .unwrap_or_default();
        let db_seeding = db_settings
            .get("seeding_enabled")
            .map(|value| value == "true");
        let db_dht = db_settings
            .get("bt_enable_dht")
            .map(|value| value == "true");
        let db_pex = db_settings
            .get("bt_enable_pex")
            .map(|value| value == "true");
        let db_lsd = db_settings
            .get("bt_enable_lsd")
            .map(|value| value == "true");

        if let Ok(mut settings) = conn_init.get_settings().await {
            let mut dirty = false;
            let seeding_value = settings.seeding_enabled.to_string();
            let dht_value = settings.bt_enable_dht.to_string();
            let pex_value = settings.bt_enable_pex.to_string();
            let lsd_value = settings.bt_enable_lsd.to_string();
            let mut missing_settings = Vec::with_capacity(4);
            if let Some(seeding) = db_seeding {
                if settings.seeding_enabled != seeding {
                    settings.seeding_enabled = seeding;
                    dirty = true;
                }
            } else {
                missing_settings.push(("seeding_enabled", seeding_value.as_str()));
            }

            if let Some(dht) = db_dht {
                if settings.bt_enable_dht != dht {
                    settings.bt_enable_dht = dht;
                    dirty = true;
                }
            } else {
                missing_settings.push(("bt_enable_dht", dht_value.as_str()));
            }

            if let Some(pex) = db_pex {
                if settings.bt_enable_pex != pex {
                    settings.bt_enable_pex = pex;
                    dirty = true;
                }
            } else {
                missing_settings.push(("bt_enable_pex", pex_value.as_str()));
            }

            if let Some(lsd) = db_lsd {
                if settings.bt_enable_lsd != lsd {
                    settings.bt_enable_lsd = lsd;
                    dirty = true;
                }
            } else {
                missing_settings.push(("bt_enable_lsd", lsd_value.as_str()));
            }

            if !missing_settings.is_empty() {
                let _ = crate::db::set_settings(&missing_settings).await;
            }

            if dirty {
                let _ = conn_init.apply_settings(settings.clone()).await;
                let _ =
                    crate::db::insert_log("INFO", "Streaming settings synchronized from Turso DB.")
                        .await;
            }

            let cache_size_str = format_cache_size(settings.cache_size);
            let seeding = settings.seeding_enabled;
            let dht = settings.bt_enable_dht;
            let pex = settings.bt_enable_pex;
            let lsd = settings.bt_enable_lsd;
            let max_conn = settings.bt_max_connections;

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_torrent_cache_size(cache_size_str.into());
                    ui.set_settings_streaming_seeding(seeding);
                    ui.set_settings_streaming_dht(dht);
                    ui.set_settings_streaming_pex(pex);
                    ui.set_settings_streaming_lsd(lsd);

                    let profile_str = if max_conn >= 200 {
                        "Ultra Fast"
                    } else if max_conn >= 100 {
                        "Fast"
                    } else {
                        "Default"
                    };
                    ui.set_settings_torrent_profile(profile_str.into());
                }
            });
        }
    });

    // Cache size callback
    ui.on_apply_cache_settings({
        let conn = connector.clone();
        let ui_weak = ui.as_weak();
        move |val_gb| {
            let conn = conn.clone();
            let ui_weak = ui_weak.clone();
            tokio::spawn(async move {
                if let Ok(mut settings) = conn.get_settings().await {
                    let bytes = if val_gb >= 50.0 {
                        10.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0
                    } else if val_gb <= 0.0 {
                        0.0
                    } else {
                        (val_gb as f64) * 1024.0 * 1024.0 * 1024.0
                    };
                    settings.cache_size = bytes;
                    if let Ok(_) = conn.apply_settings(settings).await {
                        let cache_size_str = format_cache_size(bytes);
                        let _ = crate::db::insert_log(
                            "INFO",
                            &format!("Cache size adjusted to: {}", cache_size_str),
                        )
                        .await;
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                ui.set_torrent_cache_size(cache_size_str.into());
                            }
                        });
                    }
                }
            });
        }
    });

    // Interface language callback
    ui.on_settings_change_interface_language({
        let runtime = runtime.clone();
        move |lang| {
            let rt = runtime.clone();
            let lang = language_code(lang.as_str());
            tokio::spawn(async move {
                let _ = crate::db::set_setting("interface_language", &lang).await;
                let _ = crate::db::insert_log(
                    "INFO",
                    &format!("Interface language changed to: {}", lang),
                )
                .await;
                let model = rt.model().expect("model read failed");
                let mut settings = model.ctx.profile.settings.clone();
                settings.interface_language = lang;
                drop(model);

                rt.dispatch(RuntimeAction {
                    field: None,
                    action: Action::Ctx(ActionCtx::UpdateSettings(settings)),
                });
            });
        }
    });

    // Subtitles language callback
    ui.on_settings_change_subtitles_language({
        let runtime = runtime.clone();
        move |lang| {
            let rt = runtime.clone();
            let lang = language_code(lang.as_str());
            tokio::spawn(async move {
                let _ = crate::db::set_setting("subtitles_language", &lang).await;
                let _ = crate::db::insert_log(
                    "INFO",
                    &format!("Subtitles language changed to: {}", lang),
                )
                .await;
                let model = rt.model().expect("model read failed");
                let mut settings = model.ctx.profile.settings.clone();
                settings.subtitles_language = Some(lang);
                drop(model);

                rt.dispatch(RuntimeAction {
                    field: None,
                    action: Action::Ctx(ActionCtx::UpdateSettings(settings)),
                });
            });
        }
    });

    // Torrent profile callback
    ui.on_settings_change_torrent_profile({
        let conn = connector.clone();
        move |profile| {
            let conn = conn.clone();
            let profile = profile.to_string();
            tokio::spawn(async move {
                if let Ok(mut settings) = conn.get_settings().await {
                    if profile == "default" {
                        settings.bt_max_connections = 35;
                    } else if profile == "fast" {
                        settings.bt_max_connections = 100;
                    } else if profile == "ultrafast" {
                        settings.bt_max_connections = 200;
                    }
                    let _ = crate::db::set_setting("torrent_profile", &profile).await;
                    let _ = crate::db::insert_log(
                        "INFO",
                        &format!("Torrent connections profile set to: {}", profile),
                    )
                    .await;
                    let _ = conn.apply_settings(settings).await;
                }
            });
        }
    });

    // Clear search history callback
    ui.on_settings_clear_search_history({
        let runtime = runtime.clone();
        move || {
            let rt = runtime.clone();
            tokio::spawn(async move {
                let _ = crate::db::insert_log("INFO", "Search history cleared.").await;
                rt.dispatch(RuntimeAction {
                    field: None,
                    action: Action::Ctx(ActionCtx::ClearSearchHistory),
                });
            });
        }
    });

    // Shutdown streaming server callback
    ui.on_shutdown_server(move || {
        tracing::info!("Closing the app and streaming server...");
        if let Err(error) = slint::quit_event_loop() {
            tracing::error!(%error, "failed to stop the UI event loop");
        }
    });

    // Hardware acceleration toggle callback
    ui.on_settings_change_hardware_acceleration({
        let config_cloned = config.clone();
        let runtime = runtime.clone();
        move |enabled| {
            let mut cfg = config_cloned.clone();
            cfg.hardware_acceleration = enabled;
            crate::config::save_config(&cfg);
            let rt = runtime.clone();
            tokio::spawn(async move {
                let _ = crate::db::set_setting("hardware_acceleration", &enabled.to_string()).await;
                let _ = crate::db::insert_log(
                    "INFO",
                    &format!("Hardware acceleration toggle: {}", enabled),
                )
                .await;
                if let Ok(model) = rt.model() {
                    let mut settings = model.ctx.profile.settings.clone();
                    settings.hardware_decoding = enabled;
                    drop(model);
                    rt.dispatch(RuntimeAction {
                        field: None,
                        action: Action::Ctx(ActionCtx::UpdateSettings(settings)),
                    });
                }
            });
            tracing::info!(
                "Hardware acceleration toggled to: {}. Restart required.",
                enabled
            );
        }
    });

    ui.on_settings_export_data({
        let runtime = runtime.clone();
        let ui_weak = ui.as_weak();
        move || {
            let authenticated = runtime
                .model()
                .ok()
                .is_some_and(|model| model.ctx.profile.auth.is_some());
            if !authenticated {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_settings_export_loading(false);
                    ui.set_settings_export_status("Sign in to export your data.".into());
                }
                return;
            }
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_settings_export_loading(true);
                ui.set_settings_export_status("Preparing export…".into());
            }
            runtime.dispatch(RuntimeAction {
                field: Some(AppModelField::DataExport),
                action: Action::Load(ActionLoad::DataExport),
            });
        }
    });

    ui.on_settings_change_binge_watching({
        let runtime = runtime.clone();
        move |value| update_profile_settings(&runtime, |settings| settings.binge_watching = value)
    });
    ui.on_settings_change_discord_rpc_enabled({
        let runtime = runtime.clone();
        move |value| {
            update_profile_settings(&runtime, |settings| settings.discord_rpc_enabled = value)
        }
    });
    ui.on_settings_change_tidb_api_key({
        move |value| {
            let mut cfg = crate::config::load_config();
            cfg.tidb_api_key = value.to_string();
            crate::config::save_config(&cfg);
        }
    });
    ui.on_settings_change_tidb_show_intro({
        move |value| {
            let mut cfg = crate::config::load_config();
            cfg.tidb_show_intro = value;
            crate::config::save_config(&cfg);
        }
    });
    ui.on_settings_change_tidb_show_recap({
        move |value| {
            let mut cfg = crate::config::load_config();
            cfg.tidb_show_recap = value;
            crate::config::save_config(&cfg);
        }
    });
    ui.on_settings_change_tidb_show_credits({
        move |value| {
            let mut cfg = crate::config::load_config();
            cfg.tidb_show_credits = value;
            crate::config::save_config(&cfg);
        }
    });
    ui.on_settings_change_tidb_show_preview({
        move |value| {
            let mut cfg = crate::config::load_config();
            cfg.tidb_show_preview = value;
            crate::config::save_config(&cfg);
        }
    });
    ui.on_settings_change_hide_spoilers({
        let runtime = runtime.clone();
        move |value| update_profile_settings(&runtime, |settings| settings.hide_spoilers = value)
    });
    ui.on_settings_change_gamepad_support({
        let runtime = runtime.clone();
        move |value| update_profile_settings(&runtime, |settings| settings.gamepad_support = value)
    });
    ui.on_settings_change_play_in_background({
        let runtime = runtime.clone();
        move |value| {
            update_profile_settings(&runtime, |settings| settings.play_in_background = value)
        }
    });
    ui.on_settings_change_subtitles_auto_select({
        let runtime = runtime.clone();
        move |value| {
            update_profile_settings(&runtime, |settings| settings.subtitles_auto_select = value)
        }
    });
    ui.on_settings_change_subtitles_font({
        let runtime = runtime.clone();
        move |value| {
            let value = value.trim().to_owned();
            if !value.is_empty() {
                update_profile_settings(&runtime, |settings| settings.subtitles_font = value);
            }
        }
    });
    ui.on_settings_change_subtitles_size({
        let runtime = runtime.clone();
        move |value| {
            if let Ok(value) = value.trim().parse::<u8>() {
                let value = value.clamp(50, 200);
                update_profile_settings(&runtime, |settings| settings.subtitles_size = value);
            }
        }
    });
    ui.on_settings_change_subtitles_bold({
        let runtime = runtime.clone();
        move |value| update_profile_settings(&runtime, |settings| settings.subtitles_bold = value)
    });
    ui.on_settings_change_subtitles_offset({
        let runtime = runtime.clone();
        move |value| {
            if let Ok(value) = value.trim().parse::<u8>() {
                let value = value.min(100);
                update_profile_settings(&runtime, |settings| settings.subtitles_offset = value);
            }
        }
    });
    ui.on_settings_change_seek_duration({
        let runtime = runtime.clone();
        move |value| {
            if let Ok(seconds) = value.trim().parse::<u32>() {
                let milliseconds = seconds.clamp(1, 120).saturating_mul(1_000);
                update_profile_settings(&runtime, |settings| {
                    settings.seek_time_duration = milliseconds;
                });
            }
        }
    });
    ui.on_settings_change_pause_on_minimize({
        let runtime = runtime.clone();
        move |value| {
            update_profile_settings(&runtime, |settings| settings.pause_on_minimize = value)
        }
    });
    ui.on_settings_change_quit_on_close({
        let runtime = runtime.clone();
        move |value| update_profile_settings(&runtime, |settings| settings.quit_on_close = value)
    });

    // Custom Client Settings Callbacks
    ui.on_settings_change_seeding_enabled({
        let conn = connector.clone();
        move |enabled| {
            let conn = conn.clone();
            tokio::spawn(async move {
                let _ = crate::db::set_setting("seeding_enabled", &enabled.to_string()).await;
                let _ = crate::db::insert_log(
                    "INFO",
                    &format!("Torrent seeding changed to: {}", enabled),
                )
                .await;
                if let Ok(mut settings) = conn.get_settings().await {
                    settings.seeding_enabled = enabled;
                    let _ = conn.apply_settings(settings).await;
                }
            });
        }
    });

    ui.on_settings_change_dht_enabled({
        let conn = connector.clone();
        move |enabled| {
            let conn = conn.clone();
            tokio::spawn(async move {
                let _ = crate::db::set_setting("bt_enable_dht", &enabled.to_string()).await;
                let _ =
                    crate::db::insert_log("INFO", &format!("DHT network changed to: {}", enabled))
                        .await;
                if let Ok(mut settings) = conn.get_settings().await {
                    settings.bt_enable_dht = enabled;
                    let _ = conn.apply_settings(settings).await;
                }
            });
        }
    });

    ui.on_settings_change_pex_enabled({
        let conn = connector.clone();
        move |enabled| {
            let conn = conn.clone();
            tokio::spawn(async move {
                let _ = crate::db::set_setting("bt_enable_pex", &enabled.to_string()).await;
                let _ =
                    crate::db::insert_log("INFO", &format!("PEX network changed to: {}", enabled))
                        .await;
                if let Ok(mut settings) = conn.get_settings().await {
                    settings.bt_enable_pex = enabled;
                    let _ = conn.apply_settings(settings).await;
                }
            });
        }
    });

    ui.on_settings_change_lsd_enabled({
        let conn = connector.clone();
        move |enabled| {
            let conn = conn.clone();
            tokio::spawn(async move {
                let _ = crate::db::set_setting("bt_enable_lsd", &enabled.to_string()).await;
                let _ =
                    crate::db::insert_log("INFO", &format!("LSD network changed to: {}", enabled))
                        .await;
                if let Ok(mut settings) = conn.get_settings().await {
                    settings.bt_enable_lsd = enabled;
                    let _ = conn.apply_settings(settings).await;
                }
            });
        }
    });

    // Diagnostics Logs Callbacks
    ui.on_settings_refresh_logs({
        let conn = connector.clone();
        let ui_weak = ui.as_weak();
        move || {
            let conn = conn.clone();
            let ui_weak = ui_weak.clone();
            tokio::spawn(async move {
                let mut logs_combined = String::new();

                // 1. Fetch Local Application Logs from Turso DB
                if let Ok(db_logs) = crate::db::get_logs(100).await {
                    logs_combined.push_str("--- Local Application Logs (Turso DB) ---\n");
                    for line in db_logs.iter().rev() {
                        logs_combined.push_str(line);
                        logs_combined.push_str("\n");
                    }
                    logs_combined.push_str("\n");
                }

                // 2. Fetch Streaming Server Engine Logs
                if let Ok(logs) = conn.get_logs().await {
                    let content = logs
                        .current_human_log
                        .unwrap_or_else(|| "No engine logs available.".to_string());
                    logs_combined.push_str("--- Streaming Server Engine Logs ---\n");
                    logs_combined.push_str(&content);
                } else {
                    logs_combined.push_str(
                        "--- Streaming Server Engine Logs ---\nFailed to retrieve engine logs.",
                    );
                }

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_settings_streaming_logs_text(logs_combined.into());
                    }
                });
            });
        }
    });

    ui.on_settings_open_logs_folder({
        let conn = connector.clone();
        move || {
            let conn = conn.clone();
            tokio::spawn(async move {
                if let Ok(logs) = conn.get_logs().await {
                    let path = logs.log_dir;
                    #[cfg(target_os = "windows")]
                    {
                        let _ = std::process::Command::new("explorer").arg(&path).spawn();
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
                    }
                }
            });
        }
    });
}

#[tracing::instrument(skip_all)]
pub fn sync(ui: &MainWindow, settings: &ProfileSettings) {
    let _span = tracing::info_span!("apply_ui_settings").entered();
    ui.set_settings_interface_language(language_display(&settings.interface_language).into());
    ui.set_settings_subtitles_language(
        settings
            .subtitles_language
            .as_deref()
            .map(language_display)
            .unwrap_or_else(|| "English".to_string())
            .into(),
    );
    ui.set_settings_hardware_acceleration(settings.hardware_decoding);
    ui.set_settings_binge_watching(settings.binge_watching);
    ui.set_settings_discord_rpc_enabled(settings.discord_rpc_enabled);
    ui.set_settings_hide_spoilers(settings.hide_spoilers);
    ui.set_settings_gamepad_support(settings.gamepad_support);
    ui.set_settings_play_in_background(settings.play_in_background);
    ui.set_settings_subtitles_auto_select(settings.subtitles_auto_select);
    ui.set_settings_subtitles_font(settings.subtitles_font.as_str().into());
    ui.set_settings_subtitles_size(settings.subtitles_size.to_string().into());
    ui.set_settings_subtitles_bold(settings.subtitles_bold);
    ui.set_settings_subtitles_offset(settings.subtitles_offset.to_string().into());
    ui.set_settings_seek_duration((settings.seek_time_duration / 1_000).to_string().into());
    ui.set_settings_pause_on_minimize(settings.pause_on_minimize);
    ui.set_settings_quit_on_close(settings.quit_on_close);

    // Apply the same persisted values to the native MPV controls. A
    // stream-specific override is restored when that stream finishes loading.
    ui.set_player_seek_step_seconds(settings.seek_time_duration as f32 / 1_000.0);
    ui.set_player_subtitle_size_percent(f32::from(settings.subtitles_size));
    ui.set_player_subtitle_offset_percent(f32::from(settings.subtitles_offset));
}

#[tracing::instrument(skip_all)]
pub fn sync_data_export(
    ui: &MainWindow,
    data_export: &DataExport,
    runtime: &Arc<Runtime<DesktopEnv, AppModel>>,
) {
    let _span = tracing::info_span!("apply_data_export_state").entered();
    match data_export.export_url.as_ref().map(|(_, value)| value) {
        None => {
            ui.set_settings_export_loading(false);
        }
        Some(Loadable::Loading) => {
            ui.set_settings_export_loading(true);
            ui.set_settings_export_status("Preparing export…".into());
        }
        Some(Loadable::Ready(url)) => {
            ui.set_settings_export_loading(false);
            match open::that(url.as_str()) {
                Ok(()) => ui.set_settings_export_status("Export opened in your browser.".into()),
                Err(error) => {
                    tracing::error!(%error, %url, "failed to open data export");
                    ui.set_settings_export_status(format!("Export ready: {url}").into());
                }
            }
            runtime.dispatch(RuntimeAction {
                field: Some(AppModelField::DataExport),
                action: Action::Unload,
            });
        }
        Some(Loadable::Err(error)) => {
            ui.set_settings_export_loading(false);
            ui.set_settings_export_status(format!("Export failed: {error:?}").into());
        }
    }
}
