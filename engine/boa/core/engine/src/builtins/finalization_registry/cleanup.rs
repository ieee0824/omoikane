//! Cleanup is scheduled at host job drains, never from inside the collector.
use super::FinalizationRegistry;
use crate::{
    Context, JsResult, JsValue,
    job::GenericJob,
    object::{JsObject, RootedJsObject},
};

/// Keeps cell indices stable while a cleanup callback can mutate the registry.
struct CleanupPass<'a> {
    registry: &'a JsObject<FinalizationRegistry>,
}
impl<'a> CleanupPass<'a> {
    fn new(registry: &'a JsObject<FinalizationRegistry>) -> Self {
        registry.borrow_mut().data_mut().cleanup_passes += 1;
        Self { registry }
    }
}
impl Drop for CleanupPass<'_> {
    fn drop(&mut self) {
        let mut object = self.registry.borrow_mut();
        let state = object.data_mut();
        state.cleanup_passes -= 1;
        if state.cleanup_passes == 0 {
            state.cells.retain(Option::is_some);
        }
    }
}

fn take_next_matching<T>(
    slots: &mut [Option<T>],
    cursor: &mut usize,
    mut predicate: impl FnMut(&T) -> bool,
) -> Option<T> {
    while let Some(slot) = slots.get_mut(*cursor) {
        *cursor += 1;
        if slot.as_ref().is_some_and(&mut predicate) {
            return slot.take();
        }
    }
    None
}

/// A host-owned job guard also resets scheduling when an executor discards a job.
struct PendingCleanup {
    registry: RootedJsObject<FinalizationRegistry>,
}
impl PendingCleanup {
    fn run(self, context: &mut Context) -> JsResult<JsValue> {
        FinalizationRegistry::cleanup(&self.registry.to_edge(), context)
    }
}
impl Drop for PendingCleanup {
    fn drop(&mut self) {
        self.registry.borrow_mut().data_mut().cleanup_scheduled = false;
    }
}

impl FinalizationRegistry {
    pub(crate) fn schedule(object: JsObject, context: &mut Context) {
        let Ok(registry) = object.downcast::<Self>() else {
            return;
        };
        let registry_root = registry.clone().root();
        let realm = {
            let mut object = registry.borrow_mut();
            let state = object.data_mut();
            if state.cleanup_scheduled
                || !state
                    .cells
                    .iter()
                    .flatten()
                    .any(|cell| !cell.target.is_alive())
            {
                return;
            }
            state.cleanup_scheduled = true;
            state.realm.to_rooted()
        };
        let pending = PendingCleanup {
            registry: registry_root,
        };
        context.enqueue_job(GenericJob::new(move |context| pending.run(context), realm).into());
    }
    fn cleanup(registry: &JsObject<Self>, context: &mut Context) -> JsResult<JsValue> {
        let callback = registry.borrow().data().callback.clone().root();
        let _cleanup_pass = CleanupPass::new(registry);
        let mut cursor = 0;
        loop {
            let holding = {
                let mut object = registry.borrow_mut();
                let state = object.data_mut();
                take_next_matching(&mut state.cells, &mut cursor, |cell| {
                    !cell.target.is_alive()
                })
                .map(|mut cell| std::mem::replace(&mut cell.holding, JsValue::undefined()))
            };
            let Some(holding) = holding else {
                return Ok(JsValue::undefined());
            };
            let _holding_root = holding.as_object().map(JsObject::root);
            callback.call(&JsValue::undefined(), &[holding], context)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::take_next_matching;

    #[test]
    fn cleanup_scan_is_linear_and_reuses_cell_storage() {
        const CELLS: usize = 4096;
        let mut slots: Vec<_> = (0..CELLS).map(Some).collect();
        let original_capacity = slots.capacity();
        let mut cursor = 0;
        let mut predicate_calls = 0;
        let mut holdings = Vec::with_capacity(CELLS / 2);

        while let Some(holding) = take_next_matching(&mut slots, &mut cursor, |holding| {
            predicate_calls += 1;
            *holding >= CELLS / 2
        }) {
            holdings.push(holding);
        }

        assert_eq!(predicate_calls, CELLS);
        assert_eq!(holdings, (CELLS / 2..CELLS).collect::<Vec<_>>());
        assert_eq!(slots.len(), CELLS);
        assert_eq!(slots.capacity(), original_capacity);

        slots.retain(Option::is_some);
        assert_eq!(slots.len(), CELLS / 2);
        assert_eq!(slots.capacity(), original_capacity);
    }

    #[test]
    fn cleanup_scan_keeps_order_across_reentrant_mutation() {
        let mut slots = vec![Some("first"), Some("unregistered"), Some("third")];
        let mut cursor = 0;

        assert_eq!(
            take_next_matching(&mut slots, &mut cursor, |_| true),
            Some("first")
        );
        slots[1] = None;
        slots.push(Some("registered"));

        assert_eq!(
            take_next_matching(&mut slots, &mut cursor, |_| true),
            Some("third")
        );
        assert_eq!(
            take_next_matching(&mut slots, &mut cursor, |_| true),
            Some("registered")
        );
        assert_eq!(take_next_matching(&mut slots, &mut cursor, |_| true), None);
    }
}
