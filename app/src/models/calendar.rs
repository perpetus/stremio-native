use crate::models::details::{load_meta_details_for_video, open_details_route};
use crate::models::{Fingerprint, SyncFingerprint, sync_fingerprint_changed};
use crate::{
    AppModel, CalendarCell, CalendarMediaItem, CalendarRow, MainWindow, NavigationController,
};
use core_env::DesktopEnv;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex, OnceLock};
use stremio_core::{
    models::calendar::{Calendar, YearMonthDate},
    runtime::{
        Runtime, RuntimeAction,
        msg::{Action, ActionLoad},
    },
    types::{addon::Descriptor, library::LibraryBucket},
};

const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

static LAST_SYNC_STATE: OnceLock<Mutex<Option<SyncFingerprint>>> = OnceLock::new();
static LAST_SOURCE_STATE: OnceLock<Mutex<Option<SyncFingerprint>>> = OnceLock::new();

fn month_name(month: u32) -> &'static str {
    month
        .checked_sub(1)
        .and_then(|index| MONTH_NAMES.get(index as usize))
        .copied()
        .unwrap_or("")
}

fn dispatch_calendar(
    runtime: &Arc<Runtime<DesktopEnv, AppModel>>,
    selected: Option<YearMonthDate>,
) -> bool {
    let Some((source, current_selection)) = runtime.model().ok().map(|model| {
        (
            source_fingerprint(&model.ctx.library, &model.ctx.profile.addons),
            model.calendar.selected.clone(),
        )
    }) else {
        return false;
    };
    let source_changed = {
        let cache = LAST_SOURCE_STATE.get_or_init(|| Mutex::new(None));
        let mut previous = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let changed = previous
            .as_ref()
            .is_some_and(|previous| previous != &source);
        *previous = Some(source);
        changed
    };
    let selection_changed = selected
        .as_ref()
        .is_some_and(|selected| current_selection.as_ref() != Some(selected));
    if current_selection.is_some() && !source_changed && !selection_changed {
        return false;
    }

    if source_changed {
        runtime.dispatch(RuntimeAction {
            field: Some(crate::AppModelField::Calendar),
            action: Action::Unload,
        });
    }
    runtime.dispatch(RuntimeAction {
        field: Some(crate::AppModelField::Calendar),
        action: Action::Load(ActionLoad::Calendar(selected)),
    });
    true
}

pub(crate) fn ensure_loaded(runtime: &Arc<Runtime<DesktopEnv, AppModel>>) -> bool {
    let selected = runtime
        .model()
        .ok()
        .and_then(|model| model.calendar.selected.clone());
    dispatch_calendar(runtime, selected)
}

pub fn setup(
    ui: &MainWindow,
    runtime: &Arc<Runtime<DesktopEnv, AppModel>>,
    navigation: &NavigationController,
) {
    let ui_weak = ui.as_weak();

    ui.on_calendar_previous({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        move || {
            let selected = runtime
                .model()
                .ok()
                .map(|model| model.calendar.selectable.prev.clone());
            if let Some(selected) = selected {
                if dispatch_calendar(&runtime, Some(selected))
                    && let Some(ui) = ui_weak.upgrade()
                {
                    ui.set_calendar_loading(true);
                }
            }
        }
    });

    ui.on_calendar_next({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        move || {
            let selected = runtime
                .model()
                .ok()
                .map(|model| model.calendar.selectable.next.clone());
            if let Some(selected) = selected {
                if dispatch_calendar(&runtime, Some(selected))
                    && let Some(ui) = ui_weak.upgrade()
                {
                    ui.set_calendar_loading(true);
                }
            }
        }
    });

    ui.on_calendar_item_selected({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        let navigation = navigation.clone();
        move |id, media_type, video_id| {
            let id = id.to_string();
            let media_type = media_type.to_string();
            let video_id = (!video_id.is_empty()).then(|| video_id.to_string());
            if let Some(ui) = ui_weak.upgrade() {
                open_details_route(&ui, &runtime, &navigation, &id);
            }
            load_meta_details_for_video(&runtime, id, Some(media_type), video_id);
        }
    });
}

#[tracing::instrument(skip_all)]
pub fn sync(ui: &MainWindow, calendar: &Calendar, ui_weak: &slint::Weak<MainWindow>) {
    ui.set_calendar_loading(false);
    if !sync_fingerprint_changed(&LAST_SYNC_STATE, state_fingerprint(calendar)) {
        return;
    }

    let Some(selected) = calendar.selected.as_ref() else {
        ui.set_calendar_rows(slint::ModelRc::default());
        return;
    };

    ui.set_calendar_selected_year(selected.year);
    ui.set_calendar_selected_month(month_name(selected.month).into());
    ui.set_calendar_previous_month(month_name(calendar.selectable.prev.month).into());
    ui.set_calendar_next_month(month_name(calendar.selectable.next.month).into());

    let offset = calendar.month_info.first_weekday as usize;
    let day_count = calendar.month_info.days as usize;
    let cell_count = (offset + day_count).div_ceil(7) * 7;
    let mut cells = Vec::with_capacity(cell_count);

    let rows = {
        let _span = tracing::info_span!("map_calendar_schedule").entered();
        for _ in 0..offset {
            cells.push(CalendarCell {
                blank: true,
                day: 0,
                today: false,
                items: slint::ModelRc::default(),
            });
        }

        for day in &calendar.items {
            let media = day
                .items
                .iter()
                .map(|content| CalendarMediaItem {
                    meta_id: content.meta_item.preview.id.as_str().into(),
                    media_type: content.meta_item.preview.r#type.as_str().into(),
                    video_id: content.video.id.as_str().into(),
                    title: content.meta_item.preview.name.as_str().into(),
                    poster_url: content
                        .meta_item
                        .preview
                        .poster
                        .as_ref()
                        .map(url::Url::as_str)
                        .unwrap_or_default()
                        .into(),
                    poster: crate::image_cache::get_poster_image(
                        &content.meta_item.preview.poster,
                        ui_weak,
                    ),
                })
                .collect::<Vec<_>>();

            cells.push(CalendarCell {
                blank: false,
                day: i32::try_from(day.date.day).unwrap_or_default(),
                today: calendar.month_info.today == Some(day.date.day),
                items: slint::ModelRc::new(slint::VecModel::from(media)),
            });
        }

        while cells.len() < cell_count {
            cells.push(CalendarCell {
                blank: true,
                day: 0,
                today: false,
                items: slint::ModelRc::default(),
            });
        }

        cells
            .chunks(7)
            .map(|cells| CalendarRow {
                cells: slint::ModelRc::new(slint::VecModel::from(cells.to_vec())),
            })
            .collect::<Vec<_>>()
    };

    ui.set_calendar_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
}

pub(crate) fn state_fingerprint(calendar: &Calendar) -> SyncFingerprint {
    let mut fingerprint = Fingerprint::new();
    if let Some(selected) = &calendar.selected {
        fingerprint.bool(true);
        fingerprint.u64(selected.year as u64);
        fingerprint.u64(u64::from(selected.month));
    } else {
        fingerprint.bool(false);
    }
    fingerprint.u64(calendar.selectable.prev.year as u64);
    fingerprint.u64(u64::from(calendar.selectable.prev.month));
    fingerprint.u64(calendar.selectable.next.year as u64);
    fingerprint.u64(u64::from(calendar.selectable.next.month));
    fingerprint.u64(u64::from(calendar.month_info.first_weekday));
    fingerprint.u64(u64::from(calendar.month_info.days));
    fingerprint.u64(u64::from(calendar.month_info.today.unwrap_or_default()));
    for day in &calendar.items {
        fingerprint.u64(day.date.year as u64);
        fingerprint.u64(u64::from(day.date.month));
        fingerprint.u64(u64::from(day.date.day));
        for content in &day.items {
            let preview = &content.meta_item.preview;
            fingerprint.str(&preview.id);
            fingerprint.str(&preview.r#type);
            fingerprint.str(&content.video.id);
            fingerprint.str(&preview.name);
            fingerprint.optional_str(preview.poster.as_ref().map(url::Url::as_str));
        }
    }
    fingerprint.finish()
}

pub(crate) fn source_fingerprint(
    library: &LibraryBucket,
    addons: &[Descriptor],
) -> SyncFingerprint {
    let mut relevant_items = library
        .items
        .values()
        .filter(|item| !item.removed && !item.temp)
        .collect::<Vec<_>>();
    relevant_items.sort_unstable_by(|left, right| {
        right
            .mtime
            .cmp(&left.mtime)
            .then_with(|| left.id.cmp(&right.id))
    });
    relevant_items.truncate(stremio_core::constants::CALENDAR_ITEMS_COUNT);
    // Request order does not affect the resulting schedule. Canonicalizing the
    // selected ID set avoids a metadata reload when playback only changes mtime.
    relevant_items.sort_unstable_by(|left, right| left.id.cmp(&right.id));

    let mut fingerprint = Fingerprint::new();
    for item in relevant_items {
        fingerprint.str(&item.id);
        fingerprint.str(&item.r#type);
    }
    fingerprint.bytes(&crate::models::profile_addons_fingerprint(addons));
    for addon in addons {
        for catalog in &addon.manifest.catalogs {
            for extra in catalog.extra.iter() {
                fingerprint.str(&extra.name);
                fingerprint.bool(extra.is_required);
                for option in &extra.options {
                    fingerprint.str(option);
                }
            }
        }
    }
    fingerprint.finish()
}
