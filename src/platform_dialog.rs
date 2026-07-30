//! Toolkit-independent bridge between Window modal dialogs and a frontend.

use crate::js::JavaScriptDialogRequest;

/// Why a frontend should stop presenting a dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JavaScriptDialogCloseReason {
    /// Another frontend (for example CDP) already resolved the shared request.
    ResolvedExternally,
    /// The frontend was detached or its owning window was closed.
    HostDetached,
}

/// Platform-neutral callbacks implemented by a GUI frontend.
pub trait JavaScriptDialogAdapter {
    fn open_dialog(&mut self, request: JavaScriptDialogRequest);

    fn close_dialog(
        &mut self,
        request: &JavaScriptDialogRequest,
        reason: JavaScriptDialogCloseReason,
    );
}

/// Synchronizes one runtime's dialog state with an optional GUI adapter.
///
/// Constructing this host is opt-in. Headless users can use dialog requests
/// directly without installing an adapter or implicitly requesting UI. The
/// current request is supplied on each synchronization so the browser can
/// replace its JavaScript runtime during navigation without replacing the
/// platform adapter.
pub struct JavaScriptDialogAdapterHost<A: JavaScriptDialogAdapter> {
    adapter: A,
    active: Option<JavaScriptDialogRequest>,
    attached: bool,
}

impl<A: JavaScriptDialogAdapter> JavaScriptDialogAdapterHost<A> {
    pub fn new(adapter: A) -> Self {
        Self {
            adapter,
            active: None,
            attached: true,
        }
    }

    pub fn adapter(&self) -> &A {
        &self.adapter
    }

    pub fn adapter_mut(&mut self) -> &mut A {
        &mut self.adapter
    }

    /// Propagates newly opened dialogs and externally completed requests.
    /// Frontend event loops should call this after driving page script or CDP.
    pub fn synchronize(&mut self, pending: Option<JavaScriptDialogRequest>) {
        if !self.attached {
            return;
        }

        if let Some(active) = self.active.as_ref() {
            let remains_pending = pending
                .as_ref()
                .is_some_and(|request| request.same_request(active));
            if !remains_pending {
                let active = self.active.take().expect("active request exists");
                self.adapter
                    .close_dialog(&active, JavaScriptDialogCloseReason::ResolvedExternally);
            }
        }

        if self.active.is_none() {
            if let Some(request) = pending {
                self.active = Some(request.clone());
                self.adapter.open_dialog(request);

                // An adapter may resolve immediately from `open_dialog`.
                if self
                    .active
                    .as_ref()
                    .is_some_and(|active| !active.is_pending())
                {
                    let active = self.active.take().expect("active request exists");
                    self.adapter
                        .close_dialog(&active, JavaScriptDialogCloseReason::ResolvedExternally);
                }
            }
        }
    }

    /// Detaches the frontend, dismissing a blocking request before its window
    /// or toolkit objects disappear. Calling this more than once is harmless.
    pub fn detach(&mut self) {
        if !self.attached {
            return;
        }
        self.attached = false;
        if let Some(active) = self.active.take() {
            let _ = active.dismiss();
            self.adapter
                .close_dialog(&active, JavaScriptDialogCloseReason::HostDetached);
        }
    }
}

impl<A: JavaScriptDialogAdapter> Drop for JavaScriptDialogAdapterHost<A> {
    fn drop(&mut self) {
        self.detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::js::{JavaScriptDialogError, JavaScriptDialogKind, JsRuntime};
    use boa_engine::{JsResult, JsValue};
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};

    #[derive(Default)]
    struct FakeAdapter {
        opened: Vec<JavaScriptDialogRequest>,
        closed: Vec<(u64, JavaScriptDialogCloseReason)>,
    }

    impl JavaScriptDialogAdapter for FakeAdapter {
        fn open_dialog(&mut self, request: JavaScriptDialogRequest) {
            self.opened.push(request);
        }

        fn close_dialog(
            &mut self,
            request: &JavaScriptDialogRequest,
            reason: JavaScriptDialogCloseReason,
        ) {
            self.closed.push((request.dialog().id, reason));
        }
    }

    #[derive(Default)]
    struct ImmediatelyDismissingAdapter {
        opened: usize,
        closed: usize,
    }

    impl JavaScriptDialogAdapter for ImmediatelyDismissingAdapter {
        fn open_dialog(&mut self, request: JavaScriptDialogRequest) {
            self.opened += 1;
            request.dismiss().unwrap();
        }

        fn close_dialog(
            &mut self,
            _request: &JavaScriptDialogRequest,
            _reason: JavaScriptDialogCloseReason,
        ) {
            self.closed += 1;
        }
    }

    fn poll_once<F: Future<Output = JsResult<JsValue>>>(
        future: Pin<&mut F>,
    ) -> Poll<JsResult<JsValue>> {
        let waker: &'static Waker = Waker::noop();
        future.poll(&mut Context::from_waker(waker))
    }

    fn ready(poll: Poll<JsResult<JsValue>>, message: &str) -> JsResult<JsValue> {
        match poll {
            Poll::Ready(result) => result,
            Poll::Pending => panic!("{message}"),
        }
    }

    #[test]
    fn fake_adapter_observes_metadata_and_resolves_end_to_end() {
        let mut runtime = JsRuntime::new().unwrap();
        let controller = runtime.javascript_dialog_controller();
        let mut host = JavaScriptDialogAdapterHost::new(FakeAdapter::default());
        let mut evaluation = Box::pin(runtime.eval_async("prompt('Name', 'Ada')"));
        assert!(poll_once(evaluation.as_mut()).is_pending());

        host.synchronize(controller.pending_request());
        let request = host.adapter().opened[0].clone();
        assert_eq!(request.dialog().kind, JavaScriptDialogKind::Prompt);
        assert_eq!(request.dialog().message, "Name");
        assert_eq!(request.dialog().default_prompt.as_deref(), Some("Ada"));
        request.resolve(true, Some("Grace".into())).unwrap();

        let result = ready(poll_once(evaluation.as_mut()), "evaluation should resume").unwrap();
        assert_eq!(result.as_string().unwrap().to_std_string_escaped(), "Grace");
        host.synchronize(controller.pending_request());
        assert_eq!(
            host.adapter().closed,
            vec![(
                request.dialog().id,
                JavaScriptDialogCloseReason::ResolvedExternally
            )]
        );
    }

    #[test]
    fn adapter_and_external_controller_cannot_both_resolve_request() {
        let mut runtime = JsRuntime::new().unwrap();
        let controller = runtime.javascript_dialog_controller();
        let mut host = JavaScriptDialogAdapterHost::new(FakeAdapter::default());
        let mut evaluation = Box::pin(runtime.eval_async("confirm('Continue?')"));
        assert!(poll_once(evaluation.as_mut()).is_pending());
        host.synchronize(controller.pending_request());
        let request = host.adapter().opened[0].clone();

        controller.handle(request.dialog().id, true, None).unwrap();
        assert_eq!(
            request.dismiss(),
            Err(JavaScriptDialogError::NoPendingDialog)
        );
        host.synchronize(controller.pending_request());
        assert_eq!(host.adapter().opened.len(), 1);
        assert!(poll_once(evaluation.as_mut()).is_ready());
    }

    #[test]
    fn resolution_inside_open_callback_closes_exactly_once() {
        let mut runtime = JsRuntime::new().unwrap();
        let controller = runtime.javascript_dialog_controller();
        let mut host = JavaScriptDialogAdapterHost::new(ImmediatelyDismissingAdapter::default());
        let mut evaluation = Box::pin(runtime.eval_async("confirm('Immediate')"));
        assert!(poll_once(evaluation.as_mut()).is_pending());

        host.synchronize(controller.pending_request());
        assert_eq!(host.adapter().opened, 1);
        assert_eq!(host.adapter().closed, 1);
        host.synchronize(controller.pending_request());
        assert_eq!(host.adapter().closed, 1);
        let result = ready(poll_once(evaluation.as_mut()), "dismiss should resume").unwrap();
        assert_eq!(result.as_boolean(), Some(false));
    }

    #[test]
    fn runtime_replacement_closes_old_numeric_id_before_opening_new_one() {
        let mut first_runtime = JsRuntime::new().unwrap();
        let first_controller = first_runtime.javascript_dialog_controller();
        let mut second_runtime = JsRuntime::new().unwrap();
        let second_controller = second_runtime.javascript_dialog_controller();
        let mut host = JavaScriptDialogAdapterHost::new(FakeAdapter::default());

        let mut first_evaluation = Box::pin(first_runtime.eval_async("alert('First')"));
        assert!(poll_once(first_evaluation.as_mut()).is_pending());
        let first_request = first_controller.pending_request().unwrap();
        host.synchronize(Some(first_request.clone()));
        drop(first_evaluation);

        let mut second_evaluation = Box::pin(second_runtime.eval_async("alert('Second')"));
        assert!(poll_once(second_evaluation.as_mut()).is_pending());
        let second_request = second_controller.pending_request().unwrap();
        assert_eq!(first_request.dialog().id, 1);
        assert_eq!(second_request.dialog().id, 1);
        assert_ne!(
            first_request.runtime_identity(),
            second_request.runtime_identity()
        );

        host.synchronize(Some(second_request.clone()));
        assert_eq!(host.adapter().opened.len(), 2);
        assert_eq!(host.adapter().opened[1].dialog().message, "Second");
        assert_eq!(
            host.adapter().closed,
            vec![(1, JavaScriptDialogCloseReason::ResolvedExternally)]
        );

        second_request.resolve(true, None).unwrap();
        assert!(poll_once(second_evaluation.as_mut()).is_ready());
    }

    #[test]
    fn detach_dismisses_pending_script_and_is_idempotent() {
        let mut runtime = JsRuntime::new().unwrap();
        let controller = runtime.javascript_dialog_controller();
        let mut host = JavaScriptDialogAdapterHost::new(FakeAdapter::default());
        let mut evaluation = Box::pin(runtime.eval_async("confirm('Close?')"));
        assert!(poll_once(evaluation.as_mut()).is_pending());
        host.synchronize(controller.pending_request());
        let id = host.adapter().opened[0].dialog().id;

        host.detach();
        host.detach();
        assert_eq!(
            host.adapter().closed,
            vec![(id, JavaScriptDialogCloseReason::HostDetached)]
        );
        let result = ready(poll_once(evaluation.as_mut()), "dismiss should resume").unwrap();
        assert_eq!(result.as_boolean(), Some(false));
    }

    #[test]
    fn drop_does_not_claim_a_request_that_was_never_synchronized() {
        let mut runtime = JsRuntime::new().unwrap();
        let controller = runtime.javascript_dialog_controller();
        let mut evaluation = Box::pin(runtime.eval_async("prompt('Close')"));
        assert!(poll_once(evaluation.as_mut()).is_pending());
        {
            let _host = JavaScriptDialogAdapterHost::new(FakeAdapter::default());
        }
        assert!(controller.pending_request().is_some());
        controller.pending_request().unwrap().dismiss().unwrap();
        let result = ready(
            poll_once(evaluation.as_mut()),
            "explicit dismiss should resume",
        )
        .unwrap();
        assert!(result.is_null());
    }

    #[test]
    fn requests_include_distinct_runtime_identity() {
        let first = JsRuntime::new().unwrap();
        let second = JsRuntime::new().unwrap();
        assert_ne!(
            first.javascript_dialog_controller().runtime_identity(),
            second.javascript_dialog_controller().runtime_identity()
        );
    }
}
