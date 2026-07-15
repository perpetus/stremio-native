use crate::models::details::load_meta_details;
use crate::{AppModel, CalendarCell, CalendarMediaItem, CalendarRow, MainWindow};
use core_env::DesktopEnv;
use slint::ComponentHandle;
use std::sync::Arc;
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

pub fn setup(ui: &MainWindow, runtime: &Arc<Runtime<DesktopEnv, AppModel>>) {
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
        move |id| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_loading(true);
            }
            let runtime = runtime.clone();
            let id = id.to_string();
            tokio::spawn(async move {
                load_meta_details(&runtime, id).await;
            });
        }
    });
}

#[tracing::instrument(skip_all)]
pub fn sync(ui: &MainWindow, calendar: &Calendar, ui_weak: &slint::Weak<MainWindow>) {
    ui.set_calendar_loading(false);
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
                meta_id: content.meta_item.preview.id.clone().into(),
                title: content.meta_item.preview.name.clone().into(),
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

    let rows = cells
        .chunks(7)
        .map(|cells| CalendarRow {
            cells: slint::ModelRc::new(slint::VecModel::from(cells.to_vec())),
        })
        .collect::<Vec<_>>();

    ui.set_calendar_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
}
