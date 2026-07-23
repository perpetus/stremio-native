use crate::DiscoverRow;
use crate::models::details::load_meta_details_for_video;
use crate::models::{
    Fingerprint, SyncFingerprint, catalog_media_card, clear_sync_fingerprint,
    sync_fingerprint_changed,
};
use crate::{AppModel, AppModelField, MainWindow, NavigationController, NavigationIntent};
use core_env::DesktopEnv;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex, OnceLock};
use stremio_core::{
    models::{
        catalog_with_filters::{CatalogWithFilters, Selected as CatalogSelected},
        common::Loadable,
    },
    runtime::{
        Runtime, RuntimeAction,
        msg::{Action, ActionCatalogWithFilters, ActionCtx, ActionLoad},
    },
    types::{addon::ExtraValue, resource::MetaItemPreview},
};

static LAST_SYNC_STATE: OnceLock<Mutex<Option<SyncFingerprint>>> = OnceLock::new();

pub fn clear_sync_state() {
    clear_sync_fingerprint(&LAST_SYNC_STATE);
}

pub fn setup(
    ui: &MainWindow,
    runtime: &Arc<Runtime<DesktopEnv, AppModel>>,
    navigation: &NavigationController,
) {
    let ui_weak = ui.as_weak();

    ui.on_discover_load_next_page({
        let runtime = runtime.clone();
        move || {
            let has_next_page = runtime
                .model()
                .ok()
                .is_some_and(|model| model.discover.selectable.next_page.is_some());
            if has_next_page {
                runtime.dispatch(RuntimeAction {
                    field: Some(AppModelField::Discover),
                    action: Action::CatalogWithFilters(ActionCatalogWithFilters::LoadNextPage),
                });
            }
        }
    });

    // Type change callback
    ui.on_discover_type_changed({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        move |r_type| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_discover_scroll_y(0.0);
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
                ui.set_discover_scroll_y(0.0);
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
                ui.set_discover_scroll_y(0.0);
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
                ui.set_discover_scroll_y(0.0);
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
        let navigation = navigation.clone();
        move |id, media_type, video_id| {
            let id = id.to_string();
            let transition = navigation.dispatch(NavigationIntent::SelectDiscoverPreview {
                media_id: id.clone(),
            });
            if transition.changed {
                load_meta_details_for_video(
                    &runtime,
                    id,
                    Some(media_type.to_string()),
                    (!video_id.is_empty()).then(|| video_id.to_string()),
                );
            }
        }
    });

    // Discover mirrors the official desktop interaction: one click updates
    // the preview, while a double-click opens the full details route.
    ui.on_discover_item_activated({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        let navigation = navigation.clone();
        move |id, media_type, video_id| {
            let id = id.to_string();
            if navigation.snapshot().discover_preview_id.as_deref() != Some(id.as_str()) {
                navigation.dispatch(NavigationIntent::SelectDiscoverPreview {
                    media_id: id.clone(),
                });
                load_meta_details_for_video(
                    &runtime,
                    id.clone(),
                    Some(media_type.to_string()),
                    (!video_id.is_empty()).then(|| video_id.to_string()),
                );
            }
            if let Some(ui) = ui_weak.upgrade() {
                crate::models::details::open_details_route(&ui, &runtime, &navigation, &id);
            }
        }
    });

    ui.on_play_preview({
        let ui_weak = ui_weak.clone();
        let navigation = navigation.clone();
        let runtime = runtime.clone();
        move || {
            let Some(media_id) = navigation.snapshot().discover_preview_id else {
                return;
            };
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            crate::models::details::open_details_route(&ui, &runtime, &navigation, &media_id);
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
                        if let Some(Loadable::Ready(meta_item)) = &resource.content
                            && meta_item.preview.id == id
                        {
                            meta_preview = Some(meta_item.preview.clone());
                            break;
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
    let estimated_count = {
        let _span = tracing::info_span!("compute_discover_metrics").entered();
        discover
            .catalog
            .iter()
            .filter_map(|page| {
                if let Some(Loadable::Ready(items)) = &page.content {
                    Some(items.len())
                } else {
                    None
                }
            })
            .sum::<usize>()
    };

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
            if opt.selected
                && let Some(val) = &opt.value
            {
                active_genre = val.clone();
            }
        }
    }

    let mut fingerprint = Fingerprint::new();
    fingerprint.usize(columns);
    fingerprint.str(&active_type);
    fingerprint.str(&active_catalog);
    fingerprint.str(&active_genre);
    for selectable_type in &discover.selectable.types {
        fingerprint.str(&selectable_type.r#type);
        fingerprint.bool(selectable_type.selected);
    }
    for catalog in &discover.selectable.catalogs {
        fingerprint.str(&catalog.catalog);
        fingerprint.bool(catalog.selected);
    }
    if let Some(genre_extra) = discover
        .selectable
        .extra
        .iter()
        .find(|extra| extra.name == "genre")
    {
        for option in &genre_extra.options {
            fingerprint.optional_str(option.value.as_deref());
            fingerprint.bool(option.selected);
        }
    }
    for item in &raw_items {
        fingerprint.str(&item.id);
        fingerprint.str(&item.r#type);
        fingerprint.str(&item.name);
        fingerprint.optional_str(item.poster.as_ref().map(url::Url::as_str));
        fingerprint.optional_str(item.release_info.as_deref());
        fingerprint.optional_str(item.behavior_hints.default_video_id.as_deref());
    }
    if !sync_fingerprint_changed(&LAST_SYNC_STATE, fingerprint.finish()) {
        return;
    }

    // 1. Sync Selectable Types
    let types: Vec<slint::SharedString> = discover
        .selectable
        .types
        .iter()
        .map(|t| {
            let mut value = t.r#type.clone();
            if let Some(first) = value.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
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

    let visible_items = {
        let _span = tracing::info_span!("map_visible_items").entered();
        let mut visible_items = Vec::with_capacity(raw_items.len());
        for item in raw_items {
            visible_items.push(catalog_media_card(item));
        }
        visible_items
    };

    ui.set_discover_column_count(columns as i32);

    let rows_model = {
        let _span = tracing::info_span!("chunk_discover_rows").entered();
        let chunked = crate::models::chunk_vector_owned(visible_items, columns);
        let mut slint_rows = Vec::with_capacity(chunked.len());
        for row_items in chunked {
            let row_model = slint::VecModel::from(row_items);
            slint_rows.push(DiscoverRow {
                cols: slint::ModelRc::new(row_model),
            });
        }
        slint::VecModel::from(slint_rows)
    };

    ui.set_discover_rows(slint::ModelRc::new(rows_model));
}
