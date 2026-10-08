//! Read-only IOKit inventory; each Core Foundation Create/Copy result is owned.
//! IOKit declarations follow Apple IOKitUser hid.subproj IOHIDManager.h,
//! IOHIDDevice.h, and IOHIDElement.h; Boolean is Core Foundation unsigned char.
use super::PointerCapabilities;
use std::ffi::c_void;
use std::ptr::NonNull;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDManagerCreate(allocator: *const c_void, options: u32) -> *const c_void;
    fn IOHIDManagerSetDeviceMatching(manager: *const c_void, matching: *const c_void);
    fn IOHIDManagerCopyDevices(manager: *const c_void) -> *const c_void;
    fn IOHIDDeviceConformsTo(device: *const c_void, page: u32, usage: u32) -> u8;
    fn IOHIDDeviceCopyMatchingElements(
        device: *const c_void,
        matching: *const c_void,
        options: u32,
    ) -> *const c_void;
    fn IOHIDElementGetUsagePage(element: *const c_void) -> u32;
    fn IOHIDElementGetUsage(element: *const c_void) -> u32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: *const c_void);
    fn CFSetGetCount(set: *const c_void) -> isize;
    fn CFSetGetValues(set: *const c_void, values: *mut *const c_void);
    fn CFArrayGetCount(array: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
}

/// Owns one +1 Create/Copy result, without exposing retained native references.
struct OwnedCf(NonNull<c_void>);
impl OwnedCf {
    /// The caller supplies a +1 Core Foundation Create/Copy result.
    unsafe fn from_owned(value: *const c_void) -> Option<Self> {
        NonNull::new(value.cast_mut()).map(Self)
    }
    fn as_ptr(&self) -> *const c_void {
        self.0.as_ptr()
    }
}
impl Drop for OwnedCf {
    fn drop(&mut self) {
        // SAFETY: this owner releases exactly its one retained Create/Copy result.
        unsafe { CFRelease(self.as_ptr()) };
    }
}

pub(super) fn capture() -> Option<PointerCapabilities> {
    if !objc2_foundation::NSThread::isMainThread_class() {
        return None;
    }
    // SAFETY: default allocator and zero options create a retained manager.
    let manager = unsafe { OwnedCf::from_owned(IOHIDManagerCreate(std::ptr::null(), 0)) }?;
    // SAFETY: manager is live. Null matching enumerates all devices without
    // opening them, seizing input, or requesting access to input reports.
    unsafe { IOHIDManagerSetDeviceMatching(manager.as_ptr(), std::ptr::null()) };
    // SAFETY: Copy returns an owned immutable set retaining its device objects.
    let devices = unsafe { OwnedCf::from_owned(IOHIDManagerCopyDevices(manager.as_ptr())) }?;
    // SAFETY: devices is the live CFSet returned by CopyDevices.
    let count = usize::try_from(unsafe { CFSetGetCount(devices.as_ptr()) }).ok()?;
    let mut pointers = Vec::new();
    pointers.try_reserve_exact(count).ok()?;
    pointers.resize(count, std::ptr::null());
    if count != 0 {
        // SAFETY: count initialized pointer slots match this immutable set.
        unsafe { CFSetGetValues(devices.as_ptr(), pointers.as_mut_ptr()) };
    }
    let mut result = PointerCapabilities::default();
    for device in pointers {
        if device.is_null() {
            return None;
        }
        // SAFETY: every device stays retained by devices for these local queries.
        let mouse = unsafe { IOHIDDeviceConformsTo(device, 0x01, 0x02) != 0 };
        let touchpad = unsafe { IOHIDDeviceConformsTo(device, 0x0d, 0x05) != 0 };
        let touchscreen = unsafe { IOHIDDeviceConformsTo(device, 0x0d, 0x04) != 0 };
        let pen = unsafe {
            IOHIDDeviceConformsTo(device, 0x0d, 0x01) != 0
                || IOHIDDeviceConformsTo(device, 0x0d, 0x02) != 0
                || IOHIDDeviceConformsTo(device, 0x0d, 0x03) != 0
        };
        let pen_hover = if pen {
            supports_in_range(device)?
        } else {
            false
        };
        result = result.union(super::input::hid_pointer_capabilities(
            mouse,
            touchpad,
            touchscreen,
            pen,
            pen_hover,
        ));
    }
    Some(result)
}

/// Borrows the device only during this call; copied elements have local ownership.
fn supports_in_range(device: *const c_void) -> Option<bool> {
    // SAFETY: caller keeps this valid IOHIDDevice retained. Null dictionary
    // returns all elements with +1 ownership; zero is the documented options.
    let elements = unsafe {
        OwnedCf::from_owned(IOHIDDeviceCopyMatchingElements(device, std::ptr::null(), 0))
    }?;
    // SAFETY: elements is the live CFArray returned by CopyMatchingElements.
    let count = unsafe { CFArrayGetCount(elements.as_ptr()) };
    if count < 0 {
        return None;
    }
    for index in 0..count {
        // SAFETY: index is in bounds and array keeps the IOHIDElement live.
        let element = unsafe { CFArrayGetValueAtIndex(elements.as_ptr(), index) };
        if element.is_null() {
            return None;
        }
        // HID digitizer In Range reports pen proximity independently of contact.
        // SAFETY: this is a valid element retained by the copied array.
        if unsafe {
            IOHIDElementGetUsagePage(element) == 0x0d && IOHIDElementGetUsage(element) == 0x32
        } {
            return Some(true);
        }
    }
    Some(false)
}
