use super::run_test;
use crate::{Ephemeron, Finalize, PointerSet, Rooted, Trace, Tracer};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Clone)]
struct CountTrace(Arc<AtomicUsize>);
impl Finalize for CountTrace {}
// SAFETY: this value has no GC edges; the counter observes each value trace.
unsafe impl Trace for CountTrace {
    unsafe fn trace(&self, _: &mut Tracer) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    fn run_finalizer(&self) {
        Finalize::finalize(self);
    }
}

#[test]
fn allocation_and_edge_representations_share_registry_and_trace_identity() {
    run_test(|| {
        let key = Rooted::new(7_u32);
        let traces = Arc::new(AtomicUsize::new(0));
        let roots: Vec<_> = (0..32)
            .map(|_| Ephemeron::new(&key, CountTrace(traces.clone())))
            .collect();
        let edges: Vec<_> = roots.iter().map(|root| root.clone().into_edge()).collect();
        let mut representations = Vec::new();
        let mut missing = 0;
        for edge in &edges {
            let edge_pointer = edge.erased_inner_ptr();
            let allocation_pointer = crate::BOA_GC.with(|gc| {
                *gc.borrow()
                    .young_weaks
                    .iter()
                    .find(|pointer| std::ptr::addr_eq(pointer.as_ptr(), edge_pointer.as_ptr()))
                    .expect("the rooted allocation is in the nursery")
            });
            crate::EPHEMERON_ROOT_REGISTRY.with(|registry| {
                missing += usize::from(!registry.borrow().contains(&allocation_pointer));
            });
            representations.extend([allocation_pointer, edge_pointer]);
        }
        let wide_keys: PointerSet<_> = representations.iter().copied().collect();
        let mut metadata_by_address: HashMap<_, PointerSet<_>> = HashMap::new();
        for pointer in &representations {
            metadata_by_address
                .entry(pointer.cast::<()>())
                .or_default()
                .insert(*pointer);
        }
        let metadata_counts: Vec<_> = metadata_by_address.values().map(PointerSet::len).collect();
        let mut tracer = Tracer::new_minor();
        tracer.enqueue_root(key.as_gc().as_erased_pointer());
        for pointer in &representations {
            tracer.enqueue_ephemeron(*pointer);
        }
        // SAFETY: roots retain every queued allocation throughout this diagnostic pass.
        unsafe {
            tracer.trace_until_empty();
        }
        let discovered = tracer.take_discovered_ephemerons();
        for pointer in &representations {
            tracer.enqueue_ephemeron(*pointer);
        }
        // SAFETY: roots retain every requeued allocation throughout this
        // diagnostic pass.
        unsafe {
            tracer.trace_until_empty();
        }
        assert!(
            tracer.take_discovered_ephemerons().is_empty(),
            "taking discovered ephemerons must retain collection-wide deduplication"
        );
        for pointer in &discovered {
            // SAFETY: all allocations and their key remain rooted until after this pass.
            assert!(unsafe { pointer.as_ref().minor_trace(&mut tracer) });
        }
        let value_traces = traces.load(Ordering::Relaxed);
        eprintln!(
            "EPHEMERON_IDENTITY allocations={} representations={} wide_keys={} unique_addresses={} metadata_counts={metadata_counts:?} missing_registry={} discovered={} value_traces={value_traces}",
            roots.len(),
            representations.len(),
            wide_keys.len(),
            metadata_by_address.len(),
            missing,
            discovered.len()
        );
        assert_eq!(missing, 0, "root membership must ignore coercion metadata");
        assert_eq!(
            discovered.len(),
            roots.len(),
            "discover each allocation once"
        );
        assert_eq!(
            value_traces,
            roots.len(),
            "trace each live value once in this pass"
        );
        assert_eq!(crate::registered_ephemeron_root_count(), roots.len());
        assert!(
            representations
                .iter()
                .all(|&pointer| crate::ephemeron_root_handles(pointer) == 1)
        );
        drop(edges);
        drop(roots);
        assert_eq!(crate::registered_ephemeron_root_count(), 0);
    });
}

fn heap_pointer<K: Trace + ?Sized, V: Trace>(
    edge: &crate::EphemeronEdge<K, V>,
) -> crate::EphemeronPointer {
    let pointer = edge.erased_inner_ptr();
    crate::BOA_GC.with(|gc| {
        let gc = gc.borrow();
        gc.young_weaks
            .iter()
            .chain(&gc.old_weaks)
            .copied()
            .find(|entry| std::ptr::addr_eq(entry.as_ptr(), pointer.as_ptr()))
            .expect("the retained ephemeron belongs to the collector")
    })
}

#[test]
fn set_operations_and_last_root_removal_accept_both_coercion_paths() {
    run_test(|| {
        let key = Rooted::new(11_u32);
        let edge = Ephemeron::new(&key, 17_u32).into_edge();
        let allocation = heap_pointer(&edge);
        let from_edge = edge.erased_inner_ptr();
        let mut set = crate::EphemeronSet::new();
        assert!(set.insert(allocation));
        assert!(!set.insert(from_edge));
        assert!(set.contains(&from_edge));
        assert!(set.remove(&from_edge));
        assert!(set.is_empty());
        assert!(set.insert(from_edge));
        assert!(std::ptr::eq(
            set.iter().next().unwrap().as_ptr(),
            from_edge.as_ptr()
        ));
        set.retain(|stored| std::ptr::addr_eq(stored.as_ptr(), allocation.as_ptr()));
        assert_eq!(set.len(), 1);
        assert!(set.remove(&allocation));
        set.extend([allocation, from_edge]);
        assert_eq!(set.len(), 1);
        set.clear();
        assert!(set.is_empty());
        let collected: crate::EphemeronSet = [from_edge, allocation].into_iter().collect();
        assert_eq!(collected.len(), 1);
        assert!(std::ptr::eq(
            collected.iter().next().unwrap().as_ptr(),
            from_edge.as_ptr()
        ));

        assert_eq!(crate::registered_ephemeron_root_count(), 0);
        crate::register_ephemeron_root(allocation);
        assert_eq!(crate::ephemeron_root_handles(from_edge), 1);
        crate::EPHEMERON_ROOT_REGISTRY.with(|registry| {
            assert_eq!(registry.borrow().len(), 1);
            assert!(registry.borrow().contains(&from_edge));
        });
        crate::unregister_ephemeron_root(from_edge);
        assert_eq!(crate::registered_ephemeron_root_count(), 0);
        crate::EPHEMERON_ROOT_REGISTRY.with(|registry| assert!(registry.borrow().is_empty()));
        assert_eq!(crate::ephemeron_root_handles(allocation), 0);
    });
}

#[test]
fn swept_addresses_leave_no_registry_or_remembered_entries_before_reuse() {
    run_test(|| {
        let mut previous = PointerSet::new();
        let mut reused = 0;
        for _ in 0..32 {
            let key = Rooted::new(19_u32);
            let root = Ephemeron::new(&key, 23_u32);
            let edge = root.clone().into_edge();
            let allocation = heap_pointer(&edge);
            reused += usize::from(!previous.insert(allocation.cast::<()>()));
            crate::force_minor_collect();
            crate::force_minor_collect();
            crate::REMEMBERED_EPHEMERONS.with(|remembered| {
                assert!(remembered.borrow().contains(&allocation));
                assert!(remembered.borrow().contains(&edge.erased_inner_ptr()));
            });
            drop(key);
            crate::force_collect();
            assert!(
                !root.has_value(),
                "dead key must clear the value after major mark"
            );
            drop(edge);
            drop(root);
            crate::force_collect();
            // These comparisons never dereference the now-swept pointer. Check
            // removal before the next iteration may reuse the allocation address.
            crate::EPHEMERON_ROOT_REGISTRY.with(|registry| assert!(registry.borrow().is_empty()));
            crate::REMEMBERED_EPHEMERONS.with(|remembered| {
                assert!(!remembered.borrow().contains(&allocation));
            });
            crate::BOA_GC.with(|gc| {
                let gc = gc.borrow();
                assert!(gc.young_weaks.is_empty());
                assert!(gc.old_weaks.is_empty());
            });
            assert_eq!(crate::registered_ephemeron_root_count(), 0);
        }
        eprintln!("EPHEMERON_REUSE iterations=32 observed_reused_addresses={reused}");
    });
}
