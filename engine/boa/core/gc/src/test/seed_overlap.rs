use super::run_test;
use crate::{Collector, EPHEMERON_ROOT_REGISTRY, Ephemeron, REMEMBERED_EPHEMERONS, Rooted, Tracer};

#[test]
fn rooted_remembered_seed_keeps_original_dispatch_pointer_once() {
    run_test(|| {
        let key = Rooted::new(7_u32);
        let root = Ephemeron::new(&key, ());
        let edge = root.clone().into_edge();
        let allocation = crate::BOA_GC.with(|gc| {
            *gc.borrow()
                .young_weaks
                .iter()
                .find(|pointer| {
                    std::ptr::addr_eq(pointer.as_ptr(), edge.erased_inner_ptr().as_ptr())
                })
                .expect("allocation is retained by root")
        });
        REMEMBERED_EPHEMERONS.with(|set| {
            set.borrow_mut().insert(allocation);
        });
        EPHEMERON_ROOT_REGISTRY.with(|set| {
            assert_eq!(set.borrow().len(), 1, "exactly one root allocation");
            assert!(
                set.borrow().contains(&allocation),
                "allocation alias is in root set"
            );
        });
        crate::TEMPORARY_EPHEMERON_ROOTS.with(|set| assert!(set.borrow().is_empty()));
        REMEMBERED_EPHEMERONS.with(|set| assert_eq!(set.borrow().len(), 1));
        let original = EPHEMERON_ROOT_REGISTRY.with(|set| *set.borrow().iter().next().unwrap());
        let mut tracer = Tracer::new_minor();
        Collector::seed_minor_ephemerons(&mut tracer);
        let queued = tracer.ephemeron_queue_snapshot();
        assert_eq!(queued.len(), 1, "retain one initial root seed");
        assert!(
            std::ptr::eq(queued[0].as_ptr(), original.as_ptr()),
            "retain root dispatch metadata"
        );
        // Later enqueues remain ordinary appends, even for this allocation.
        tracer.enqueue_ephemeron(allocation);
        assert_eq!(tracer.ephemeron_queue_snapshot().len(), 2);
    });
}

#[test]
fn remembered_seed_without_external_root_is_not_omitted() {
    run_test(|| {
        let key = Rooted::new(9_u32);
        let edge = Ephemeron::new(&key, ()).into_edge();
        let pointer = edge.erased_inner_ptr();
        assert!(EPHEMERON_ROOT_REGISTRY.with(|set| set.borrow().is_empty()));
        REMEMBERED_EPHEMERONS.with(|set| {
            set.borrow_mut().insert(pointer);
        });
        let mut tracer = Tracer::new_minor();
        // No allocation or collection occurs while this unregistered edge is used.
        Collector::seed_minor_ephemerons(&mut tracer);
        let queued = tracer.ephemeron_queue_snapshot();
        assert_eq!(queued.len(), 1);
        assert!(std::ptr::eq(queued[0].as_ptr(), pointer.as_ptr()));
    });
}

#[derive(Clone)]
struct CountTrace(std::sync::Arc<std::sync::atomic::AtomicUsize>);
impl crate::Finalize for CountTrace {}
// SAFETY: the counter owns no GC edges and tracing only increments it.
unsafe impl crate::Trace for CountTrace {
    unsafe fn trace(&self, _: &mut Tracer) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    fn run_finalizer(&self) {
        crate::Finalize::finalize(self);
    }
}

#[test]
fn overlapping_non_unit_seed_traces_value_once_in_real_minor_mark() {
    run_test(|| {
        let key = Rooted::new(7_u32);
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let root = Ephemeron::new(&key, CountTrace(calls.clone()));
        let edge = root.clone().into_edge();
        REMEMBERED_EPHEMERONS.with(|set| {
            set.borrow_mut().insert(edge.erased_inner_ptr());
        });
        let pending = Collector::mark_minor(&[]);
        assert!(pending.is_empty());
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(*root.key().expect("key remains live"), 7);
    });
}

#[test]
fn cleared_overlapping_seed_is_still_discovered() {
    run_test(|| {
        let key = Rooted::new(7_u32);
        let root = Ephemeron::new(&key, ());
        drop(key);
        crate::force_collect();
        assert!(!root.has_value());
        let edge = root.clone().into_edge();
        REMEMBERED_EPHEMERONS.with(|set| {
            set.borrow_mut().insert(edge.erased_inner_ptr());
        });
        let mut tracer = Tracer::new_minor();
        Collector::seed_minor_ephemerons(&mut tracer);
        assert_eq!(tracer.ephemeron_queue_snapshot().len(), 1);
        // SAFETY: the explicit root retains the allocation; cleared data has no key.
        unsafe { tracer.trace_until_empty() };
        assert_eq!(tracer.take_discovered_ephemerons().len(), 1);
        assert!(!root.has_value());
    });
}

#[test]
fn old_unit_overlapping_seed_retains_existing_pop_time_noop() {
    run_test(|| {
        let key = Rooted::new(7_u32);
        let root = Ephemeron::new(&key, ());
        crate::force_minor_collect();
        crate::force_minor_collect();
        let edge = root.clone().into_edge();
        let pointer = edge.erased_inner_ptr();
        // SAFETY: root retains the ephemeron and key throughout these reads.
        assert!(unsafe { pointer.as_ref() }.header().is_old());
        // SAFETY: key is explicitly rooted throughout this header read.
        assert!(unsafe { key.as_gc().inner_ptr.as_ref() }.header.is_old());
        REMEMBERED_EPHEMERONS.with(|set| {
            set.borrow_mut().insert(pointer);
        });
        let mut tracer = Tracer::new_minor();
        Collector::seed_minor_ephemerons(&mut tracer);
        assert_eq!(tracer.ephemeron_queue_snapshot().len(), 1);
        // SAFETY: both the key and ephemeron are rooted until after this pass.
        unsafe { tracer.trace_until_empty() };
        assert!(tracer.take_discovered_ephemerons().is_empty());
        assert_eq!(*root.key().expect("old live key"), 7);
    });
}
