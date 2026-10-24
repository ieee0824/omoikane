//! Paint-order reuse for one immutable, adjusted layout generation.

use super::*;
use std::rc::Rc;

/// Owns the layout whose addresses identify lazily prepared paint phases.
/// Replace this together with the adjusted layout on style, paint, layout or
/// scroll changes. Geometry and pointer eligibility are still checked per query.
pub(crate) struct HitTestTree {
    root: Rc<LayoutBox>,
    phases: HitTestPhaseCache,
}

impl HitTestTree {
    #[cfg(test)]
    pub(crate) fn prepared_phase_count(&self) -> usize {
        self.phases.entries.len()
    }

    pub(crate) fn new(root: Rc<LayoutBox>) -> Self {
        Self {
            root,
            phases: HitTestPhaseCache::default(),
        }
    }

    pub(crate) fn query(
        &mut self,
        resolver: &mut StyleResolver,
        viewport: Rect,
        x: f32,
        y: f32,
        all: bool,
    ) -> Vec<NodeHandle> {
        let mut hits = if all {
            HitCollector::All {
                hits: Vec::new(),
                seen: HashSet::new(),
            }
        } else {
            HitCollector::First(None)
        };
        hit_test_box(
            &self.root,
            resolver,
            AffineTransform::identity(),
            Some(viewport),
            viewport,
            x,
            y,
            &mut hits,
            true,
            &mut self.phases,
        );
        match hits {
            HitCollector::First(hit) => hit.into_iter().collect(),
            HitCollector::All { hits, .. } => hits,
        }
    }
}

/// Scoped to an owned immutable tree, so addresses cannot outlive their boxes.
#[derive(Default)]
pub(super) struct HitTestPhaseCache {
    entries: HashMap<(usize, bool), Rc<HitTestPhases>>,
}

impl HitTestPhaseCache {
    pub(super) fn phases(
        &mut self,
        layout: &LayoutBox,
        resolver: &mut StyleResolver,
        include_descendants: bool,
    ) -> Rc<HitTestPhases> {
        let key = (std::ptr::from_ref(layout) as usize, include_descendants);
        self.entries
            .entry(key)
            .or_insert_with(|| {
                Rc::new(partition_children_for_hit_test(
                    layout,
                    resolver,
                    include_descendants,
                ))
            })
            .clone()
    }
}
