//! Owned replay of the ordinary painter's axis-aligned overflow clips.
use super::*;

pub(super) const MAX_OWNED_OVERFLOW_CLIPS: usize = 16;
const MAX_CLIP_COMPONENT: f32 = 1_048_576.0;

#[derive(Clone, Debug, PartialEq)]
struct OwnedOverflowClip {
    owner_node_id: usize,
    dimensions: crate::layout::BoxDimensions,
    overflow: crate::layout::Overflow,
}

impl OwnedOverflowClip {
    fn capture(layout: &LayoutBox) -> Option<Self> {
        let dimensions = layout.dimensions;
        let content = dimensions.content;
        let padding = dimensions.padding;
        let components = [
            content.x,
            content.y,
            content.width,
            content.height,
            padding.top,
            padding.right,
            padding.bottom,
            padding.left,
        ];
        if content.width < 0.0
            || content.height < 0.0
            || !components
                .into_iter()
                .all(|value| value.is_finite() && value.abs() <= MAX_CLIP_COMPONENT)
            || [padding.top, padding.right, padding.bottom, padding.left]
                .into_iter()
                .any(|value| value < 0.0)
            || !rect_is_bounded(padding_box_at_scroll(dimensions, (0.0, 0.0)))
        {
            return None;
        }
        Some(Self {
            owner_node_id: layout.node.identity(),
            dimensions,
            overflow: layout.overflow,
        })
    }

    fn replay(&self, inherited: Rect, viewport: Rect, scroll: (f64, f64)) -> Option<Rect> {
        let padding_box = padding_box_at_scroll(self.dimensions, scroll);
        rect_is_bounded(padding_box).then_some(())?;
        overflow_clip_rect(self.overflow, padding_box, Some(inherited), viewport)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClipChainRejection {
    Capacity,
    Unrepresentable,
}

#[derive(Clone, Copy)]
pub(super) struct ClipChainCheckpoint {
    length: usize,
    rejection: Option<ClipChainRejection>,
}

/// Root-to-target numeric clip operations. DOM/layout owners are never retained.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct OwnedOverflowClipChain {
    operations: Vec<OwnedOverflowClip>,
    rejection: Option<ClipChainRejection>,
}

impl OwnedOverflowClipChain {
    pub(super) fn checkpoint(&self) -> ClipChainCheckpoint {
        ClipChainCheckpoint {
            length: self.operations.len(),
            rejection: self.rejection,
        }
    }

    pub(super) fn truncate(&mut self, checkpoint: ClipChainCheckpoint) {
        self.operations.truncate(checkpoint.length);
        self.rejection = checkpoint.rejection;
    }

    pub(super) fn is_empty(&self) -> bool {
        self.operations.is_empty() && self.rejection.is_none()
    }

    pub(super) fn push(&mut self, layout: &LayoutBox) {
        if self.rejection.is_some() {
            return;
        }
        if self.operations.len() >= MAX_OWNED_OVERFLOW_CLIPS {
            self.rejection = Some(ClipChainRejection::Capacity);
            return;
        }
        match OwnedOverflowClip::capture(layout) {
            Some(operation) => self.operations.push(operation),
            None => self.rejection = Some(ClipChainRejection::Unrepresentable),
        }
    }

    pub(super) fn capacity_exceeded(&self) -> bool {
        self.rejection == Some(ClipChainRejection::Capacity)
    }

    pub(super) fn unrepresentable(&self) -> bool {
        self.rejection == Some(ClipChainRejection::Unrepresentable)
    }

    pub(super) fn replay(&self, viewport: Rect, scroll: (f64, f64)) -> Option<Rect> {
        self.rejection.is_none().then_some(())?;
        self.operations
            .iter()
            .try_fold(viewport, |clip, operation| {
                operation.replay(clip, viewport, scroll)
            })
    }

    pub(super) fn owner_node_ids(&self) -> impl Iterator<Item = usize> + '_ {
        self.operations
            .iter()
            .map(|operation| operation.owner_node_id)
    }

    pub(super) fn contains_owner(&self, owner_node_id: usize) -> bool {
        self.operations
            .iter()
            .any(|operation| operation.owner_node_id == owner_node_id)
    }

    pub(super) fn heap_bytes(&self) -> Option<usize> {
        self.operations
            .capacity()
            .checked_mul(std::mem::size_of::<OwnedOverflowClip>())
    }
}

fn padding_box_at_scroll(mut dimensions: crate::layout::BoxDimensions, scroll: (f64, f64)) -> Rect {
    // Match apply_scroll_offsets followed by padding_box_rect, including f32
    // conversion and operation order.
    dimensions.content.x += -(scroll.0 as f32);
    dimensions.content.y += -(scroll.1 as f32);
    let content = dimensions.content;
    let padding = dimensions.padding;
    Rect {
        x: content.x - padding.left,
        y: content.y - padding.top,
        width: content.width + padding.left + padding.right,
        height: content.height + padding.top + padding.bottom,
    }
}

fn rect_is_bounded(rect: Rect) -> bool {
    rect.width >= 0.0
        && rect.height >= 0.0
        && [
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            rect.x + rect.width,
            rect.y + rect.height,
        ]
        .into_iter()
        .all(|value| value.is_finite() && value.abs() <= MAX_CLIP_COMPONENT * 2.0)
}
