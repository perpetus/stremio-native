use crate::models::details::{load_meta_details_for_video, open_details_route};
use crate::models::{Fingerprint, SyncFingerprint, catalog_name_index, sync_fingerprint_changed};
use crate::{
    AppModel, AppModelField, BoardSection, MainWindow, MediaCardItem, NavigationController,
    NavigationIntent, SearchSuggestion,
};
use core_env::DesktopEnv;
use slint::{ComponentHandle, ModelRc, VecModel};
use std::sync::{Arc, Mutex, OnceLock};
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
    types::addon::{Descriptor, ExtraValue},
};

static LAST_LOCAL_SYNC_STATE: OnceLock<Mutex<Option<SyncFingerprint>>> = OnceLock::new();
static LAST_RESULTS_SYNC_STATE: OnceLock<Mutex<Option<SyncFingerprint>>> = OnceLock::new();

pub fn setup(
    ui: &MainWindow,
    runtime: &Arc<Runtime<DesktopEnv, AppModel>>,
    navigation: &NavigationController,
) {
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
        let navigation = navigation.clone();
        move |query| {
            let query = query.trim().to_owned();
            if query.is_empty() {
                return;
            }

            if let Some(ui) = ui_weak.upgrade() {
                ui.set_search_query(query.as_str().into());
                ui.set_search_suggestions(ModelRc::new(VecModel::from(Vec::new())));
                ui.set_search_sections(ModelRc::new(VecModel::from(Vec::new())));
                ui.set_search_catalog_count(0);
                ui.set_search_loading(true);
                navigation.dispatch_and_project(
                    &ui,
                    NavigationIntent::OpenSearch {
                        query: query.clone(),
                    },
                );
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

    ui.on_global_search_suggestion_selected({
        let runtime = runtime.clone();
        let ui_weak = ui.as_weak();
        let navigation = navigation.clone();
        move |id, media_type| {
            if let Some(ui) = ui_weak.upgrade() {
                open_details(&ui, &runtime, &navigation, id, media_type, None);
            }
        }
    });

    ui.on_search_item_selected({
        let runtime = runtime.clone();
        let ui_weak = ui.as_weak();
        let navigation = navigation.clone();
        move |id, media_type, video_id| {
            if let Some(ui) = ui_weak.upgrade() {
                let video_id = (!video_id.is_empty()).then_some(video_id);
                open_details(&ui, &runtime, &navigation, id, media_type, video_id);
            }
        }
    });
}

fn open_details(
    ui: &MainWindow,
    runtime: &Arc<Runtime<DesktopEnv, AppModel>>,
    navigation: &NavigationController,
    id: slint::SharedString,
    media_type: slint::SharedString,
    video_id: Option<slint::SharedString>,
) {
    let id = id.to_string();
    ui.set_search_suggestions(ModelRc::new(VecModel::from(Vec::new())));
    open_details_route(ui, runtime, navigation, &id);
    load_meta_details_for_video(
        runtime,
        id,
        Some(media_type.to_string()),
        video_id.map(|value| value.to_string()),
    );
}

pub fn sync_local_search(
    ui: &MainWindow,
    local_search: &LocalSearch,
    ui_weak: &slint::Weak<MainWindow>,
) {
    let _span = tracing::info_span!("sync_local_search_suggestions").entered();
    let mut fingerprint = Fingerprint::new();
    for item in &local_search.search_results {
        fingerprint.str(&item.id);
        fingerprint.str(&item.r#type);
        fingerprint.str(&item.name);
        fingerprint.optional_str(item.release_info.as_deref());
        fingerprint.optional_str(item.poster.as_ref().map(url::Url::as_str));
    }
    if !sync_fingerprint_changed(&LAST_LOCAL_SYNC_STATE, fingerprint.finish()) {
        return;
    }

    let suggestions = local_search
        .search_results
        .iter()
        .map(|item| SearchSuggestion {
            id: item.id.as_str().into(),
            media_type: item.r#type.as_str().into(),
            title: item.name.as_str().into(),
            release_info: item.release_info.as_deref().unwrap_or_default().into(),
            poster_url: item
                .poster
                .as_ref()
                .map(url::Url::as_str)
                .unwrap_or_default()
                .into(),
            poster: crate::image_cache::get_poster_image(&item.poster, ui_weak),
        })
        .collect::<Vec<_>>();
    ui.set_search_suggestions(ModelRc::new(VecModel::from(suggestions)));
}

pub fn sync_results(
    ui: &MainWindow,
    search: &CatalogsWithExtra,
    addons: &[Descriptor],
    _ui_weak: &slint::Weak<MainWindow>,
) {
    let catalog_count = search.catalogs.iter().map(Vec::len).sum::<usize>();
    let loading = search.catalogs.iter().flatten().any(|page| {
        page.content
            .as_ref()
            .is_none_or(|content| matches!(content, Loadable::Loading))
    });

    let mut fingerprint = Fingerprint::new();
    fingerprint.usize(catalog_count);
    fingerprint.bool(loading);
    for addon in addons {
        fingerprint.str(addon.transport_url.as_str());
        for catalog in &addon.manifest.catalogs {
            fingerprint.str(&catalog.id);
            fingerprint.str(&catalog.r#type);
            fingerprint.optional_str(catalog.name.as_deref());
        }
    }
    for catalog in &search.catalogs {
        for page in catalog {
            fingerprint.str(page.request.base.as_str());
            fingerprint.str(&page.request.path.r#type);
            fingerprint.str(&page.request.path.id);
            for extra in &page.request.path.extra {
                fingerprint.str(&extra.name);
                fingerprint.str(&extra.value);
            }
            match &page.content {
                None => fingerprint.u64(0),
                Some(Loadable::Loading) => fingerprint.u64(1),
                Some(Loadable::Ready(items)) => {
                    fingerprint.u64(2);
                    for item in items.iter().take(20) {
                        fingerprint.str(&item.id);
                        fingerprint.str(&item.r#type);
                        fingerprint.str(&item.name);
                        fingerprint.optional_str(item.poster.as_ref().map(url::Url::as_str));
                        fingerprint.optional_str(item.release_info.as_deref());
                    }
                }
                Some(Loadable::Err(_)) => fingerprint.u64(3),
            }
        }
    }
    if !sync_fingerprint_changed(&LAST_RESULTS_SYNC_STATE, fingerprint.finish()) {
        return;
    }

    ui.set_search_catalog_count(i32::try_from(catalog_count).unwrap_or(i32::MAX));
    ui.set_search_loading(loading);

    let mut sections = Vec::with_capacity(search.catalogs.len());
    {
        let _span = tracing::info_span!("map_search_sections").entered();
        let catalog_names = catalog_name_index(addons);
        for catalog in &search.catalogs {
            for page in catalog {
                let Some(Loadable::Ready(items)) = &page.content else {
                    continue;
                };

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

                let cards = {
                    let _span_cards = tracing::info_span!("create_search_cards").entered();
                    items
                        .iter()
                        .take(20)
                        .map(|item| MediaCardItem {
                            id: item.id.as_str().into(),
                            media_type: page.request.path.r#type.as_str().into(),
                            video_id: "".into(),
                            title: item.name.as_str().into(),
                            poster_url: item
                                .poster
                                .as_ref()
                                .map(url::Url::as_str)
                                .unwrap_or_default()
                                .into(),
                            poster: crate::image_cache::get_cached_image(&item.poster),
                            description: item.release_info.as_deref().unwrap_or_default().into(),
                            show_checkmark: false,
                            show_progress: false,
                            progress_value: 0.0,
                        })
                        .collect::<Vec<_>>()
                };

                sections.push(BoardSection {
                    title: format!("{catalog_name} – {media_type}").into(),
                    r_type: page.request.path.r#type.as_str().into(),
                    catalog_id: page.request.path.id.as_str().into(),
                    addon_base: page.request.base.as_str().into(),
                    items: ModelRc::new(VecModel::from(cards)),
                    is_continue_watching: false,
                });
            }
        }
    }

    ui.set_search_sections(ModelRc::new(VecModel::from(sections)));
}
