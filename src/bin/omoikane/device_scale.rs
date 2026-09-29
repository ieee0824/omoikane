//! Mapping between the native surface's physical pixels and page CSS pixels.
//!
//! The engine paints at one device pixel per CSS pixel. On displays whose
//! scale factor differs from 1, the page is laid out in CSS pixels and each
//! physical pixel `p` shows CSS pixel `floor(p / scale)`. Pointer input maps
//! `p` to `p / scale`, so an integral pointer position lands on the CSS pixel
//! drawn under it.

/// A validated window scale factor (physical pixels per CSS pixel).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DeviceScale(f64);

impl DeviceScale {
    /// Accepts a finite positive factor and falls back to 1 otherwise.
    pub(super) fn new(factor: f64) -> Self {
        Self(if factor.is_finite() && factor > 0.0 {
            factor
        } else {
            1.0
        })
    }

    /// Returns the factor used for both painting and pointer input.
    pub(super) fn factor(self) -> f64 {
        self.0
    }

    /// Returns the page viewport, in CSS pixels, for a physical surface size.
    ///
    /// Rounds up so every physical pixel has a CSS pixel to sample. A zero
    /// physical dimension stays zero.
    pub(super) fn page_viewport(self, physical_width: u32, physical_height: u32) -> (u32, u32) {
        (
            self.css_extent(physical_width),
            self.css_extent(physical_height),
        )
    }

    fn css_extent(self, physical: u32) -> u32 {
        if physical == 0 {
            return 0;
        }
        // Ignore float noise such as 2560 / 1.25 = 2048.0000000000002.
        let css = (f64::from(physical) / self.0 - 1e-9).ceil();
        css.max(1.0) as u32
    }

    /// Returns the CSS pixel index shown at physical pixel `physical`, clamped
    /// to a page extent of `css_extent` pixels (which must be non-zero).
    fn css_index(self, physical: u32, css_extent: u32) -> usize {
        let index = (f64::from(physical) / self.0).floor() as u32;
        index.min(css_extent - 1) as usize
    }
}

/// Copies an RGBA page frame into a `0RGB` native surface, scaling it by
/// `scale` with nearest-neighbour sampling.
///
/// `source` holds `source_width * source_height` RGBA pixels and `target`
/// holds `target_width * target_height` surface pixels. Sampling is clamped
/// to the source's last row and column. A source buffer shorter than its
/// declared size stops the copy at the first missing row instead of panicking.
pub(super) fn blit_scaled(
    source: &[u8],
    (source_width, source_height): (u32, u32),
    target: &mut [u32],
    (target_width, target_height): (u32, u32),
    scale: DeviceScale,
) {
    if source_width == 0 || source_height == 0 || target_width == 0 {
        return;
    }
    let columns: Vec<usize> = (0..target_width)
        .map(|x| scale.css_index(x, source_width))
        .collect();
    let source_stride = source_width as usize * 4;
    for (y, row) in target
        .chunks_exact_mut(target_width as usize)
        .take(target_height as usize)
        .enumerate()
    {
        let source_y = scale.css_index(y as u32, source_height);
        let Some(source_row) = source.get(source_y * source_stride..(source_y + 1) * source_stride)
        else {
            return;
        };
        for (destination, &column) in row.iter_mut().zip(&columns) {
            let pixel = &source_row[column * 4..column * 4 + 3];
            *destination =
                u32::from(pixel[0]) << 16 | u32::from(pixel[1]) << 8 | u32::from(pixel[2]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .flat_map(|(x, y)| pixel(x, y))
            .collect()
    }

    #[test]
    fn rejects_non_finite_and_non_positive_scale_factors() {
        for factor in [0.0, -2.0, f64::NAN, f64::INFINITY] {
            assert_eq!(DeviceScale::new(factor), DeviceScale::new(1.0));
        }
        assert_eq!(DeviceScale::new(2.0).page_viewport(2560, 1600), (1280, 800));
    }

    #[test]
    fn page_viewport_is_the_css_size_that_covers_the_physical_surface() {
        assert_eq!(DeviceScale::new(1.0).page_viewport(1280, 720), (1280, 720));
        assert_eq!(DeviceScale::new(2.0).page_viewport(2561, 1), (1281, 1));
        assert_eq!(
            DeviceScale::new(1.25).page_viewport(2560, 1600),
            (2048, 1280)
        );
        assert_eq!(DeviceScale::new(1.5).page_viewport(1001, 3), (668, 2));
        assert_eq!(DeviceScale::new(2.0).page_viewport(0, 720), (0, 360));
        assert_eq!(DeviceScale::new(0.5).page_viewport(100, 1), (200, 2));
        assert_eq!(DeviceScale::new(1e9).page_viewport(3, 3), (1, 1));
    }

    #[test]
    fn scales_each_css_pixel_to_a_block_of_physical_pixels() {
        let source = rgba(2, 2, |x, y| [x as u8 * 100, y as u8 * 100, 7, 255]);
        let mut target = vec![0; 16];
        blit_scaled(&source, (2, 2), &mut target, (4, 4), DeviceScale::new(2.0));
        let expected = |x: u32, y: u32| (x / 2 * 100) << 16 | (y / 2 * 100) << 8 | 7;
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(target[(y * 4 + x) as usize], expected(x, y), "({x}, {y})");
            }
        }
    }

    #[test]
    fn fractional_scales_fill_every_physical_pixel_from_the_page() {
        for factor in [0.5, 1.25, 1.5, 1.75, 3.0] {
            let scale = DeviceScale::new(factor);
            let (width, height) = (101, 37);
            let (css_width, css_height) = scale.page_viewport(width, height);
            let source = rgba(css_width, css_height, |x, _| {
                [(x % 250) as u8 + 1, 0, 0, 255]
            });
            let mut target = vec![0; (width * height) as usize];
            blit_scaled(
                &source,
                (css_width, css_height),
                &mut target,
                (width, height),
                scale,
            );
            for x in 0..width {
                let expected_column = (f64::from(x) / factor).floor() as u32;
                assert!(expected_column < css_width, "{factor}: {x}");
                assert_eq!(
                    target[x as usize] >> 16,
                    expected_column % 250 + 1,
                    "{factor}: {x}"
                );
            }
            assert!(target.iter().all(|&pixel| pixel != 0), "{factor}");
        }
    }

    #[test]
    fn mismatched_buffers_clamp_sampling_without_panicking() {
        let source = rgba(1, 1, |_, _| [1, 2, 3, 255]);
        let mut target = vec![0; 4];
        blit_scaled(&source, (1, 1), &mut target, (2, 2), DeviceScale::new(1.0));
        assert_eq!(target, vec![0x010203; 4]);

        blit_scaled(
            &source[..2],
            (1, 1),
            &mut target,
            (2, 2),
            DeviceScale::new(1.0),
        );
        blit_scaled(&source, (0, 1), &mut target, (2, 2), DeviceScale::new(1.0));
        assert_eq!(target, vec![0x010203; 4]);
    }
}
