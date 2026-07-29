//! Shared backing-store registry for script-created HTML canvas elements.

use std::cell::RefCell;
use std::collections::HashMap;

use base64::Engine as _;

use crate::dom::{NodeHandle, WeakNodeHandle};
use crate::paint::{Canvas, Image};

struct BackingStore {
    owner: WeakNodeHandle,
    image: Image,
}

thread_local! {
    static BACKING_STORES: RefCell<HashMap<usize, BackingStore>> = RefCell::new(HashMap::new());
}

/// Replaces one canvas element's RGBA backing store.
pub fn commit(owner: &NodeHandle, width: u32, height: u32, pixels: Vec<u8>) -> bool {
    let Ok(image) = Image::new(width, height, pixels) else {
        return false;
    };
    BACKING_STORES.with(|stores| {
        let mut stores = stores.borrow_mut();
        stores.retain(|_, store| store.owner.is_alive());
        stores.insert(
            owner.identity(),
            BackingStore {
                owner: owner.downgrade(),
                image,
            },
        );
    });
    true
}

/// Returns a snapshot of a canvas element's current pixels.
pub fn image(id: usize) -> Option<Image> {
    BACKING_STORES.with(|stores| {
        let mut stores = stores.borrow_mut();
        stores.retain(|_, store| store.owner.is_alive());
        stores.get(&id).map(|store| store.image.clone())
    })
}

/// Encodes a canvas element as a PNG data URL.
pub fn png_data_url(id: usize) -> Option<String> {
    let image = image(id)?;
    let canvas = Canvas::from_rgba(image.width(), image.height(), image.pixels().to_vec())?;
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(canvas.encode_png())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn released_canvas_nodes_drop_their_backing_store() {
        let node = NodeHandle::element("canvas");
        let id = node.identity();
        assert!(commit(&node, 1, 1, vec![1, 2, 3, 255]));
        assert!(image(id).is_some());
        drop(node);
        assert!(image(id).is_none());
    }
}
