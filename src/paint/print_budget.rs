//! Allocation limits for a complete printed document, checked before painting.

use super::{Canvas, PaintError};
use crate::layout::Rect;

/// Maximum width or height of a printed page canvas, in CSS pixels.
pub const MAX_PRINT_PAGE_DIMENSION: u32 = 16_384;
/// Maximum combined pixels retained by all canvases in one printed document.
///
/// This limits returned RGBA buffers to 256 MiB. Temporary paint surfaces,
/// orientation buffers and PNG encoding buffers are additional allocations.
pub const MAX_PRINT_DOCUMENT_PIXELS: u64 = 67_108_864;

pub(super) fn dimensions(sheet: Rect) -> Result<(u32, u32), PaintError> {
    if !sheet.width.is_finite()
        || !sheet.height.is_finite()
        || sheet.width <= 0.0
        || sheet.height <= 0.0
    {
        return Err(PaintError::InvalidImageBuffer);
    }
    if sheet.width.ceil() > MAX_PRINT_PAGE_DIMENSION as f32
        || sheet.height.ceil() > MAX_PRINT_PAGE_DIMENSION as f32
    {
        return Err(PaintError::PrintCanvasBudgetExceeded);
    }
    // The finite, positive, bounded values above are representable as u32.
    Ok((sheet.width.ceil() as u32, sheet.height.ceil() as u32))
}

pub(super) fn validate_document(
    sheets: impl IntoIterator<Item = Rect>,
) -> Result<Vec<(u32, u32)>, PaintError> {
    let mut total_pixels = 0_u64;
    sheets
        .into_iter()
        .map(|sheet| {
            let (width, height) = dimensions(sheet)?;
            rgba_bytes(width, height)?;
            let page_pixels = u64::from(width)
                .checked_mul(u64::from(height))
                .ok_or(PaintError::PrintCanvasBudgetExceeded)?;
            total_pixels = total_pixels
                .checked_add(page_pixels)
                .ok_or(PaintError::PrintCanvasBudgetExceeded)?;
            if total_pixels > MAX_PRINT_DOCUMENT_PIXELS {
                return Err(PaintError::PrintCanvasBudgetExceeded);
            }
            Ok((width, height))
        })
        .collect()
}

fn rgba_bytes(width: u32, height: u32) -> Result<usize, PaintError> {
    let exceeded = PaintError::PrintCanvasBudgetExceeded;
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(exceeded.clone())?;
    if width == 0
        || height == 0
        || width > MAX_PRINT_PAGE_DIMENSION
        || height > MAX_PRINT_PAGE_DIMENSION
        || pixels > MAX_PRINT_DOCUMENT_PIXELS
    {
        return Err(exceeded);
    }
    let rgba = usize::try_from(pixels)
        .ok()
        .and_then(|count| count.checked_mul(4))
        .ok_or(exceeded.clone())?;
    // PNG adds one filter byte per row. Check the larger row count so both
    // rotate-left and rotate-right also remain safe to encode.
    let raw = rgba
        .checked_add(width.max(height) as usize)
        .ok_or(exceeded.clone())?;
    let blocks = raw
        .checked_add(u16::MAX as usize - 1)
        .map(|bytes| bytes / u16::MAX as usize)
        .ok_or(exceeded.clone())?;
    let idat = blocks
        .checked_mul(5)
        .and_then(|overhead| raw.checked_add(overhead))
        .and_then(|bytes| bytes.checked_add(6))
        .ok_or(exceeded.clone())?;
    // PNG chunk lengths and dimensions must fit 31 bits. The dimension cap
    // above also keeps painting's u32 pixel-index arithmetic representable.
    i32::try_from(idat).map_err(|_| exceeded.clone())?;
    let png = idat.checked_add(8 + 25 + 12 + 12).ok_or(exceeded.clone())?;
    isize::try_from(png).map_err(|_| exceeded)?;
    Ok(rgba)
}

pub(super) fn new_canvas(width: u32, height: u32) -> Result<Canvas, PaintError> {
    let bytes = rgba_bytes(width, height)?;
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(bytes)
        .map_err(|_| PaintError::PrintCanvasAllocationFailed)?;
    pixels.resize(bytes, 0);
    Ok(Canvas {
        width,
        height,
        pixels,
        animation_regions: Vec::new(),
        ink_probe: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(width: f32, height: f32) -> Rect {
        Rect {
            width,
            height,
            ..Rect::default()
        }
    }

    #[test]
    fn print_budget_rounds_up_before_counting_pixels() {
        assert_eq!(dimensions(sheet(0.1, 20.1)), Ok((1, 21)));
        assert_eq!(
            dimensions(sheet(16_384.5, 1.0)),
            Err(PaintError::PrintCanvasBudgetExceeded)
        );
        assert_eq!(
            validate_document([sheet(8192.0, 8192.1)]),
            Err(PaintError::PrintCanvasBudgetExceeded)
        );
    }

    #[test]
    fn print_budget_checks_all_pages_without_allocating_pixels() {
        let page = sheet(8192.0, 4096.0);
        assert_eq!(validate_document([page, page]).unwrap().len(), 2);
        assert_eq!(
            validate_document([page, page, sheet(1.0, 1.0)]),
            Err(PaintError::PrintCanvasBudgetExceeded)
        );
        assert_eq!(
            validate_document([sheet(794.0, 1123.0); 75]).unwrap().len(),
            75
        );
        // Valid individual pages can still exceed the document budget.
        assert_eq!(
            validate_document([sheet(794.0, 1123.0); 76]),
            Err(PaintError::PrintCanvasBudgetExceeded)
        );
    }

    #[test]
    fn print_budget_rejects_invalid_and_unrepresentable_dimensions() {
        for invalid in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                dimensions(sheet(invalid, 1.0)),
                Err(PaintError::InvalidImageBuffer)
            );
            assert_eq!(
                dimensions(sheet(1.0, invalid)),
                Err(PaintError::InvalidImageBuffer)
            );
        }
        assert_eq!(
            dimensions(sheet(f32::MAX, 1.0)),
            Err(PaintError::PrintCanvasBudgetExceeded)
        );
        assert_eq!(
            rgba_bytes(u32::MAX, u32::MAX),
            Err(PaintError::PrintCanvasBudgetExceeded)
        );
    }

    #[test]
    fn print_budget_supports_png_and_rotation_at_the_pixel_limit() {
        let bytes = (MAX_PRINT_DOCUMENT_PIXELS * 4) as usize;
        assert_eq!(rgba_bytes(16_384, 4096), Ok(bytes));
        assert_eq!(rgba_bytes(4096, 16_384), Ok(bytes));
        let canvas = new_canvas(3, 2).unwrap();
        assert_eq!(canvas.pixels().len(), 24);
        let decoded = super::super::Image::decode_png(&canvas.encode_png()).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (3, 2));
    }
}
