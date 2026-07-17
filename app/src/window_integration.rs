use std::{cell::RefCell, rc::Rc};

use playback_mpv::SoftwareFrameSource;
use slint::winit_030::winit;

use crate::{MainWindow, PlayerVideoSurfaceMode};

#[cfg(windows)]
use crate::platform::windows::VideoHostWindow;

#[derive(Clone)]
pub struct WindowIntegration {
    state: Rc<RefCell<WindowIntegrationState>>,
}

struct WindowIntegrationState {
    #[cfg(windows)]
    video_host: Option<VideoHostWindow>,
    software_source: Option<SoftwareFrameSource>,
    player_surface_visible: bool,
    occluded: bool,
}

impl WindowIntegration {
    pub fn new(
        #[cfg(windows)] video_host: Option<VideoHostWindow>,
        software_source: Option<SoftwareFrameSource>,
    ) -> Self {
        Self {
            state: Rc::new(RefCell::new(WindowIntegrationState {
                #[cfg(windows)]
                video_host,
                software_source,
                player_surface_visible: false,
                occluded: false,
            })),
        }
    }

    pub fn install_surface_callback(&self, ui: &MainWindow) {
        let integration = self.clone();
        ui.on_player_surface_state_changed(move |visible, mode| {
            integration.set_player_surface_state(visible, mode);
        });
        self.set_player_surface_state(ui.get_show_player(), ui.get_player_video_surface_mode());
    }

    pub fn handle_winit_event(
        &self,
        window: &winit::window::Window,
        event: &winit::event::WindowEvent,
    ) {
        let mut state = self.state.borrow_mut();

        #[cfg(windows)]
        if let Some(host) = state.video_host.as_mut() {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

            match window.window_handle().map(|handle| handle.as_raw()) {
                Ok(RawWindowHandle::Win32(handle)) => {
                    host.attach_main_handle(handle.hwnd.get());
                }
                Ok(_) => tracing::error!("Slint did not expose a Win32 main-window handle"),
                Err(error) => tracing::warn!(%error, "could not read Slint window handle"),
            }
        }

        if let winit::event::WindowEvent::Occluded(occluded) = event {
            state.occluded = *occluded;
            #[cfg(windows)]
            if let Some(host) = state.video_host.as_mut() {
                host.set_occluded(*occluded);
            }
        }

        let minimized = window.is_minimized().unwrap_or(false);
        let software_visible = state.player_surface_visible && !state.occluded && !minimized;
        if let Some(source) = state.software_source.as_ref() {
            let size = window.inner_size();
            source.set_target_size(size.width, size.height);
            source.set_visible(software_visible);
        }

        #[cfg(windows)]
        if let Some(host) = state.video_host.as_mut() {
            host.sync();
        }
    }

    pub fn shutdown(&self) {
        let mut state = self.state.borrow_mut();
        state.player_surface_visible = false;
        if let Some(source) = state.software_source.as_ref() {
            source.set_visible(false);
            source.clear_wakeup_callback();
        }
        #[cfg(windows)]
        if let Some(host) = state.video_host.as_mut() {
            host.set_requested_visible(false);
        }
    }

    fn set_player_surface_state(&self, visible: bool, mode: PlayerVideoSurfaceMode) {
        let mut state = self.state.borrow_mut();
        let native_visible = visible && mode == PlayerVideoSurfaceMode::NativeWindow;
        let image_visible = visible && mode == PlayerVideoSurfaceMode::Image;
        state.player_surface_visible = native_visible || image_visible;

        #[cfg(windows)]
        if let Some(host) = state.video_host.as_mut() {
            host.set_requested_visible(native_visible);
        }
        if let Some(source) = state.software_source.as_ref() {
            source.set_visible(image_visible && !state.occluded);
        }
    }
}
