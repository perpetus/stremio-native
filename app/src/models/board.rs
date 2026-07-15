use crate::AppModel;
use crate::MainWindow;
use crate::models::details::load_meta_details_for_video;
use crate::{BoardSection, MediaCardItem};
use core_env::DesktopEnv;
use slint::ComponentHandle;
use std::sync::Arc;
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

pub fn setup(ui: &MainWindow, runtime: &Arc<Runtime<DesktopEnv, AppModel>>) {
    let ui_weak = ui.as_weak();

    ui.on_board_item_selected({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        move |id, media_type, video_id| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_loading(true);
            }
            let id = id.to_string();
            let media_type = media_type.to_string();
            let video_id = (!video_id.is_empty()).then(|| video_id.to_string());
            let rt = runtime.clone();
            tokio::spawn(async move {
                load_meta_details_for_video(&rt, id, Some(media_type), video_id).await;
            });
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
                ui.set_active_tab(2); // Go to Library (tab 2)
                ui.invoke_tab_changed(2);
            }
        }
    });

    ui.on_board_see_all_clicked({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        move |r_type, catalog_id, addon_base| {
            let rt = runtime.clone();
            let ui_weak = ui_weak.clone();
            let r_type = r_type.to_string();
            let catalog_id = catalog_id.to_string();
            let addon_base = addon_base.to_string();

            tokio::spawn(async move {
                if let Ok(url) = url::Url::parse(&addon_base) {
                    let request = stremio_core::types::addon::ResourceRequest {
                        base: url,
                        path: stremio_core::types::addon::ResourcePath {
                            resource: "catalog".to_string(),
                            r#type: r_type,
                            id: catalog_id,
                            extra: vec![],
                        },
                    };
                    rt.dispatch(RuntimeAction {
                        field: None,
                        action: Action::Load(ActionLoad::CatalogWithFilters(Some(
                            stremio_core::models::catalog_with_filters::Selected { request },
                        ))),
                    });

                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak.upgrade() {
                            ui.set_active_tab(1); // Go to Discover (tab 1)
                            ui.invoke_tab_changed(1);
                        }
                    });
                }
            });
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
    // 1. Sync Continue Watching
    let continue_watching_items: Vec<MediaCardItem> = {
        let mut items = Vec::with_capacity(continue_watching.items.len());
        for item in &continue_watching.items {
            let library_item = &item.library_item;
            let prog = library_item.progress();
            items.push(MediaCardItem {
                id: library_item.id.clone().into(),
                media_type: library_item.r#type.clone().into(),
                video_id: library_item
                    .state
                    .video_id
                    .clone()
                    .unwrap_or_default()
                    .into(),
                title: library_item.name.clone().into(),
                poster_url: library_item
                    .poster
                    .as_ref()
                    .map(Url::as_str)
                    .unwrap_or_default()
                    .into(),
                poster: crate::image_cache::get_cached_image(&library_item.poster),
                description: library_item.r#type.clone().into(),
                show_checkmark: false,
                show_progress: prog > 0.0,
                progress_value: (prog / 100.0).clamp(0.0, 1.0) as f32,
            });
        }
        items
    };
    let continue_watching_model = slint::VecModel::from(continue_watching_items);
    ui.set_board_continue_watching(slint::ModelRc::new(continue_watching_model));

    // 2. Sync Board Sections
    let mut board_sections = Vec::with_capacity(board.catalogs.len());
    for catalog in &board.catalogs {
        for page in catalog {
            if let Some(Loadable::Ready(items)) = &page.content {
                // Find catalog title in profile addons
                let catalog_name = addons
                    .iter()
                    .find(|addon| addon.transport_url == page.request.base)
                    .and_then(|addon| {
                        addon.manifest.catalogs.iter().find(|c| {
                            c.id == page.request.path.id && c.r#type == page.request.path.r#type
                        })
                    })
                    .map(|c| c.name.as_deref().unwrap_or(&c.id).to_owned())
                    .unwrap_or_else(|| page.request.path.id.clone());
                let mut media_type = page.request.path.r#type.clone();
                if let Some(first) = media_type.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                let title = format!("{catalog_name} – {media_type}");

                let section_items: Vec<MediaCardItem> = {
                    let mut s_items = Vec::with_capacity(items.len().min(10));
                    for item in items.iter().take(10) {
                        s_items.push(MediaCardItem {
                            id: item.id.clone().into(),
                            media_type: page.request.path.r#type.clone().into(),
                            video_id: "".into(),
                            title: item.name.clone().into(),
                            poster_url: item
                                .poster
                                .as_ref()
                                .map(Url::as_str)
                                .unwrap_or_default()
                                .into(),
                            poster: crate::image_cache::get_cached_image(&item.poster),
                            description: item.release_info.clone().unwrap_or_default().into(),
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
                    r_type: page.request.path.r#type.clone().into(),
                    catalog_id: page.request.path.id.clone().into(),
                    addon_base: page.request.base.as_str().into(),
                    items: slint::ModelRc::new(section_items_model),
                });
            }
        }
    }

    let sections_model = slint::VecModel::from(board_sections);
    ui.set_board_sections(slint::ModelRc::new(sections_model));
}
