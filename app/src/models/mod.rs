pub mod addons;
pub mod auth;
pub mod board;
pub mod calendar;
pub mod details;
pub mod discover;
pub mod library;
pub mod search;
pub mod settings;

use crate::MainWindow;
use crate::MediaCardItem;
use slint::{ComponentHandle, Model, ModelRc};
use std::collections::HashSet;

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
    let requested = urls.iter().map(String::as_str).collect::<HashSet<_>>();
    let mut updated = 0;

    updated += patch_cards(&ui.get_board_continue_watching(), &requested);
    let sections = ui.get_board_sections();
    for index in 0..sections.row_count() {
        if let Some(section) = sections.row_data(index) {
            updated += patch_cards(&section.items, &requested);
        }
    }

    let discover = ui.get_discover_rows();
    for index in 0..discover.row_count() {
        if let Some(row) = discover.row_data(index) {
            updated += patch_cards(&row.cols, &requested);
        }
    }

    let library = ui.get_library_rows();
    for index in 0..library.row_count() {
        if let Some(row) = library.row_data(index) {
            updated += patch_cards(&row.cols, &requested);
        }
    }

    let search = ui.get_search_sections();
    for index in 0..search.row_count() {
        if let Some(section) = search.row_data(index) {
            updated += patch_cards(&section.items, &requested);
        }
    }

    updated
}

fn patch_cards(model: &ModelRc<MediaCardItem>, requested: &HashSet<&str>) -> usize {
    let mut updated = 0;
    for index in 0..model.row_count() {
        let Some(mut card) = model.row_data(index) else {
            continue;
        };
        if !requested.contains(card.poster_url.as_str()) {
            continue;
        }
        let poster = crate::image_cache::get_cached_image_url(card.poster_url.as_str());
        if poster.size().width == 0 {
            continue;
        }
        card.poster = poster;
        model.set_row_data(index, card);
        updated += 1;
    }
    updated
}
