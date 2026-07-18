#[cfg(target_os = "windows")]
use slint::winit_030::winit;

#[cfg(target_os = "windows")]
pub fn apply(window: &winit::window::Window) -> bool {
    apply_windows_caption(window)
}

#[cfg(not(target_os = "windows"))]
pub fn apply(_window: &slint::winit_030::winit::window::Window) -> bool {
    true
}

#[cfg(target_os = "windows")]
fn apply_windows_caption(window: &winit::window::Window) -> bool {
    use std::{ffi::c_void, mem::size_of};

    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DwmSetWindowAttribute,
    };
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(window_handle) = window.window_handle() else {
        tracing::warn!("failed to obtain the native window handle for caption styling");
        return false;
    };
    let RawWindowHandle::Win32(window_handle) = window_handle.as_raw() else {
        return false;
    };

    const CAPTION_COLOR: u32 = colorref(0x15, 0x12, 0x2b);
    const TEXT_COLOR: u32 = colorref(0xff, 0xff, 0xff);
    let hwnd = window_handle.hwnd.get() as *mut c_void;

    // SAFETY: `hwnd` comes from the live winit window and both attributes point
    // to correctly sized COLORREF values for the duration of each call.
    let (caption_result, text_result) = unsafe {
        (
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_CAPTION_COLOR as u32,
                &CAPTION_COLOR as *const u32 as *const c_void,
                size_of::<u32>() as u32,
            ),
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_TEXT_COLOR as u32,
                &TEXT_COLOR as *const u32 as *const c_void,
                size_of::<u32>() as u32,
            ),
        )
    };

    if caption_result < 0 || text_result < 0 {
        tracing::warn!(
            caption_result,
            text_result,
            "Windows rejected one or more title-bar color attributes"
        );
        false
    } else {
        tracing::info!("official Stremio title-bar colors applied");
        true
    }
}

#[cfg(target_os = "windows")]
const fn colorref(red: u32, green: u32, blue: u32) -> u32 {
    red | (green << 8) | (blue << 16)
}
