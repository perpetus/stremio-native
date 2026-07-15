use crate::AppModel;
use crate::MainWindow;
use crate::models::details::load_meta_details;
use crate::{DiscoverRow, MediaCardItem};
use core_env::DesktopEnv;
use slint::ComponentHandle;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use stremio_core::{
    models::{
        catalog_with_filters::{CatalogWithFilters, Selected as CatalogSelected},
        common::Loadable,
    },
    runtime::{
        Runtime, RuntimeAction,
        msg::{Action, ActionCtx, ActionLoad},
    },
    types::{addon::ExtraValue, resource::MetaItemPreview},
};

#[derive(Debug, PartialEq, Clone)]
struct SyncState {
    active_type: String,
    active_catalog: String,
    active_genre: String,
    available_types: Vec<String>,
    available_catalogs: Vec<String>,
    available_genres: Vec<String>,
    columns: usize,
    item_ids: Vec<String>,
}

static LAST_SYNC_STATE: OnceLock<Mutex<Option<SyncState>>> = OnceLock::new();

pub fn clear_sync_state() {
    if let Some(mutex) = LAST_SYNC_STATE.get() {
        if let Ok(mut guard) = mutex.lock() {
            *guard = None;
        }
    }
}

pub fn setup(ui: &MainWindow, runtime: &Arc<Runtime<DesktopEnv, AppModel>>) {
    let ui_weak = ui.as_weak();

    // Type change callback
    ui.on_discover_type_changed({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        move |r_type| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_discover_scroll_y(0.0.into());
            }
            clear_sync_state();
            let rt = runtime.clone();
            let r_type = r_type.to_string();
            tokio::spawn(async move {
                let model = rt.model().expect("model read failed");
                if let Some(selectable_type) = model
                    .discover
                    .selectable
                    .types
                    .iter()
                    .find(|t| t.r#type.eq_ignore_ascii_case(&r_type))
                {
                    let request = selectable_type.request.clone();
                    drop(model);
                    rt.dispatch(RuntimeAction {
                        field: None,
                        action: Action::Load(ActionLoad::CatalogWithFilters(Some(
                            CatalogSelected { request },
                        ))),
                    });
                }
            });
        }
    });

    // Catalog change callback
    ui.on_discover_catalog_changed({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        move |cat_name| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_discover_scroll_y(0.0.into());
            }
            clear_sync_state();
            let rt = runtime.clone();
            let cat_name = cat_name.to_string();
            tokio::spawn(async move {
                let model = rt.model().expect("model read failed");
                if let Some(selectable_cat) = model
                    .discover
                    .selectable
                    .catalogs
                    .iter()
                    .find(|c| c.catalog == cat_name)
                {
                    let request = selectable_cat.request.clone();
                    drop(model);
                    rt.dispatch(RuntimeAction {
                        field: None,
                        action: Action::Load(ActionLoad::CatalogWithFilters(Some(
                            CatalogSelected { request },
                        ))),
                    });
                }
            });
        }
    });

    // Genre change callback
    ui.on_discover_genre_changed({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        move |genre_val| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_discover_scroll_y(0.0.into());
            }
            clear_sync_state();
            let rt = runtime.clone();
            let genre_val = genre_val.to_string();
            tokio::spawn(async move {
                let model = rt.model().expect("model read failed");
                if let Some(extra_field) = model
                    .discover
                    .selectable
                    .extra
                    .iter()
                    .find(|e| e.name == "genre")
                {
                    let val_opt = if genre_val == "Genre" || genre_val == "All Genres" {
                        None
                    } else {
                        Some(genre_val.clone())
                    };
                    if let Some(opt) = extra_field.options.iter().find(|o| o.value == val_opt) {
                        let request = opt.request.clone();
                        drop(model);
                        rt.dispatch(RuntimeAction {
                            field: None,
                            action: Action::Load(ActionLoad::CatalogWithFilters(Some(
                                CatalogSelected { request },
                            ))),
                        });
                    }
                }
            });
        }
    });

    // Search query callback
    ui.on_discover_search_changed({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        move |query| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_discover_scroll_y(0.0.into());
            }
            clear_sync_state();
            let rt = runtime.clone();
            let query = query.to_string();
            tokio::spawn(async move {
                let model = rt.model().expect("model read failed");
                if query.is_empty() {
                    let request = model
                        .discover
                        .selectable
                        .types
                        .first()
                        .map(|t| t.request.clone())
                        .unwrap_or_else(|| {
                            model
                                .discover
                                .selected
                                .as_ref()
                                .map(|s| s.request.clone())
                                .unwrap()
                        });
                    drop(model);
                    rt.dispatch(RuntimeAction {
                        field: None,
                        action: Action::Load(ActionLoad::CatalogWithFilters(Some(
                            CatalogSelected { request },
                        ))),
                    });
                    return;
                }
                if let Some(_selectable_extra) = model
                    .discover
                    .selectable
                    .extra
                    .iter()
                    .find(|e| e.name == "search")
                {
                    let mut request = model
                        .discover
                        .selected
                        .as_ref()
                        .map(|s| s.request.clone())
                        .unwrap_or_else(|| {
                            model
                                .discover
                                .selectable
                                .types
                                .first()
                                .unwrap()
                                .request
                                .clone()
                        });
                    request.path.extra.retain(|e| e.name != "search");
                    request.path.extra.push(ExtraValue {
                        name: "search".to_string(),
                        value: query,
                    });
                    drop(model);
                    rt.dispatch(RuntimeAction {
                        field: None,
                        action: Action::Load(ActionLoad::CatalogWithFilters(Some(
                            CatalogSelected { request },
                        ))),
                    });
                }
            });
        }
    });

    // Item selection callback
    ui.on_discover_item_selected({
        let runtime = runtime.clone();
        move |id| {
            let rt = runtime.clone();
            let id = id.to_string();
            tokio::spawn(async move {
                load_meta_details(&rt, id).await;
            });
        }
    });

    ui.on_play_preview({
        let ui_weak = ui_weak.clone();
        move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_show_details(true);
            }
        }
    });

    ui.on_toggle_library_preview({
        let runtime = runtime.clone();
        move || {
            let rt = runtime.clone();
            tokio::spawn(async move {
                let model = rt.model().expect("model read failed");
                if let Some(selected) = &model.meta_details.selected {
                    let id = selected.meta_path.id.clone();
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
}

#[tracing::instrument(skip_all)]
pub fn sync(
    ui: &MainWindow,
    discover: &CatalogWithFilters<MetaItemPreview>,
    _ui_weak: &slint::Weak<MainWindow>,
    _runtime: &Arc<Runtime<DesktopEnv, AppModel>>,
) {
    // Calculate items count first to compute indices
    let estimated_count = discover
        .catalog
        .iter()
        .filter_map(|page| {
            if let Some(Loadable::Ready(items)) = &page.content {
                Some(items.len())
            } else {
                None
            }
        })
        .sum::<usize>();

    let mut raw_items = Vec::with_capacity(estimated_count);
    for page in &discover.catalog {
        if let Some(Loadable::Ready(items)) = &page.content {
            for item in items {
                raw_items.push(item);
            }
        }
    }

    let metrics = crate::models::media_grid_metrics(ui);
    // The official Discover route reserves a fixed 29rem metadata preview on
    // desktop. Account for that drawer when chunking the virtualized grid.
    let columns = metrics.columns.saturating_sub(2).max(4);

    // Compute active filters
    let active_type_raw = discover
        .selected
        .as_ref()
        .map(|s| s.request.path.r#type.clone())
        .unwrap_or_else(|| "movie".to_string());
    let active_type = {
        let mut value = active_type_raw.clone();
        if let Some(first) = value.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        value
    };

    let active_catalog = discover
        .selectable
        .catalogs
        .iter()
        .find(|c| c.selected)
        .map(|c| c.catalog.clone())
        .unwrap_or_else(|| "".to_string());

    let mut active_genre = String::new();
    if let Some(genre_extra) = discover.selectable.extra.iter().find(|e| e.name == "genre") {
        for opt in &genre_extra.options {
            if opt.selected {
                if let Some(val) = &opt.value {
                    active_genre = val.clone();
                }
            }
        }
    }

    // Build available options lists
    let available_types: Vec<String> = discover
        .selectable
        .types
        .iter()
        .map(|t| {
            let mut value = t.r#type.clone();
            if let Some(first) = value.get_mut(0..1) { first.make_ascii_uppercase(); }
            value
        })
        .collect();
    let available_catalogs: Vec<String> = discover
        .selectable
        .catalogs
        .iter()
        .map(|c| c.catalog.clone())
        .collect();
    let mut available_genres = vec!["Genre".to_string()];
    if let Some(genre_extra) = discover.selectable.extra.iter().find(|e| e.name == "genre") {
        for opt in &genre_extra.options {
            if let Some(val) = &opt.value {
                available_genres.push(val.clone());
            }
        }
    }

    let item_ids = raw_items.iter().map(|item| item.id.clone()).collect();

    let current_state = SyncState {
        active_type: active_type.clone(),
        active_catalog: active_catalog.clone(),
        active_genre: active_genre.clone(),
        available_types,
        available_catalogs,
        available_genres,
        columns,
        item_ids,
    };

    // Check dirty flag
    let state_mutex = LAST_SYNC_STATE.get_or_init(|| Mutex::new(None));
    {
        let mut last_state_guard = state_mutex.lock().unwrap();
        if let Some(last_state) = &*last_state_guard {
            if *last_state == current_state {
                // No changes, skip sync!
                return;
            }
        }
        *last_state_guard = Some(current_state);
    }

    // 1. Sync Selectable Types
    let types: Vec<slint::SharedString> = discover
        .selectable
        .types
        .iter()
        .map(|t| {
            let mut value = t.r#type.clone();
            if let Some(first) = value.get_mut(0..1) { first.make_ascii_uppercase(); }
            slint::SharedString::from(value)
        })
        .collect();
    let types_model = slint::VecModel::from(types);
    ui.set_discover_types(slint::ModelRc::new(types_model));
    ui.set_discover_active_type(active_type.into());

    // 2. Sync Selectable Catalogs
    let catalogs: Vec<slint::SharedString> = discover
        .selectable
        .catalogs
        .iter()
        .map(|c| slint::SharedString::from(&c.catalog))
        .collect();
    let catalogs_model = slint::VecModel::from(catalogs);
    ui.set_discover_catalogs(slint::ModelRc::new(catalogs_model));
    ui.set_discover_active_catalog(active_catalog.into());

    // 3. Sync Selectable Genres (extra filter named "genre")
    let mut genres = vec![slint::SharedString::from("Genre")];
    if let Some(genre_extra) = discover.selectable.extra.iter().find(|e| e.name == "genre") {
        for opt in &genre_extra.options {
            if let Some(val) = &opt.value {
                genres.push(slint::SharedString::from(val));
            }
        }
    }
    let genres_model = slint::VecModel::from(genres);
    ui.set_discover_genres(slint::ModelRc::new(genres_model));
    ui.set_discover_active_genre(active_genre.into());

    let mut visible_items = Vec::with_capacity(raw_items.len());
    for item in raw_items {
        visible_items.push(MediaCardItem {
            id: item.id.clone().into(),
            media_type: item.r#type.clone().into(),
            video_id: "".into(),
            title: item.name.clone().into(),
            poster_url: item
                .poster
                .as_ref()
                .map(url::Url::as_str)
                .unwrap_or_default()
                .into(),
            poster: crate::image_cache::get_cached_image(&item.poster),
            description: item.release_info.clone().unwrap_or_default().into(),
            show_checkmark: false,
            show_progress: false,
            progress_value: 0.0,
        });
    }

    ui.set_discover_column_count(columns as i32);

    let chunked = crate::models::chunk_vector_owned(visible_items, columns);
    let mut slint_rows = Vec::with_capacity(chunked.len());
    for row_items in chunked {
        let row_model = slint::VecModel::from(row_items);
        slint_rows.push(DiscoverRow {
            cols: slint::ModelRc::new(row_model),
        });
    }

    let rows_model = slint::VecModel::from(slint_rows);
    ui.set_discover_rows(slint::ModelRc::new(rows_model));
}
