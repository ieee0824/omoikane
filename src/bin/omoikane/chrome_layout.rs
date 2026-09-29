//! Window regions for the GUI toolbar and page, and their coordinate mapping.
//!
//! The layout is a pure function of the window's physical size, its
//! [`DeviceScale`] and whether the toolbar is shown. Painting, the page
//! viewport and pointer input all read the same [`ChromeLayout`], so the
//! toolbar offset is applied exactly once. Pointer Lock's relative motion is
//! a delta, not a position, and does not pass through this mapping.

// BrowserApp starts painting and routing input through this layout in
// #1137–#1138.
#![cfg_attr(not(test), allow(dead_code))]

use super::device_scale::DeviceScale;

/// Toolbar height in logical pixels (CSS pixels of the browser UI).
pub(super) const TOOLBAR_HEIGHT: f64 = 40.0;

/// An axis-aligned rectangle in physical window pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PhysicalRect {
    pub(super) x: u32,
    pub(super) y: u32,
    pub(super) width: u32,
    pub(super) height: u32,
}

impl PhysicalRect {
    fn contains(self, x: f64, y: f64) -> bool {
        x >= f64::from(self.x)
            && y >= f64::from(self.y)
            && x < f64::from(self.x) + f64::from(self.width)
            && y < f64::from(self.y) + f64::from(self.height)
    }
}

/// The window region under a physical position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum WindowRegion {
    /// Inside the toolbar, in logical pixels from the toolbar's top-left.
    Toolbar { x: f64, y: f64 },
    /// Inside the page, in CSS pixels from the page's top-left.
    Page { x: f64, y: f64 },
    /// Outside the window, which happens while a button is held.
    Outside,
}

/// Toolbar and page areas of one window size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ChromeLayout {
    scale: DeviceScale,
    toolbar: PhysicalRect,
    page: PhysicalRect,
}

impl ChromeLayout {
    /// Splits a `physical_width` x `physical_height` window into a toolbar
    /// strip at the top and the page below it.
    ///
    /// The toolbar is [`TOOLBAR_HEIGHT`] logical pixels rounded to whole
    /// physical pixels, and is clipped when the window is shorter. With
    /// `show_toolbar` false (for example while the page is fullscreen) the
    /// page covers the whole window.
    pub(super) fn new(
        physical_width: u32,
        physical_height: u32,
        scale: DeviceScale,
        show_toolbar: bool,
    ) -> Self {
        let toolbar_height = if show_toolbar {
            ((TOOLBAR_HEIGHT * scale.factor()).round() as u32).min(physical_height)
        } else {
            0
        };
        Self {
            scale,
            toolbar: PhysicalRect {
                x: 0,
                y: 0,
                width: physical_width,
                height: toolbar_height,
            },
            page: PhysicalRect {
                x: 0,
                y: toolbar_height,
                width: physical_width,
                height: physical_height - toolbar_height,
            },
        }
    }

    /// Returns the toolbar area; its height is zero when hidden.
    pub(super) fn toolbar(&self) -> PhysicalRect {
        self.toolbar
    }

    /// Returns the area the page frame is drawn into.
    pub(super) fn page(&self) -> PhysicalRect {
        self.page
    }

    /// Returns the page viewport in CSS pixels. A zero dimension means there
    /// is no page area to paint.
    pub(super) fn page_viewport(&self) -> (u32, u32) {
        self.scale.page_viewport(self.page.width, self.page.height)
    }

    /// Maps a physical window position to page CSS pixels, even outside the
    /// page area, so input captured by the page keeps consistent coordinates.
    pub(super) fn page_point(&self, physical_x: f64, physical_y: f64) -> (f64, f64) {
        let factor = self.scale.factor();
        (
            (physical_x - f64::from(self.page.x)) / factor,
            (physical_y - f64::from(self.page.y)) / factor,
        )
    }

    /// Returns the region under a physical window position.
    pub(super) fn region_at(&self, physical_x: f64, physical_y: f64) -> WindowRegion {
        let factor = self.scale.factor();
        if self.toolbar.contains(physical_x, physical_y) {
            WindowRegion::Toolbar {
                x: (physical_x - f64::from(self.toolbar.x)) / factor,
                y: (physical_y - f64::from(self.toolbar.y)) / factor,
            }
        } else if self.page.contains(physical_x, physical_y) {
            let (x, y) = self.page_point(physical_x, physical_y);
            WindowRegion::Page { x, y }
        } else {
            WindowRegion::Outside
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(width: u32, height: u32, factor: f64) -> ChromeLayout {
        ChromeLayout::new(width, height, DeviceScale::new(factor), true)
    }

    #[test]
    fn toolbar_takes_its_logical_height_and_the_page_gets_the_rest() {
        let at_1x = layout(1280, 720, 1.0);
        assert_eq!(
            at_1x.toolbar(),
            PhysicalRect {
                x: 0,
                y: 0,
                width: 1280,
                height: 40
            }
        );
        assert_eq!(
            at_1x.page(),
            PhysicalRect {
                x: 0,
                y: 40,
                width: 1280,
                height: 680
            }
        );
        assert_eq!(at_1x.page_viewport(), (1280, 680));

        let at_2x = layout(2560, 1440, 2.0);
        assert_eq!(at_2x.toolbar().height, 80);
        assert_eq!(at_2x.page().height, 1360);
        assert_eq!(at_2x.page_viewport(), (1280, 680));
    }

    #[test]
    fn fractional_scales_round_the_toolbar_to_whole_physical_pixels() {
        for (factor, toolbar) in [(1.25, 50), (1.5, 60), (1.75, 70), (2.2, 88), (1.1, 44)] {
            let layout = layout(1001, 800, factor);
            assert_eq!(layout.toolbar().height, toolbar, "{factor}");
            assert_eq!(layout.page().y, toolbar, "{factor}");
            assert_eq!(layout.page().height, 800 - toolbar, "{factor}");
            assert_eq!(
                layout.page_viewport(),
                DeviceScale::new(factor).page_viewport(1001, 800 - toolbar),
                "{factor}"
            );
        }
    }

    #[test]
    fn hidden_toolbar_gives_the_page_the_whole_window() {
        let layout = ChromeLayout::new(2560, 1440, DeviceScale::new(2.0), false);
        assert_eq!(layout.toolbar().height, 0);
        assert_eq!(
            layout.page(),
            PhysicalRect {
                x: 0,
                y: 0,
                width: 2560,
                height: 1440
            }
        );
        assert_eq!(layout.page_viewport(), (1280, 720));
        assert_eq!(
            layout.region_at(0.0, 0.0),
            WindowRegion::Page { x: 0.0, y: 0.0 }
        );
    }

    #[test]
    fn short_and_empty_windows_never_produce_negative_areas() {
        let short = layout(300, 30, 1.0);
        assert_eq!(short.toolbar().height, 30);
        assert_eq!(short.page().height, 0);
        assert_eq!(short.page_viewport(), (300, 0));
        assert_eq!(
            short.region_at(10.0, 29.0),
            WindowRegion::Toolbar { x: 10.0, y: 29.0 }
        );
        assert_eq!(short.region_at(10.0, 30.0), WindowRegion::Outside);

        let empty = layout(0, 0, 2.0);
        assert_eq!(empty.toolbar(), PhysicalRect::default());
        assert_eq!(empty.page(), PhysicalRect::default());
        assert_eq!(empty.page_viewport(), (0, 0));
        assert_eq!(empty.region_at(0.0, 0.0), WindowRegion::Outside);
    }

    #[test]
    fn toolbar_offset_is_applied_once_at_the_region_boundary() {
        let layout = layout(2560, 1440, 2.0);
        assert_eq!(
            layout.region_at(0.0, 79.0),
            WindowRegion::Toolbar { x: 0.0, y: 39.5 }
        );
        // The first page row is CSS y = 0, not y = 40 or y = 80.
        assert_eq!(
            layout.region_at(0.0, 80.0),
            WindowRegion::Page { x: 0.0, y: 0.0 }
        );
        assert_eq!(
            layout.region_at(301.0, 380.0),
            WindowRegion::Page { x: 150.5, y: 150.0 }
        );
        assert_eq!(
            layout.region_at(2559.0, 1439.0),
            WindowRegion::Page {
                x: 1279.5,
                y: 679.5
            }
        );
        assert_eq!(layout.region_at(2560.0, 500.0), WindowRegion::Outside);
        assert_eq!(layout.region_at(-1.0, 500.0), WindowRegion::Outside);
        assert_eq!(layout.region_at(10.0, 1440.0), WindowRegion::Outside);
    }

    #[test]
    fn page_point_keeps_mapping_positions_outside_the_page() {
        let layout = layout(1280, 720, 1.0);
        assert_eq!(layout.page_point(-5.0, 0.0), (-5.0, -40.0));
        assert_eq!(layout.page_point(1300.0, 800.0), (1300.0, 760.0));
    }

    #[test]
    fn identical_inputs_produce_identical_layouts() {
        assert_eq!(layout(1366, 768, 1.5), layout(1366, 768, 1.5));
        assert_ne!(layout(1366, 768, 1.5), layout(1366, 768, 2.0));
        assert_eq!(layout(10, 10, f64::NAN), layout(10, 10, 1.0));
    }
}
