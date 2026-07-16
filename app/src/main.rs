#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

slint::include_modules!();

use std::sync::Arc;

use stremio_core::{
    models::{
        calendar::Calendar,
        catalog_with_filters::CatalogWithFilters,
        catalogs_with_extra::CatalogsWithExtra,
        continue_watching_preview::ContinueWatchingPreview,
        ctx::Ctx,
        installed_addons_with_filters::InstalledAddonsWithFilters,
        library_with_filters::{ContinueWatchingFilter, LibraryWithFilters, NotRemovedFilter},
        local_search::LocalSearch,
        player::Player,
        streaming_server::StreamingServer,
    },
    runtime::{
        Env, Runtime, RuntimeAction,
        msg::{Action, ActionLoad},
    },
    types::{addon::Descriptor, resource::MetaItemPreview},
};

use core_env::DesktopEnv;

mod config;
pub mod db;
pub mod image_cache;
mod models;
mod mpv_integration;
mod performance;
mod playback;
mod shortcuts;

// Modular sub-files
mod app_model;
mod callbacks;
mod discord;
mod event_loop;
mod logger;
mod navigation;
mod theintrodb;

// Re-exports/Usage
pub use app_model::{AppModel, AppModelField, get_icon_data};
pub use discord::DiscordRpc;
pub use navigation::{DetailsPresentation, NavigationController, NavigationIntent, Tab};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Core callbacks may originate on native threads (notably libmpv's actor),
    // so register the process runtime before any model or playback work starts.
    core_env::install_runtime_handle(tokio::runtime::Handle::current());

    // 1. Initialize durable logging before any fallible application setup.
    let profile = performance::ProfileConfig::from_args(std::env::args());

    // Initialize logger and keep workers alive
    let _guards = logger::init_logger(&profile)?;
    tracing::info!("Starting Stremio-Rust GUI client...");

    let res = run_app(&profile).await;
    if let Err(ref e) = res {
        tracing::error!(error = ?e, "Stremio-Rust execution failed with error");
        let _ = db::insert_log("ERROR", &format!("Application crash: {:?}", e)).await;
    }
    res
}

async fn run_app(profile_config: &performance::ProfileConfig) -> anyhow::Result<()> {
    let _run_span = tracing::info_span!("run_app").entered();

    // 1. Initialize local database
    db::init_db(std::path::PathBuf::from("storage")).await?;

    // 2. Initialize application configuration
    config::init_config().await;

    // 3. Load Config File
    let config = {
        let _span = tracing::info_span!("load_config").entered();
        config::load_config()
    };

    // MPV's render API needs a native OpenGL context even when video decoding
    // itself is configured for software fallback.
    unsafe {
        std::env::set_var("SLINT_BACKEND", "winit-femtovg");
    }

    // Initialize the embedded stream server
    let server_cfg = stream_server::ServerConfig {
        http_addr: std::net::SocketAddr::from(([127, 0, 0, 1], config.torrent_port)),
        print_startup: true,
        init_logging: false,
        ..stream_server::ServerConfig::embedded()
    };

    tracing::info!("Launching stream-server engine...");
    let server_handle = tokio::task::spawn_blocking(move || stream_server::start(server_cfg))
        .await
        .map_err(|e| anyhow::anyhow!("Failed to spawn blocking task: {}", e))?
        .map_err(|e| anyhow::anyhow!("Failed to start streaming server: {}", e))?;

    // Icon fonts are registered/embedded at compile time via app.slint imports.
    tracing::info!("Icon fonts registered at compile time.");

    // 5. Initialize Slint MainWindow UI
    let ui = MainWindow::new()?;
    tracing::info!("MainWindow created");
    let ui_weak = ui.as_weak();

    // Apply Dynamic Theme to Slint Global Theme Singleton
    let theme = ui.global::<Theme>();
    if let Some(c) = config::parse_color(&config.theme.background) {
        theme.set_background(c);
    }
    if let Some(c) = config::parse_color(&config.theme.secondary_background) {
        theme.set_secondary_background(c);
    }
    if let Some(c) = config::parse_color(&config.theme.sidebar_background) {
        theme.set_sidebar_background(c);
    }
    if let Some(c) = config::parse_color(&config.theme.modal_background) {
        theme.set_modal_background(c);
    }
    if let Some(c) = config::parse_color(&config.theme.drawer_background) {
        theme.set_drawer_background(c);
    }
    if let Some(c) = config::parse_color(&config.theme.control_background) {
        theme.set_control_background(c);
    }
    if let Some(c) = config::parse_color(&config.theme.overlay) {
        theme.set_overlay(c);
    }
    if let Some(c) = config::parse_color(&config.theme.overlay_hover) {
        theme.set_overlay_hover(c);
    }
    if let Some(c) = config::parse_color(&config.theme.overlay_pressed) {
        theme.set_overlay_pressed(c);
    }
    if let Some(c) = config::parse_color(&config.theme.divider) {
        theme.set_divider(c);
    }
    if let Some(c) = config::parse_color(&config.theme.scrim) {
        theme.set_scrim(c);
    }
    if let Some(c) = config::parse_color(&config.theme.accent) {
        theme.set_accent(c);
    }
    if let Some(c) = config::parse_color(&config.theme.accent_hover) {
        theme.set_accent_hover(c);
    }
    if let Some(c) = config::parse_color(&config.theme.success) {
        theme.set_success(c);
    }
    if let Some(c) = config::parse_color(&config.theme.warning) {
        theme.set_warning(c);
    }
    if let Some(c) = config::parse_color(&config.theme.info) {
        theme.set_info(c);
    }
    if let Some(c) = config::parse_color(&config.theme.danger) {
        theme.set_danger(c);
    }
    if let Some(c) = config::parse_color(&config.theme.focus) {
        theme.set_focus(c);
    }
    if let Some(c) = config::parse_color(&config.theme.title_bar) {
        theme.set_title_bar(c);
    }
    if let Some(c) = config::parse_color(&config.theme.card_background) {
        theme.set_card_background(c);
    }
    if let Some(c) = config::parse_color(&config.theme.card_border) {
        theme.set_card_border(c);
    }
    if let Some(c) = config::parse_color(&config.theme.text_primary) {
        theme.set_text_primary(c);
    }
    if let Some(c) = config::parse_color(&config.theme.text_secondary) {
        theme.set_text_secondary(c);
    }
    if let Some(c) = config::parse_color(&config.theme.text_muted) {
        theme.set_text_muted(c);
    }
    if let Some(c) = config::parse_color(&config.theme.skeleton_base) {
        theme.set_skeleton_base(c);
    }
    if let Some(c) = config::parse_color(&config.theme.skeleton_shimmer) {
        theme.set_skeleton_shimmer(c);
    }

    // Load and set UI icons
    ui.set_board_icon(get_icon_data(iconflow::Pack::Lucide, "home"));
    ui.set_discover_icon(get_icon_data(iconflow::Pack::Lucide, "compass"));
    ui.set_library_icon(get_icon_data(iconflow::Pack::Lucide, "folder"));
    ui.set_calendar_icon(get_icon_data(iconflow::Pack::Lucide, "calendar-days"));
    ui.set_addons_icon(get_icon_data(iconflow::Pack::Lucide, "toy-brick"));
    ui.set_settings_icon(get_icon_data(iconflow::Pack::Lucide, "settings"));
    ui.set_logout_icon(get_icon_data(iconflow::Pack::Lucide, "log-out"));
    ui.set_mail_icon(get_icon_data(iconflow::Pack::Lucide, "mail"));
    ui.set_lock_icon(get_icon_data(iconflow::Pack::Lucide, "lock"));
    ui.set_search_icon(get_icon_data(iconflow::Pack::Lucide, "search"));
    ui.set_facebook_icon(get_icon_data(iconflow::Pack::Bootstrap, "facebook"));
    ui.set_apple_icon(get_icon_data(iconflow::Pack::Bootstrap, "apple"));

    // Set player-specific icons
    ui.set_icon_play(get_icon_data(iconflow::Pack::Lucide, "play"));
    ui.set_icon_pause(get_icon_data(iconflow::Pack::Lucide, "pause"));
    ui.set_icon_next(get_icon_data(iconflow::Pack::Lucide, "skip-forward"));
    ui.set_icon_volume_high(get_icon_data(iconflow::Pack::Lucide, "volume-2"));
    ui.set_icon_volume_low(get_icon_data(iconflow::Pack::Lucide, "volume-1"));
    ui.set_icon_volume_mute(get_icon_data(iconflow::Pack::Lucide, "volume-x"));
    ui.set_icon_fullscreen(get_icon_data(iconflow::Pack::Lucide, "expand"));
    ui.set_icon_subtitles(get_icon_data(iconflow::Pack::Lucide, "message-square"));
    ui.set_icon_audio(get_icon_data(iconflow::Pack::Lucide, "music"));
    ui.set_icon_speed(get_icon_data(iconflow::Pack::Lucide, "gauge"));
    ui.set_icon_stats(get_icon_data(iconflow::Pack::Lucide, "activity"));
    ui.set_icon_options(get_icon_data(iconflow::Pack::Lucide, "sliders"));
    ui.set_icon_menu(get_icon_data(iconflow::Pack::Lucide, "menu"));
    ui.set_icon_back(get_icon_data(iconflow::Pack::Lucide, "arrow-left"));
    ui.set_refresh_icon(get_icon_data(iconflow::Pack::Lucide, "refresh-cw"));
    ui.set_folder_icon(get_icon_data(iconflow::Pack::Lucide, "folder-open"));
    ui.set_icon_link(get_icon_data(iconflow::Pack::Lucide, "link"));
    ui.set_icon_magnet(get_icon_data(iconflow::Pack::Lucide, "magnet"));
    ui.set_icon_download(get_icon_data(iconflow::Pack::Lucide, "download"));
    ui.set_icon_eye(get_icon_data(iconflow::Pack::Lucide, "eye"));
    ui.set_icon_eye_off(get_icon_data(iconflow::Pack::Lucide, "eye-off"));
    ui.set_icon_clapperboard(get_icon_data(iconflow::Pack::Lucide, "clapperboard"));

    // Set initial configuration parameters
    let navigation = NavigationController::new(config.active_tab);
    navigation.project(&ui);
    ui.set_server_url(format!("http://{}", server_handle.http_addr()).into());
    ui.set_server_status("Online".into());
    ui.set_settings_hardware_acceleration(config.hardware_acceleration);
    ui.set_settings_tidb_api_key(config.tidb_api_key.clone().into());
    ui.set_settings_tidb_show_intro(config.tidb_show_intro);
    ui.set_settings_tidb_show_recap(config.tidb_show_recap);
    ui.set_settings_tidb_show_credits(config.tidb_show_credits);
    ui.set_settings_tidb_show_preview(config.tidb_show_preview);

    // 6. Initialize stremio-core Storage Buckets & Ctx
    let (
        profile,
        library,
        streams_bucket,
        server_urls,
        notifications,
        search_history,
        dismissed_events,
    ) = {
        let _span = tracing::info_span!("load_all_storage_buckets").entered();

        let profile_path = std::path::PathBuf::from("storage").join("profile.json");
        let library_recent_path = std::path::PathBuf::from("storage").join("library_recent.json");
        let library_path = std::path::PathBuf::from("storage").join("library.json");
        let profile_size = std::fs::metadata(&profile_path)
            .map(|m| m.len())
            .unwrap_or(0);
        let library_recent_size = std::fs::metadata(&library_recent_path)
            .map(|m| m.len())
            .unwrap_or(0);
        let library_size = std::fs::metadata(&library_path)
            .map(|m| m.len())
            .unwrap_or(0);
        tracing::info!(
            profile_size_bytes = profile_size,
            library_recent_size_bytes = library_recent_size,
            library_size_bytes = library_size,
            "Startup: loaded database file sizes from disk"
        );

        let mut profile =
            DesktopEnv::get_storage::<stremio_core::types::profile::Profile>("profile")
                .await
                .unwrap_or_default()
                .unwrap_or_default();
        profile.settings.streaming_server_url =
            url::Url::parse(&format!("http://{}", server_handle.http_addr()))?;
        let (
            library_recent_result,
            library_result,
            streams_result,
            server_urls_result,
            notifications_result,
            search_history_result,
            dismissed_events_result,
        ) = tokio::join!(
            DesktopEnv::get_storage::<stremio_core::types::library::LibraryBucket>(
                "library_recent"
            ),
            DesktopEnv::get_storage::<stremio_core::types::library::LibraryBucket>("library"),
            DesktopEnv::get_storage::<stremio_core::types::streams::StreamsBucket>("streams"),
            DesktopEnv::get_storage::<stremio_core::types::server_urls::ServerUrlsBucket>(
                "server_urls"
            ),
            DesktopEnv::get_storage::<stremio_core::types::notifications::NotificationsBucket>(
                "notifications"
            ),
            DesktopEnv::get_storage::<stremio_core::types::search_history::SearchHistoryBucket>(
                "search_history"
            ),
            DesktopEnv::get_storage::<stremio_core::types::events::DismissedEventsBucket>(
                "dismissed_events"
            ),
        );

        let mut library = stremio_core::types::library::LibraryBucket::new(profile.uid(), vec![]);
        if let Some(recent_bucket) = library_recent_result.unwrap_or_default() {
            library.merge_bucket(recent_bucket);
        }
        if let Some(other_bucket) = library_result.unwrap_or_default() {
            library.merge_bucket(other_bucket);
        }
        let streams_bucket = streams_result
            .unwrap_or_default()
            .unwrap_or_else(|| stremio_core::types::streams::StreamsBucket::new(profile.uid()));
        let server_urls = server_urls_result.unwrap_or_default().unwrap_or_else(|| {
            stremio_core::types::server_urls::ServerUrlsBucket::new::<DesktopEnv>(profile.uid())
        });
        let notifications = notifications_result.unwrap_or_default().unwrap_or_else(|| {
            stremio_core::types::notifications::NotificationsBucket::new::<DesktopEnv>(
                profile.uid(),
                vec![],
            )
        });
        let search_history = search_history_result
            .unwrap_or_default()
            .unwrap_or_else(|| {
                stremio_core::types::search_history::SearchHistoryBucket::new(profile.uid())
            });
        let dismissed_events = dismissed_events_result
            .unwrap_or_default()
            .unwrap_or_else(|| {
                stremio_core::types::events::DismissedEventsBucket::new(profile.uid())
            });

        tracing::info!(
            addons_count = profile.addons.len(),
            library_items_count = library.items.len(),
            notifications_count = notifications.items.len(),
            search_history_count = search_history.items.len(),
            "Startup: loaded storage items metadata"
        );

        (
            profile,
            library,
            streams_bucket,
            server_urls,
            notifications,
            search_history,
            dismissed_events,
        )
    };

    let (continue_watching_preview, continue_watching_preview_effects) =
        ContinueWatchingPreview::new(&library, &notifications);
    let (discover, discover_effects) = CatalogWithFilters::<MetaItemPreview>::new(&profile);
    let (library_, library_effects) =
        LibraryWithFilters::<NotRemovedFilter>::new(&library, &notifications);
    let (continue_watching, continue_watching_effects) =
        LibraryWithFilters::<ContinueWatchingFilter>::new(&library, &notifications);
    let (remote_addons, remote_addons_effects) = CatalogWithFilters::<Descriptor>::new(&profile);
    let (installed_addons, installed_addons_effects) = InstalledAddonsWithFilters::new(&profile);
    let (streaming_server, streaming_server_effects) = StreamingServer::new::<DesktopEnv>(&profile);
    let (local_search, local_search_effects) = LocalSearch::new::<DesktopEnv>();
    let board = CatalogsWithExtra::default();
    let search = CatalogsWithExtra::default();

    let model = AppModel {
        ctx: Ctx::new(
            profile,
            library,
            streams_bucket,
            server_urls,
            notifications,
            search_history,
            dismissed_events,
        ),
        auth_link: Default::default(),
        data_export: Default::default(),
        continue_watching_preview,
        board,
        discover,
        library: library_,
        continue_watching,
        search,
        local_search,
        calendar: Calendar::default(),
        meta_details: Default::default(),
        player: Player {
            collect_seek_logs: true,
            ..Default::default()
        },
        remote_addons,
        installed_addons,
        addon_details: Default::default(),
        streaming_server,
    };

    let mut all_effects = Vec::new();
    all_effects.extend(continue_watching_preview_effects);
    all_effects.extend(discover_effects);
    all_effects.extend(library_effects);
    all_effects.extend(continue_watching_effects);
    all_effects.extend(remote_addons_effects);
    all_effects.extend(installed_addons_effects);
    all_effects.extend(streaming_server_effects);
    all_effects.extend(local_search_effects);

    let (runtime, rx) = Runtime::<DesktopEnv, _>::new(model, all_effects, 1000);
    let runtime = Arc::new(runtime);

    // Patch completed images directly into the existing Slint models.
    {
        let ui_weak_refresh = ui_weak.clone();
        image_cache::set_refresh_callback(move |completed_urls| {
            if let Some(ui) = ui_weak_refresh.upgrade() {
                models::refresh_cached_media_images(&ui, &completed_urls);
            }
        });
    }

    ui.on_request_poster(|url| image_cache::request_image(url.as_str()));

    // Register tab changed callback to force a sync of the newly active tab's page
    {
        let rt = runtime.clone();
        let ui_weak_tab = ui_weak.clone();
        let navigation_tab = navigation.clone();
        ui.on_tab_changed(move |tab| {
            let _tab_span = tracing::info_span!("Tab_Changed", tab = tab).entered();
            tracing::info!(tab = tab, "Active tab changed by user");
            let Ok(selected_tab) = Tab::try_from(tab) else {
                tracing::warn!(tab, "ignoring invalid tab navigation");
                return;
            };
            if let Some(ui) = ui_weak_tab.upgrade() {
                navigation_tab.dispatch_and_project(&ui, NavigationIntent::SelectTab(selected_tab));
                ui.set_loading(false);
            }
            queue_tab_sync(
                selected_tab,
                rt.clone(),
                ui_weak_tab.clone(),
                navigation_tab.clone(),
            );
            if selected_tab == Tab::Calendar {
                if let Some(ui) = ui_weak_tab.upgrade() {
                    // Calendar navigation has its own loading state. Reusing
                    // the global page flag could leave Board in its loading
                    // layout if a calendar request completed after a tab race.
                    ui.set_calendar_loading(true);
                }
                rt.dispatch(RuntimeAction {
                    field: None,
                    action: Action::Load(ActionLoad::Calendar(None)),
                });
            }
        });
    }

    let discord_rpc = Arc::new(discord::DiscordRpc::new());
    let playback_selections = Arc::new(playback::PlaybackSelections::default());
    let hardware_decoding = runtime
        .model()
        .ok()
        .map(|model| model.ctx.profile.settings.hardware_decoding)
        .unwrap_or(config.hardware_acceleration);
    let mut native_playback = match mpv_integration::NativePlayback::start(
        &ui,
        &runtime,
        hardware_decoding,
        navigation.clone(),
        discord_rpc.clone(),
        tokio::runtime::Handle::current(),
    ) {
        Ok(playback) => Some(playback),
        Err(error) => {
            tracing::error!(%error, "native MPV playback is unavailable");
            None
        }
    };
    let native_playback_bridge = native_playback
        .as_ref()
        .map(mpv_integration::NativePlayback::bridge);

    // 7. Spawn Stremio-Core event loop receiver to sync with Slint UI
    event_loop::start_event_loop(
        rx,
        runtime.clone(),
        ui_weak.clone(),
        playback_selections.clone(),
        native_playback_bridge.clone(),
        navigation.clone(),
        discord_rpc.clone(),
    );

    // 8. Hook up Slint callbacks to Ctx and Action dispatches
    callbacks::setup_ui_callbacks(
        &ui,
        &runtime,
        &playback_selections,
        &native_playback_bridge,
        ui_weak.clone(),
        &config,
        navigation.clone(),
    );
    shortcuts::install_platform_shortcuts(&ui);

    // 10. Run the Slint main window event loop
    tracing::info!("Stremio-Rust GUI loop starting...");
    let performance_reporter = profile_config
        .mode
        .enabled()
        .then(performance::spawn_reporter)
        .flatten();

    // Automatically load the catalogs upon startup
    callbacks::trigger_initial_load(&runtime, &navigation);

    let ui_result = ui.run();
    if let Some(reporter) = performance_reporter {
        reporter.abort();
    }
    let hide_result = ui.hide();
    drop(ui);

    let playback_result = match native_playback.take() {
        Some(playback) => playback.shutdown(),
        None => Ok(()),
    };

    if let Err(error) = server_handle.shutdown() {
        tracing::warn!(%error, "stream-server was already stopped");
    }
    let server_result = server_handle.join();

    ui_result?;
    hide_result?;
    playback_result?;
    match server_result? {
        Some(source) => tracing::info!(?source, "stream-server stopped"),
        None => tracing::info!("stream-server stopped"),
    }
    Ok(())
}

fn queue_tab_sync(
    tab: Tab,
    runtime: Arc<Runtime<DesktopEnv, AppModel>>,
    ui_weak: slint::Weak<MainWindow>,
    navigation: NavigationController,
) {
    match tab {
        Tab::Board => {
            tokio::spawn(async move {
                let snapshot = runtime.model().ok().map(|model| {
                    (
                        model.continue_watching_preview.clone(),
                        model.board.clone(),
                        model.ctx.profile.addons.clone(),
                    )
                });
                let Some((continue_watching, board, addons)) = snapshot else {
                    return;
                };
                let ui_weak_sync = ui_weak.clone();
                let runtime_sync = runtime.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if navigation.active_tab_index() != Tab::Board.index() {
                        return;
                    }
                    if let Some(ui) = ui_weak.upgrade() {
                        models::board::sync(
                            &ui,
                            &continue_watching,
                            &board,
                            &addons,
                            &ui_weak_sync,
                            &runtime_sync,
                        );
                    }
                });
            });
        }
        Tab::Discover => {
            crate::models::discover::clear_sync_state();
            tokio::spawn(async move {
                let snapshot = runtime.model().ok().map(|model| model.discover.clone());
                let Some(discover) = snapshot else {
                    return;
                };
                let ui_weak_sync = ui_weak.clone();
                let runtime_sync = runtime.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if navigation.active_tab_index() != Tab::Discover.index() {
                        return;
                    }
                    if let Some(ui) = ui_weak.upgrade() {
                        models::discover::sync(&ui, &discover, &ui_weak_sync, &runtime_sync);
                    }
                });
            });
        }
        Tab::Library => {
            crate::models::library::clear_sync_state();
            tokio::spawn(async move {
                let snapshot = runtime.model().ok().map(|model| model.library.clone());
                let Some(library) = snapshot else {
                    return;
                };
                let ui_weak_sync = ui_weak.clone();
                let runtime_sync = runtime.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if navigation.active_tab_index() != Tab::Library.index() {
                        return;
                    }
                    if let Some(ui) = ui_weak.upgrade() {
                        models::library::sync(&ui, &library, &ui_weak_sync, &runtime_sync);
                    }
                });
            });
        }
        Tab::Addons => {
            tokio::spawn(async move {
                let snapshot = runtime.model().ok().map(|model| {
                    (
                        model.remote_addons.clone(),
                        model.ctx.profile.addons.clone(),
                    )
                });
                let Some((remote_addons, installed_addons)) = snapshot else {
                    return;
                };
                let ui_weak_sync = ui_weak.clone();
                let runtime_sync = runtime.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if navigation.active_tab_index() != Tab::Addons.index() {
                        return;
                    }
                    if let Some(ui) = ui_weak.upgrade() {
                        models::addons::sync(
                            &ui,
                            &remote_addons,
                            &installed_addons,
                            &ui_weak_sync,
                            &runtime_sync,
                        );
                    }
                });
            });
        }
        Tab::Calendar => {
            tokio::spawn(async move {
                let snapshot = runtime.model().ok().map(|model| model.calendar.clone());
                let Some(calendar) = snapshot else {
                    return;
                };
                let ui_weak_sync = ui_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if navigation.active_tab_index() != Tab::Calendar.index() {
                        return;
                    }
                    if let Some(ui) = ui_weak.upgrade() {
                        models::calendar::sync(&ui, &calendar, &ui_weak_sync);
                    }
                });
            });
        }
        Tab::Settings => {}
    }
}
