//! Native cursor-grab policy for the GUI's pointer-lock host.

use winit::window::{CursorGrabMode, Window};

/// Whether locked button/wheel input comes from raw device events.
pub(super) fn uses_raw_buttons(window: &Window) -> bool {
    #[cfg(target_os = "linux")]
    {
        use winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
        window
            .display_handle()
            .is_ok_and(|handle| matches!(handle.as_raw(), RawDisplayHandle::Xlib(_)))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = window;
        false
    }
}

/// Acquires or releases a grab on the event-loop thread.
pub(super) fn set_grab(window: &Window, grab: bool) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    if let Some(result) = x11::set_grab(window, grab) {
        return result;
    }

    if grab {
        window
            .set_cursor_grab(CursorGrabMode::Locked)
            .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined))
    } else {
        window.set_cursor_grab(CursorGrabMode::None)
    }
    .map_err(|error| error.to_string())
}

#[cfg(target_os = "linux")]
mod x11 {
    use std::sync::OnceLock;
    use winit::raw_window_handle::{
        HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
    };
    use winit::window::Window;
    use x11_dl::xlib;

    pub(super) fn set_grab(window: &Window, grab: bool) -> Option<Result<(), String>> {
        let RawDisplayHandle::Xlib(display) = window.display_handle().ok()?.as_raw() else {
            return None;
        };
        let RawWindowHandle::Xlib(handle) = window.window_handle().ok()?.as_raw() else {
            return None;
        };
        Some((|| {
            static XLIB: OnceLock<Result<xlib::Xlib, String>> = OnceLock::new();
            let xlib = XLIB
                .get_or_init(|| xlib::Xlib::open().map_err(|error| error.to_string()))
                .as_ref()
                .map_err(Clone::clone)?;
            let display = display.display.ok_or("X11 display is unavailable")?;
            // SAFETY: Winit owns these handles for the lifetime of `window`.
            // This helper only runs on the window's event-loop thread. Xlib
            // shares Winit's existing connection and we retain no raw handles.
            unsafe {
                let display = display.as_ptr().cast();
                if grab {
                    // owner_events=true duplicates XI_RawMotion under a core
                    // grab. An exclusive grab with no core mouse-event mask
                    // uses only Winit's XI2 raw input, including buttons/wheel.
                    // Never deduplicate deltas by value or timestamp: consecutive
                    // physical events may legitimately have identical values.
                    let status = (xlib.XGrabPointer)(
                        display,
                        handle.window,
                        xlib::False,
                        0,
                        xlib::GrabModeAsync,
                        xlib::GrabModeAsync,
                        handle.window,
                        0,
                        xlib::CurrentTime,
                    );
                    if status != xlib::GrabSuccess {
                        return Err(format!("X11 pointer grab was rejected ({status})"));
                    }
                } else {
                    (xlib.XUngrabPointer)(display, xlib::CurrentTime);
                }
                (xlib.XFlush)(display);
            }
            Ok(())
        })())
    }
}
