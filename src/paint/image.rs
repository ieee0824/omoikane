//! Image decoding (PNG, JPEG, GIF) and data URI parsing.

use std::io::Cursor;
use std::io::Read;

use base64::Engine;
use flate2::read::ZlibDecoder;

use super::{DataUri, Image, PaintError};

use crate::layout::Rect;
use crate::css::ComputedStyle;
use crate::css::ComputedValue;

pub(crate) fn decode_gif(bytes: &[u8]) -> Result<Image, PaintError> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options
        .read_info(Cursor::new(bytes))
        .map_err(|_| PaintError::InvalidImageBuffer)?;
    let canvas_width = u32::from(decoder.width());
    let canvas_height = u32::from(decoder.height());
    let frame = decoder
        .read_next_frame()
        .map_err(|_| PaintError::InvalidImageBuffer)?
        .ok_or(PaintError::InvalidImageBuffer)?;
    let mut pixels = vec![0; canvas_width as usize * canvas_height as usize * 4];
    let frame_width = usize::from(frame.width);
    for y in 0..usize::from(frame.height) {
        let source_start = y * frame_width * 4;
        let target_start = ((y + usize::from(frame.top)) * canvas_width as usize
            + usize::from(frame.left)) * 4;
        let length = frame_width * 4;
        if target_start + length > pixels.len() || source_start + length > frame.buffer.len() {
            return Err(PaintError::InvalidImageBuffer);
        }
        pixels[target_start..target_start + length]
            .copy_from_slice(&frame.buffer[source_start..source_start + length]);
    }
    Image::new(canvas_width, canvas_height, pixels)
}

pub(crate) fn decode_png(bytes: &[u8]) -> Result<Image, PaintError> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = match decoder.read_info() {
        Ok(reader) => reader,
        Err(_) => return decode_png_fallback(bytes),
    };
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = match reader.next_frame(&mut buffer) {
        Ok(info) => info,
        Err(_) => return decode_png_fallback(bytes),
    };
    let pixels = &buffer[..info.buffer_size()];

    let rgba = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity(info.width as usize * info.height as usize * 4);
            for chunk in pixels.chunks_exact(3) {
                out.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
            out
        }
        png::ColorType::GrayscaleAlpha => {
            let mut out = Vec::with_capacity(info.width as usize * info.height as usize * 4);
            for chunk in pixels.chunks_exact(2) {
                out.extend_from_slice(&[chunk[0], chunk[0], chunk[0], chunk[1]]);
            }
            out
        }
        png::ColorType::Grayscale => {
            let mut out = Vec::with_capacity(info.width as usize * info.height as usize * 4);
            for value in pixels {
                out.extend_from_slice(&[*value, *value, *value, 255]);
            }
            out
        }
        _ => return Err(PaintError::UnsupportedPngFormat),
    };

    Image::new(info.width, info.height, rgba)
}

pub(crate) fn decode_png_fallback(bytes: &[u8]) -> Result<Image, PaintError> {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < SIGNATURE.len() || &bytes[..8] != SIGNATURE {
        return Err(PaintError::InvalidPngSignature);
    }

    let mut index = 8usize;
    let mut width = None;
    let mut height = None;
    let mut bit_depth = 0u8;
    let mut color_type = 0u8;
    let mut interlace = 0u8;
    let mut compressed = Vec::new();

    while index + 8 <= bytes.len() {
        let length = u32::from_be_bytes(bytes[index..index + 4].try_into().unwrap()) as usize;
        index += 4;
        let chunk_type = &bytes[index..index + 4];
        index += 4;
        if index + length + 4 > bytes.len() {
            return Err(PaintError::CorruptPng);
        }
        let data = &bytes[index..index + length];
        index += length;
        index += 4;

        match chunk_type {
            b"IHDR" => {
                if data.len() < 13 {
                    return Err(PaintError::MissingPngHeader);
                }
                width = Some(u32::from_be_bytes(data[0..4].try_into().unwrap()));
                height = Some(u32::from_be_bytes(data[4..8].try_into().unwrap()));
                bit_depth = data[8];
                color_type = data[9];
                interlace = data[12];
            }
            b"IDAT" => compressed.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
    }

    let width = width.ok_or(PaintError::MissingPngHeader)?;
    let height = height.ok_or(PaintError::MissingPngHeader)?;
    if bit_depth != 8 || interlace != 0 {
        return Err(PaintError::UnsupportedPngFormat);
    }

    let bytes_per_pixel = match color_type {
        0 => 1usize,
        2 => 3usize,
        4 => 2usize,
        6 => 4usize,
        _ => return Err(PaintError::UnsupportedPngFormat),
    };
    let stride = width as usize * bytes_per_pixel;
    let expected = height as usize * (1 + stride);
    let mut decompressed = Vec::new();
    ZlibDecoder::new(Cursor::new(compressed))
        .read_to_end(&mut decompressed)
        .map_err(|_| PaintError::DecompressionFailed)?;
    if decompressed.len() < expected {
        return Err(PaintError::CorruptPng);
    }

    let mut raw = vec![0u8; height as usize * stride];
    for row in 0..height as usize {
        let filter = decompressed[row * (stride + 1)];
        let src = &decompressed[row * (stride + 1) + 1..row * (stride + 1) + 1 + stride];
        let (previous_rows, current_and_rest) = raw.split_at_mut(row * stride);
        let dest = &mut current_and_rest[..stride];
        let prev = if row == 0 {
            None
        } else {
            Some(&previous_rows[(row - 1) * stride..row * stride])
        };
        unfilter_png_scanline(filter, src, prev, dest, bytes_per_pixel)?;
    }

    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    match color_type {
        0 => {
            for &value in &raw {
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
        }
        2 => {
            for chunk in raw.chunks_exact(3) {
                rgba.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
        }
        4 => {
            for chunk in raw.chunks_exact(2) {
                rgba.extend_from_slice(&[chunk[0], chunk[0], chunk[0], chunk[1]]);
            }
        }
        6 => rgba = raw,
        _ => return Err(PaintError::UnsupportedPngFormat),
    }

    Image::new(width, height, rgba)
}

pub(crate) fn unfilter_png_scanline(
    filter: u8,
    src: &[u8],
    prev: Option<&[u8]>,
    dest: &mut [u8],
    bytes_per_pixel: usize,
) -> Result<(), PaintError> {
    match filter {
        0 => dest.copy_from_slice(src),
        1 => {
            for index in 0..src.len() {
                let left = if index >= bytes_per_pixel {
                    dest[index - bytes_per_pixel]
                } else {
                    0
                };
                dest[index] = src[index].wrapping_add(left);
            }
        }
        2 => {
            for index in 0..src.len() {
                let up = prev.map(|row| row[index]).unwrap_or(0);
                dest[index] = src[index].wrapping_add(up);
            }
        }
        3 => {
            for index in 0..src.len() {
                let left = if index >= bytes_per_pixel {
                    dest[index - bytes_per_pixel]
                } else {
                    0
                };
                let up = prev.map(|row| row[index]).unwrap_or(0);
                dest[index] = src[index].wrapping_add(((left as u16 + up as u16) / 2) as u8);
            }
        }
        4 => {
            for index in 0..src.len() {
                let a = if index >= bytes_per_pixel {
                    dest[index - bytes_per_pixel]
                } else {
                    0
                };
                let b = prev.map(|row| row[index]).unwrap_or(0);
                let c = if index >= bytes_per_pixel {
                    prev.map(|row| row[index - bytes_per_pixel]).unwrap_or(0)
                } else {
                    0
                };
                dest[index] = src[index].wrapping_add(paeth_predictor(a, b, c));
            }
        }
        _ => return Err(PaintError::UnsupportedPngFormat),
    }
    Ok(())
}

pub(crate) fn paeth_predictor(a: u8, b: u8, c: u8) -> u8 {
    let a = a as i32;
    let b = b as i32;
    let c = c as i32;
    let p = a + b - c;
    let pa = (p - a).abs();
    let pb = (p - b).abs();
    let pc = (p - c).abs();
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

pub(crate) fn decode_jpeg(bytes: &[u8]) -> Result<Image, PaintError> {
    use jpeg_decoder::Decoder;

    let mut decoder = Decoder::new(bytes);
    let pixels = decoder.decode().map_err(|_| PaintError::InvalidJpeg)?;
    let info = decoder.info().ok_or(PaintError::InvalidJpeg)?;

    let expected_pixels = info.width as usize * info.height as usize;

    let rgba = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => {
            // Validate buffer size matches expected RGB24 data
            let expected_bytes = expected_pixels * 3;
            if pixels.len() != expected_bytes {
                return Err(PaintError::InvalidJpeg);
            }
            let chunks = pixels.chunks_exact(3);
            if !chunks.remainder().is_empty() {
                return Err(PaintError::InvalidJpeg);
            }
            let mut out = Vec::with_capacity(expected_pixels * 4);
            for chunk in chunks {
                out.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
            out
        }
        jpeg_decoder::PixelFormat::L8 => {
            // Validate buffer size matches expected grayscale data
            if pixels.len() != expected_pixels {
                return Err(PaintError::InvalidJpeg);
            }
            let mut out = Vec::with_capacity(expected_pixels * 4);
            for value in pixels {
                out.extend_from_slice(&[value, value, value, 255]);
            }
            out
        }
        _ => return Err(PaintError::UnsupportedJpegFormat),
    };

    Image::new(info.width as u32, info.height as u32, rgba)
}

/// Parses a `data:` URI into either text or binary content.
pub fn parse_data_uri(uri: &str) -> Result<DataUri, PaintError> {
    let payload = uri
        .strip_prefix("data:")
        .ok_or(PaintError::InvalidDataUri)?;
    let (metadata, data) = payload.split_once(',').ok_or(PaintError::InvalidDataUri)?;
    let mut mime_type = "text/plain".to_string();
    let mut is_base64 = false;

    if !metadata.is_empty() {
        for (index, part) in metadata.split(';').enumerate() {
            if index == 0 && !part.is_empty() {
                mime_type = part.to_string();
                continue;
            }
            if part.eq_ignore_ascii_case("base64") {
                is_base64 = true;
            }
        }
    }

    if is_base64 {
        let decoded_payload = percent_decode(data);
        let data = base64::engine::general_purpose::STANDARD
            .decode(decoded_payload)
            .map_err(|_| PaintError::InvalidBase64)?;
        Ok(DataUri::Binary { mime_type, data })
    } else {
        Ok(DataUri::Text {
            mime_type,
            data: percent_decode(data),
        })
    }
}

pub(crate) fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                out.push((high << 4) | low);
                index += 3;
                continue;
            }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn parse_background_image_value(value: &str) -> Option<Image> {
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case("none") {
        return None;
    }
    let url = trimmed
        .strip_prefix("url(")
        .and_then(|value| value.strip_suffix(')'))
        .unwrap_or(trimmed)
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_start_matches("\\\"")
        .trim_end_matches("\\\"")
        .trim_start_matches("\\'")
        .trim_end_matches("\\'");
    crate::layout::decode_or_fetch_image_asset(url).or_else(|| {
        let data_uri = parse_data_uri(url).ok()?;
        match data_uri {
            DataUri::Binary { mime_type, data } if mime_type.eq_ignore_ascii_case("image/png") => {
                Image::decode_png(&data).ok()
            }
            DataUri::Binary { mime_type, data }
                if mime_type.eq_ignore_ascii_case("image/jpeg")
                    || mime_type.eq_ignore_ascii_case("image/jpg") =>
            {
                Image::decode_jpeg(&data).ok()
            }
            _ => None,
        }
    })
}

/// Returns the rect a replaced element's content paints into, per CSS Images'
/// `object-fit` and `object-position`.
///
/// The rect is the concrete object size placed inside `content_box`. For
/// `cover`, and for `none` with an intrinsic size larger than the box, it
/// deliberately extends outside `content_box`: callers clip to the content box,
/// which is what turns the overflow into a crop. An image with no usable
/// intrinsic size falls back to `fill`.
pub(crate) fn object_fit_destination(
    content_box: Rect,
    image_w: f32,
    image_h: f32,
    style: &ComputedStyle,
) -> Rect {
    if image_w <= 0.0 || image_h <= 0.0 {
        return content_box;
    }
    let scale_to_fit = content_box.width / image_w;
    let scale_to_cover_height = content_box.height / image_h;
    let (width, height) = match object_fit_keyword(style).as_str() {
        "contain" => {
            let scale = scale_to_fit.min(scale_to_cover_height);
            (image_w * scale, image_h * scale)
        }
        "cover" => {
            let scale = scale_to_fit.max(scale_to_cover_height);
            (image_w * scale, image_h * scale)
        }
        "none" => (image_w, image_h),
        // `scale-down` is the smaller of `none` and `contain`, so it only scales
        // when the intrinsic size does not fit.
        "scale-down" => {
            if image_w <= content_box.width && image_h <= content_box.height {
                (image_w, image_h)
            } else {
                let scale = scale_to_fit.min(scale_to_cover_height);
                (image_w * scale, image_h * scale)
            }
        }
        // `fill` and anything unexpected stretch to the content box.
        _ => (content_box.width, content_box.height),
    };
    let (offset_x, offset_y) = object_position_offsets(
        style,
        content_box.width - width,
        content_box.height - height,
    );
    Rect {
        x: content_box.x + offset_x,
        y: content_box.y + offset_y,
        width,
        height,
    }
}

fn object_fit_keyword(style: &ComputedStyle) -> String {
    match style.get("object-fit") {
        Some(ComputedValue::Keyword(keyword)) => keyword.to_ascii_lowercase(),
        _ => "fill".to_string(),
    }
}

/// Resolves `object-position` into pixel offsets from the content box's origin.
///
/// `free_x` / `free_y` are the leftover space on each axis (content box minus
/// object). They are negative when the object is larger than the box, which is
/// how a percentage picks the slice `cover` keeps: `100%` then shifts the object
/// left by exactly its overflow.
fn object_position_offsets(style: &ComputedStyle, free_x: f32, free_y: f32) -> (f32, f32) {
    let Some(ComputedValue::Keyword(value)) = style.get("object-position") else {
        return (free_x * 0.5, free_y * 0.5);
    };
    let mut components = value.split_whitespace();
    let x = components.next();
    let y = components.next();
    (
        resolve_object_position_component(x, free_x),
        resolve_object_position_component(y, free_y),
    )
}

/// Resolves one computed `object-position` component. Computed values are always
/// a percentage or a pixel length (see `render_object_position_value`); anything
/// else falls back to centring so a partial value cannot push content off-box.
fn resolve_object_position_component(component: Option<&str>, free_space: f32) -> f32 {
    let Some(component) = component else {
        return free_space * 0.5;
    };
    if let Some(percentage) = component.strip_suffix('%') {
        return percentage
            .parse::<f32>()
            .map(|percentage| free_space * percentage / 100.0)
            .unwrap_or(free_space * 0.5);
    }
    if let Some(px) = component.strip_suffix("px") {
        return px.parse::<f32>().unwrap_or(free_space * 0.5);
    }
    free_space * 0.5
}

/// Computes the background-size dimensions given the style and the painting area.
/// Returns `(tile_width, tile_height)`.
pub(crate) fn background_size(style: &ComputedStyle, area: Rect, image_w: f32, image_h: f32) -> (f32, f32) {
    image_size(style, "background-size", area, image_w, image_h)
}

/// Computes mask tile dimensions with the same sizing rules as backgrounds.
pub(crate) fn mask_size(style: &ComputedStyle, area: Rect, image_w: f32, image_h: f32) -> (f32, f32) {
    image_size(style, "mask-size", area, image_w, image_h)
}

fn image_size(
    style: &ComputedStyle,
    property: &str,
    area: Rect,
    image_w: f32,
    image_h: f32,
) -> (f32, f32) {
    // Single Px value (e.g. background-size: 100px — width only, height auto)
    if let Some(ComputedValue::Px(px)) = style.get(property) {
        let w = *px;
        let h = if image_w > 0.0 {
            image_h * (w / image_w)
        } else {
            image_h
        };
        return (w, h);
    }

    // Single Percentage value (e.g. background-size: 50% — width only, height auto)
    if let Some(ComputedValue::Percentage(pct)) = style.get(property) {
        let w = area.width * (*pct / 100.0);
        let h = if image_w > 0.0 {
            image_h * (w / image_w)
        } else {
            image_h
        };
        return (w, h);
    }

    let kw = match style.get(property) {
        Some(ComputedValue::Keyword(kw)) => kw.clone(),
        _ => return (image_w, image_h),
    };

    let kw_lower = kw.to_ascii_lowercase();
    match kw_lower.as_str() {
        "cover" => {
            if image_w <= 0.0 || image_h <= 0.0 {
                return (area.width, area.height);
            }
            let scale_w = area.width / image_w;
            let scale_h = area.height / image_h;
            let scale = scale_w.max(scale_h);
            (image_w * scale, image_h * scale)
        }
        "contain" => {
            if image_w <= 0.0 || image_h <= 0.0 {
                return (area.width, area.height);
            }
            let scale_w = area.width / image_w;
            let scale_h = area.height / image_h;
            let scale = scale_w.min(scale_h);
            (image_w * scale, image_h * scale)
        }
        "auto" | "" => (image_w, image_h),
        other => {
            // Try to parse "Wpx Hpx" or "W% H%" etc. from a keyword string
            // (e.g. when background-size: 100px 50px is stored as keyword "100px 50px")
            let mut parts = other.split_whitespace();
            let w = parts
                .next()
                .and_then(|t| parse_size_token(t, area.width))
                .unwrap_or(image_w);
            let h = parts
                .next()
                .and_then(|t| parse_size_token(t, area.height))
                .unwrap_or_else(|| {
                    if image_w > 0.0 {
                        image_h * (w / image_w)
                    } else {
                        image_h
                    }
                });
            (w, h)
        }
    }
}

/// Parses a single background-size value like "100px", "50%", "auto".
/// `container` is the relevant container dimension for percentage resolution.
pub(crate) fn parse_size_token(part: &str, container: f32) -> Option<f32> {
    if part == "auto" {
        return None;
    }
    if let Some(px) = part.strip_suffix("px") {
        return px.parse::<f32>().ok();
    }
    if let Some(pct) = part.strip_suffix('%') {
        return pct.parse::<f32>().ok().map(|p| container * p / 100.0);
    }
    None
}
