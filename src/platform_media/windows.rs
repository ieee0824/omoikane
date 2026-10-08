//! Read-only Win32 accessibility settings, converted into owned values.
use super::{PlatformMediaPreferences, windows_preferences};
use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SPI_GETCLIENTAREAANIMATION, SPI_GETHIGHCONTRAST, SystemParametersInfoW,
};

pub(super) fn capture() -> PlatformMediaPreferences {
    let mut animations: i32 = 0;
    // SAFETY: this read action writes one Win32 BOOL into a live i32. No
    // setting is modified; uiParam and update flags are zero as documented.
    let animations = unsafe {
        (SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            std::ptr::from_mut(&mut animations).cast(),
            0,
        ) != 0)
            .then_some(animations != 0)
    };
    let size = std::mem::size_of::<HIGHCONTRASTW>() as u32;
    let mut contrast = HIGHCONTRASTW {
        cbSize: size,
        dwFlags: 0,
        lpszDefaultScheme: std::ptr::null_mut(),
    };
    // SAFETY: HIGHCONTRASTW has its correct cbSize and remains live for the
    // call. We read only the flags; the system-owned scheme pointer is unused.
    let high_contrast = unsafe {
        (SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            size,
            std::ptr::from_mut(&mut contrast).cast(),
            0,
        ) != 0)
            .then_some(contrast.dwFlags & HCF_HIGHCONTRASTON != 0)
    };
    let mut preferences = windows_preferences(animations, high_contrast);
    if high_contrast == Some(true) {
        preferences.forced_color_palette = capture_palette();
    }
    preferences.pointers = capture_pointers();
    preferences
}

/// Reads system-owned colors without allocating or retaining native resources.
fn capture_palette() -> Option<crate::css::ForcedColorPalette> {
    use windows_sys::Win32::Graphics::Gdi::{
        COLOR_3DFACE, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT,
        COLOR_HOTLIGHT, COLOR_WINDOW, COLOR_WINDOWTEXT, GetSysColor, GetSysColorBrush,
        SYS_COLOR_INDEX,
    };
    fn read(index: SYS_COLOR_INDEX) -> Option<[u8; 3]> {
        // SAFETY: both calls take only a documented color index. The returned
        // brush belongs to the system; it is checked but never used or deleted.
        // A zero COLORREF is valid black, so availability uses the brush instead.
        unsafe {
            if GetSysColorBrush(index).is_null() {
                return None;
            }
            let color = GetSysColor(index);
            Some([color as u8, (color >> 8) as u8, (color >> 16) as u8])
        }
    }
    let canvas = read(COLOR_WINDOW)?;
    let canvas_text = read(COLOR_WINDOWTEXT)?;
    let link_text = read(COLOR_HOTLIGHT)?;
    Some(crate::css::ForcedColorPalette {
        canvas,
        canvas_text,
        link_text,
        // Win32 exposes one accessible hyperlink color, shared by both states.
        visited_text: link_text,
        button_face: read(COLOR_3DFACE)?,
        button_text: read(COLOR_BTNTEXT)?,
        field: canvas,
        field_text: canvas_text,
        gray_text: read(COLOR_GRAYTEXT)?,
        highlight: read(COLOR_HIGHLIGHT)?,
        highlight_text: read(COLOR_HIGHLIGHTTEXT)?,
    })
}

/// Returns the local mouse inventory; remote sessions require observed input.
/// Raw Input excludes RDP devices, so absence there cannot establish no pointer.
fn capture_mouse_presence() -> Option<bool> {
    use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, GetLastError};
    use windows_sys::Win32::UI::Input::{GetRawInputDeviceList, RAWINPUTDEVICELIST, RIM_TYPEMOUSE};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_REMOTESESSION};

    // SAFETY: GetSystemMetrics takes no pointers and does not change settings.
    if unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0 {
        return None;
    }
    let size = std::mem::size_of::<RAWINPUTDEVICELIST>() as u32;
    // A device can appear between the sizing and filling calls. Bound retries
    // so rapidly changing hardware cannot keep the preference monitor busy.
    for _ in 0..3 {
        let mut count = 0;
        // SAFETY: a null buffer requests only the count into a live u32.
        if unsafe { GetRawInputDeviceList(std::ptr::null_mut(), &mut count, size) } == u32::MAX {
            return None;
        }
        if count == 0 {
            return Some(false);
        }
        let mut devices = Vec::new();
        devices.try_reserve_exact(count as usize).ok()?;
        devices.resize(count as usize, RAWINPUTDEVICELIST::default());
        // SAFETY: all count entries are initialized and writable. Handles in
        // these records belong to the system and are never closed here.
        let written = unsafe { GetRawInputDeviceList(devices.as_mut_ptr(), &mut count, size) };
        if written == u32::MAX {
            // SAFETY: inspect the immediately preceding thread-local error.
            if unsafe { GetLastError() } == ERROR_INSUFFICIENT_BUFFER {
                continue;
            }
            return None;
        }
        let written = usize::try_from(written).ok()?;
        if written > devices.len() {
            return None;
        }
        return Some(
            devices[..written]
                .iter()
                .any(|device| device.dwType == RIM_TYPEMOUSE),
        );
    }
    None
}

/// Combines local Raw Input mice with Windows' digitizer inventory.
/// An incomplete inventory stays unavailable rather than reporting no devices.
pub(super) fn capture_pointers() -> Option<super::PointerCapabilities> {
    let mouse = capture_mouse_presence()?;
    let mut capabilities = super::PointerCapabilities {
        fine: mouse,
        fine_hover: mouse,
        ..super::PointerCapabilities::default()
    };
    for device in pointer_devices()? {
        capabilities = capabilities.union(super::input::windows_pointer_type(
            device.pointerDeviceType,
        )?);
    }
    Some(capabilities)
}

fn pointer_devices() -> Option<Vec<windows_sys::Win32::UI::Controls::POINTER_DEVICE_INFO>> {
    use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, GetLastError};
    use windows_sys::Win32::UI::Controls::POINTER_DEVICE_INFO;
    use windows_sys::Win32::UI::Input::Pointer::GetPointerDevices;
    for _ in 0..3 {
        let mut count = 0;
        // SAFETY: the null array requests only the number into a live u32.
        if unsafe { GetPointerDevices(&mut count, std::ptr::null_mut()) } == 0 {
            return None;
        }
        if count == 0 {
            return Some(Vec::new());
        }
        let mut devices = Vec::new();
        devices.try_reserve_exact(count as usize).ok()?;
        devices.resize(count as usize, POINTER_DEVICE_INFO::default());
        // SAFETY: count fully initialized entries are live and writable. All
        // device and monitor handles are system-owned; we do not close them.
        if unsafe { GetPointerDevices(&mut count, devices.as_mut_ptr()) } == 0 {
            // SAFETY: inspect the immediately preceding thread-local error.
            if unsafe { GetLastError() } == ERROR_INSUFFICIENT_BUFFER {
                continue;
            }
            return None;
        }
        let count = usize::try_from(count).ok()?;
        if count > devices.len() {
            return None;
        }
        devices.truncate(count);
        return Some(devices);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_system_palette_can_be_read_independently_of_high_contrast_capture() {
        let _palette = capture_palette().expect("Win32 system colors must be available");
        // Resolving the captured values must not depend on high-contrast mode
        // being enabled on the runner; a zero COLORREF remains valid black.
    }
}
