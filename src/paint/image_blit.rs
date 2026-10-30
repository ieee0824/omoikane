//! Exact integer image copies; callers retain animation and recording ownership.
use super::{Canvas, Color, Image, ImageSampling, Rect, blend_pixel};
use std::ops::Range;

impl Canvas {
    /// Avoids scaling arithmetic for a normalized, opaque-opacity 1:1 image.
    /// Returns false before touching pixels when ordinary sampling is required.
    pub(super) fn try_draw_unscaled_image(
        &mut self,
        image: &Image,
        destination: Rect,
        opacity: f32,
        xs: Range<i32>,
        ys: Range<i32>,
    ) -> bool {
        if opacity != 1.0
            || destination.width != image.width as f32
            || destination.height != image.height as f32
            || image.width >= 16_777_216
            || image.height >= 16_777_216
            || ![destination.x, destination.y]
                .into_iter()
                .all(|v| v.is_finite() && v.fract() == 0.0 && v.abs() < 16_777_216.0)
        {
            return false;
        }
        // The original sampler converts destination addresses to f32. Keep
        // that path outside its exact integer domain, even for integer origins.
        if xs.end > 16_777_216 || ys.end > 16_777_216 {
            return false;
        }
        if xs.start < 0 || ys.start < 0 || xs.start >= xs.end || ys.start >= ys.end {
            return false;
        }
        let sx = i64::from(xs.start) - destination.x as i64;
        let sy = i64::from(ys.start) - destination.y as i64;
        let width = xs.end - xs.start;
        let height = ys.end - ys.start;
        if xs.end as u32 > self.width
            || ys.end as u32 > self.height
            || sx < 0
            || sy < 0
            || sx + i64::from(width) > i64::from(image.width)
            || sy + i64::from(height) > i64::from(image.height)
        {
            return false;
        }
        let bytes = width as usize * 4;
        for row in 0..height as usize {
            let source = ((sy as usize + row) * image.width as usize + sx as usize) * 4;
            let target = ((ys.start as usize + row) * self.width as usize + xs.start as usize) * 4;
            let source = &image.pixels[source..source + bytes];
            let target = &mut self.pixels[target..target + bytes];
            if source.chunks_exact(4).all(|pixel| pixel[3] == 255) {
                target.copy_from_slice(source);
                continue;
            }
            for (dst, src) in target.chunks_exact_mut(4).zip(source.chunks_exact(4)) {
                match src[3] {
                    255 => dst.copy_from_slice(src),
                    0 => {
                        // Ordinary source-over canonicalizes fully transparent RGB.
                        if dst[3] == 0 {
                            dst.fill(0);
                        }
                    }
                    _ => blend_pixel(dst, Color::rgba(src[0], src[1], src[2], src[3])),
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint::{intersect, normalize_rect};

    // Preserve the original scaling loop as the reference for the new copy path.
    fn sampled_reference(
        canvas: &mut Canvas,
        image: &Image,
        rect: Rect,
        clip: Option<Rect>,
        opacity: f32,
    ) {
        let rect = normalize_rect(rect).unwrap();
        let area = match clip.and_then(normalize_rect) {
            Some(clip) => match intersect(rect, clip) {
                Some(area) => area,
                None => return,
            },
            None => rect,
        };
        let xs = area.x.floor().max(0.0) as i32
            ..(area.x + area.width).ceil().min(canvas.width as f32) as i32;
        let ys = area.y.floor().max(0.0) as i32
            ..(area.y + area.height).ceil().min(canvas.height as f32) as i32;
        canvas.draw_scaled_image_pixels(image, rect, opacity, ImageSampling::Legacy, xs, ys);
    }

    #[test]
    fn integer_copy_keeps_sampling_beyond_exact_float_pixel_addresses() {
        let origin = 16_777_214;
        let mut canvas = Canvas::new(16_777_220, 1);
        let image = Image::new(6, 1, (1..=6).flat_map(|red| [red, 0, 0, 255]).collect()).unwrap();
        canvas.draw_image(&image, origin as f32, 0.0);
        let reds: Vec<_> = canvas.pixels[origin * 4..]
            .chunks_exact(4)
            .map(|pixel| pixel[0])
            .collect();
        // Preserve the existing sampler's rounding at huge addresses; the
        // exact-copy optimization is confined to representable pixel positions.
        assert_eq!(reds, [1, 2, 3, 3, 5, 0]);
    }

    #[test]
    fn integer_image_copy_matches_sampling_for_all_source_and_destination_alphas() {
        let source: Vec<u8> = (0..256)
            .flat_map(|y| (0..256).flat_map(move |x| [x as u8, y as u8, 93, x as u8]))
            .collect();
        let image = Image::new(256, 256, source).unwrap();
        let mut actual = Canvas::new(256, 256);
        for (index, pixel) in actual.pixels.chunks_exact_mut(4).enumerate() {
            pixel.copy_from_slice(&[71, 129, 233, (index / 256) as u8]);
        }
        let mut expected = actual.clone();
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            width: 256.0,
            height: 256.0,
        };
        sampled_reference(&mut expected, &image, rect, None, 1.0);
        actual.draw_image(&image, 0.0, 0.0);
        assert_eq!(actual.pixels, expected.pixels);
        assert_eq!(&actual.pixels[..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn integer_image_copy_preserves_offsets_clipping_and_opaque_rows() {
        let pixels: Vec<u8> = (0..17)
            .flat_map(|y| {
                (0..23).flat_map(move |x| {
                    let alpha = if y % 2 == 0 {
                        255
                    } else {
                        [0, 1, 128, 254, 255][x % 5]
                    };
                    [x as u8, y as u8, 177, alpha]
                })
            })
            .collect();
        let image = Image::new(23, 17, pixels).unwrap();
        for (x, y) in [
            (-7.0, -5.0),
            (3.0, 4.0),
            (20.0, 15.0),
            (30.0, 26.0),
            (0.5, 1.25),
        ] {
            for clip in [
                None,
                Some(Rect {
                    x: 1.5,
                    y: 2.25,
                    width: 14.25,
                    height: 13.5,
                }),
            ] {
                for opacity in [1.0, 0.5] {
                    let rect = Rect {
                        x,
                        y,
                        width: 23.0,
                        height: 17.0,
                    };
                    let mut actual = Canvas::new(31, 27);
                    actual.fill_rect(
                        Rect {
                            x: 0.0,
                            y: 0.0,
                            width: 31.0,
                            height: 27.0,
                        },
                        Color::rgba(31, 63, 127, 128),
                    );
                    let mut expected = actual.clone();
                    sampled_reference(&mut expected, &image, rect, clip, opacity);
                    actual.draw_image_scaled_clipped_with_opacity(&image, rect, clip, opacity);
                    assert_eq!(
                        actual.pixels, expected.pixels,
                        "position={x},{y},clip={clip:?},opacity={opacity}"
                    );
                }
            }
        }
    }
}
