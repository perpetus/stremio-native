use crate::AppModel;
use crate::EpisodeItem;
use crate::MainWindow;
use core_env::DesktopEnv;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex, OnceLock};
use stremio_core::{
    models::{
        common::Loadable,
        meta_details::{MetaDetails, Selected as DetailsSelected},
    },
    runtime::{
        Runtime, RuntimeAction,
        msg::{Action, ActionCtx, ActionLoad, ActionMetaDetails},
    },
    types::{
        addon::ResourcePath,
        library::LibraryBucket,
        resource::{MetaItem, Video},
    },
};

// Thread-safe caches to track season/episode indexes locally
static ACTIVE_SEASON: OnceLock<Mutex<i32>> = OnceLock::new();
static ACTIVE_EPISODE_IDX: OnceLock<Mutex<usize>> = OnceLock::new();
static EPISODE_SEARCH_QUERY: OnceLock<Mutex<String>> = OnceLock::new();
static LAST_LOADED_ID: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn get_active_season() -> &'static Mutex<i32> {
    ACTIVE_SEASON.get_or_init(|| Mutex::new(1))
}

fn get_active_episode_idx() -> &'static Mutex<usize> {
    ACTIVE_EPISODE_IDX.get_or_init(|| Mutex::new(0))
}

fn get_search_query() -> &'static Mutex<String> {
    EPISODE_SEARCH_QUERY.get_or_init(|| Mutex::new(String::new()))
}

/// Core function to load meta details and streams for an item
pub async fn load_meta_details(rt: &Arc<Runtime<DesktopEnv, AppModel>>, id: String) {
    load_meta_details_for_video(rt, id, None, None).await;
}

pub async fn load_meta_details_for_video(
    rt: &Arc<Runtime<DesktopEnv, AppModel>>,
    id: String,
    media_type: Option<String>,
    video_id: Option<String>,
) {
    let r_type = match media_type.filter(|value| !value.is_empty()) {
        Some(media_type) => media_type,
        None => {
            let model = rt.model().expect("model read failed");
            model
                .discover
                .selected
                .as_ref()
                .map(|selected| selected.request.path.r#type.clone())
                .unwrap_or_else(|| "movie".to_string())
        }
    };

    // Reset season/episode selections for new details lookup
    if let Ok(mut s) = get_active_season().lock() {
        *s = 1;
    }
    if let Ok(mut ep) = get_active_episode_idx().lock() {
        *ep = 0;
    }

    let meta_path = ResourcePath {
        resource: "meta".to_string(),
        r#type: r_type.clone(),
        id: id.clone(),
        extra: vec![],
    };

    let stream_id = video_id.filter(|value| !value.is_empty()).unwrap_or(id);

    let stream_path = ResourcePath {
        resource: "stream".to_string(),
        r#type: r_type,
        id: stream_id,
        extra: vec![],
    };

    rt.dispatch(RuntimeAction {
        field: None,
        action: Action::Load(ActionLoad::MetaDetails(DetailsSelected {
            meta_path,
            stream_path: Some(stream_path),
            guess_stream: false,
        })),
    });
}

pub fn setup(ui: &MainWindow, runtime: &Arc<Runtime<DesktopEnv, AppModel>>) {
    let ui_weak = ui.as_weak();

    // Toggle library callback (Add / Remove)
    ui.on_details_toggle_library({
        let runtime = runtime.clone();
        move || {
            let rt = runtime.clone();
            tokio::spawn(async move {
                let model = rt.model().expect("model read failed");
                if let Some(selected) = &model.meta_details.selected {
                    let id = selected.meta_path.id.clone();
                    // Find preview
                    let mut meta_preview = None;
                    for resource in &model.meta_details.meta_items {
                        if let Some(Loadable::Ready(meta_item)) = &resource.content {
                            if meta_item.preview.id == id {
                                meta_preview = Some(meta_item.preview.clone());
                                break;
                            }
                        }
                    }

                    let is_in_library = model
                        .ctx
                        .library
                        .items
                        .get(&id)
                        .map(|item| !item.removed)
                        .unwrap_or(false);
                    drop(model);

                    if is_in_library {
                        rt.dispatch(RuntimeAction {
                            field: None,
                            action: Action::Ctx(ActionCtx::RemoveFromLibrary(id)),
                        });
                    } else if let Some(preview) = meta_preview {
                        rt.dispatch(RuntimeAction {
                            field: None,
                            action: Action::Ctx(ActionCtx::AddToLibrary(preview)),
                        });
                    }
                }
            });
        }
    });

    ui.on_details_toggle_watched({
        let runtime = runtime.clone();
        move || {
            let is_watched = runtime
                .model()
                .ok()
                .and_then(|model| model.meta_details.library_item.clone())
                .map(|item| item.watched())
                .unwrap_or(false);
            runtime.dispatch(RuntimeAction {
                field: None,
                action: Action::MetaDetails(ActionMetaDetails::MarkAsWatched(!is_watched)),
            });
        }
    });

    // Season changed callback
    ui.on_details_season_changed({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        move |season| {
            if let Ok(mut s) = get_active_season().lock() {
                *s = season;
            }
            if let Ok(mut ep) = get_active_episode_idx().lock() {
                *ep = 0;
            } // reset episode

            let rt = runtime.clone();
            let ui_weak = ui_weak.clone();
            tokio::spawn(async move {
                reload_stream_for_selected_episode(&rt, &ui_weak).await;
            });
        }
    });

    // Episode changed callback
    ui.on_details_episode_changed({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        move |episode_idx| {
            if let Ok(mut ep) = get_active_episode_idx().lock() {
                *ep = episode_idx as usize;
            }

            let rt = runtime.clone();
            let ui_weak = ui_weak.clone();
            tokio::spawn(async move {
                reload_stream_for_selected_episode(&rt, &ui_weak).await;
            });
        }
    });

    // Search query changed callback
    ui.on_details_episode_search_changed({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        move |q| {
            if let Ok(mut query) = get_search_query().lock() {
                *query = q.to_string();
            }
            // Trigger sync to update the filtered list
            if let Some(ui) = ui_weak.upgrade() {
                if let Ok(model) = runtime.model() {
                    let ui_sync = ui_weak.clone();
                    let rt_sync = runtime.clone();
                    sync(
                        &ui,
                        &model.meta_details,
                        &model.ctx.library,
                        &ui_sync,
                        &rt_sync,
                    );
                }
            }
        }
    });

    // Toggle episode watched callback
    ui.on_details_toggle_episode_watched({
        let runtime = runtime.clone();
        move |video_id| {
            let rt = runtime.clone();
            let video_id = video_id.to_string();
            tokio::spawn(async move {
                let model = rt.model().expect("model read failed");

                // Find the video details
                let mut target_video = None;
                for resource in &model.meta_details.meta_items {
                    if let Some(Loadable::Ready(meta_item)) = &resource.content {
                        if let Some(v) = meta_item.videos.iter().find(|video| video.id == video_id)
                        {
                            target_video = Some(v.clone());
                            break;
                        }
                    }
                }

                if let Some(video) = target_video {
                    let is_watched = model
                        .meta_details
                        .watched
                        .as_ref()
                        .map(|watched| watched.get_video(&video.id))
                        .unwrap_or(false);
                    drop(model);

                    // Dispatch toggle watched action
                    rt.dispatch(RuntimeAction {
                        field: None,
                        action: Action::MetaDetails(ActionMetaDetails::MarkVideoAsWatched(
                            video,
                            !is_watched,
                        )),
                    });
                }
            });
        }
    });

    // Toggle season watched callback
    ui.on_details_toggle_season_watched({
        let runtime = runtime.clone();
        move |season| {
            let rt = runtime.clone();
            tokio::spawn(async move {
                let model = rt.model().expect("model read failed");
                if let Some(selected) = &model.meta_details.selected {
                    let id = selected.meta_path.id.clone();

                    // Find meta item to resolve videos list
                    let mut meta_item: Option<MetaItem> = None;
                    for resource in &model.meta_details.meta_items {
                        if let Some(Loadable::Ready(item)) = &resource.content {
                            if item.preview.id == id {
                                meta_item = Some(item.clone());
                                break;
                            }
                        }
                    }

                    if let Some(meta) = meta_item {
                        // Gather all videos in the target season
                        let season_videos: Vec<&Video> = meta
                            .videos
                            .iter()
                            .filter(|v| {
                                v.series_info
                                    .as_ref()
                                    .map(|info| info.season as i32 == season)
                                    .unwrap_or(false)
                            })
                            .collect();

                        if !season_videos.is_empty() {
                            // If ALL videos in the season are watched, we unmark them. Otherwise, mark them.
                            let mut all_watched = true;
                            if let Some(watched) = &model.meta_details.watched {
                                for v in &season_videos {
                                    if !watched.get_video(&v.id) {
                                        all_watched = false;
                                        break;
                                    }
                                }
                            } else {
                                all_watched = false;
                            }
                            drop(model);

                            rt.dispatch(RuntimeAction {
                                field: None,
                                action: Action::MetaDetails(
                                    ActionMetaDetails::MarkSeasonAsWatched(
                                        season as u32,
                                        !all_watched,
                                    ),
                                ),
                            });
                        }
                    }
                }
            });
        }
    });
}

async fn reload_stream_for_selected_episode(
    rt: &Arc<Runtime<DesktopEnv, AppModel>>,
    _ui_weak: &slint::Weak<MainWindow>,
) {
    let model = rt.model().expect("model read failed");
    if let Some(selected) = &model.meta_details.selected {
        let meta_path = selected.meta_path.clone();

        // Find meta item to resolve videos list
        let mut meta_item: Option<MetaItem> = None;
        for resource in &model.meta_details.meta_items {
            if let Some(Loadable::Ready(item)) = &resource.content {
                if item.preview.id == meta_path.id {
                    meta_item = Some(item.clone());
                    break;
                }
            }
        }

        if let Some(meta) = meta_item {
            let active_season = *get_active_season().lock().unwrap();
            let active_episode_idx = *get_active_episode_idx().lock().unwrap();

            // Filter episodes matching selected season
            let episodes: Vec<&Video> = meta
                .videos
                .iter()
                .filter(|v| {
                    v.series_info
                        .as_ref()
                        .map(|info| info.season as i32 == active_season)
                        .unwrap_or(false)
                })
                .collect();

            if let Some(video) = episodes.get(active_episode_idx) {
                let video_id = video.id.clone();
                drop(model);

                // Dispatch load details with specific episode ID for stream resolution
                rt.dispatch(RuntimeAction {
                    field: None,
                    action: Action::Load(ActionLoad::MetaDetails(DetailsSelected {
                        meta_path,
                        stream_path: Some(ResourcePath {
                            resource: "stream".to_string(),
                            r#type: "series".to_string(),
                            id: video_id,
                            extra: vec![],
                        }),
                        guess_stream: false,
                    })),
                });
                return;
            }
        }
    }
}

fn sync_series_details(ui: &MainWindow, meta_item: Option<MetaItem>) {
    let is_series = ui.get_detail_is_series();
    if is_series {
        if let Some(meta) = meta_item {
            // Get available seasons
            let mut seasons: Vec<i32> = meta
                .videos
                .iter()
                .filter_map(|v| v.series_info.as_ref().map(|info| info.season as i32))
                .collect();
            seasons.sort();
            seasons.dedup();

            let slint_seasons: Vec<i32> = seasons.clone();
            let seasons_model = slint::VecModel::from(slint_seasons);
            ui.set_detail_seasons(slint::ModelRc::new(seasons_model));

            // Default to season 1 if active season not found in list
            let active_season = {
                let mut s = get_active_season().lock().unwrap();
                if !seasons.contains(&*s) && !seasons.is_empty() {
                    *s = seasons[0];
                }
                *s
            };
            ui.set_detail_active_season(active_season);

            // Get episodes matching active season
            let episodes: Vec<&Video> = meta
                .videos
                .iter()
                .filter(|v| {
                    v.series_info
                        .as_ref()
                        .map(|info| info.season as i32 == active_season)
                        .unwrap_or(false)
                })
                .collect();

            let episode_names: Vec<slint::SharedString> = episodes
                .iter()
                .map(|v| {
                    let ep_num = v.series_info.as_ref().map(|info| info.episode).unwrap_or(0);
                    slint::SharedString::from(format!("Episode {}: {}", ep_num, v.title))
                })
                .collect();

            let ep_names_model = slint::VecModel::from(episode_names);
            ui.set_detail_episode_names(slint::ModelRc::new(ep_names_model));

            let active_episode_idx = {
                let mut ep = get_active_episode_idx().lock().unwrap();
                if *ep >= episodes.len() {
                    *ep = 0;
                }
                *ep
            };
            ui.set_detail_active_episode_idx(active_episode_idx as i32);
        }
    }
}

fn category_matches(category: &str, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| category.eq_ignore_ascii_case(candidate))
}

fn projected_links(meta: &MetaItem, categories: &[&str]) -> Vec<slint::SharedString> {
    meta.preview
        .links
        .iter()
        .filter(|link| category_matches(&link.category, categories))
        .map(|link| slint::SharedString::from(&link.name))
        .collect()
}

#[tracing::instrument(skip_all)]
pub fn sync(
    ui: &MainWindow,
    meta_details: &MetaDetails,
    library: &LibraryBucket,
    ui_weak: &slint::Weak<MainWindow>,
    _runtime: &Arc<Runtime<DesktopEnv, AppModel>>,
) {
    let mut detail_title = String::new();
    let mut detail_description = String::new();
    let mut detail_released_date = String::new();
    let mut detail_runtime = String::new();
    let mut detail_rating = String::new();
    let mut meta_item: Option<MetaItem> = None;

    for resource in &meta_details.meta_items {
        if let Some(Loadable::Ready(item)) = &resource.content {
            detail_title = item.preview.name.clone();
            detail_description = item.preview.description.clone().unwrap_or_default();
            detail_released_date = item
                .preview
                .release_info
                .clone()
                .or_else(|| item.preview.released.map(|dt| dt.format("%Y").to_string()))
                .unwrap_or_default();
            detail_runtime = item.preview.runtime.clone().unwrap_or_default();
            detail_rating = item
                .preview
                .links
                .iter()
                .find(|link| category_matches(&link.category, &["imdb"]))
                .map(|link| link.name.clone())
                .unwrap_or_default();
            meta_item = Some(item.clone());
            break;
        }
    }

    if meta_item.is_none() {
        return;
    }

    // Keep the underlying details state current even while Discover renders
    // the compact preview. Trailer playback and the Show action both reuse
    // these bindings without forcing a second metadata request.
    ui.set_detail_title(detail_title.clone().into());
    ui.set_detail_description(detail_description.clone().into());
    ui.set_detail_year(detail_released_date.clone().into());
    ui.set_detail_runtime(detail_runtime.clone().into());
    ui.set_detail_rating(detail_rating.clone().into());
    if let Some(meta) = &meta_item {
        ui.set_detail_poster(crate::image_cache::get_poster_image(
            &meta.preview.poster,
            ui_weak,
        ));
        ui.set_detail_has_trailer(!meta.preview.trailer_streams.is_empty());
    }

    let active_tab = ui.get_active_tab();
    let user_wants_full_details = ui.get_show_details();

    if active_tab == 1 && !user_wants_full_details {
        // User is browsing Discover page and has not clicked Play/Show yet:
        // Update the right-side split screen preview panel instead of navigating!
        ui.set_discover_preview_title(detail_title.into());
        ui.set_discover_preview_description(detail_description.into());
        ui.set_discover_preview_year(detail_released_date.into());
        ui.set_discover_preview_rating(detail_rating.into());
        ui.set_discover_preview_runtime(detail_runtime.into());
        ui.set_discover_has_preview(true);

        if let Some(meta) = &meta_item {
            let poster = crate::image_cache::get_poster_image(&meta.preview.poster, ui_weak);
            ui.set_discover_preview_poster(poster);

            // Sync genres from links
            let slint_genres = projected_links(meta, &["genre", "genres"]);
            let genres_model = slint::VecModel::from(slint_genres);
            ui.set_discover_preview_genres(slint::ModelRc::new(genres_model));

            // Sync cast from links
            let cast_names = projected_links(meta, &["actor", "actors", "cast"]);
            let cast_model = slint::VecModel::from(cast_names);
            ui.set_discover_preview_cast(slint::ModelRc::new(cast_model));

            let directors = projected_links(meta, &["director", "directors"]);
            ui.set_discover_preview_directors(slint::ModelRc::new(slint::VecModel::from(
                directors,
            )));
            ui.set_discover_preview_has_trailer(!meta.preview.trailer_streams.is_empty());
        }

        // Sync Library State for discover preview
        if let Some(selected) = &meta_details.selected {
            let is_in_library = library
                .items
                .get(&selected.meta_path.id)
                .map(|item| !item.removed)
                .unwrap_or(false);
            ui.set_discover_preview_is_in_library(is_in_library);
        }
        ui.set_discover_preview_is_watched(
            meta_details
                .library_item
                .as_ref()
                .map(|item| item.watched())
                .unwrap_or(false),
        );
    } else {
        // Otherwise (Board, Library, or if user clicked play):
        // Navigate to full details view overlay
        ui.set_detail_title(detail_title.into());
        ui.set_detail_description(detail_description.into());
        ui.set_detail_year(detail_released_date.into());
        ui.set_detail_runtime(detail_runtime.into());
        ui.set_detail_rating(detail_rating.into());
        ui.set_show_details(true);

        if let Some(meta) = &meta_item {
            let poster = crate::image_cache::get_poster_image(&meta.preview.poster, ui_weak);
            ui.set_detail_poster(poster);

            let genres = projected_links(meta, &["genre", "genres"]);
            ui.set_detail_genres(slint::ModelRc::new(slint::VecModel::from(genres)));

            let cast = projected_links(meta, &["actor", "actors", "cast"]);
            ui.set_detail_cast(slint::ModelRc::new(slint::VecModel::from(cast)));
            let directors = projected_links(meta, &["director", "directors"]);
            ui.set_detail_directors(slint::ModelRc::new(slint::VecModel::from(directors)));
            ui.set_detail_has_trailer(!meta.preview.trailer_streams.is_empty());
        }

        // Sync library details
        if let Some(selected) = &meta_details.selected {
            let is_in_library = library
                .items
                .get(&selected.meta_path.id)
                .map(|item| !item.removed)
                .unwrap_or(false);
            ui.set_detail_is_in_library(is_in_library);
        }
        ui.set_detail_is_watched(
            meta_details
                .library_item
                .as_ref()
                .map(|item| item.watched())
                .unwrap_or(false),
        );

        // Detect if loaded item has changed to reset in-stream-view/search query
        let current_id = meta_details
            .selected
            .as_ref()
            .map(|s| s.meta_path.id.clone());
        let id_changed = {
            let mut last_id_guard = LAST_LOADED_ID
                .get_or_init(|| Mutex::new(None))
                .lock()
                .unwrap();
            if *last_id_guard != current_id {
                *last_id_guard = current_id.clone();
                true
            } else {
                false
            }
        };

        if id_changed {
            if let Ok(mut query) = get_search_query().lock() {
                query.clear();
            }
            ui.set_detail_episode_search_query("".into());
            ui.set_detail_in_stream_view(false);
        }

        // Map EpisodeItem list
        let is_series = meta_details
            .selected
            .as_ref()
            .map(|s| s.meta_path.r#type == "series")
            .unwrap_or(false);
        ui.set_detail_is_series(is_series);

        let mut slint_episodes = Vec::new();
        if is_series {
            if let Some(meta) = &meta_item {
                let active_season = *get_active_season().lock().unwrap();

                // Filter episodes matching active season
                let episodes: Vec<&Video> = meta
                    .videos
                    .iter()
                    .filter(|v| {
                        v.series_info
                            .as_ref()
                            .map(|info| info.season as i32 == active_season)
                            .unwrap_or(false)
                    })
                    .collect();

                let search_query = get_search_query().lock().unwrap().to_lowercase();

                for video in &episodes {
                    let ep_num = video
                        .series_info
                        .as_ref()
                        .map(|info| info.episode)
                        .unwrap_or(0);
                    let title = &video.title;
                    let released_str = video
                        .released
                        .map(|dt| dt.format("%b %d, %Y").to_string())
                        .unwrap_or_else(|| "Upcoming".to_string());

                    if !search_query.is_empty() {
                        let matches_title = title.to_lowercase().contains(&search_query);
                        let matches_num = ep_num.to_string().contains(&search_query);
                        let matches_released = released_str.to_lowercase().contains(&search_query);
                        if !matches_title && !matches_num && !matches_released {
                            continue;
                        }
                    }

                    let is_watched = meta_details
                        .watched
                        .as_ref()
                        .map(|watched| watched.get_video(&video.id))
                        .unwrap_or(false);

                    let is_upcoming = video
                        .released
                        .map(|dt| dt > chrono::Utc::now())
                        .unwrap_or(false);

                    let thumb_url = video
                        .thumbnail
                        .as_ref()
                        .and_then(|url_str| url::Url::parse(url_str).ok());
                    let thumb_img = crate::image_cache::get_poster_image(&thumb_url, ui_weak);

                    slint_episodes.push(EpisodeItem {
                        id: video.id.clone().into(),
                        title: video.title.clone().into(),
                        released: released_str.into(),
                        thumbnail: thumb_img,
                        season: active_season,
                        episode_num: ep_num as i32,
                        is_upcoming,
                        is_watched,
                    });
                }
            }
        }

        let episodes_model = slint::VecModel::from(slint_episodes);
        ui.set_detail_episodes(slint::ModelRc::new(episodes_model));

        sync_series_details(ui, meta_item);
    }
}
