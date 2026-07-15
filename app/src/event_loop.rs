use crate::{
    ACTIVE_TAB, MainWindow, StreamLink,
    app_model::{AppModel, AppModelField, format_rate},
    image_cache, models,
    mpv_integration::NativePlaybackBridge,
    playback::PlaybackSelections,
};
use core_env::DesktopEnv;
use futures::StreamExt;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use stremio_core::{
    models::common::Loadable,
    runtime::{Runtime, RuntimeEvent, msg::Event},
};

const STATE_COALESCE_WINDOW: std::time::Duration = std::time::Duration::from_millis(4);

pub fn start_event_loop(
    mut rx: futures::channel::mpsc::Receiver<RuntimeEvent<DesktopEnv, AppModel>>,
    runtime: Arc<Runtime<DesktopEnv, AppModel>>,
    ui_weak: slint::Weak<MainWindow>,
    playback_selections: Arc<PlaybackSelections>,
    native_playback_bridge: Option<NativePlaybackBridge>,
) {
    tokio::spawn(async move {
        let mut first_render = true;
        while let Some(event) = rx.next().await {
            match event {
                RuntimeEvent::NewState(mut fields, ..) => {
                    // Core effects frequently resolve in short bursts. Merge those fields before
                    // projecting anything into Slint so one logical update produces one UI patch.
                    let deadline = tokio::time::Instant::now() + STATE_COALESCE_WINDOW;
                    loop {
                        match tokio::time::timeout_at(deadline, rx.next()).await {
                            Ok(Some(RuntimeEvent::NewState(next_fields, ..))) => {
                                for field in next_fields {
                                    if !fields.contains(&field) {
                                        fields.push(field);
                                    }
                                }
                            }
                            Ok(Some(RuntimeEvent::CoreEvent(event))) => {
                                handle_core_event(event, &ui_weak);
                            }
                            Ok(None) | Err(_) => break,
                        }
                    }
                    let _state_span = tracing::info_span!("NewState", ?fields).entered();
                    let state_started = std::time::Instant::now();
                    let lock_start = std::time::Instant::now();
                    let model = runtime.model().expect("model read failed");
                    let lock_elapsed = lock_start.elapsed().as_millis();
                    if lock_elapsed > 15 {
                        tracing::warn!(
                            elapsed_ms = lock_elapsed,
                            "Model read lock acquisition took too long in NewState"
                        );
                    }
                    let ui_weak_clone = ui_weak.clone();

                    let active_tab = ACTIVE_TAB.load(Ordering::Relaxed);
                    let profile_sync_needed = first_render || fields.contains(&AppModelField::Ctx);
                    let auth_update = profile_sync_needed.then(|| {
                        let email = model
                            .ctx
                            .profile
                            .auth
                            .as_ref()
                            .map(|auth| auth.user.email.clone())
                            .unwrap_or_default();
                        let avatar = model
                            .ctx
                            .profile
                            .auth
                            .as_ref()
                            .and_then(|auth| auth.user.avatar.clone());
                        (email, avatar)
                    });

                    let board_sync_needed = (first_render
                        || fields.contains(&AppModelField::ContinueWatching)
                        || fields.contains(&AppModelField::ContinueWatchingPreview)
                        || fields.contains(&AppModelField::Board)
                        || fields.contains(&AppModelField::Ctx))
                        && active_tab == 0;
                    let discover_sync_needed = (first_render
                        || fields.contains(&AppModelField::Discover))
                        && active_tab == 1;
                    let library_sync_needed = (first_render
                        || fields.contains(&AppModelField::Library))
                        && active_tab == 2;
                    let addons_sync_needed = (first_render
                        || fields.contains(&AppModelField::RemoteAddons)
                        || fields.contains(&AppModelField::InstalledAddons)
                        || fields.contains(&AppModelField::Ctx))
                        && active_tab == 3;
                    let addon_details_sync_needed = fields.contains(&AppModelField::AddonDetails);
                    let meta_details_changed = fields.contains(&AppModelField::MetaDetails);
                    let details_sync_needed =
                        meta_details_changed || fields.contains(&AppModelField::Ctx);
                    let settings_sync_needed =
                        first_render || (fields.contains(&AppModelField::Ctx) && active_tab == 4);
                    let data_export_sync_needed =
                        fields.contains(&AppModelField::DataExport) && active_tab == 4;
                    let calendar_sync_needed = (first_render
                        || fields.contains(&AppModelField::Calendar)
                        || fields.contains(&AppModelField::Ctx))
                        && active_tab == 5;
                    let search_sync_needed = (first_render
                        || fields.contains(&AppModelField::Search)
                        || fields.contains(&AppModelField::Ctx))
                        && active_tab == 6;
                    let local_search_sync_needed = fields.contains(&AppModelField::LocalSearch);
                    let player_sync_needed = fields.contains(&AppModelField::Player);
                    let streaming_stats_sync_needed =
                        first_render || fields.contains(&AppModelField::StreamingServer);

                    first_render = false;

                    // Clone core submodels for thread-safe UI thread updates (only if needed)
                    let continue_watching_cloned = if board_sync_needed {
                        Some(model.continue_watching_preview.clone())
                    } else {
                        None
                    };
                    let board_cloned = if board_sync_needed {
                        Some(model.board.clone())
                    } else {
                        None
                    };
                    let addons_cloned = if board_sync_needed || addons_sync_needed {
                        Some(model.ctx.profile.addons.clone())
                    } else {
                        None
                    };
                    let discover_cloned = if discover_sync_needed {
                        Some(model.discover.clone())
                    } else {
                        None
                    };
                    let library_cloned = if library_sync_needed {
                        Some(model.library.clone())
                    } else {
                        None
                    };
                    let remote_addons_cloned = if addons_sync_needed {
                        Some(model.remote_addons.clone())
                    } else {
                        None
                    };
                    let addon_details_cloned =
                        addon_details_sync_needed.then(|| model.addon_details.clone());
                    let meta_details_cloned = if details_sync_needed {
                        Some(model.meta_details.clone())
                    } else {
                        None
                    };
                    let library_bucket_cloned = if details_sync_needed {
                        Some(model.ctx.library.clone())
                    } else {
                        None
                    };
                    let settings_cloned = if settings_sync_needed {
                        Some(model.ctx.profile.settings.clone())
                    } else {
                        None
                    };
                    let data_export_cloned =
                        data_export_sync_needed.then(|| model.data_export.clone());
                    let calendar_cloned = if calendar_sync_needed {
                        Some(model.calendar.clone())
                    } else {
                        None
                    };
                    let search_cloned = search_sync_needed.then(|| model.search.clone());
                    let search_profile_cloned =
                        search_sync_needed.then(|| model.ctx.profile.clone());
                    let local_search_cloned =
                        local_search_sync_needed.then(|| model.local_search.clone());
                    let streaming_stats = streaming_stats_sync_needed
                        .then(|| model.streaming_server.statistics.clone())
                        .flatten()
                        .and_then(|statistics| match statistics {
                            Loadable::Ready(statistics) => Some((
                                format_rate(statistics.download_speed),
                                format_rate(statistics.upload_speed),
                                i32::try_from(statistics.peers).unwrap_or(i32::MAX),
                                (statistics.stream_progress * 100.0).clamp(0.0, 100.0) as f32,
                            )),
                            _ => None,
                        });

                    let stream_selection_views = details_sync_needed
                        .then(|| {
                            playback_selections
                                .rebuild(&model.meta_details, &model.ctx.profile.addons)
                        })
                        .unwrap_or_default();
                    let trailer_selection_id = details_sync_needed
                        .then(|| playback_selections.trailer_id())
                        .flatten();
                    let player_cloned = player_sync_needed.then(|| model.player.clone());

                    // Drop the model guard before invoking event loop
                    drop(model);
                    if let (Some(playback), Some(player)) =
                        (&native_playback_bridge, player_cloned.as_ref())
                    {
                        playback.sync_player(player, &ui_weak_clone);
                    }
                    let ui_weak_for_sync = ui_weak_clone.clone();
                    let runtime_for_sync = runtime.clone();

                    let _ = slint::invoke_from_event_loop(move || {
                        let patch_started = std::time::Instant::now();
                        let _ui_span = tracing::info_span!("UI_Thread_Sync").entered();
                        if let Some(ui) = ui_weak_clone.upgrade() {
                            if let Some((email, avatar_url)) = auth_update.as_ref() {
                                let current_username = ui.get_username().to_string();
                                if !email.is_empty() {
                                    ui.set_username(email.clone().into());
                                    let avatar_letter = email
                                        .chars()
                                        .next()
                                        .unwrap_or('U')
                                        .to_string()
                                        .to_uppercase();
                                    ui.set_avatar_letter(avatar_letter.into());

                                    if let Some(url_str) = avatar_url {
                                        if let Ok(url) = url::Url::parse(url_str) {
                                            let img = image_cache::get_poster_image(
                                                &Some(url),
                                                &ui_weak_for_sync,
                                            );
                                            ui.set_avatar_image(img);
                                            ui.set_has_avatar_image(true);
                                        } else {
                                            ui.set_has_avatar_image(false);
                                        }
                                    } else {
                                        ui.set_has_avatar_image(false);
                                    }
                                } else if current_username != "Guest" {
                                    ui.set_username("".into());
                                    ui.set_has_avatar_image(false);
                                }
                            }

                            if board_sync_needed
                                || discover_sync_needed
                                || library_sync_needed
                                || addons_sync_needed
                                || details_sync_needed
                                || calendar_sync_needed
                                || search_sync_needed
                            {
                                ui.set_loading(false);
                            }

                            if streaming_stats_sync_needed {
                                if let Some((download, upload, peers, progress)) = &streaming_stats
                                {
                                    ui.set_player_download_speed(download.clone().into());
                                    ui.set_player_upload_speed(upload.clone().into());
                                    ui.set_player_peer_count(*peers);
                                    ui.set_player_stream_progress(*progress);
                                } else {
                                    ui.set_player_download_speed("".into());
                                    ui.set_player_upload_speed("".into());
                                    ui.set_player_peer_count(0);
                                    ui.set_player_stream_progress(0.0);
                                }
                            }

                            // Sync stream links
                            if details_sync_needed {
                                ui.set_detail_trailer_selection_id(
                                    trailer_selection_id.clone().unwrap_or_default().into(),
                                );
                                if let Some(meta_details) = &meta_details_cloned {
                                    let mut background_url = None;
                                    for resource in &meta_details.meta_items {
                                        if let Some(Loadable::Ready(item)) = &resource.content {
                                            background_url = item.preview.background.clone();
                                            break;
                                        }
                                    }
                                    let bg_image = crate::image_cache::get_poster_image(
                                        &background_url,
                                        &ui_weak_for_sync,
                                    );
                                    ui.set_detail_background(bg_image);
                                }

                                let stream_links: Vec<StreamLink> = stream_selection_views
                                    .into_iter()
                                    .map(|link| StreamLink {
                                        id: link.id.into(),
                                        name: link.name.into(),
                                        description: link.description.into(),
                                        provider: link.provider.into(),
                                    })
                                    .collect();
                                let mut providers = vec![slint::SharedString::from("All")];
                                for stream in &stream_links {
                                    if !providers.iter().any(|provider| provider == &stream.provider)
                                    {
                                        providers.push(stream.provider.clone());
                                    }
                                }
                                let stream_model = slint::VecModel::from(stream_links);
                                ui.set_stream_links(slint::ModelRc::new(stream_model));
                                ui.set_detail_stream_providers(slint::ModelRc::new(
                                    slint::VecModel::from(providers),
                                ));
                                if let Some(meta_details) = &meta_details_cloned {
                                    let loading_count = meta_details
                                        .streams
                                        .iter()
                                        .filter(|resource| {
                                            matches!(resource.content, Some(Loadable::Loading))
                                        })
                                        .count();
                                    ui.set_detail_stream_loading_count(
                                        i32::try_from(loading_count).unwrap_or(i32::MAX),
                                    );
                                }
                            }

                            // Sync submodels (only when needed AND viewing the corresponding active tab)
                            let active_tab = ui.get_active_tab();

                            if local_search_sync_needed {
                                if let Some(local_search) = &local_search_cloned {
                                    models::search::sync_local_search(
                                        &ui,
                                        local_search,
                                        &ui_weak_for_sync,
                                    );
                                }
                            }

                            if addon_details_sync_needed && ui.get_addon_details_open() {
                                if let Some(addon_details) = &addon_details_cloned {
                                    models::addons::sync_details(
                                        &ui,
                                        addon_details,
                                        &ui_weak_for_sync,
                                    );
                                }
                            }
                            if board_sync_needed && active_tab != 0 {
                                tracing::warn!(
                                    "Out-of-focus tab rendering warning: Board sync requested but active tab is {}",
                                    active_tab
                                );
                            }
                            if discover_sync_needed && active_tab != 1 {
                                tracing::warn!(
                                    "Out-of-focus tab rendering warning: Discover sync requested but active tab is {}",
                                    active_tab
                                );
                            }
                            if library_sync_needed && active_tab != 2 {
                                tracing::warn!(
                                    "Out-of-focus tab rendering warning: Library sync requested but active tab is {}",
                                    active_tab
                                );
                            }
                            if addons_sync_needed && active_tab != 3 {
                                tracing::warn!(
                                    "Out-of-focus tab rendering warning: Addons sync requested but active tab is {}",
                                    active_tab
                                );
                            }
                            if calendar_sync_needed && active_tab != 5 {
                                tracing::warn!(
                                    "Out-of-focus tab rendering warning: Calendar sync requested but active tab is {}",
                                    active_tab
                                );
                            }
                            if search_sync_needed && active_tab != 6 {
                                tracing::warn!(
                                    "Out-of-focus tab rendering warning: Search sync requested but active tab is {}",
                                    active_tab
                                );
                            }

                            if active_tab == 0 && board_sync_needed {
                                if let (Some(cw), Some(brd), Some(ads)) =
                                    (&continue_watching_cloned, &board_cloned, &addons_cloned)
                                {
                                    models::board::sync(
                                        &ui,
                                        cw,
                                        brd,
                                        ads,
                                        &ui_weak_for_sync,
                                        &runtime_for_sync,
                                    );
                                }
                            } else if active_tab == 1 && discover_sync_needed {
                                if let Some(disc) = &discover_cloned {
                                    models::discover::sync(
                                        &ui,
                                        disc,
                                        &ui_weak_for_sync,
                                        &runtime_for_sync,
                                    );
                                }
                            } else if active_tab == 2 && library_sync_needed {
                                if let Some(lib) = &library_cloned {
                                    models::library::sync(
                                        &ui,
                                        lib,
                                        &ui_weak_for_sync,
                                        &runtime_for_sync,
                                    );
                                }
                            } else if active_tab == 3 && addons_sync_needed {
                                if let (Some(rem), Some(ads)) =
                                    (&remote_addons_cloned, &addons_cloned)
                                {
                                    models::addons::sync(
                                        &ui,
                                        rem,
                                        ads,
                                        &ui_weak_for_sync,
                                        &runtime_for_sync,
                                    );
                                }
                            } else if active_tab == 5 && calendar_sync_needed {
                                if let Some(calendar) = &calendar_cloned {
                                    models::calendar::sync(&ui, calendar, &ui_weak_for_sync);
                                }
                            } else if active_tab == 6 && search_sync_needed {
                                if let (Some(search), Some(profile)) =
                                    (&search_cloned, &search_profile_cloned)
                                {
                                    models::search::sync_results(
                                        &ui,
                                        search,
                                        profile,
                                        &ui_weak_for_sync,
                                    );
                                }
                            }

                            // Details page overlays all tabs, so it is synced regardless of active_tab
                            if details_sync_needed
                                && (meta_details_changed
                                    || ui.get_show_details()
                                    || active_tab == 1)
                            {
                                if let (Some(det), Some(lib_b)) =
                                    (&meta_details_cloned, &library_bucket_cloned)
                                {
                                    models::details::sync(
                                        &ui,
                                        det,
                                        lib_b,
                                        &ui_weak_for_sync,
                                        &runtime_for_sync,
                                    );
                                }
                            }

                            if settings_sync_needed {
                                if let Some(set) = &settings_cloned {
                                    models::settings::sync(&ui, set);
                                }
                            }
                            if data_export_sync_needed {
                                if let Some(data_export) = &data_export_cloned {
                                    models::settings::sync_data_export(
                                        &ui,
                                        data_export,
                                        &runtime_for_sync,
                                    );
                                }
                            }
                        }
                        crate::performance::counters().record_ui_patch(patch_started.elapsed());
                    });
                    tracing::trace!(
                        elapsed_micros = state_started.elapsed().as_micros(),
                        "core state projected and queued"
                    );
                }
                RuntimeEvent::CoreEvent(event) => handle_core_event(event, &ui_weak),
            }
        }
    });
}

fn handle_core_event(event: Event, ui_weak: &slint::Weak<MainWindow>) {
    tracing::info!(?event, "core event");
    if let Event::Error { error, .. } = &event {
        let ui_weak = ui_weak.clone();
        let message = format!("{error:?}");
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_error_message(message.into());
                ui.set_loading(false);
            }
        });
    }
}
