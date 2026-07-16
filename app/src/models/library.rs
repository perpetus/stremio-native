use crate::models::details::load_meta_details_for_video;
use crate::models::{
    Fingerprint, SyncFingerprint, clear_sync_fingerprint, sync_fingerprint_changed,
};
use crate::{AppModel, MainWindow, NavigationController, NavigationIntent};
use crate::{LibraryRow, MediaCardItem};
use core_env::DesktopEnv;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex, OnceLock};
use stremio_core::{
    models::library_with_filters::{
        LibraryRequest, LibraryWithFilters, NotRemovedFilter, Selected, Sort,
    },
    runtime::{
        Runtime, RuntimeAction,
        msg::{Action, ActionLoad},
    },
};

static SEARCH_QUERY: OnceLock<Mutex<String>> = OnceLock::new();

static LAST_SYNC_STATE: OnceLock<Mutex<Option<SyncFingerprint>>> = OnceLock::new();

fn get_search_query() -> &'static Mutex<String> {
    SEARCH_QUERY.get_or_init(|| Mutex::new(String::new()))
}

fn sort_from_label(label: &str) -> Sort {
    match label {
        "A–Z" | "A-Z" => Sort::Name,
        "Z–A" | "Z-A" => Sort::NameReverse,
        "Most Watched" => Sort::TimesWatched,
        "Watched" => Sort::Watched,
        "Not Watched" => Sort::NotWatched,
        _ => Sort::LastWatched,
    }
}

fn sort_label(sort: &Sort) -> &'static str {
    match sort {
        Sort::LastWatched => "Last Watched",
        Sort::Name => "A–Z",
        Sort::NameReverse => "Z–A",
        Sort::TimesWatched => "Most Watched",
        Sort::Watched => "Watched",
        Sort::NotWatched => "Not Watched",
    }
}

fn type_from_label(label: &str) -> Option<String> {
    match label {
        "All" => None,
        "Movies" => Some("movie".to_owned()),
        "Series" => Some("series".to_owned()),
        "Others" => Some("other".to_owned()),
        value => Some(value.to_lowercase()),
    }
}

fn type_label(value: Option<&str>) -> &'static str {
    match value {
        Some("movie") => "Movies",
        Some("series") => "Series",
        Some("other") => "Others",
        _ => "All",
    }
}

pub fn clear_sync_state() {
    clear_sync_fingerprint(&LAST_SYNC_STATE);
}

pub fn setup(
    ui: &MainWindow,
    runtime: &Arc<Runtime<DesktopEnv, AppModel>>,
    navigation: &NavigationController,
) {
    let ui_weak = ui.as_weak();

    // Type change callback
    ui.on_library_type_changed({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        move |t| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_library_scroll_y(0.0.into());
            }
            clear_sync_state();
            let rt = runtime.clone();
            let r_type = type_from_label(t.as_str());
            let sort = ui_weak
                .upgrade()
                .map(|ui| sort_from_label(ui.get_library_active_sort().as_str()))
                .unwrap_or_default();

            tokio::spawn(async move {
                rt.dispatch(RuntimeAction {
                    field: None,
                    action: Action::Load(ActionLoad::LibraryWithFilters(Selected {
                        request: LibraryRequest {
                            r#type: r_type,
                            sort,
                            page: Default::default(),
                        },
                    })),
                });
            });
        }
    });

    ui.on_library_sort_changed({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        move |label| {
            let r_type = ui_weak
                .upgrade()
                .and_then(|ui| type_from_label(ui.get_library_active_type().as_str()));
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_library_scroll_y(0.0.into());
            }
            clear_sync_state();
            let rt = runtime.clone();
            let sort = sort_from_label(label.as_str());
            tokio::spawn(async move {
                rt.dispatch(RuntimeAction {
                    field: None,
                    action: Action::Load(ActionLoad::LibraryWithFilters(Selected {
                        request: LibraryRequest {
                            r#type: r_type,
                            sort,
                            page: Default::default(),
                        },
                    })),
                });
            });
        }
    });

    // Local Search changed callback
    ui.on_library_search_changed({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        move |query| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_library_scroll_y(0.0.into());
            }
            clear_sync_state();
            if let Ok(mut q) = get_search_query().lock() {
                *q = query.to_string();
            }

            // Trigger refresh immediately
            if let Some(ui) = ui_weak.upgrade() {
                if let Ok(model) = runtime.model() {
                    let ui_sync = ui_weak.clone();
                    let rt_sync = runtime.clone();
                    sync(&ui, &model.library, &ui_sync, &rt_sync);
                }
            }
        }
    });

    // Item selection callback
    ui.on_library_item_selected({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        let navigation = navigation.clone();
        move |id, media_type| {
            let id = id.to_string();
            let media_type = media_type.to_string();
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_details_loading(true);
                navigation.dispatch_and_project(
                    &ui,
                    NavigationIntent::OpenDetails {
                        media_id: id.clone(),
                    },
                );
            }
            load_meta_details_for_video(&runtime, id, Some(media_type), None);
        }
    });
}

#[tracing::instrument(skip_all)]
pub fn sync(
    ui: &MainWindow,
    library: &LibraryWithFilters<NotRemovedFilter>,
    _ui_weak: &slint::Weak<MainWindow>,
    _runtime: &Arc<Runtime<DesktopEnv, AppModel>>,
) {
    let query = get_search_query()
        .lock()
        .map(|q| q.clone())
        .unwrap_or_default();
    let normalized_query = query.to_lowercase();

    // 1. Filter raw items based on query
    let (raw_items, columns) = {
        let _span = tracing::info_span!("filter_library_items").entered();
        let mut raw_items = Vec::with_capacity(library.catalog.len());
        for item in &library.catalog {
            // Apply search query match
            if !normalized_query.is_empty() && !item.name.to_lowercase().contains(&normalized_query)
            {
                continue;
            }
            raw_items.push(item);
        }

        let metrics = crate::models::media_grid_metrics(ui);
        let columns = metrics.columns;

        (raw_items, columns)
    };

    let mut fingerprint = Fingerprint::new();
    fingerprint.usize(columns);
    fingerprint.str(&normalized_query);
    if let Some(selected) = &library.selected {
        fingerprint.optional_str(selected.request.r#type.as_deref());
        fingerprint.str(sort_label(&selected.request.sort));
    }
    for item in &raw_items {
        fingerprint.str(&item.id);
        fingerprint.str(&item.r#type);
        fingerprint.optional_str(item.state.video_id.as_deref());
        fingerprint.str(&item.name);
        fingerprint.optional_str(item.poster.as_ref().map(url::Url::as_str));
    }
    if !sync_fingerprint_changed(&LAST_SYNC_STATE, fingerprint.finish()) {
        return;
    }

    if let Some(selected) = &library.selected {
        ui.set_library_active_type(type_label(selected.request.r#type.as_deref()).into());
        ui.set_library_active_sort(sort_label(&selected.request.sort).into());
    }

    let visible_items = {
        let _span = tracing::info_span!("map_library_cards").entered();
        let mut visible_items = Vec::with_capacity(raw_items.len());
        for item in raw_items {
            visible_items.push(MediaCardItem {
                id: item.id.as_str().into(),
                media_type: item.r#type.as_str().into(),
                video_id: item.state.video_id.as_deref().unwrap_or_default().into(),
                title: item.name.as_str().into(),
                poster_url: item
                    .poster
                    .as_ref()
                    .map(url::Url::as_str)
                    .unwrap_or_default()
                    .into(),
                poster: crate::image_cache::get_cached_image(&item.poster),
                description: item.r#type.as_str().into(),
                show_checkmark: true,
                show_progress: false,
                progress_value: 0.0,
            });
        }
        visible_items
    };

    ui.set_library_column_count(columns as i32);

    let rows_model = {
        let _span = tracing::info_span!("chunk_library_rows").entered();
        let chunked = crate::models::chunk_vector_owned(visible_items, columns);
        let mut slint_rows = Vec::with_capacity(chunked.len());
        for row_items in chunked {
            let row_model = slint::VecModel::from(row_items);
            slint_rows.push(LibraryRow {
                cols: slint::ModelRc::new(row_model),
            });
        }
        slint::VecModel::from(slint_rows)
    };

    ui.set_library_rows(slint::ModelRc::new(rows_model));
}
