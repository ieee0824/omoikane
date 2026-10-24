//! One rendering opportunity's bounded, immutable gradient background tile.
//!
//! The scoped thread-local bridge serves painter primitives which do not take a
//! frame context. It owns no DOM/resolver references and is empty outside an
//! explicit render scope. Nested document painting shares that same allocation.
use super::{Canvas, Image, MAX_TILE_PIXELS, Rect, color};
use std::cell::RefCell;
use std::sync::Arc;

#[derive(Clone, PartialEq)]
struct TileKey {
    gradient: color::Gradient,
    width_bits: u32,
    height_bits: u32,
    width: u32,
    height: u32,
}

struct Entry {
    key: TileKey,
    image: Arc<Image>,
}

impl Entry {
    fn bytes(&self) -> usize {
        self.image
            .pixels
            .capacity()
            .saturating_add(self.key.gradient.cache_key_heap_bytes())
            .saturating_add(std::mem::size_of::<Self>())
            .saturating_add(std::mem::size_of::<Image>())
            .saturating_add(2 * std::mem::size_of::<usize>())
    }
}

#[derive(Default)]
struct FrameCache {
    entry: Option<Entry>,
    frozen: usize,
}

thread_local! {
    static CACHE: RefCell<Option<FrameCache>> = const { RefCell::new(None) };
    #[cfg(test)]
    static GENERATED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Owns the cache lifetime; nested scopes borrow only the implicit capability.
pub(crate) struct GradientTileCacheScope {
    owner: bool,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

/// Starts one bounded cache shared by ordinary and retained rendering.
pub(crate) fn gradient_tile_cache_scope() -> GradientTileCacheScope {
    let owner = CACHE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            false
        } else {
            *slot = Some(FrameCache::default());
            true
        }
    });
    GradientTileCacheScope {
        owner,
        _thread: std::marker::PhantomData,
    }
}

impl Drop for GradientTileCacheScope {
    fn drop(&mut self) {
        if self.owner {
            CACHE.with(|slot| *slot.borrow_mut() = None);
        }
    }
}

/// Prevents new cache allocations while retained budgets are being consumed.
pub(super) struct FrozenGradientTiles;

pub(super) fn freeze_gradient_tiles() -> FrozenGradientTiles {
    CACHE.with(|slot| {
        if let Some(cache) = slot.borrow_mut().as_mut() {
            cache.frozen += 1;
        }
    });
    FrozenGradientTiles
}

impl Drop for FrozenGradientTiles {
    fn drop(&mut self) {
        CACHE.with(|slot| {
            if let Some(cache) = slot.borrow_mut().as_mut() {
                cache.frozen -= 1;
            }
        });
    }
}

/// Charges the live tile against a retained peak, evicting it if necessary.
/// Capture is frozen, so eviction cannot be followed by a larger cached tile.
/// An oversized retained request still fails exactly as before.
pub(super) fn retained_peak_fits(peak: usize, limit: usize) -> bool {
    if peak > limit {
        CACHE.with(|slot| {
            if let Some(cache) = slot.borrow_mut().as_mut() {
                cache.entry = None;
            }
        });
        return false;
    }
    CACHE.with(|slot| {
        if let Some(cache) = slot.borrow_mut().as_mut()
            && cache
                .entry
                .as_ref()
                .is_some_and(|entry| entry.bytes() > limit - peak)
        {
            cache.entry = None;
        }
    });
    true
}

/// Returns exactly the historical local-origin raster. Position, clipping and
/// repetition are deliberately left to the existing image compositing path.
pub(super) fn gradient_tile(
    gradient: &color::Gradient,
    tile_width: f32,
    tile_height: f32,
    width: u32,
    height: u32,
) -> Option<Arc<Image>> {
    if u64::from(width) * u64::from(height) > MAX_TILE_PIXELS {
        return None;
    }
    let key = TileKey {
        gradient: gradient.clone(),
        width_bits: tile_width.to_bits(),
        height_bits: tile_height.to_bits(),
        width,
        height,
    };
    let cached = CACHE.with(|slot| {
        let slot = slot.borrow();
        slot.as_ref()?
            .entry
            .as_ref()
            .filter(|entry| entry.key == key)
            .map(|entry| Arc::clone(&entry.image))
    });
    if cached.is_some() {
        return cached;
    }
    // Drop the previous allocation before creating another. During capture a
    // miss uses the historical transient raster and never adds retained memory.
    let store = CACHE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(cache) = slot.as_mut() else {
            return false;
        };
        cache.entry = None;
        cache.frozen == 0
    });
    #[cfg(test)]
    GENERATED.with(|count| count.set(count.get() + 1));
    let mut canvas = Canvas::new(width, height);
    color::paint_gradient(
        &mut canvas,
        gradient,
        Rect {
            x: 0.0,
            y: 0.0,
            width: tile_width,
            height: tile_height,
        },
        None,
    );
    let image = Arc::new(Image::new(width, height, canvas.pixels).ok()?);
    if store {
        CACHE.with(|slot| {
            if let Some(cache) = slot.borrow_mut().as_mut() {
                let entry = Entry {
                    key,
                    image: Arc::clone(&image),
                };
                if entry.bytes() <= (MAX_TILE_PIXELS as usize).saturating_mul(4) {
                    cache.entry = Some(entry);
                }
            }
        });
    }
    Some(image)
}

#[cfg(test)]
pub(super) fn generation_count() -> usize {
    GENERATED.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests;
