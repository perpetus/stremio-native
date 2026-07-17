use std::{cell::RefCell, rc::Rc, time::Duration};

use slint::{
    ComponentHandle, Model, Timer,
    winit_030::{EventResult, WinitWindowAccessor, winit},
};
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::{MainWindow, window_integration::WindowIntegration};

const SPACE_HOLD_DELAY: Duration = Duration::from_millis(400);
const SPACE_HOLD_SPEED: f32 = 2.0;
const VOLUME_STEP: f32 = 0.05;
const SUBTITLE_DELAY_STEP: f32 = 0.25;
const PLAYBACK_SPEED_STEP: f32 = 0.25;
const MIN_PLAYBACK_SPEED: f32 = 0.25;
const MAX_PLAYBACK_SPEED: f32 = 2.0;

#[derive(Default)]
struct ShortcutState {
    modifiers: ModifiersState,
    space_generation: u64,
    space_press: Option<SpacePress>,
    last_subtitle_index: i32,
}

struct SpacePress {
    generation: u64,
    long_press: bool,
    restore_speed: f32,
}

#[derive(Clone, Copy)]
enum PlayerMenu {
    Subtitles,
    Audio,
    Speed,
    Statistics,
    EpisodeInfo,
}

/// Installs all application and player shortcuts through winit's logical-key
/// API. Slint still reports whether its root shortcut scope owns focus, so
/// native shortcuts do not override text editing or transient controls.
pub fn install_platform_shortcuts(ui: &MainWindow, window_integration: WindowIntegration) {
    let weak_ui = ui.as_weak();
    let state = Rc::new(RefCell::new(ShortcutState::default()));

    ui.window().on_winit_window_event(move |window, event| {
        window.with_winit_window(|winit_window| {
            window_integration.handle_winit_event(winit_window, event);
        });
        let Some(ui) = weak_ui.upgrade() else {
            return EventResult::Propagate;
        };

        match event {
            winit::event::WindowEvent::ModifiersChanged(modifiers) => {
                state.borrow_mut().modifiers = modifiers.state();
                EventResult::Propagate
            }
            winit::event::WindowEvent::KeyboardInput { event, .. } => {
                if event.repeat {
                    return if is_space(&event.logical_key) && state.borrow().space_press.is_some() {
                        EventResult::PreventDefault
                    } else {
                        EventResult::Propagate
                    };
                }

                let handled = match event.state {
                    winit::event::ElementState::Pressed => {
                        handle_key_pressed(&ui, &state, &event.logical_key)
                    }
                    winit::event::ElementState::Released => {
                        handle_key_released(&ui, &state, &event.logical_key)
                    }
                };

                if handled {
                    EventResult::PreventDefault
                } else {
                    EventResult::Propagate
                }
            }
            winit::event::WindowEvent::Focused(false) => {
                state.borrow_mut().modifiers = ModifiersState::default();
                cancel_space_hold(&ui, &state);
                EventResult::Propagate
            }
            winit::event::WindowEvent::Occluded(true)
                if ui.get_show_player()
                    && ui.get_settings_pause_on_minimize()
                    && !ui.get_player_paused() =>
            {
                cancel_space_hold(&ui, &state);
                ui.invoke_player_toggle_pause();
                EventResult::Propagate
            }
            _ => EventResult::Propagate,
        }
    });
}

fn handle_key_pressed(ui: &MainWindow, state: &Rc<RefCell<ShortcutState>>, key: &Key) -> bool {
    let modifiers = state.borrow().modifiers;
    let player_active = ui.get_shortcut_player_active();

    if player_active && handle_media_key(ui, key) {
        return true;
    }

    if player_active && is_space(key) && no_modifiers(modifiers) && !player_menu_open(ui) {
        begin_space_hold(ui, state);
        return true;
    }

    if player_active {
        return handle_player_key(ui, state, key, modifiers);
    }

    if matches!(key, Key::Named(NamedKey::Escape)) && ui.get_profile_menu_open() {
        ui.set_profile_menu_open(false);
        return true;
    }

    if !ui.get_application_shortcuts_focused() {
        return false;
    }

    handle_application_key(ui, key, modifiers)
}

fn handle_key_released(ui: &MainWindow, state: &Rc<RefCell<ShortcutState>>, key: &Key) -> bool {
    if !is_space(key) {
        return false;
    }

    let press = {
        let mut state = state.borrow_mut();
        state.space_generation = state.space_generation.wrapping_add(1);
        state.space_press.take()
    };

    let Some(press) = press else {
        return false;
    };

    if press.long_press {
        restore_playback_speed(ui, press.restore_speed);
    } else if ui.get_shortcut_player_active() {
        ui.invoke_player_toggle_pause();
    }
    ui.invoke_player_activity();
    true
}

fn handle_application_key(ui: &MainWindow, key: &Key, modifiers: ModifiersState) -> bool {
    if matches!(key, Key::Named(NamedKey::Backspace)) {
        if primary_modifier_only(modifiers) {
            ui.invoke_navigation_forward();
            return true;
        }
        if no_modifiers(modifiers) {
            ui.invoke_navigation_back();
            return true;
        }
    }

    if primary_modifier_only(modifiers) && character(key) == Some("/") {
        ui.set_settings_active_category(4);
        ui.invoke_tab_changed(4);
        return true;
    }

    if !no_modifiers(modifiers) {
        return false;
    }

    match character(key) {
        Some("0") => ui.invoke_tab_changed(6),
        Some("1") => ui.invoke_tab_changed(0),
        Some("2") => ui.invoke_tab_changed(1),
        Some("3") => ui.invoke_tab_changed(2),
        Some("4") => ui.invoke_tab_changed(5),
        Some("5") => ui.invoke_tab_changed(3),
        Some("6") => ui.invoke_tab_changed(4),
        Some("f") => ui.invoke_toggle_fullscreen(),
        _ => return false,
    }
    true
}

fn handle_player_key(
    ui: &MainWindow,
    state: &Rc<RefCell<ShortcutState>>,
    key: &Key,
    modifiers: ModifiersState,
) -> bool {
    if matches!(key, Key::Named(NamedKey::Escape)) && no_modifiers(modifiers) {
        if player_menu_open(ui) {
            close_player_menus(ui);
            ui.invoke_player_activity();
        } else {
            ui.invoke_player_close();
        }
        return true;
    }

    if shift_only(modifiers) && character(key) == Some("n") && ui.get_player_is_series() {
        close_player_menus(ui);
        ui.invoke_player_play_episode(ui.get_player_active_episode_idx() + 1);
        return true;
    }

    if no_modifiers(modifiers) {
        let menu = match character(key) {
            Some("s") if ui.get_player_subtitles_tracks().row_count() > 0 => {
                Some(PlayerMenu::Subtitles)
            }
            Some("a") if ui.get_player_audio_tracks().row_count() > 0 => Some(PlayerMenu::Audio),
            Some("i") if ui.get_player_is_series() => Some(PlayerMenu::EpisodeInfo),
            Some("r") => Some(PlayerMenu::Speed),
            Some("d") => Some(PlayerMenu::Statistics),
            _ => None,
        };
        if let Some(menu) = menu {
            toggle_player_menu(ui, menu);
            return true;
        }
    }

    if player_menu_open(ui) {
        return false;
    }

    if no_modifiers(modifiers) {
        if matches!(key, Key::Named(NamedKey::ArrowLeft)) {
            ui.invoke_player_seek_relative(-ui.get_player_seek_step_seconds());
            player_activity(ui);
            return true;
        }
        if matches!(key, Key::Named(NamedKey::ArrowRight)) {
            ui.invoke_player_seek_relative(ui.get_player_seek_step_seconds());
            player_activity(ui);
            return true;
        }
        if matches!(key, Key::Named(NamedKey::ArrowUp)) {
            set_volume(ui, ui.get_player_volume() + VOLUME_STEP);
            return true;
        }
        if matches!(key, Key::Named(NamedKey::ArrowDown)) {
            set_volume(ui, ui.get_player_volume() - VOLUME_STEP);
            return true;
        }

        match character(key) {
            Some("f") => ui.invoke_toggle_fullscreen(),
            Some("k") => ui.invoke_player_toggle_pause(),
            Some("m") => ui.invoke_player_toggle_mute(),
            Some("-") => adjust_subtitle_size(ui, -1),
            Some("=") => adjust_subtitle_size(ui, 1),
            Some("g") => adjust_subtitle_delay(ui, -SUBTITLE_DELAY_STEP),
            Some("h") => adjust_subtitle_delay(ui, SUBTITLE_DELAY_STEP),
            Some("[") => adjust_playback_speed(ui, -PLAYBACK_SPEED_STEP),
            Some("]") => adjust_playback_speed(ui, PLAYBACK_SPEED_STEP),
            Some("c") => toggle_subtitles(ui, state),
            _ => return false,
        }
        player_activity(ui);
        return true;
    }

    if shift_only(modifiers) {
        if matches!(key, Key::Named(NamedKey::ArrowLeft)) {
            ui.invoke_player_seek_relative(-ui.get_player_short_seek_step_seconds());
            player_activity(ui);
            return true;
        }
        if matches!(key, Key::Named(NamedKey::ArrowRight)) {
            ui.invoke_player_seek_relative(ui.get_player_short_seek_step_seconds());
            player_activity(ui);
            return true;
        }
    }

    false
}

fn handle_media_key(ui: &MainWindow, key: &Key) -> bool {
    match key {
        Key::Named(NamedKey::MediaPlayPause) => ui.invoke_player_toggle_pause(),
        Key::Named(NamedKey::MediaPlay) if ui.get_player_paused() => {
            ui.invoke_player_toggle_pause();
        }
        Key::Named(NamedKey::MediaPause) if !ui.get_player_paused() => {
            ui.invoke_player_toggle_pause();
        }
        Key::Named(NamedKey::MediaTrackNext) if ui.get_player_is_series() => {
            ui.invoke_player_play_episode(ui.get_player_active_episode_idx() + 1);
        }
        _ => return false,
    }
    player_activity(ui);
    true
}

fn begin_space_hold(ui: &MainWindow, state: &Rc<RefCell<ShortcutState>>) {
    let generation = {
        let mut state = state.borrow_mut();
        state.space_generation = state.space_generation.wrapping_add(1);
        let generation = state.space_generation;
        state.space_press = Some(SpacePress {
            generation,
            long_press: false,
            restore_speed: ui.get_player_playback_speed(),
        });
        generation
    };

    ui.set_player_controls_visible(true);
    ui.invoke_player_activity();

    let weak_ui = ui.as_weak();
    let weak_state = Rc::downgrade(state);
    Timer::single_shot(SPACE_HOLD_DELAY, move || {
        let (Some(ui), Some(state)) = (weak_ui.upgrade(), weak_state.upgrade()) else {
            return;
        };
        if !ui.get_shortcut_player_active() || player_menu_open(&ui) {
            return;
        }

        let should_accelerate = {
            let mut state = state.borrow_mut();
            let Some(press) = state.space_press.as_mut() else {
                return;
            };
            if press.generation != generation {
                return;
            }
            press.long_press = true;
            true
        };

        if should_accelerate {
            ui.set_player_playback_speed(SPACE_HOLD_SPEED);
            ui.invoke_player_change_speed(SPACE_HOLD_SPEED);
            ui.invoke_player_activity();
        }
    });
}

fn cancel_space_hold(ui: &MainWindow, state: &Rc<RefCell<ShortcutState>>) {
    let press = {
        let mut state = state.borrow_mut();
        state.space_generation = state.space_generation.wrapping_add(1);
        state.space_press.take()
    };
    if let Some(press) = press.filter(|press| press.long_press) {
        restore_playback_speed(ui, press.restore_speed);
    }
}

fn restore_playback_speed(ui: &MainWindow, speed: f32) {
    ui.set_player_playback_speed(speed);
    ui.invoke_player_change_speed(speed);
}

fn player_menu_open(ui: &MainWindow) -> bool {
    ui.get_player_show_subtitles_menu()
        || ui.get_player_show_audio_menu()
        || ui.get_player_show_speed_menu()
        || ui.get_player_show_stats_menu()
        || ui.get_player_show_options_menu()
        || ui.get_player_show_playlist_drawer()
        || ui.get_player_show_context_menu()
}

fn close_player_menus(ui: &MainWindow) {
    ui.set_player_show_subtitles_menu(false);
    ui.set_player_show_audio_menu(false);
    ui.set_player_show_speed_menu(false);
    ui.set_player_show_stats_menu(false);
    ui.set_player_show_options_menu(false);
    ui.set_player_show_playlist_drawer(false);
    ui.set_player_show_context_menu(false);
}

fn toggle_player_menu(ui: &MainWindow, menu: PlayerMenu) {
    let was_open = match menu {
        PlayerMenu::Subtitles => ui.get_player_show_subtitles_menu(),
        PlayerMenu::Audio => ui.get_player_show_audio_menu(),
        PlayerMenu::Speed => ui.get_player_show_speed_menu(),
        PlayerMenu::Statistics => ui.get_player_show_stats_menu(),
        PlayerMenu::EpisodeInfo => ui.get_player_show_playlist_drawer(),
    };
    close_player_menus(ui);

    if !was_open {
        match menu {
            PlayerMenu::Subtitles => ui.set_player_show_subtitles_menu(true),
            PlayerMenu::Audio => ui.set_player_show_audio_menu(true),
            PlayerMenu::Speed => ui.set_player_show_speed_menu(true),
            PlayerMenu::Statistics => ui.set_player_show_stats_menu(true),
            PlayerMenu::EpisodeInfo => ui.set_player_show_playlist_drawer(true),
        }
    }
    player_activity(ui);
}

fn set_volume(ui: &MainWindow, volume: f32) {
    let volume = volume.clamp(0.0, 1.0);
    ui.set_player_volume(volume);
    ui.invoke_player_change_volume(volume);
    player_activity(ui);
}

fn adjust_subtitle_size(ui: &MainWindow, direction: i8) {
    let size = stepped_subtitle_size(ui.get_player_subtitle_size_percent(), direction);
    ui.set_player_subtitle_size_percent(size);
    ui.invoke_player_change_subtitle_size(size);
}

fn stepped_subtitle_size(current: f32, direction: i8) -> f32 {
    const SIZES: [f32; 7] = [75.0, 100.0, 125.0, 150.0, 175.0, 200.0, 250.0];
    if direction > 0 {
        SIZES
            .into_iter()
            .find(|size| *size > current)
            .unwrap_or(250.0)
    } else {
        SIZES
            .into_iter()
            .rev()
            .find(|size| *size < current)
            .unwrap_or(75.0)
    }
}

fn adjust_subtitle_delay(ui: &MainWindow, delta: f32) {
    let delay = ui.get_player_subtitle_delay_seconds() + delta;
    ui.set_player_subtitle_delay_seconds(delay);
    ui.invoke_player_change_subtitle_delay(delay);
}

fn adjust_playback_speed(ui: &MainWindow, delta: f32) {
    let speed =
        (ui.get_player_playback_speed() + delta).clamp(MIN_PLAYBACK_SPEED, MAX_PLAYBACK_SPEED);
    ui.set_player_playback_speed(speed);
    ui.invoke_player_change_speed(speed);
}

fn toggle_subtitles(ui: &MainWindow, state: &Rc<RefCell<ShortcutState>>) {
    if ui.get_player_active_subtitle_idx() >= 0 {
        state.borrow_mut().last_subtitle_index = ui.get_player_active_subtitle_idx();
        ui.invoke_player_change_subtitle(-1);
    } else if ui.get_player_subtitles_tracks().row_count() > 0 {
        ui.invoke_player_change_subtitle(state.borrow().last_subtitle_index);
    }
}

fn player_activity(ui: &MainWindow) {
    ui.set_player_controls_visible(true);
    ui.invoke_player_activity();
}

fn character(key: &Key) -> Option<&str> {
    match key {
        Key::Character(value) => Some(value.as_str()),
        _ => None,
    }
}

fn is_space(key: &Key) -> bool {
    matches!(key, Key::Named(NamedKey::Space))
}

fn no_modifiers(modifiers: ModifiersState) -> bool {
    modifiers.is_empty()
}

fn shift_only(modifiers: ModifiersState) -> bool {
    modifiers.shift_key()
        && !modifiers.control_key()
        && !modifiers.alt_key()
        && !modifiers.super_key()
}

#[cfg(target_os = "macos")]
fn primary_modifier_only(modifiers: ModifiersState) -> bool {
    modifiers.super_key()
        && !modifiers.shift_key()
        && !modifiers.control_key()
        && !modifiers.alt_key()
}

#[cfg(not(target_os = "macos"))]
fn primary_modifier_only(modifiers: ModifiersState) -> bool {
    modifiers.control_key()
        && !modifiers.shift_key()
        && !modifiers.alt_key()
        && !modifiers.super_key()
}

#[cfg(test)]
mod tests {
    use super::stepped_subtitle_size;

    #[test]
    fn subtitle_size_steps_across_supported_values() {
        assert_eq!(stepped_subtitle_size(100.0, 1), 125.0);
        assert_eq!(stepped_subtitle_size(125.0, -1), 100.0);
        assert_eq!(stepped_subtitle_size(90.0, 1), 100.0);
        assert_eq!(stepped_subtitle_size(90.0, -1), 75.0);
    }

    #[test]
    fn subtitle_size_clamps_at_supported_limits() {
        assert_eq!(stepped_subtitle_size(250.0, 1), 250.0);
        assert_eq!(stepped_subtitle_size(75.0, -1), 75.0);
    }
}
