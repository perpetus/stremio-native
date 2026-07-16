use slint::{
    ComponentHandle,
    winit_030::{EventResult, WinitWindowAccessor, winit},
};

use crate::MainWindow;

/// Installs the keyboard events that Slint cannot represent, notably the
/// operating system's media keys. Normal application and player shortcuts
/// remain declarative in `app.slint` so editable controls retain standard key
/// behavior.
pub fn install_platform_shortcuts(ui: &MainWindow) {
    let weak_ui = ui.as_weak();

    ui.window().on_winit_window_event(move |_window, event| {
        let Some(ui) = weak_ui.upgrade() else {
            return EventResult::Propagate;
        };

        match event {
            winit::event::WindowEvent::KeyboardInput { event, .. }
                if event.state == winit::event::ElementState::Pressed && !event.repeat =>
            {
                if !ui.get_show_player() {
                    return EventResult::Propagate;
                }

                use winit::keyboard::{Key, NamedKey};
                match &event.logical_key {
                    Key::Named(NamedKey::MediaPlayPause) => {
                        ui.invoke_player_toggle_pause();
                    }
                    Key::Named(NamedKey::MediaPlay) if ui.get_player_paused() => {
                        ui.invoke_player_toggle_pause();
                    }
                    Key::Named(NamedKey::MediaPause) if !ui.get_player_paused() => {
                        ui.invoke_player_toggle_pause();
                    }
                    Key::Named(NamedKey::MediaTrackNext) if ui.get_player_is_series() => {
                        ui.invoke_player_play_episode(ui.get_player_active_episode_idx() + 1);
                    }
                    _ => return EventResult::Propagate,
                }

                ui.invoke_player_activity();
                EventResult::PreventDefault
            }
            winit::event::WindowEvent::Occluded(true)
                if ui.get_show_player()
                    && ui.get_settings_pause_on_minimize()
                    && !ui.get_player_paused() =>
            {
                ui.invoke_player_toggle_pause();
                EventResult::Propagate
            }
            _ => EventResult::Propagate,
        }
    });
}
