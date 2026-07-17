use crate::models::details::{load_meta_details_for_video, selected_details_are_ready};
use crate::models::{Fingerprint, SyncFingerprint, sync_fingerprint_changed};
use crate::{
    AppModel, CalendarCell, CalendarMediaItem, CalendarRow, MainWindow, NavigationController,
    NavigationIntent,
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

fn month_name(month: u32) -> &'static str {
    month
        .checked_sub(1)
        .and_then(|index| MONTH_NAMES.get(index as usize))
        .copied()
        .unwrap_or("")
}

fn dispatch_month(runtime: &Arc<Runtime<DesktopEnv, AppModel>>, selected: YearMonthDate) {
    runtime.dispatch(RuntimeAction {
        field: None,
        action: Action::Load(ActionLoad::Calendar(Some(selected))),
    });
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
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_calendar_loading(true);
                }
                dispatch_month(&runtime, selected);
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
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_calendar_loading(true);
                }
                dispatch_month(&runtime, selected);
            }
        }
    });

    ui.on_calendar_item_selected({
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        let navigation = navigation.clone();
        move |id, media_type| {
            let id = id.to_string();
            let media_type = media_type.to_string();
            let details_ready = selected_details_are_ready(&runtime, &id);
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_details_loading(!details_ready);
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
            fingerprint.str(&preview.name);
            fingerprint.optional_str(preview.poster.as_ref().map(url::Url::as_str));
        }
    }
    fingerprint.finish()
}
