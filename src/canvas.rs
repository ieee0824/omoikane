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
    commit_with_change(owner, width, height, pixels).is_some()
}

/// Commits validated pixels and reports whether dimensions or pixel bytes changed.
/// An identical submission succeeds without replacing the existing image.
pub(crate) fn commit_with_change(
    owner: &NodeHandle,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
) -> Option<bool> {
    let Ok(image) = Image::new(width, height, pixels) else {
        return None;
    };
    BACKING_STORES.with(|stores| {
        let mut stores = stores.borrow_mut();
        stores.retain(|_, store| store.owner.is_alive());
        if stores
            .get(&owner.identity())
            .is_some_and(|store| store.image.retained_pixels_exact(&image))
        {
            return Some(false);
        }
        stores.insert(
            owner.identity(),
            BackingStore {
                owner: owner.downgrade(),
                image,
            },
        );
        Some(true)
    })
}

/// Returns a snapshot of a canvas element's current pixels.
pub fn image(id: usize) -> Option<Image> {
    BACKING_STORES.with(|stores| {
        let mut stores = stores.borrow_mut();
        stores.retain(|_, store| store.owner.is_alive());
        stores.get(&id).map(|store| store.image.clone())
    })
}

/// Returns backing dimensions without cloning its RGBA pixels.
pub(crate) fn backing_dimensions(id: usize) -> Option<(u32, u32)> {
    BACKING_STORES.with(|stores| {
        let mut stores = stores.borrow_mut();
        stores.retain(|_, store| store.owner.is_alive());
        stores
            .get(&id)
            .map(|store| (store.image.width(), store.image.height()))
    })
}

/// Encodes a canvas element as a PNG data URL.
pub fn png_data_url(id: usize) -> Option<String> {
    let image = image(id)?;
    png_data_url_from_rgba(image.width(), image.height(), image.pixels().to_vec())
}

/// Encodes a caller-owned RGBA snapshot through the same PNG path used by
/// HTML canvas elements. This is used by OffscreenCanvas, whose backing store
/// lives in the JavaScript realm rather than the DOM node registry.
pub fn png_data_url_from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Option<String> {
    let canvas = Canvas::from_rgba(width, height, pixels)?;
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(canvas.encode_png())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_canvas_submission_succeeds_without_changing_backing() {
        let node = NodeHandle::element("canvas");
        assert_eq!(
            commit_with_change(&node, 1, 1, vec![1, 2, 3, 255]),
            Some(true)
        );
        assert_eq!(
            commit_with_change(&node, 1, 1, vec![1, 2, 3, 255]),
            Some(false)
        );
        assert!(commit(&node, 1, 1, vec![1, 2, 3, 255]));
        assert_eq!(
            commit_with_change(&node, 1, 1, vec![2, 2, 3, 255]),
            Some(true)
        );
        assert_eq!(commit_with_change(&node, 2, 1, vec![2; 8]), Some(true));
        assert_eq!(commit_with_change(&node, 2, 1, vec![2; 7]), None);
        assert_eq!(backing_dimensions(node.identity()), Some((2, 1)));
    }

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
