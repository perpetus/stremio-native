use crate::models::details::{load_meta_details_for_video, open_details_route};
use crate::models::{Fingerprint, SyncFingerprint, catalog_name_index, sync_fingerprint_changed};
use crate::{AppModel, MainWindow, NavigationController};
use crate::{BoardSection, MediaCardItem};
use core_env::DesktopEnv;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex, OnceLock};
use stremio_core::{
    models::{
        catalogs_with_extra::CatalogsWithExtra, common::Loadable,
        continue_watching_preview::ContinueWatchingPreview,
    },
    runtime::{
        Runtime, RuntimeAction,
        msg::{Action, ActionCtx, ActionLoad},
    },
    types::addon::Descriptor,
};
use url::Url;

static LAST_SYNC_STATE: OnceLock<Mutex<Option<SyncFingerprint>>> = OnceLock::new();

pub fn setup(
    ui: &MainWindow,
    runtime: &Arc<Runtime<DesktopEnv, AppModel>>,
    navigation: &NavigationController,
) {
    let ui_weak = ui.as_weak();

    ui.on_board_item_selected({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        let navigation = navigation.clone();
        move |id, media_type, video_id| {
            let id = id.to_string();
            if let Some(ui) = ui_weak.upgrade() {
                open_details_route(&ui, &runtime, &navigation, &id);
            }
            let media_type = media_type.to_string();
            let video_id = (!video_id.is_empty()).then(|| video_id.to_string());
            load_meta_details_for_video(&runtime, id, Some(media_type), video_id);
        }
    });

    ui.on_remove_continue_watching({
        let runtime = runtime.clone();
        move |id| {
            let rt = runtime.clone();
            let id = id.to_string();
            tokio::spawn(async move {
                rt.dispatch(RuntimeAction {
                    field: None,
                    action: Action::Ctx(ActionCtx::RewindLibraryItem(id.clone())),
                });
                rt.dispatch(RuntimeAction {
                    field: None,
                    action: Action::Ctx(ActionCtx::DismissNotificationItem(id)),
                });
            });
        }
    });

    ui.on_board_see_all_continue_watching({
        let ui_weak = ui_weak.clone();
        move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.invoke_tab_changed(2);
            }
        }
    });

    ui.on_board_see_all_clicked({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        move |r_type, catalog_id, addon_base| {
            let r_type = r_type.to_string();
            let catalog_id = catalog_id.to_string();
            let addon_base = addon_base.to_string();

            let Ok(url) = url::Url::parse(&addon_base) else {
                tracing::warn!(%addon_base, "cannot navigate to an invalid catalog URL");
                return;
            };
            let request = stremio_core::types::addon::ResourceRequest {
                base: url,
                path: stremio_core::types::addon::ResourcePath {
                    resource: "catalog".to_string(),
                    r#type: r_type,
                    id: catalog_id,
                    extra: vec![],
                },
            };
            runtime.dispatch(RuntimeAction {
                field: None,
                action: Action::Load(ActionLoad::CatalogWithFilters(Some(
                    stremio_core::models::catalog_with_filters::Selected { request },
                ))),
            });
            if let Some(ui) = ui_weak.upgrade() {
                ui.invoke_tab_changed(1);
            }
        }
    });
}

#[tracing::instrument(skip_all)]
pub fn sync(
    ui: &MainWindow,
    continue_watching: &ContinueWatchingPreview,
    board: &CatalogsWithExtra,
    addons: &[Descriptor],
    _ui_weak: &slint::Weak<MainWindow>,
    _runtime: &Arc<Runtime<DesktopEnv, AppModel>>,
) {
    let mut fingerprint = Fingerprint::new();
    for item in &continue_watching.items {
        let library_item = &item.library_item;
        fingerprint.str(&library_item.id);
        fingerprint.str(&library_item.r#type);
        fingerprint.optional_str(library_item.state.video_id.as_deref());
        fingerprint.str(&library_item.name);
        fingerprint.optional_str(library_item.poster.as_ref().map(Url::as_str));
        fingerprint.u64(library_item.progress().to_bits());
    }
    for addon in addons {
        fingerprint.str(addon.transport_url.as_str());
        for catalog in &addon.manifest.catalogs {
            fingerprint.str(&catalog.id);
            fingerprint.str(&catalog.r#type);
            fingerprint.optional_str(catalog.name.as_deref());
        }
    }
    for catalog in &board.catalogs {
        for page in catalog {
            fingerprint.str(page.request.base.as_str());
            fingerprint.str(&page.request.path.r#type);
            fingerprint.str(&page.request.path.id);
            let ready_items = match &page.content {
                Some(Loadable::Ready(items)) => Some(items),
                _ => None,
            };
            fingerprint.bool(ready_items.is_some());
            if let Some(items) = ready_items {
                for item in items.iter().take(10) {
                    fingerprint.str(&item.id);
                    fingerprint.str(&item.r#type);
                    fingerprint.str(&item.name);
                    fingerprint.optional_str(item.poster.as_ref().map(Url::as_str));
                    fingerprint.optional_str(item.release_info.as_deref());
                }
            }
        }
    }
    if !sync_fingerprint_changed(&LAST_SYNC_STATE, fingerprint.finish()) {
        return;
    }

    // 1. Sync Continue Watching
    let continue_watching_items: Vec<MediaCardItem> = {
        let _span = tracing::info_span!("sync_continue_watching").entered();
        let mut items = Vec::with_capacity(continue_watching.items.len());
        for item in &continue_watching.items {
            let library_item = &item.library_item;
            let prog = library_item.progress();
            items.push(MediaCardItem {
                id: library_item.id.as_str().into(),
                media_type: library_item.r#type.as_str().into(),
                video_id: library_item
                    .state
                    .video_id
                    .as_deref()
                    .unwrap_or_default()
                    .into(),
                title: library_item.name.as_str().into(),
                poster_url: library_item
                    .poster
                    .as_ref()
                    .map(Url::as_str)
                    .unwrap_or_default()
                    .into(),
                poster: crate::image_cache::get_cached_image(&library_item.poster),
                description: library_item.r#type.as_str().into(),
                show_checkmark: false,
                show_progress: prog > 0.0,
                progress_value: (prog / 100.0).clamp(0.0, 1.0) as f32,
            });
        }
        items
    };
    let has_continue_watching = !continue_watching_items.is_empty();
    let continue_watching_model =
        slint::ModelRc::new(slint::VecModel::from(continue_watching_items));
    ui.set_board_continue_watching(continue_watching_model.clone());

    // 2. Sync Board Sections
    let board_sections = {
        let _span = tracing::info_span!("sync_board_sections").entered();
        let catalog_names = catalog_name_index(addons);
        let mut board_sections =
            Vec::with_capacity(board.catalogs.len() + if has_continue_watching { 1 } else { 0 });
        if has_continue_watching {
            board_sections.push(BoardSection {
                title: "Continue watching".into(),
                r_type: "".into(),
                catalog_id: "".into(),
                addon_base: "".into(),
                items: continue_watching_model,
                is_continue_watching: true,
            });
        }
        for catalog in &board.catalogs {
            for page in catalog {
                if let Some(Loadable::Ready(items)) = &page.content {
                    let catalog_name = catalog_names
                        .get(&(
                            page.request.base.as_str(),
                            page.request.path.id.as_str(),
                            page.request.path.r#type.as_str(),
                        ))
                        .copied()
                        .unwrap_or(page.request.path.id.as_str());
                    let mut media_type = page.request.path.r#type.clone();
                    if let Some(first) = media_type.get_mut(0..1) {
                        first.make_ascii_uppercase();
                    }
                    let title = format!("{catalog_name} – {media_type}");

                    let section_items: Vec<MediaCardItem> = {
                        let mut s_items = Vec::with_capacity(items.len().min(10));
                        for item in items.iter().take(10) {
                            s_items.push(MediaCardItem {
                                id: item.id.as_str().into(),
                                media_type: page.request.path.r#type.as_str().into(),
                                video_id: "".into(),
                                title: item.name.as_str().into(),
                                poster_url: item
                                    .poster
                                    .as_ref()
                                    .map(Url::as_str)
                                    .unwrap_or_default()
                                    .into(),
                                poster: crate::image_cache::get_cached_image(&item.poster),
                                description: item
                                    .release_info
                                    .as_deref()
                                    .unwrap_or_default()
                                    .into(),
                                show_checkmark: false,
                                show_progress: false,
                                progress_value: 0.0,
                            });
                        }
                        s_items
                    };

                    let section_items_model = slint::VecModel::from(section_items);
                    board_sections.push(BoardSection {
                        title: title.into(),
                        r_type: page.request.path.r#type.as_str().into(),
                        catalog_id: page.request.path.id.as_str().into(),
                        addon_base: page.request.base.as_str().into(),
                        items: slint::ModelRc::new(section_items_model),
                        is_continue_watching: false,
                    });
                }
            }
        }
        board_sections
    };

    let sections_model = slint::VecModel::from(board_sections);
    ui.set_board_sections(slint::ModelRc::new(sections_model));
}
