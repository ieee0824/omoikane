//! Intrinsic measurements owned by a single synchronous layout pass.
//!
//! DOM, resolver contexts, font registries, image policy/base/timestamp and
//! content-visibility inputs must remain fixed within `with_pass`. Resource
//! workers cannot publish completions here: publication requires explicit
//! polling outside layout. Image dependency recording is a document-pass set,
//! so the first measurement preserves coverage for subsequent cache hits.
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Kind {
    Minimum,
    Maximum,
}

#[derive(Default)]
struct MeasurementPass {
    widths: HashMap<(usize, Kind, bool), f32>,
}

thread_local! {
    static PASS: RefCell<Option<MeasurementPass>> = const { RefCell::new(None) };
    #[cfg(test)]
    static TEST_STATE: RefCell<(bool, usize, usize)> = const { RefCell::new((false, 0, 0)) };
}

struct Scope(Option<MeasurementPass>);
impl Drop for Scope {
    fn drop(&mut self) {
        PASS.with(|slot| {
            slot.replace(self.0.take());
        });
    }
}

// Nested invocations get independent storage and restore the outer pass.
// Call separately for every container-query iteration, after counter setup.
pub(super) fn with_pass<T>(operation: impl FnOnce() -> T) -> T {
    let _scope = Scope(PASS.with(|slot| slot.replace(Some(MeasurementPass::default()))));
    operation()
}

pub(super) fn measure(
    node: usize,
    kind: Kind,
    ignore_own_width: bool,
    compute: impl FnOnce() -> f32,
) -> f32 {
    let key = (node, kind, ignore_own_width);
    #[cfg(test)]
    let disabled = TEST_STATE.with(|state| state.borrow().0);
    #[cfg(not(test))]
    let disabled = false;
    let cached = (!disabled)
        .then(|| {
            PASS.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .and_then(|pass| pass.widths.get(&key).copied())
            })
        })
        .flatten();
    if let Some(width) = cached {
        super::profile::record_intrinsic_hit();
        #[cfg(test)]
        TEST_STATE.with(|state| state.borrow_mut().2 += 1);
        return width;
    }
    #[cfg(test)]
    TEST_STATE.with(|state| state.borrow_mut().1 += 1);
    // No TLS borrow crosses recursive measurement or style/resource resolution.
    let _phase = super::profile::Scope::begin(super::profile::Phase::IntrinsicRaw);
    let width = compute();
    if !disabled {
        PASS.with(|slot| {
            if let Some(pass) = slot.borrow_mut().as_mut() {
                pass.widths.insert(key, width);
            }
        });
    }
    width
}

#[cfg(test)]
pub(super) fn probe<T>(disabled: bool, operation: impl FnOnce() -> T) -> (T, usize, usize) {
    struct Restore((bool, usize, usize));
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_STATE.with(|state| {
                state.replace(self.0);
            });
        }
    }
    let _restore = Restore(TEST_STATE.with(|state| state.replace((disabled, 0, 0))));
    let result = operation();
    let (_, raw, hits) = TEST_STATE.with(|state| *state.borrow());
    (result, raw, hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::{Origin, StyleResolver, parse_stylesheet};
    use crate::dom::NodeHandle;
    use crate::layout::{LayoutBox, Rect, intrinsic_width, layout_tree, minimum_content_width};

    fn fixture(depth: usize) -> (NodeHandle, Vec<NodeHandle>, StyleResolver) {
        let root = NodeHandle::element("div");
        let mut ancestors = vec![root.clone()];
        let mut parent = root.clone();
        for _ in 0..depth {
            let child = NodeHandle::element("div");
            parent.append_child(child.clone());
            ancestors.push(child.clone());
            parent = child;
        }
        parent.append_child(NodeHandle::text("intrinsic measurement fixture"));
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("div {display:flex;align-items:flex-start}").unwrap(),
        );
        (root, ancestors, resolver)
    }

    #[test]
    fn non_wrapping_text_reuses_max_content_measurement_within_each_pass() {
        for (white_space, shares_measurement) in [
            ("normal", false),
            ("pre-wrap", false),
            ("pre-line", false),
            ("nowrap", true),
            ("pre", true),
        ] {
            let parent = NodeHandle::element("span");
            let text = NodeHandle::text("wide   text\nwith spacing");
            parent.append_child(text.clone());
            let mut resolver = StyleResolver::new();
            resolver.add_stylesheet(
                Origin::Author,
                parse_stylesheet(&format!(
                    "span{{white-space:{white_space};font-size:21px;letter-spacing:2px}}"
                ))
                .unwrap(),
            );

            for maximum_first in [false, true] {
                let ((minimum, maximum), raw, hits) = probe(false, || {
                    with_pass(|| {
                        if maximum_first {
                            let maximum = intrinsic_width(&text, &mut resolver);
                            let minimum = minimum_content_width(&text, &mut resolver);
                            (minimum, maximum)
                        } else {
                            let minimum = minimum_content_width(&text, &mut resolver);
                            let maximum = intrinsic_width(&text, &mut resolver);
                            (minimum, maximum)
                        }
                    })
                });
                if shares_measurement {
                    assert_eq!(minimum, maximum, "white-space:{white_space}");
                    assert_eq!(raw, 1, "white-space:{white_space}");
                    assert_eq!(hits, 1, "white-space:{white_space}");
                } else {
                    assert_eq!(raw, 2, "white-space:{white_space}");
                }
            }

            if shares_measurement {
                let before = with_pass(|| intrinsic_width(&text, &mut resolver));
                text.set_data("substantially wider replacement text");
                let ((minimum, maximum), raw, hits) = probe(false, || {
                    with_pass(|| {
                        let minimum = minimum_content_width(&text, &mut resolver);
                        let maximum = intrinsic_width(&text, &mut resolver);
                        (minimum, maximum)
                    })
                });
                assert!(maximum > before, "white-space:{white_space}");
                assert_eq!(minimum, maximum, "white-space:{white_space}");
                assert_eq!((raw, hits), (1, 1), "white-space:{white_space}");
            }
        }
    }

    #[test]
    fn recursive_measurements_bound_raw_subtree_work() {
        for depth in [8, 16, 32] {
            let (_, nodes, mut resolver) = fixture(depth);
            let leaf_text = nodes.last().unwrap().first_child().unwrap();
            let mut widths = || {
                nodes
                    .iter()
                    // Flex measures anonymous text outside the intrinsic
                    // wrapper. Query it explicitly to cover both cache kinds.
                    .chain(std::iter::once(&leaf_text))
                    .map(|node| {
                        (
                            intrinsic_width(node, &mut resolver),
                            minimum_content_width(node, &mut resolver),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let (uncached, baseline, _) = probe(true, || with_pass(&mut widths));
            let (cached, raw, hits) = probe(false, || with_pass(&mut widths));
            assert_eq!(cached, uncached);
            assert_eq!(raw, 2 * (nodes.len() + 1));
            assert!(hits >= 2 * depth);
            assert!(baseline > raw * depth / 2);
        }
    }

    fn geometry(layout: &LayoutBox, out: &mut Vec<Rect>) {
        out.push(layout.dimensions.content);
        for line in &layout.lines {
            out.push(line.rect);
        }
        for child in &layout.children {
            geometry(child, out);
        }
    }

    #[test]
    fn nested_flex_matches_geometry_and_pixels_and_new_dom_style_pass() {
        let (root, nodes, mut resolver) = fixture(12);
        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            width: 320.0,
            height: 200.0,
        };
        for changed in [false, true] {
            if changed {
                nodes
                    .last()
                    .unwrap()
                    .append_child(NodeHandle::text(" changed text"));
                resolver.add_stylesheet(
                    Origin::Author,
                    parse_stylesheet("div{font-size:23px}").unwrap(),
                );
            }
            let (baseline, _, _) = probe(true, || {
                layout_tree(&root, &mut resolver, viewport).unwrap()
            });
            let (cached, _, hits) = probe(false, || {
                layout_tree(&root, &mut resolver, viewport).unwrap()
            });
            let mut baseline_geometry = Vec::new();
            let mut cached_geometry = Vec::new();
            geometry(&baseline, &mut baseline_geometry);
            geometry(&cached, &mut cached_geometry);
            assert_eq!(cached_geometry, baseline_geometry);
            assert_eq!(cached, baseline, "complete layout tree must match");
            assert!(hits > 0);
            assert_eq!(
                crate::paint::paint_layout(&cached, &mut resolver, viewport).pixels(),
                crate::paint::paint_layout(&baseline, &mut resolver, viewport).pixels()
            );
        }
    }

    #[test]
    fn released_ephemeral_node_identity_cannot_hit_a_later_node_measurement() {
        with_pass(|| {
            let first = NodeHandle::element("span");
            let first_id = first.identity();
            let weak = first.downgrade();
            assert_eq!(measure(first_id, Kind::Maximum, false, || 11.0), 11.0);
            drop(first);
            assert!(
                weak.upgrade().is_none(),
                "memo must not keep dead DOM nodes alive"
            );
            drop(weak);
            // No assumption about allocator address reuse: identity is minted
            // monotonically in NodeHandle::new, independently of Rc addresses.
            let later = NodeHandle::element("span");
            assert!(later.identity() > first_id);
            assert_eq!(
                measure(later.identity(), Kind::Maximum, false, || 29.0),
                29.0
            );
            assert_eq!(
                measure(first_id, Kind::Maximum, false, || panic!("old entry lost")),
                11.0
            );
        });
    }

    #[test]
    fn kinds_and_ignore_width_are_separate_and_unwind_restores_outer_scope() {
        let value = with_pass(|| {
            assert_eq!(measure(1, Kind::Maximum, false, || 10.0), 10.0);
            assert_eq!(measure(1, Kind::Maximum, true, || 20.0), 20.0);
            assert_eq!(measure(1, Kind::Minimum, false, || 30.0), 30.0);
            assert_eq!(measure(1, Kind::Minimum, true, || 35.0), 35.0);
            let _ = std::panic::catch_unwind(|| {
                with_pass(|| {
                    measure(1, Kind::Maximum, false, || 99.0);
                    panic!("inner pass");
                })
            });
            measure(1, Kind::Maximum, false, || panic!("outer cache lost"))
        });
        assert_eq!(value, 10.0);
        assert!(PASS.with(|slot| slot.borrow().is_none()));
        assert_eq!(
            with_pass(|| measure(1, Kind::Maximum, false, || 40.0)),
            40.0
        );
    }

    fn assert_render_equal(
        root: &NodeHandle,
        resolver: &mut StyleResolver,
        viewport: Rect,
        web_fonts: Option<&crate::font::WebFontRegistry>,
    ) -> LayoutBox {
        let baseline = probe(true, || layout_tree(root, resolver, viewport).unwrap()).0;
        let cached = probe(false, || layout_tree(root, resolver, viewport).unwrap()).0;
        assert_eq!(cached, baseline, "entire geometry/fragments must match");
        let baseline_pixels = crate::paint::paint_layout_with_web_fonts(
            &baseline,
            resolver,
            viewport,
            Vec::new(),
            web_fonts,
        );
        let cached_pixels = crate::paint::paint_layout_with_web_fonts(
            &cached,
            resolver,
            viewport,
            Vec::new(),
            web_fonts,
        );
        assert_eq!(cached_pixels.pixels(), baseline_pixels.pixels());
        cached
    }

    #[test]
    fn new_font_registry_recomputes_actual_intrinsic_geometry_and_pixels() {
        use crate::font::{Font, FontStyle, FontWeight, WebFontRegistry};
        use crate::layout::with_layout_fonts;
        use std::sync::Arc;
        let root = NodeHandle::element("div");
        let item = NodeHandle::element("span");
        item.append_child(NodeHandle::text("AABB"));
        root.append_child(item.clone());
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "div{display:flex;align-items:flex-start}span{font:20px Fixture;width:max-content}",
            )
            .unwrap(),
        );
        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            width: 200.0,
            height: 64.0,
        };
        let mut widths = Vec::new();
        for bytes in [
            &include_bytes!(
                "../../tests/fixtures/anonymized-font-selection/OmoikaneFixture-Regular.ttf"
            )[..],
            &include_bytes!(
                "../../tests/fixtures/anonymized-font-selection/OmoikaneFixture-Condensed.ttf"
            )[..],
        ] {
            let mut registry = WebFontRegistry::new();
            registry.push(
                "Fixture",
                FontWeight(400),
                FontStyle::Normal,
                Font::load_from_bytes(bytes.to_vec()).unwrap(),
            );
            let registry = Arc::new(registry);
            resolver.set_web_fonts(Arc::clone(&registry));
            let layout = with_layout_fonts(Vec::new(), Some(Arc::clone(&registry)), || {
                assert_render_equal(&root, &mut resolver, viewport, Some(registry.as_ref()))
            });
            widths.push(
                layout
                    .children
                    .iter()
                    .find(|child| child.node == item)
                    .unwrap()
                    .dimensions
                    .content
                    .width,
            );
        }
        // The committed CC0 faces have A/B advances 500 versus 350 / 1000 em.
        assert_eq!(widths, vec![40.0, 28.0]);
    }

    #[test]
    fn container_query_iterations_do_not_share_intrinsic_measurements() {
        let body = NodeHandle::element("body");
        let container = NodeHandle::element("section");
        let flex = NodeHandle::element("div");
        let item = NodeHandle::element("article");
        let leaf = NodeHandle::element("span");
        item.append_child(leaf);
        flex.append_child(item.clone());
        container.append_child(flex);
        body.append_child(container);
        let css = "body{margin:0}section{width:500px;container-type:inline-size} \
            div{display:flex;align-items:flex-start}span{display:block;width:10px;height:7px;background:red} \
            @container (width >= 400px){span{width:100px}}";
        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            width: 600.0,
            height: 64.0,
        };
        let run = |disabled| {
            // Independent empty container snapshots are essential: using one
            // already-converged resolver would not exercise per-iteration reset.
            let mut resolver = StyleResolver::new();
            resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());
            let layout = probe(disabled, || {
                layout_tree(&body, &mut resolver, viewport).unwrap()
            })
            .0;
            let pixels = crate::paint::paint_layout(&layout, &mut resolver, viewport)
                .pixels()
                .to_vec();
            (layout, pixels)
        };
        let baseline = run(true);
        let cached = run(false);
        assert_eq!(cached, baseline);
        fn find<'a>(layout: &'a LayoutBox, node: &NodeHandle) -> Option<&'a LayoutBox> {
            if &layout.node == node {
                return Some(layout);
            }
            layout.children.iter().find_map(|child| find(child, node))
        }
        assert_eq!(
            find(&cached.0, &item).unwrap().dimensions.content.width,
            100.0
        );
    }
}
