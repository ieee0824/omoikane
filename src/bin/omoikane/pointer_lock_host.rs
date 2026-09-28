//! Native cursor-grab policy for the GUI's pointer-lock host.

use winit::window::{CursorGrabMode, Window};

/// Failures while acquiring or releasing the native pointer grab.
#[derive(Debug)]
pub(super) enum PointerGrabError {
    /// Winit could not apply the requested cursor grab.
    Cursor(winit::error::ExternalError),
    /// The X11 library could not be loaded.
    #[cfg(target_os = "linux")]
    XlibOpen(x11_dl::error::OpenError),
    /// Winit did not provide a usable X11 display handle.
    #[cfg(target_os = "linux")]
    DisplayUnavailable,
    /// The X server rejected the pointer grab with this status.
    #[cfg(target_os = "linux")]
    X11Rejected(i32),
}

impl std::fmt::Display for PointerGrabError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cursor(error) => write!(f, "{error}"),
            #[cfg(target_os = "linux")]
            Self::XlibOpen(error) => write!(f, "{error}"),
            #[cfg(target_os = "linux")]
            Self::DisplayUnavailable => write!(f, "X11 display is unavailable"),
            #[cfg(target_os = "linux")]
            Self::X11Rejected(status) => write!(f, "X11 pointer grab was rejected ({status})"),
        }
    }
}

impl std::error::Error for PointerGrabError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Cursor(error) => Some(error),
            #[cfg(target_os = "linux")]
            Self::XlibOpen(error) => Some(error),
            #[cfg(target_os = "linux")]
            _ => None,
        }
    }
}

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
pub(super) fn set_grab(window: &Window, grab: bool) -> Result<(), PointerGrabError> {
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
    .map_err(PointerGrabError::Cursor)
}

#[cfg(target_os = "linux")]
mod x11 {
    use super::PointerGrabError;
    use std::sync::OnceLock;
    use winit::raw_window_handle::{
        HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
    };
    use winit::window::Window;
    use x11_dl::xlib;

    pub(super) fn set_grab(window: &Window, grab: bool) -> Option<Result<(), PointerGrabError>> {
        let RawDisplayHandle::Xlib(display) = window.display_handle().ok()?.as_raw() else {
            return None;
        };
        let RawWindowHandle::Xlib(handle) = window.window_handle().ok()?.as_raw() else {
            return None;
        };
        Some((|| {
            static XLIB: OnceLock<Result<xlib::Xlib, x11_dl::error::OpenError>> = OnceLock::new();
            let xlib = XLIB
                .get_or_init(xlib::Xlib::open)
                .as_ref()
                .map_err(|error| PointerGrabError::XlibOpen(error.clone()))?;
            let display = display
                .display
                .ok_or(PointerGrabError::DisplayUnavailable)?;
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
                        return Err(PointerGrabError::X11Rejected(status));
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

#[cfg(test)]
mod tests {
    use super::PointerGrabError;

    #[test]
    fn pointer_grab_errors_keep_their_visible_messages_and_sources() {
        let ignored = PointerGrabError::Cursor(winit::error::ExternalError::Ignored);
        assert_eq!(ignored.to_string(), "Operation was ignored");
        assert!(std::error::Error::source(&ignored).is_some());

        #[cfg(target_os = "linux")]
        {
            assert_eq!(
                PointerGrabError::DisplayUnavailable.to_string(),
                "X11 display is unavailable"
            );
            assert_eq!(
                PointerGrabError::X11Rejected(1).to_string(),
                "X11 pointer grab was rejected (1)"
            );
        }
    }
}
