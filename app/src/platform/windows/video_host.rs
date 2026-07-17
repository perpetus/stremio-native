use std::{marker::PhantomData, num::NonZeroU32, rc::Rc};

use thiserror::Error;
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::{
            BLACK_BRUSH, BeginPaint, ClientToScreen, EndPaint, FillRect, GetStockObject, HBRUSH,
            PAINTSTRUCT,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, IsIconic,
            IsWindowVisible, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
            SWP_NOOWNERZORDER, SetWindowPos, ShowWindow, WM_ERASEBKGND, WM_PAINT, WNDCLASSW,
            WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
        },
    },
    core::w,
};

const CLASS_NAME: windows::core::PCWSTR = w!("StremioNativeVideoHost");

/// A non-activating top-level window kept directly behind Slint's main window.
///
/// libmpv receives this HWND and creates its own video child. The host is
/// intentionally unowned so Windows never forces it above the Slint overlay.
pub struct VideoHostWindow {
    hwnd: HWND,
    main_hwnd: Option<HWND>,
    requested_visible: bool,
    occluded: bool,
    _ui_thread_only: PhantomData<Rc<()>>,
}

impl VideoHostWindow {
    pub fn create() -> Result<Self, VideoHostError> {
        // SAFETY: The current module handle is valid for the process lifetime.
        let module = unsafe { GetModuleHandleW(None) }.map_err(VideoHostError::Windows)?;
        let instance = HINSTANCE(module.0);
        // SAFETY: GetStockObject returns a process-owned stock brush which must
        // not be deleted by this window class.
        let black_brush = HBRUSH(unsafe { GetStockObject(BLACK_BRUSH) }.0);
        let class = WNDCLASSW {
            lpfnWndProc: Some(video_host_window_proc),
            hInstance: instance,
            hbrBackground: black_brush,
            lpszClassName: CLASS_NAME,
            ..WNDCLASSW::default()
        };
        // SAFETY: The class structure and static class name remain valid for
        // the synchronous registration call. A previous registration is fine.
        unsafe { RegisterClassW(&class) };

        // SAFETY: All handles and strings are valid. Passing no parent keeps
        // this popup unowned; it starts hidden at a harmless 1x1 size.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                CLASS_NAME,
                w!("Stremio Video"),
                WS_POPUP | WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(instance),
                None,
            )
        }
        .map_err(VideoHostError::Windows)?;

        Ok(Self {
            hwnd,
            main_hwnd: None,
            requested_visible: false,
            occluded: false,
            _ui_thread_only: PhantomData,
        })
    }

    pub fn native_target(&self) -> Result<NonZeroU32, VideoHostError> {
        let raw = self.hwnd.0 as usize;
        let value = u32::try_from(raw).map_err(|_| VideoHostError::HandleOutOfRange(raw))?;
        NonZeroU32::new(value).ok_or(VideoHostError::NullHandle)
    }

    pub fn attach_main_handle(&mut self, raw: isize) {
        let hwnd = HWND(raw as *mut core::ffi::c_void);
        if self.main_hwnd != Some(hwnd) {
            self.main_hwnd = Some(hwnd);
            tracing::info!(main_hwnd = raw, video_host_hwnd = ?self.hwnd.0, "video host attached to Slint window");
        }
        self.sync();
    }

    pub fn set_requested_visible(&mut self, visible: bool) {
        if self.requested_visible != visible {
            self.requested_visible = visible;
            tracing::debug!(visible, "native video host visibility requested");
        }
        self.sync();
    }

    pub fn set_occluded(&mut self, occluded: bool) {
        self.occluded = occluded;
        self.sync();
    }

    pub fn sync(&mut self) {
        let Some(main_hwnd) = self.main_hwnd else {
            self.hide();
            return;
        };

        // SAFETY: Both HWNDs remain owned by their respective UI objects while
        // this method runs on the Slint event-loop thread.
        let should_show = self.requested_visible
            && !self.occluded
            && unsafe { IsWindowVisible(main_hwnd).as_bool() }
            && !unsafe { IsIconic(main_hwnd).as_bool() };
        if !should_show {
            self.hide();
            return;
        }

        let mut client = RECT::default();
        // SAFETY: The RECT output is valid and main_hwnd is a live window.
        if let Err(error) = unsafe { GetClientRect(main_hwnd, &mut client) } {
            tracing::warn!(%error, "could not read Slint client rectangle");
            self.hide();
            return;
        }
        let width = client.right.saturating_sub(client.left);
        let height = client.bottom.saturating_sub(client.top);
        if width <= 0 || height <= 0 {
            self.hide();
            return;
        }
        let mut origin = POINT {
            x: client.left,
            y: client.top,
        };
        // SAFETY: origin is writable and main_hwnd is valid.
        if !unsafe { ClientToScreen(main_hwnd, &mut origin) }.as_bool() {
            tracing::warn!("could not translate Slint client rectangle to screen coordinates");
            self.hide();
            return;
        }

        // Passing the Slint HWND as the insertion point places this unowned
        // host immediately behind it without activating either window.
        // SAFETY: Coordinates are physical Win32 pixels and both HWNDs are live.
        if let Err(error) = unsafe {
            SetWindowPos(
                self.hwnd,
                Some(main_hwnd),
                origin.x,
                origin.y,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            )
        } {
            tracing::warn!(%error, "could not synchronize native video host geometry");
            self.hide();
            return;
        }
        // SAFETY: Showing without activation preserves keyboard focus on Slint.
        let _ = unsafe { ShowWindow(self.hwnd, SW_SHOWNOACTIVATE) };
    }

    pub fn hide(&self) {
        // SAFETY: hwnd remains valid until Drop and SW_HIDE never activates it.
        let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
    }
}

impl Drop for VideoHostWindow {
    fn drop(&mut self) {
        self.hide();
        // SAFETY: This object uniquely owns the host HWND and destroys it once
        // on the same UI thread that created it.
        if let Err(error) = unsafe { DestroyWindow(self.hwnd) } {
            tracing::warn!(%error, "could not destroy native video host");
        }
    }
}

unsafe extern "system" fn video_host_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            // SAFETY: Windows supplied hwnd for this callback and paint remains
            // live between the paired BeginPaint/EndPaint calls.
            let device = unsafe { BeginPaint(hwnd, &mut paint) };
            let mut client = RECT::default();
            // SAFETY: The outputs and HWND are valid for this paint callback.
            if unsafe { GetClientRect(hwnd, &mut client) }.is_ok() {
                let brush = HBRUSH(unsafe { GetStockObject(BLACK_BRUSH) }.0);
                // SAFETY: device, rectangle, and process-owned brush are valid.
                unsafe { FillRect(device, &client, brush) };
            }
            // SAFETY: Completes the BeginPaint call above.
            let _ = unsafe { EndPaint(hwnd, &paint) };
            LRESULT(0)
        }
        _ => {
            // SAFETY: Unhandled messages retain normal Win32 behavior.
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
    }
}

#[derive(Debug, Error)]
pub enum VideoHostError {
    #[error("Win32 video-host operation failed: {0}")]
    Windows(windows::core::Error),
    #[error("the native video-host HWND is null")]
    NullHandle,
    #[error("the native video-host HWND {0:#x} does not fit libmpv's 32-bit wid contract")]
    HandleOutOfRange(usize),
}

#[cfg(test)]
mod tests {
    use super::VideoHostWindow;

    #[test]
    fn video_host_produces_a_non_zero_mpv_target() {
        let host = VideoHostWindow::create().expect("video host should be created");
        assert_ne!(host.native_target().expect("valid wid").get(), 0);
    }
}
