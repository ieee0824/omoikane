//! Capture owned pointing-device capabilities from a separate X11 connection.
use super::PointerCapabilities;
use std::ffi::CStr;
use std::ptr;
use x11_dl::{xinput2, xlib};

struct DisplayConnection {
    api: xlib::Xlib,
    display: *mut xlib::Display,
}

impl Drop for DisplayConnection {
    fn drop(&mut self) {
        // SAFETY: this connection owns the successfully opened display.
        unsafe {
            (self.api.XCloseDisplay)(self.display);
        }
    }
}

pub(super) fn capture() -> Option<PointerCapabilities> {
    // Xwayland does not describe the compositor's complete input inventory.
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return None;
    }
    let api = xlib::Xlib::open().ok()?;
    let input = xinput2::XInput2::open().ok()?;
    // SAFETY: initialize thread support before opening our private connection.
    if unsafe { (api.XInitThreads)() } == 0 {
        return None;
    }
    let display = unsafe { (api.XOpenDisplay)(ptr::null()) };
    if display.is_null() {
        return None;
    }
    let connection = DisplayConnection { api, display };
    let (mut opcode, mut event, mut error) = (0, 0, 0);
    // SAFETY: the display and output pointers are valid for these calls.
    if unsafe {
        (connection.api.XQueryExtension)(
            display,
            c"XInputExtension".as_ptr(),
            &mut opcode,
            &mut event,
            &mut error,
        )
    } == 0
    {
        return None;
    }
    let (mut major, mut minor) = (2, 2);
    if unsafe { (input.XIQueryVersion)(display, &mut major, &mut minor) } != 0 {
        return None;
    }
    if major < 2 || (major == 2 && minor < 2) {
        return None;
    }
    let mut count = 0;
    let devices = unsafe { (input.XIQueryDevice)(display, xinput2::XIAllDevices, &mut count) };
    if devices.is_null() {
        return None;
    }
    let mut capabilities = PointerCapabilities::default();
    if count >= 0 {
        // SAFETY: XIQueryDevice supplies count initialized records until freed.
        for device in unsafe { std::slice::from_raw_parts(devices, count as usize) } {
            capabilities = capabilities.union(device_capabilities(&connection, device));
        }
    }
    // SAFETY: the returned allocation is released exactly once with its API.
    unsafe {
        (input.XIFreeDeviceInfo)(devices);
    }
    (count >= 0).then_some(capabilities)
}

fn device_capabilities(
    connection: &DisplayConnection,
    device: &xinput2::XIDeviceInfo,
) -> PointerCapabilities {
    if device.enabled == 0 || device._use != xinput2::XISlavePointer {
        return PointerCapabilities::default();
    }
    // XTEST slaves synthesize events rather than describe available hardware.
    if !device.name.is_null()
        && unsafe { CStr::from_ptr(device.name) }
            .to_bytes()
            .windows(5)
            .any(|part| part == b"XTEST")
    {
        return PointerCapabilities::default();
    }
    let mut direct_touch = false;
    let mut pen_pressure = false;
    let mut hover_distance = false;
    if device.num_classes > 0 && !device.classes.is_null() {
        // SAFETY: class pointers belong to the live XIQueryDevice allocation.
        for class in
            unsafe { std::slice::from_raw_parts(device.classes, device.num_classes as usize) }
        {
            if !class.is_null() && unsafe { (**class)._type } == xinput2::XITouchClass {
                direct_touch |= unsafe { (*(*class).cast::<xinput2::XITouchClassInfo>()).mode }
                    == xinput2::XIDirectTouch;
            } else if !class.is_null() && unsafe { (**class)._type } == xinput2::XIValuatorClass {
                // SAFETY: a valuator class has the corresponding native layout.
                let label = unsafe { (*(*class).cast::<xinput2::XIValuatorClassInfo>()).label };
                if let Some(name) = atom_name(connection, label) {
                    pen_pressure |= name == b"Abs Pressure";
                    hover_distance |= name == b"Abs Distance";
                }
            }
        }
    }
    let mut capabilities = classify_pointer(direct_touch);
    if pen_pressure && !direct_touch {
        capabilities.fine_hover = hover_distance;
    }
    capabilities
}

fn atom_name(connection: &DisplayConnection, atom: xlib::Atom) -> Option<Vec<u8>> {
    if atom == 0 {
        return None;
    }
    // SAFETY: atom originates from this display's device inventory.
    let name = unsafe { (connection.api.XGetAtomName)(connection.display, atom) };
    if name.is_null() {
        return None;
    }
    let owned = unsafe { CStr::from_ptr(name) }.to_bytes().to_vec();
    // SAFETY: XGetAtomName allocations must be released with XFree.
    unsafe {
        (connection.api.XFree)(name.cast());
    }
    Some(owned)
}

fn classify_pointer(direct_touch: bool) -> PointerCapabilities {
    PointerCapabilities {
        fine: !direct_touch,
        fine_hover: !direct_touch,
        coarse: direct_touch,
        coarse_hover: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn direct_touch_is_coarse_and_indirect_pointer_is_fine() {
        assert_eq!(
            classify_pointer(true),
            PointerCapabilities {
                coarse: true,
                ..Default::default()
            }
        );
        assert_eq!(
            classify_pointer(false),
            PointerCapabilities {
                fine: true,
                fine_hover: true,
                ..Default::default()
            }
        );
    }
    #[test]
    fn native_x11_inventory_when_explicitly_requested() {
        if std::env::var_os("OMOIKANE_TEST_X11_INPUT").is_none() {
            return;
        }
        let inventory = capture().expect("XInput2 inventory must be available");
        assert!(
            inventory.fine && inventory.fine_hover,
            "Xvfb mouse: {inventory:?}"
        );
    }
}
