pub mod addons;
pub mod auth;
pub mod board;
pub mod calendar;
pub mod details;
pub mod discover;
pub mod library;
pub mod search;
pub mod settings;

use crate::{
    AddonItem, CalendarMediaItem, EpisodeItem, MainWindow, MediaCardItem, SearchSuggestion,
};
use slint::{ComponentHandle, Model, ModelRc};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};
use stremio_core::types::addon::Descriptor;

pub(crate) type SyncFingerprint = [u8; 32];

pub(crate) struct Fingerprint(blake3::Hasher);

impl Fingerprint {
    pub(crate) fn new() -> Self {
        Self(blake3::Hasher::new())
    }

    pub(crate) fn str(&mut self, value: &str) {
        self.usize(value.len());
        self.0.update(value.as_bytes());
    }

    pub(crate) fn bytes(&mut self, value: &[u8]) {
        self.usize(value.len());
        self.0.update(value);
    }

    pub(crate) fn optional_str(&mut self, value: Option<&str>) {
        self.bool(value.is_some());
        if let Some(value) = value {
            self.str(value);
        }
    }

    pub(crate) fn bool(&mut self, value: bool) {
        self.0.update(&[u8::from(value)]);
    }

    pub(crate) fn usize(&mut self, value: usize) {
        self.0.update(&value.to_le_bytes());
    }

    pub(crate) fn u64(&mut self, value: u64) {
        self.0.update(&value.to_le_bytes());
    }

    pub(crate) fn finish(self) -> SyncFingerprint {
        *self.0.finalize().as_bytes()
    }
}

pub(crate) fn sync_fingerprint_changed(
    cache: &OnceLock<Mutex<Option<SyncFingerprint>>>,
    fingerprint: SyncFingerprint,
) -> bool {
    let cache = cache.get_or_init(|| Mutex::new(None));
    let mut previous = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if previous.as_ref() == Some(&fingerprint) {
        false
    } else {
        *previous = Some(fingerprint);
        true
    }
}

pub(crate) fn clear_sync_fingerprint(cache: &OnceLock<Mutex<Option<SyncFingerprint>>>) {
    if let Some(cache) = cache.get() {
        *cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

pub(crate) fn profile_addons_fingerprint(addons: &[Descriptor]) -> SyncFingerprint {
    let mut fingerprint = Fingerprint::new();
    for addon in addons {
        fingerprint.str(addon.transport_url.as_str());
        fingerprint.str(&addon.manifest.id);
        fingerprint.str(&addon.manifest.name);
        fingerprint.optional_str(addon.manifest.description.as_deref());
        fingerprint.optional_str(addon.manifest.logo.as_ref().map(url::Url::as_str));
        fingerprint.u64(addon.manifest.version.major);
        fingerprint.u64(addon.manifest.version.minor);
        fingerprint.u64(addon.manifest.version.patch);
        fingerprint.str(addon.manifest.version.pre.as_str());
        fingerprint.str(addon.manifest.version.build.as_str());
        for addon_type in &addon.manifest.types {
            fingerprint.str(addon_type);
        }
        fingerprint.bool(addon.manifest.behavior_hints.configurable);
        fingerprint.bool(addon.manifest.behavior_hints.configuration_required);
        for catalog in &addon.manifest.catalogs {
            fingerprint.str(&catalog.id);
            fingerprint.str(&catalog.r#type);
            fingerprint.optional_str(catalog.name.as_deref());
        }
    }
    fingerprint.finish()
}

pub(crate) fn profile_catalogs_fingerprint(addons: &[Descriptor]) -> SyncFingerprint {
    let mut fingerprint = Fingerprint::new();
    for addon in addons {
        fingerprint.str(addon.transport_url.as_str());
        fingerprint.str(&addon.manifest.id);
        for catalog in &addon.manifest.catalogs {
            fingerprint.str(&catalog.id);
            fingerprint.str(&catalog.r#type);
            fingerprint.optional_str(catalog.name.as_deref());
        }
    }
    fingerprint.finish()
}

pub(crate) fn catalog_name_index<'a>(
    addons: &'a [Descriptor],
) -> HashMap<(&'a str, &'a str, &'a str), &'a str> {
    let entry_count = addons
        .iter()
        .map(|addon| addon.manifest.catalogs.len())
        .sum();
    let mut names = HashMap::with_capacity(entry_count);
    for addon in addons {
        for catalog in &addon.manifest.catalogs {
            names.insert(
                (
                    addon.transport_url.as_str(),
                    catalog.id.as_str(),
                    catalog.r#type.as_str(),
                ),
                catalog.name.as_deref().unwrap_or(catalog.id.as_str()),
            );
        }
    }
    names
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MediaGridMetrics {
    pub columns: usize,
}

/// Keep Rust-side row virtualization aligned with the responsive Slint media grid.
pub(crate) fn media_grid_metrics(ui: &MainWindow) -> MediaGridMetrics {
    let window = ui.window();
    let logical_width = window.size().width as f32 / window.scale_factor().max(1.0);
    let provisional_unit = if logical_width > 2800.0 {
        18.0
    } else if logical_width > 2200.0 {
        16.0
    } else if logical_width > 1600.0 {
        15.0
    } else {
        14.0
    };
    let provisional_page_width = logical_width - 6.0 * provisional_unit;
    let unit = if provisional_page_width > 2700.0 {
        18.0
    } else if provisional_page_width > 2100.0 {
        16.0
    } else if provisional_page_width > 1500.0 {
        15.0
    } else {
        14.0
    };
    let page_width = (logical_width - 6.0 * unit).max(1.0);
    let columns: usize = if page_width > 2100.0 {
        9
    } else if page_width > 1500.0 {
        8
    } else if page_width > 1200.0 {
        7
    } else if page_width > 900.0 {
        6
    } else {
        4
    };
    MediaGridMetrics { columns }
}

// Helper to chunk an owned vector into rows of specified size without cloning
pub fn chunk_vector_owned<T>(items: Vec<T>, chunk_size: usize) -> Vec<Vec<T>> {
    if items.is_empty() {
        return Vec::new();
    }
    let mut rows = Vec::with_capacity((items.len() + chunk_size - 1) / chunk_size);
    let mut iter = items.into_iter();
    loop {
        let chunk: Vec<T> = iter.by_ref().take(chunk_size).collect();
        if chunk.is_empty() {
            break;
        }
        rows.push(chunk);
    }
    rows
}

/// Updates only the cards whose async image requests just completed. Keeping
/// the existing `ModelRc`s alive avoids rebuilding every catalog row when a
/// poster becomes available.
pub(crate) fn refresh_cached_media_images(ui: &MainWindow, urls: &[String]) -> usize {
    let images = urls
        .iter()
        .filter_map(|url| cached_image(url).map(|image| (url.as_str(), image)))
        .collect::<HashMap<_, _>>();
    let mut updated = 0;

    updated += patch_cards(&ui.get_board_continue_watching(), &images);
    let sections = ui.get_board_sections();
    for index in 0..sections.row_count() {
        if let Some(section) = sections.row_data(index) {
            updated += patch_cards(&section.items, &images);
        }
    }

    let discover = ui.get_discover_rows();
    for index in 0..discover.row_count() {
        if let Some(row) = discover.row_data(index) {
            updated += patch_cards(&row.cols, &images);
        }
    }

    let library = ui.get_library_rows();
    for index in 0..library.row_count() {
        if let Some(row) = library.row_data(index) {
            updated += patch_cards(&row.cols, &images);
        }
    }

    let search = ui.get_search_sections();
    for index in 0..search.row_count() {
        if let Some(section) = search.row_data(index) {
            updated += patch_cards(&section.items, &images);
        }
    }

    updated += patch_search_suggestions(&ui.get_search_suggestions(), &images);
    updated += patch_addons(&ui.get_addons_list(), &images);
    updated += patch_episode_items(&ui.get_detail_episodes(), &images);

    let mut addon_details = ui.get_addon_details_addon();
    if let Some(logo) = images.get(addon_details.logo_url.as_str()) {
        addon_details.logo = logo.clone();
        ui.set_addon_details_addon(addon_details);
        updated += 1;
    }

    let detail_poster_url = ui.get_detail_poster_url();
    if let Some(poster) = images.get(detail_poster_url.as_str()) {
        ui.set_detail_poster(poster.clone());
        updated += 1;
    }
    let detail_background_url = ui.get_detail_background_url();
    if let Some(background) = images.get(detail_background_url.as_str()) {
        ui.set_detail_background(background.clone());
        updated += 1;
    }
    let discover_preview_poster_url = ui.get_discover_preview_poster_url();
    if let Some(poster) = images.get(discover_preview_poster_url.as_str()) {
        ui.set_discover_preview_poster(poster.clone());
        updated += 1;
    }

    let calendar = ui.get_calendar_rows();
    for row_index in 0..calendar.row_count() {
        let Some(row) = calendar.row_data(row_index) else {
            continue;
        };
        for cell_index in 0..row.cells.row_count() {
            let Some(cell) = row.cells.row_data(cell_index) else {
                continue;
            };
            updated += patch_calendar_items(&cell.items, &images);
        }
    }

    updated
}

fn cached_image(url: &str) -> Option<slint::Image> {
    let image = crate::image_cache::get_cached_image_url(url);
    (image.size().width > 0).then_some(image)
}

fn patch_cards(model: &ModelRc<MediaCardItem>, images: &HashMap<&str, slint::Image>) -> usize {
    let mut updated = 0;
    for index in 0..model.row_count() {
        let Some(mut card) = model.row_data(index) else {
            continue;
        };
        let Some(poster) = images.get(card.poster_url.as_str()) else {
            continue;
        };
        card.poster = poster.clone();
        model.set_row_data(index, card);
        updated += 1;
    }
    updated
}

fn patch_search_suggestions(
    model: &ModelRc<SearchSuggestion>,
    images: &HashMap<&str, slint::Image>,
) -> usize {
    let mut updated = 0;
    for index in 0..model.row_count() {
        let Some(mut suggestion) = model.row_data(index) else {
            continue;
        };
        let Some(poster) = images.get(suggestion.poster_url.as_str()) else {
            continue;
        };
        suggestion.poster = poster.clone();
        model.set_row_data(index, suggestion);
        updated += 1;
    }
    updated
}

fn patch_addons(model: &ModelRc<AddonItem>, images: &HashMap<&str, slint::Image>) -> usize {
    let mut updated = 0;
    for index in 0..model.row_count() {
        let Some(mut addon) = model.row_data(index) else {
            continue;
        };
        let Some(logo) = images.get(addon.logo_url.as_str()) else {
            continue;
        };
        addon.logo = logo.clone();
        model.set_row_data(index, addon);
        updated += 1;
    }
    updated
}

fn patch_calendar_items(
    model: &ModelRc<CalendarMediaItem>,
    images: &HashMap<&str, slint::Image>,
) -> usize {
    let mut updated = 0;
    for index in 0..model.row_count() {
        let Some(mut item) = model.row_data(index) else {
            continue;
        };
        let Some(poster) = images.get(item.poster_url.as_str()) else {
            continue;
        };
        item.poster = poster.clone();
        model.set_row_data(index, item);
        updated += 1;
    }
    updated
}

fn patch_episode_items(
    model: &ModelRc<EpisodeItem>,
    images: &HashMap<&str, slint::Image>,
) -> usize {
    let mut updated = 0;
    for index in 0..model.row_count() {
        let Some(mut episode) = model.row_data(index) else {
            continue;
        };
        let Some(thumbnail) = images.get(episode.thumbnail_url.as_str()) else {
            continue;
        };
        episode.thumbnail = thumbnail.clone();
        model.set_row_data(index, episode);
        updated += 1;
    }
    updated
}
