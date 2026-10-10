use super::{Harness, run_test};
use crate::{
    Ephemeron, Finalize, GcEdge, GcRefCell, Rooted, Trace, Tracer, WeakGcEdge, WeakMap,
    force_collect, force_minor_collect,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Debug, Finalize, Trace)]
struct OldRoot {
    holder: GcEdge<OldHolder>,
}

#[derive(Debug, Finalize, Trace)]
struct OldHolder {
    child: GcRefCell<Option<GcEdge<u32>>>,
}

fn is_old<T: Trace + ?Sized>(root: &Rooted<T>) -> bool {
    // SAFETY: the explicit root keeps the allocation alive.
    unsafe { root.as_gc().inner_ptr.as_ref() }.header.is_old()
}

fn edge_is_old<T: Trace + ?Sized>(edge: &GcEdge<T>) -> bool {
    // SAFETY: the edge is valid for the duration of this test.
    unsafe { edge.as_gc().inner_ptr.as_ref() }.header.is_old()
}

#[test]
fn minor_collection_sweeps_garbage_and_promotes_survivors() {
    run_test(|| {
        let live = Rooted::new(1_u32);
        let _garbage = GcEdge::new(2_u32);

        force_minor_collect();
        Harness::assert_strong_allocations(1);
        assert!(!is_old(&live));

        force_minor_collect();
        assert!(is_old(&live));
        Harness::assert_strong_allocations(1);
    });
}

#[derive(Debug)]
struct TraceCounter(Arc<AtomicUsize>);

impl Finalize for TraceCounter {}

unsafe impl Trace for TraceCounter {
    unsafe fn trace(&self, _tracer: &mut Tracer) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn run_finalizer(&self) {
        Finalize::finalize(self);
    }
}

#[test]
fn minor_collection_does_not_trace_old_roots() {
    run_test(|| {
        let traces = Arc::new(AtomicUsize::new(0));
        let root = Rooted::new(TraceCounter(Arc::clone(&traces)));

        force_minor_collect();
        force_minor_collect();
        assert!(is_old(&root));
        let after_promotion = traces.load(Ordering::Relaxed);

        force_minor_collect();
        assert_eq!(traces.load(Ordering::Relaxed), after_promotion);
    });
}

#[test]
fn major_collection_reclaims_old_garbage() {
    run_test(|| {
        let root = Rooted::new(1_u32);
        force_minor_collect();
        force_minor_collect();
        assert!(is_old(&root));

        drop(root);
        force_collect();
        Harness::assert_empty_gc();
    });
}

#[derive(Debug)]
struct FinalizerWritesYoungEdge {
    parent: GcEdge<OldHolder>,
    child: GcEdge<u32>,
}

impl Finalize for FinalizerWritesYoungEdge {
    fn finalize(&self) {
        *self.parent.child.borrow_mut() = Some(self.child.clone());
    }
}

unsafe impl Trace for FinalizerWritesYoungEdge {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        // SAFETY: forwarding the collector's trace call preserves its pointer
        // validity contract.
        unsafe {
            self.parent.trace(tracer);
            self.child.trace(tracer);
        }
    }

    fn run_finalizer(&self) {
        Finalize::finalize(self);
    }
}

#[test]
fn major_second_mark_observes_old_parent_writes_from_finalizers() {
    run_test(|| {
        let parent = Rooted::new(OldHolder {
            child: GcRefCell::new(None),
        });
        force_minor_collect();
        force_minor_collect();
        assert!(is_old(&parent));

        let child = GcEdge::new(42_u32);
        let _unreachable_finalizer = GcEdge::new(FinalizerWritesYoungEdge {
            parent: parent.clone().into_edge(),
            child,
        });

        force_collect();

        assert_eq!(
            parent.child.borrow().as_ref().map(|value| **value),
            Some(42)
        );
    });
}

#[test]
fn mutable_cell_write_remembers_old_to_young_edge() {
    run_test(|| {
        let root = Rooted::new(OldRoot {
            holder: GcEdge::new(OldHolder {
                child: GcRefCell::new(None),
            }),
        });

        force_minor_collect();
        force_minor_collect();
        assert!(is_old(&root));
        assert!(edge_is_old(&root.holder));

        *root.holder.child.borrow_mut() = Some(GcEdge::new(42));
        force_minor_collect();

        assert_eq!(
            root.holder.child.borrow().as_ref().map(|value| **value),
            Some(42)
        );
    });
}

#[test]
fn major_collection_preserves_dirty_old_parent_for_the_next_minor() {
    run_test(|| {
        let parent = Rooted::new(OldHolder {
            child: GcRefCell::new(None),
        });
        force_minor_collect();
        force_minor_collect();
        assert!(is_old(&parent));

        let child = GcEdge::new(42);
        *parent.child.borrow_mut() = Some(child);
        // The major trace sees the old-to-young edge before a minor collection
        // has materialized it as a direct remembered nursery root.
        force_collect();
        force_minor_collect();

        assert_eq!(
            parent.child.borrow().as_ref().map(|value| **value),
            Some(42)
        );
    });
}

#[test]
fn promotion_remembers_a_young_child_already_marked_by_the_minor_pass() {
    run_test(|| {
        let parent = Rooted::new(OldHolder {
            child: GcRefCell::new(None),
        });

        // Age the parent once before creating the child, so the parent is
        // promoted on the next minor pass while the child remains young.
        force_minor_collect();
        *parent.child.borrow_mut() = Some(GcEdge::new(42));
        force_minor_collect();
        assert!(is_old(&parent));

        // The child was already marked by the ordinary nursery walk when the
        // parent was promoted. The promotion scan must still retain it as an
        // old-to-young remembered root.
        force_minor_collect();
        Harness::assert_strong_allocations(2);
        assert_eq!(
            parent.child.borrow().as_ref().map(|value| **value),
            Some(42)
        );
    });
}

#[test]
fn dirty_parent_scan_preserves_previously_enqueued_roots() {
    run_test(|| {
        let parent = Rooted::new(OldHolder {
            child: GcRefCell::new(None),
        });
        force_minor_collect();
        force_minor_collect();
        assert!(is_old(&parent));

        let independent_root = Rooted::new(7_u32);
        *parent.child.borrow_mut() = Some(GcEdge::new(42));
        force_minor_collect();

        Harness::assert_strong_allocations(3);
        assert_eq!(*independent_root, 7);
        assert_eq!(
            parent.child.borrow().as_ref().map(|value| **value),
            Some(42)
        );
    });
}

#[derive(Debug, Finalize)]
struct CountingOldHolder {
    traces: Arc<AtomicUsize>,
    child: GcRefCell<Option<GcEdge<u32>>>,
}

unsafe impl Trace for CountingOldHolder {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        self.traces.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwarding the collector's trace call preserves its pointer
        // validity contract.
        unsafe { self.child.trace(tracer) };
    }

    fn run_finalizer(&self) {
        Finalize::finalize(self);
        self.child.run_finalizer();
    }
}

#[test]
fn old_parent_is_rescanned_only_after_another_write() {
    run_test(|| {
        let traces = Arc::new(AtomicUsize::new(0));
        let holder = Rooted::new(CountingOldHolder {
            traces: Arc::clone(&traces),
            child: GcRefCell::new(None),
        });

        force_minor_collect();
        force_minor_collect();
        assert!(is_old(&holder));

        *holder.child.borrow_mut() = Some(GcEdge::new(1));
        let before_first_write = traces.load(Ordering::Relaxed);
        force_minor_collect();
        let after_first_write = traces.load(Ordering::Relaxed);
        assert_eq!(after_first_write, before_first_write + 1);

        // The surviving child keeps this nursery collection non-empty. With no
        // intervening mutation, the old holder must not be traced again.
        force_minor_collect();
        assert_eq!(traces.load(Ordering::Relaxed), after_first_write);

        *holder.child.borrow_mut() = Some(GcEdge::new(2));
        force_minor_collect();
        assert_eq!(traces.load(Ordering::Relaxed), after_first_write + 1);
        assert_eq!(holder.child.borrow().as_ref().map(|value| **value), Some(2));
    });
}

fn run_ephemeron_generation_case(key_old: bool, value_old: bool) {
    let key = Rooted::new(0_u32);
    if key_old {
        force_minor_collect();
        force_minor_collect();
        assert!(is_old(&key));
    }

    let value = Rooted::new(99_u32);
    if value_old {
        force_minor_collect();
        force_minor_collect();
        assert!(is_old(&value));
    }
    let value = value.into_edge();

    let ephemeron = Ephemeron::new(&key, value);
    force_minor_collect();

    assert_eq!(ephemeron.value().as_deref().copied(), Some(99));
}

#[test]
fn minor_ephemeron_tracing_handles_all_generation_pairs() {
    run_test(|| {
        for key_old in [false, true] {
            for value_old in [false, true] {
                run_ephemeron_generation_case(key_old, value_old);
                force_collect();
            }
        }
    });
}

#[test]
fn promoted_ephemeron_value_installs_old_graph_barriers() {
    run_test(|| {
        let key = Rooted::new(0_u32);
        let holder = GcEdge::new(OldHolder {
            child: GcRefCell::new(None),
        });
        let ephemeron = Ephemeron::new(&key, holder.clone());

        force_minor_collect();
        force_minor_collect();
        assert!(is_old(&key));
        assert!(edge_is_old(&holder));

        // The holder is kept alive only by the rooted ephemeron. A write to its
        // old cell must still remember the newly allocated child.
        *holder.child.borrow_mut() = Some(GcEdge::new(42));
        force_minor_collect();

        assert_eq!(
            holder.child.borrow().as_ref().map(|value| **value),
            Some(42)
        );
        assert!(ephemeron.has_value());
    });
}

#[test]
fn ephemeron_promotion_leaves_young_value_unmarked_between_collections() {
    run_test(|| {
        let key = Rooted::new(0_u32);
        force_minor_collect();
        force_minor_collect();

        let edge = crate::EphemeronEdge::new(
            &key.clone().into_edge(),
            GcRefCell::new(None::<GcEdge<OldHolder>>),
        );
        let _root = Ephemeron::from_edge(edge.clone());
        force_minor_collect();

        let holder = GcEdge::new(OldHolder {
            child: GcRefCell::new(None),
        });
        // SAFETY: the registered ephemeron root keeps its immutable value
        // storage alive, and no collection runs while the cell is borrowed.
        *unsafe { edge.inner().value() }.unwrap().borrow_mut() = Some(holder.clone());
        force_minor_collect();

        assert!(!edge_is_old(&holder));
        // SAFETY: the live key and registered ephemeron retain this value.
        assert!(
            !unsafe { holder.as_gc().inner_ptr.as_ref() }
                .header
                .is_minor_marked(),
            "promotion tracing must not leave a mark for the next collection"
        );

        *holder.child.borrow_mut() = Some(GcEdge::new(42));
        force_minor_collect();
        assert_eq!(
            holder.child.borrow().as_ref().map(|value| **value),
            Some(42)
        );
    });
}

#[test]
fn retargeting_an_old_weak_edge_reuses_its_ephemeron() {
    run_test(|| {
        let first = Rooted::new(1_u32);
        let mut weak = WeakGcEdge::new_rooted(&first);
        let _weak_root = weak.root();

        force_minor_collect();
        force_minor_collect();
        Harness::assert_ephemeron_allocations(1);
        Harness::assert_remembered_ephemerons(1);

        let second = Rooted::new(2_u32);
        let second_edge = second.clone().into_edge();
        weak.retarget_edge(&second_edge);
        Harness::assert_ephemeron_allocations(1);

        force_minor_collect();
        assert_eq!(weak.upgrade().as_deref().copied(), Some(2));

        drop(first);
        drop(second);
        force_collect();
        assert!(weak.upgrade().is_none());
    });
}

#[test]
fn old_ephemeron_keys_are_conservative_only_until_major_collection() {
    run_test(|| {
        let key = Rooted::new(0_u32);
        force_minor_collect();
        force_minor_collect();

        let ephemeron = Ephemeron::new(&key, GcEdge::new(99_u32));
        drop(key);

        force_minor_collect();
        assert!(ephemeron.has_value());

        force_collect();
        assert!(!ephemeron.has_value());
    });
}

#[test]
fn minor_collection_keeps_live_weak_maps_and_clears_dead_keys() {
    run_test(|| {
        let mut map = WeakMap::new();
        let key = Rooted::new(7_u32);
        map.insert(&key, 11_u32);

        force_minor_collect();
        assert_eq!(map.inner.borrow().len(), 1);

        drop(key);
        force_minor_collect();
        assert_eq!(map.inner.borrow().len(), 0);
    });
}

#[test]
fn major_collection_preserves_dirty_parent_reached_through_ephemeron() {
    run_test(|| {
        let key = Rooted::new(0_u32);
        let parent = Rooted::new(OldHolder {
            child: GcRefCell::new(None),
        });
        force_minor_collect();
        force_minor_collect();
        assert!(is_old(&parent));

        *parent.child.borrow_mut() = Some(GcEdge::new(42_u32));
        let ephemeron = Ephemeron::new(&key, parent.into_edge());
        force_collect();
        force_minor_collect();

        // Check allocation membership before dereferencing a potentially swept
        // child, so a regression reports a failure instead of invoking UB.
        let parent = ephemeron.value().expect("live key retains the parent");
        let child = parent.child.borrow();
        let pointer = child
            .as_ref()
            .expect("parent retains its child")
            .as_gc()
            .inner_ptr
            .cast();
        crate::BOA_GC.with(|gc| {
            let gc = gc.borrow();
            assert!(
                gc.youngs.contains(&pointer) || gc.old_strongs.contains(&pointer),
                "ephemeron-reachable old parent must retain its young child"
            );
        });
        assert_eq!(child.as_ref().map(|value| **value), Some(42));
    });
}

#[test]
fn dirty_ephemeron_parent_is_reclaimed_when_its_key_dies() {
    run_test(|| {
        let key = Rooted::new(0_u32);
        let parent = Rooted::new(OldHolder {
            child: GcRefCell::new(None),
        });
        force_minor_collect();
        force_minor_collect();
        *parent.child.borrow_mut() = Some(GcEdge::new(42_u32));
        let ephemeron = Ephemeron::new(&key, parent.into_edge());
        drop(key);

        force_collect();
        assert!(!ephemeron.has_value());
        Harness::assert_strong_allocations(0);
        force_minor_collect();
        Harness::assert_strong_allocations(0);
    });
}

#[test]
fn old_unit_ephemeron_survives_minor_but_does_not_root_its_key_for_major() {
    run_test(|| {
        let key = Rooted::new(7_u32);
        let weak = WeakGcEdge::new_rooted(&key);
        let _weak_root = weak.root();
        force_minor_collect();
        force_minor_collect();
        assert!(is_old(&key));
        assert!(weak.inner().inner().header.is_old());

        drop(key);
        let _minor_seed = Rooted::new(1_u32);
        force_minor_collect();
        assert_eq!(weak.upgrade().as_deref().copied(), Some(7));

        force_collect();
        assert!(weak.upgrade().is_none());
        let _next_minor_seed = Rooted::new(2_u32);
        force_minor_collect();
        assert!(!weak.is_upgradable());
    });
}

#[test]
fn young_unit_ephemeron_with_old_key_remains_a_valid_weak_handle() {
    run_test(|| {
        let key = Rooted::new(7_u32);
        force_minor_collect();
        force_minor_collect();
        assert!(is_old(&key));

        let weak = WeakGcEdge::new_rooted(&key);
        let _weak_root = weak.root();
        assert!(weak.inner().inner().header.is_young());
        let pointer = weak.inner().erased_inner_ptr();
        force_minor_collect();
        assert_ephemeron_allocation_exists(pointer);
        assert_eq!(weak.upgrade().as_deref().copied(), Some(7));
        force_minor_collect();
        assert_ephemeron_allocation_exists(pointer);
        assert_eq!(weak.upgrade().as_deref().copied(), Some(7));

        drop(key);
        force_collect();
        assert!(weak.upgrade().is_none());
    });
}

#[test]
fn old_unit_ephemeron_retarget_to_dead_young_key_clears_and_can_retarget_again() {
    run_test(|| {
        let first = Rooted::new(1_u32);
        let mut weak = WeakGcEdge::new_rooted(&first);
        let _weak_root = weak.root();
        force_minor_collect();
        force_minor_collect();
        assert!(is_old(&first));
        assert!(weak.inner().inner().header.is_old());

        let dead_young = Rooted::new(2_u32).into_edge();
        assert!(!edge_is_old(&dead_young));
        weak.retarget_edge(&dead_young);
        drop(dead_young);
        force_minor_collect();
        assert!(!weak.is_upgradable());

        let live_young = Rooted::new(3_u32);
        assert!(!is_old(&live_young));
        weak.retarget_edge(&live_young.clone().into_edge());
        force_minor_collect();
        assert_eq!(weak.upgrade().as_deref().copied(), Some(3));
        force_minor_collect();
        assert!(is_old(&live_young));
        assert_eq!(weak.upgrade().as_deref().copied(), Some(3));

        drop(live_young);
        force_collect();
        assert!(!weak.is_upgradable());
    });
}

#[test]
fn old_non_unit_zero_sized_ephemeron_preserves_its_custom_trace() {
    thread_local! {
        static TRACE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }
    struct ZeroSizedTrace;
    impl Finalize for ZeroSizedTrace {}
    unsafe impl Trace for ZeroSizedTrace {
        unsafe fn trace(&self, _tracer: &mut Tracer) {
            TRACE_CALLS.with(|calls| calls.set(calls.get() + 1));
        }

        fn run_finalizer(&self) {
            Finalize::finalize(self);
        }
    }

    run_test(|| {
        let key = Rooted::new(7_u32);
        let edge = crate::EphemeronEdge::new(&key.clone().into_edge(), ZeroSizedTrace);
        let _ephemeron = Ephemeron::from_edge(edge.clone());
        force_minor_collect();
        force_minor_collect();
        assert!(is_old(&key));
        assert!(edge.inner().header.is_old());
        let before = TRACE_CALLS.with(std::cell::Cell::get);

        let _minor_seed = Rooted::new(1_u32);
        force_minor_collect();
        assert!(TRACE_CALLS.with(std::cell::Cell::get) > before);
    });
}

fn assert_ephemeron_allocation_exists(pointer: crate::EphemeronPointer) {
    crate::BOA_GC.with(|gc| {
        let gc = gc.borrow();
        assert!(
            gc.young_weaks
                .iter()
                .chain(&gc.old_weaks)
                .any(|entry| std::ptr::addr_eq(entry.as_ptr(), pointer.as_ptr())),
            "a rooted ephemeron must remain allocated before accessing its key"
        );
    });
}

#[derive(Debug, Finalize, Trace)]
struct MinorRetargetKey {
    value: GcEdge<u32>,
}

type MinorRetargetParent = GcRefCell<Option<GcEdge<MinorRetargetKey>>>;
type MinorRetargetWeak = GcRefCell<WeakGcEdge<MinorRetargetKey>>;

#[derive(Debug)]
struct MinorRetargetFinalizer {
    parent: GcEdge<MinorRetargetParent>,
    weak: GcEdge<MinorRetargetWeak>,
    target: GcEdge<MinorRetargetKey>,
    resurrect: bool,
}

impl Finalize for MinorRetargetFinalizer {
    fn finalize(&self) {
        self.weak.borrow_mut().retarget_edge(&self.target);
        if self.resurrect {
            *self.parent.borrow_mut() = Some(self.target.clone());
        }
    }
}

// SAFETY: all three GC edges are forwarded to the collector without retaining
// a borrow or changing any edge during tracing.
unsafe impl Trace for MinorRetargetFinalizer {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        unsafe {
            self.parent.trace(tracer);
            self.weak.trace(tracer);
            self.target.trace(tracer);
        }
    }

    fn run_finalizer(&self) {
        Finalize::finalize(self);
    }
}

/// Checks allocation identity without dereferencing a possibly swept pointer.
fn assert_strong_allocation_presence(pointer: crate::GcErasedPointer, present: bool) {
    crate::BOA_GC.with(|gc| {
        let gc = gc.borrow();
        assert_eq!(
            gc.youngs
                .iter()
                .chain(&gc.old_strongs)
                .any(|entry| std::ptr::addr_eq(entry.as_ptr(), pointer.as_ptr())),
            present,
            "allocation membership must be checked before following a GC edge"
        );
    });
}

fn check_minor_finalizer_retarget(resurrect: bool) {
    let original = Rooted::new(MinorRetargetKey {
        value: GcEdge::new(7_u32),
    });
    let parent: Rooted<MinorRetargetParent> = Rooted::new(GcRefCell::new(None));
    let weak = Rooted::new(GcRefCell::new(WeakGcEdge::new_rooted(&original)));
    let ephemeron_pointer = weak.borrow().inner().erased_inner_ptr();
    force_minor_collect();
    force_minor_collect();
    assert!(is_old(&original));
    assert!(is_old(&parent));
    assert!(is_old(&weak));
    assert_ephemeron_allocation_exists(ephemeron_pointer);
    assert!(weak.borrow().inner().inner().header.is_old());

    let value = GcEdge::new(99_u32);
    let value_pointer = value.as_gc().inner_ptr.cast();
    let target = GcEdge::new(MinorRetargetKey { value });
    let target_pointer = target.as_gc().inner_ptr.cast();
    assert!(!edge_is_old(&target));
    assert!(!edge_is_old(&target.value));
    let _unreachable_finalizer = GcEdge::new(MinorRetargetFinalizer {
        parent: parent.clone().into_edge(),
        weak: weak.clone().into_edge(),
        target,
        resurrect,
    });

    // The first mark sees an old unit ephemeron with its original old key.
    // Only finalization retargets it; the second mark must use the new key.
    force_minor_collect();
    assert_ephemeron_allocation_exists(ephemeron_pointer);
    assert_strong_allocation_presence(target_pointer, resurrect);
    assert_strong_allocation_presence(value_pointer, resurrect);
    if resurrect {
        let weak_key = weak
            .borrow()
            .upgrade()
            .expect("resurrected key remains live");
        assert!(std::ptr::addr_eq(
            weak_key.as_gc().inner_ptr.as_ptr(),
            target_pointer.as_ptr()
        ));
        assert_eq!(*weak_key.value, 99);
        assert!(!edge_is_old(&weak_key));
        assert!(!edge_is_old(&weak_key.value));
        assert_eq!(parent.borrow().as_ref().map(|key| *key.value), Some(99));
        drop(weak_key);
        *parent.borrow_mut() = None;

        // A weak retarget does not make the newly resurrected graph a major root.
        force_collect();
        assert_ephemeron_allocation_exists(ephemeron_pointer);
        assert_strong_allocation_presence(target_pointer, false);
        assert_strong_allocation_presence(value_pointer, false);
        assert!(!weak.borrow().is_upgradable());
    } else {
        assert!(parent.borrow().is_none());
        assert!(!weak.borrow().is_upgradable());
    }
}

#[test]
fn minor_second_mark_observes_finalizer_retarget_without_rooting_a_dead_key() {
    run_test(|| check_minor_finalizer_retarget(true));
    run_test(|| check_minor_finalizer_retarget(false));
}

#[test]
fn promoted_weak_map_tracker_keeps_new_entries_until_their_keys_die() {
    run_test(|| {
        let mut map: WeakMap<u32, GcEdge<u32>> = WeakMap::new();
        // This isolated collector has one externally rooted ephemeron: the
        // unit-valued WeakMap registry tracker, not the unrooted map entries.
        let tracker = crate::EPHEMERON_ROOT_REGISTRY.with(|roots| {
            let roots = roots.borrow();
            assert_eq!(roots.len(), 1);
            *roots
                .iter()
                .next()
                .expect("WeakMap registry tracker exists")
        });
        let original_key = Rooted::new(1_u32);
        let original_value = GcEdge::new(11_u32);
        let original_value_pointer = original_value.as_gc().inner_ptr.cast();
        map.insert(&original_key, original_value);
        force_minor_collect();
        force_minor_collect();
        assert!(is_old(&map.inner));
        assert!(is_old(&original_key));
        assert_ephemeron_allocation_exists(tracker);
        // SAFETY: membership was verified immediately above; no collection,
        // allocation or callback occurs before this header read.
        assert!(unsafe { tracker.as_ref() }.header().is_old());
        assert_strong_allocation_presence(original_value_pointer, true);
        assert_eq!(map.get(&original_key).as_deref().copied(), Some(11));

        let young_key = Rooted::new(2_u32);
        let young_value = GcEdge::new(42_u32);
        let young_value_pointer = young_value.as_gc().inner_ptr.cast();
        map.insert(&young_key, young_value);
        force_minor_collect();
        assert_ephemeron_allocation_exists(tracker);
        assert_strong_allocation_presence(young_value_pointer, true);
        assert_eq!(map.get(&young_key).as_deref().copied(), Some(42));
        assert_eq!(map.get(&original_key).as_deref().copied(), Some(11));
        assert_eq!(map.inner.borrow().len(), 2);
        assert!(!is_old(&young_key));

        drop(young_key);
        force_minor_collect();
        assert_ephemeron_allocation_exists(tracker);
        assert_strong_allocation_presence(young_value_pointer, false);
        assert_eq!(map.inner.borrow().len(), 1);
        assert_eq!(map.get(&original_key).as_deref().copied(), Some(11));

        drop(original_key);
        force_collect();
        assert_ephemeron_allocation_exists(tracker);
        assert_strong_allocation_presence(original_value_pointer, false);
        assert!(map.inner.borrow().is_empty());
        assert!(crate::has_weak_maps());
        drop(map);
        force_collect();
        assert!(!crate::has_weak_maps());
        Harness::assert_strong_allocations(0);
        // The registry drops its unit ephemeron root after sweep, so a second
        // major collection reclaims that now-unrooted tracker allocation.
        force_collect();
        Harness::assert_empty_gc();
    });
}
