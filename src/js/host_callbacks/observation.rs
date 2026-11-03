//! Test-only observation of successful host calls, isolated by thread and scope.
use super::*;

type Calls = Rc<RefCell<Vec<(usize, &'static str, u32)>>>;
thread_local! {
    static OBSERVER: RefCell<Option<Calls>> = const { RefCell::new(None) };
}

struct Restore(Option<Calls>);
impl Drop for Restore {
    fn drop(&mut self) {
        OBSERVER.with(|slot| slot.replace(self.0.take()));
    }
}

pub(super) fn record(document_id: usize, caller: &'static std::panic::Location<'static>) {
    OBSERVER.with(|slot| {
        if let Some(calls) = slot.borrow().as_ref() {
            calls
                .borrow_mut()
                .push((document_id, caller.file(), caller.line()));
        }
    });
}

impl JsRuntime {
    pub(crate) fn observe_stylesheet_host_flushes<T>(
        run: impl FnOnce() -> T,
    ) -> (T, Vec<(usize, &'static str, u32)>) {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let previous = OBSERVER.with(|slot| slot.replace(Some(calls.clone())));
        let restore = Restore(previous);
        let result = run();
        drop(restore);
        let rows = std::mem::take(&mut *calls.borrow_mut());
        (result, rows)
    }
}

#[test]
fn nested_and_panicking_observers_restore_the_previous_scope() {
    let (_, outer) = JsRuntime::observe_stylesheet_host_flushes(|| {
        record(1, std::panic::Location::caller());
        let (_, inner) = JsRuntime::observe_stylesheet_host_flushes(|| {
            record(2, std::panic::Location::caller());
        });
        assert_eq!(inner.iter().map(|row| row.0).collect::<Vec<_>>(), vec![2]);
        let panic = std::panic::catch_unwind(|| {
            JsRuntime::observe_stylesheet_host_flushes(|| panic!("observer fixture"));
        });
        assert!(panic.is_err());
        record(3, std::panic::Location::caller());
    });
    assert_eq!(
        outer.iter().map(|row| row.0).collect::<Vec<_>>(),
        vec![1, 3]
    );
    OBSERVER.with(|slot| assert!(slot.borrow().is_none()));
}
