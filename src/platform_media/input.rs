//! Owned pointer capabilities and host input-device observations.
use crate::css::MediaEnvironment;
use std::collections::HashMap;
use std::hash::Hash;

/// Capabilities of the pointing devices available to the presentation host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PointerCapabilities {
    /// A device can point accurately.
    pub fine: bool,
    /// A device points with limited accuracy, such as a touchscreen.
    pub coarse: bool,
    /// A fine device can hover without activation.
    pub fine_hover: bool,
    /// A coarse device can hover without activation.
    pub coarse_hover: bool,
}

impl PointerCapabilities {
    /// Combines available device capabilities without querying the platform.
    pub fn union(self, other: Self) -> Self {
        Self {
            fine: self.fine || other.fine,
            coarse: self.coarse || other.coarse,
            fine_hover: self.fine_hover || other.fine_hover,
            coarse_hover: self.coarse_hover || other.coarse_hover,
        }
    }

    /// Sets pointer and hover features, preferring a fine primary device.
    /// Hover is independent of accuracy; a pen need not support hovering.
    pub fn apply_to(self, environment: &mut MediaEnvironment) {
        let fine = self.fine || self.fine_hover;
        let coarse = self.coarse || self.coarse_hover;
        environment.set_available_pointers(fine, coarse);
        environment.set_feature(
            "pointer",
            if fine {
                "fine"
            } else if coarse {
                "coarse"
            } else {
                "none"
            },
        );
        let primary_hover = if fine {
            self.fine_hover
        } else {
            self.coarse_hover
        };
        environment.set_feature("hover", if primary_hover { "hover" } else { "none" });
        environment.set_feature(
            "any-hover",
            if self.fine_hover || self.coarse_hover {
                "hover"
            } else {
                "none"
            },
        );
    }
}

/// Converts Win32 pointer-device categories without querying host state.
/// Windows defines its pen category as an active stylus supporting hover:
/// https://learn.microsoft.com/en-us/windows-hardware/design/component-guidelines/windows-pointer-device-overview
#[cfg(any(all(target_os = "windows", feature = "gui"), test))]
pub(super) fn windows_pointer_type(kind: i32) -> Option<PointerCapabilities> {
    match kind {
        // POINTER_DEVICE_TYPE_INTEGRATED_PEN, EXTERNAL_PEN, TOUCH_PAD.
        1 | 2 | 4 => Some(PointerCapabilities {
            fine: true,
            fine_hover: true,
            ..PointerCapabilities::default()
        }),
        // POINTER_DEVICE_TYPE_TOUCH has contact, rather than pen, semantics.
        3 => Some(PointerCapabilities {
            coarse: true,
            ..PointerCapabilities::default()
        }),
        _ => None,
    }
}

/// Maps independent HID pointing collections to an owned capability union.
#[cfg(any(all(target_os = "macos", feature = "gui"), test))]
pub(super) fn hid_pointer_capabilities(
    mouse: bool,
    touchpad: bool,
    touchscreen: bool,
    pen: bool,
    pen_hover: bool,
) -> PointerCapabilities {
    PointerCapabilities {
        fine: mouse || touchpad || pen,
        coarse: touchscreen,
        fine_hover: mouse || touchpad || (pen && pen_hover),
        coarse_hover: false,
    }
}

/// Tracks platform snapshots and observed devices using owned device IDs.
/// A native snapshot is authoritative; observations cover unavailable capture.
pub struct PointerDeviceTracker<Id> {
    native: Option<PointerCapabilities>,
    observed: HashMap<Id, PointerCapabilities>,
    observed_any: bool,
}

impl<Id: Eq + Hash> PointerDeviceTracker<Id> {
    /// Creates a tracker with an optional native inventory.
    pub fn new(native: Option<PointerCapabilities>) -> Self {
        Self {
            native,
            observed: HashMap::new(),
            observed_any: false,
        }
    }

    /// Replaces the native inventory; `None` leaves observations authoritative.
    pub fn set_native(&mut self, native: Option<PointerCapabilities>) {
        self.native = native;
    }

    /// Records capabilities actually observed for a device.
    pub fn observe(&mut self, id: Id, capabilities: PointerCapabilities) -> bool {
        self.observed_any = true;
        let previous = self.observed.get(&id).copied().unwrap_or_default();
        let next = previous.union(capabilities);
        self.observed.insert(id, next);
        previous != next
    }

    /// Removes a device without assuming that touch contact end means removal.
    pub fn remove(&mut self, id: &Id) -> bool {
        self.observed.remove(id).is_some()
    }

    /// Returns native capabilities, observations, or an explicit host fallback.
    /// After observed devices are removed, the result is empty rather than fallback.
    pub fn snapshot(&self, fallback: PointerCapabilities) -> PointerCapabilities {
        self.native.unwrap_or_else(|| {
            if !self.observed_any {
                return fallback;
            }
            self.observed
                .values()
                .copied()
                .fold(PointerCapabilities::default(), PointerCapabilities::union)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const MOUSE: PointerCapabilities = PointerCapabilities {
        fine: true,
        fine_hover: true,
        coarse: false,
        coarse_hover: false,
    };
    const TOUCH: PointerCapabilities = PointerCapabilities {
        fine: false,
        fine_hover: false,
        coarse: true,
        coarse_hover: false,
    };

    #[test]
    fn windows_native_device_categories_preserve_mixed_inventory() {
        let pen = windows_pointer_type(1).unwrap();
        assert_eq!(pen, MOUSE);
        assert_eq!(windows_pointer_type(2), Some(pen));
        assert_eq!(windows_pointer_type(4), Some(MOUSE));
        assert_eq!(windows_pointer_type(3), Some(TOUCH));
        let mut environment = MediaEnvironment::default();
        pen.union(TOUCH).apply_to(&mut environment);
        for text in ["(pointer: fine)", "(hover: hover)", "(any-pointer: coarse)"] {
            let queries = crate::css::parse_media_query_list(text).unwrap();
            assert!(
                crate::css::evaluate_media_query_with_environment(
                    &queries[0],
                    800.0,
                    600.0,
                    &environment
                ),
                "{text}"
            );
        }
    }

    #[test]
    fn unknown_windows_pointer_category_is_unavailable() {
        assert_eq!(windows_pointer_type(0), None);
        assert_eq!(windows_pointer_type(-1), None);
        assert_eq!(windows_pointer_type(5), None);
    }

    #[test]
    fn hid_pen_hover_is_independent_and_composite_devices_keep_touch() {
        let pen = hid_pointer_capabilities(false, false, false, true, false);
        assert!(pen.fine);
        assert!(!pen.fine_hover);
        assert_eq!(
            hid_pointer_capabilities(false, false, true, true, true),
            MOUSE.union(TOUCH)
        );
        assert_eq!(
            hid_pointer_capabilities(false, true, false, false, false),
            MOUSE
        );
        assert_eq!(
            hid_pointer_capabilities(false, false, false, false, true),
            PointerCapabilities::default()
        );
    }

    #[test]
    fn input_observations_and_removal_replace_fallback() {
        let mut tracker = PointerDeviceTracker::new(None);
        assert_eq!(tracker.snapshot(MOUSE), MOUSE);
        assert!(tracker.observe(1, TOUCH));
        assert_eq!(tracker.snapshot(MOUSE), TOUCH);
        assert!(!tracker.observe(1, TOUCH));
        assert!(tracker.observe(2, MOUSE));
        assert_eq!(tracker.snapshot(MOUSE), TOUCH.union(MOUSE));
        assert!(tracker.remove(&2));
        assert_eq!(tracker.snapshot(MOUSE), TOUCH);
        assert!(tracker.remove(&1));
        assert_eq!(tracker.snapshot(MOUSE), PointerCapabilities::default());
    }

    #[test]
    fn native_inventory_is_authoritative_and_can_be_replaced() {
        let mut tracker = PointerDeviceTracker::new(Some(TOUCH));
        tracker.observe(1, MOUSE);
        assert_eq!(tracker.snapshot(MOUSE), TOUCH);
        tracker.set_native(Some(PointerCapabilities::default()));
        assert_eq!(tracker.snapshot(MOUSE), PointerCapabilities::default());
        tracker.set_native(None);
        assert_eq!(tracker.snapshot(MOUSE), MOUSE);
    }

    #[test]
    fn fine_nonhovering_pen_and_hovering_coarse_device_remain_independent() {
        let mut environment = MediaEnvironment::default();
        PointerCapabilities {
            fine: true,
            coarse: true,
            fine_hover: false,
            coarse_hover: true,
        }
        .apply_to(&mut environment);
        let query = |text: &str| {
            let queries = crate::css::parse_media_query_list(text).unwrap();
            crate::css::evaluate_media_query_with_environment(
                &queries[0],
                800.0,
                600.0,
                &environment,
            )
        };
        assert!(query("(pointer: fine)"));
        assert!(query("(hover: none)"));
        assert!(query("(any-hover: hover)"));
        assert!(query("(any-pointer: fine) and (any-pointer: coarse)"));
    }
}
