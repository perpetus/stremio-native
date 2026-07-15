use crate::models::details::load_meta_details_for_video;
use crate::{AppModel, AppModelField, BoardSection, MainWindow, MediaCardItem, SearchSuggestion};
use core_env::DesktopEnv;
use slint::{ComponentHandle, ModelRc, VecModel};
use std::sync::Arc;
use stremio_core::{
    models::{
        catalogs_with_extra::{CatalogsWithExtra, Selected},
        common::Loadable,
        local_search::LocalSearch,
    },
    runtime::{
        Runtime, RuntimeAction,
        msg::{Action, ActionCatalogsWithExtra, ActionLoad, ActionSearch},
    },
    types::{addon::ExtraValue, profile::Profile},
};

pub fn setup(ui: &MainWindow, runtime: &Arc<Runtime<DesktopEnv, AppModel>>) {
    ui.on_global_search_edited({
        let runtime = runtime.clone();
        move |query| {
            runtime.dispatch(RuntimeAction {
                field: Some(AppModelField::LocalSearch),
                action: Action::Search(ActionSearch::Search {
                    search_query: query.trim().to_owned(),
                    max_results: 5,
                }),
            });
        }
    });

    ui.on_global_search_submitted({
        let runtime = runtime.clone();
        let ui_weak = ui.as_weak();
        move |query| {
            let query = query.trim().to_owned();
            if query.is_empty() {
                return;
            }

            if let Some(ui) = ui_weak.upgrade() {
                ui.set_search_query(query.clone().into());
                ui.set_search_suggestions(ModelRc::new(VecModel::from(Vec::new())));
                ui.set_search_sections(ModelRc::new(VecModel::from(Vec::new())));
                ui.set_search_catalog_count(0);
                ui.set_search_loading(true);
                ui.set_show_details(false);
                ui.set_active_tab(6);
                ui.set_loading(true);
                ui.invoke_tab_changed(6);
                // tab-changed projects the previous search model
                // synchronously; keep the new request visibly pending until
                // the core publishes its selected catalogs.
                ui.set_search_loading(true);
            }

            runtime.dispatch(RuntimeAction {
                field: Some(AppModelField::Search),
                action: Action::Load(ActionLoad::CatalogsWithExtra(Selected {
                    r#type: None,
                    extra: vec![ExtraValue {
                        name: "search".to_owned(),
                        value: query,
                    }],
                })),
            });
            runtime.dispatch(RuntimeAction {
                field: Some(AppModelField::Search),
                action: Action::CatalogsWithExtra(ActionCatalogsWithExtra::LoadRange(0..20)),
            });
        }
    });

    let open_details = |ui: &MainWindow,
                        runtime: &Arc<Runtime<DesktopEnv, AppModel>>,
                        id: slint::SharedString,
                        media_type: slint::SharedString,
                        video_id: Option<slint::SharedString>| {
        ui.set_loading(true);
        ui.set_search_suggestions(ModelRc::new(VecModel::from(Vec::new())));
        let runtime = runtime.clone();
        let id = id.to_string();
        let media_type = media_type.to_string();
        let video_id = video_id.map(|value| value.to_string());
        tokio::spawn(async move {
            load_meta_details_for_video(&runtime, id, Some(media_type), video_id).await;
        });
    };

    ui.on_global_search_suggestion_selected({
        let runtime = runtime.clone();
        let ui_weak = ui.as_weak();
        move |id, media_type| {
            if let Some(ui) = ui_weak.upgrade() {
                open_details(&ui, &runtime, id, media_type, None);
            }
        }
    });

    ui.on_search_item_selected({
        let runtime = runtime.clone();
        let ui_weak = ui.as_weak();
        move |id, media_type, video_id| {
            if let Some(ui) = ui_weak.upgrade() {
                let video_id = (!video_id.is_empty()).then_some(video_id);
                open_details(&ui, &runtime, id, media_type, video_id);
            }
        }
    });
}

pub fn sync_local_search(
    ui: &MainWindow,
    local_search: &LocalSearch,
    ui_weak: &slint::Weak<MainWindow>,
) {
    let suggestions = local_search
        .search_results
        .iter()
        .map(|item| SearchSuggestion {
            id: item.id.clone().into(),
            media_type: item.r#type.clone().into(),
            title: item.name.clone().into(),
            release_info: item.release_info.clone().unwrap_or_default().into(),
            poster: crate::image_cache::get_poster_image(&item.poster, ui_weak),
        })
        .collect::<Vec<_>>();
    ui.set_search_suggestions(ModelRc::new(VecModel::from(suggestions)));
}

pub fn sync_results(
    ui: &MainWindow,
    search: &CatalogsWithExtra,
    profile: &Profile,
    _ui_weak: &slint::Weak<MainWindow>,
) {
    let catalog_count = search.catalogs.iter().map(Vec::len).sum::<usize>();
    let loading = search.catalogs.iter().flatten().any(|page| {
        page.content
            .as_ref()
            .is_none_or(|content| matches!(content, Loadable::Loading))
    });
    ui.set_search_catalog_count(i32::try_from(catalog_count).unwrap_or(i32::MAX));
    ui.set_search_loading(loading);

    let mut sections = Vec::with_capacity(search.catalogs.len());
    for catalog in &search.catalogs {
        for page in catalog {
            let Some(Loadable::Ready(items)) = &page.content else {
                continue;
            };

            let catalog_name = profile
                .addons
                .iter()
                .find(|addon| addon.transport_url == page.request.base)
                .and_then(|addon| {
                    addon.manifest.catalogs.iter().find(|candidate| {
                        candidate.id == page.request.path.id
                            && candidate.r#type == page.request.path.r#type
                    })
                })
                .map(|catalog| catalog.name.as_deref().unwrap_or(&catalog.id).to_owned())
                .unwrap_or_else(|| page.request.path.id.clone());

            let mut media_type = page.request.path.r#type.clone();
            if let Some(first) = media_type.get_mut(0..1) {
                first.make_ascii_uppercase();
            }

            let cards = items
                .iter()
                .take(20)
                .map(|item| MediaCardItem {
                    id: item.id.clone().into(),
                    media_type: page.request.path.r#type.clone().into(),
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
                })
                .collect::<Vec<_>>();

            sections.push(BoardSection {
                title: format!("{catalog_name} – {media_type}").into(),
                r_type: page.request.path.r#type.clone().into(),
                catalog_id: page.request.path.id.clone().into(),
                addon_base: page.request.base.as_str().into(),
                items: ModelRc::new(VecModel::from(cards)),
            });
        }
    }

    ui.set_search_sections(ModelRc::new(VecModel::from(sections)));
}
