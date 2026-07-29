//! Shared backing-store registry for script-created HTML canvas elements.

use std::cell::RefCell;
use std::collections::HashMap;

use base64::Engine as _;

use crate::paint::{Canvas, Color, Image};

thread_local! {
    static BACKING_STORES: RefCell<HashMap<usize, Image>> = RefCell::new(HashMap::new());
}

/// Replaces one canvas element's RGBA backing store.
pub fn commit(id: usize, width: u32, height: u32, pixels: Vec<u8>) -> bool {
    let Ok(image) = Image::new(width, height, pixels) else {
        return false;
    };
    BACKING_STORES.with(|stores| stores.borrow_mut().insert(id, image));
    true
}

/// Returns a snapshot of a canvas element's current pixels.
pub fn image(id: usize) -> Option<Image> {
    BACKING_STORES.with(|stores| stores.borrow().get(&id).cloned())
}

/// Encodes a canvas element as a PNG data URL.
pub fn png_data_url(id: usize) -> Option<String> {
    let image = image(id)?;
    let mut canvas = Canvas::new(image.width(), image.height());
    for y in 0..image.height() {
        for x in 0..image.width() {
            let offset = ((y * image.width() + x) * 4) as usize;
            canvas.set_pixel(
                x,
                y,
                Color::rgba(
                    image.pixels()[offset],
                    image.pixels()[offset + 1],
                    image.pixels()[offset + 2],
                    image.pixels()[offset + 3],
                ),
            );
        }
    }
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(canvas.encode_png())
    ))
}
