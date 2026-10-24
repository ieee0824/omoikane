//! Document ownership for suspendable classic-script evaluation.

use std::ops::{Deref, DerefMut};

use super::*;

pub(super) fn document_realm(runtime: &mut JsRuntime, document_id: usize) -> JsResult<Realm> {
    if document_id == runtime.document().identity() {
        return runtime
            .host_state
            .borrow()
            .main_realm
            .clone()
            .ok_or_else(|| {
                JsNativeError::reference()
                    .with_message("main document Realm is no longer live")
                    .into()
            });
    }
    runtime.realm_for_document(document_id)?.ok_or_else(|| {
        JsNativeError::reference()
            .with_message("script document Realm is no longer live")
            .into()
    })
}

/// A local exclusive view of the evaluation context. This borrow never escapes
/// its evaluation future; cancellation restores the caller's Realm as well.
pub(super) struct ContextScope<'a> {
    context: &'a mut Context,
    previous: Option<Realm>,
}

impl<'a> ContextScope<'a> {
    pub(super) fn new(context: &'a mut Context, realm: Option<Realm>) -> Self {
        let previous = realm.map(|realm| context.enter_realm(realm));
        Self { context, previous }
    }
}

impl Deref for ContextScope<'_> {
    type Target = Context;

    fn deref(&self) -> &Context {
        self.context
    }
}

impl DerefMut for ContextScope<'_> {
    fn deref_mut(&mut self) -> &mut Context {
        self.context
    }
}

impl Drop for ContextScope<'_> {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            self.context.enter_realm(previous);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child_runtime() -> (JsRuntime, usize) {
        let mut runtime = JsRuntime::with_document_and_url(
            crate::html::TreeBuilder::parse("<body><p id='parent'>parent</p><iframe></iframe>")
                .document(),
            "http://localhost/",
        )
        .unwrap();
        runtime
            .eval("globalThis.childWindow = document.querySelector('iframe').contentWindow")
            .unwrap();
        let document_id = runtime
            .host_state
            .borrow()
            .iframe_documents
            .values()
            .next()
            .unwrap()
            .document
            .identity();
        (runtime, document_id)
    }

    #[test]
    fn cancelling_child_evaluation_restores_parent_realm_and_environment() {
        let (mut runtime, document_id) = child_runtime();
        let parent = runtime.context.realm().clone();
        {
            let mut evaluation =
                Box::pin(runtime.eval_async_for_document(
                    "globalThis.childOnly = 1; while (true) {}",
                    document_id,
                ));
            let mut context = TaskContext::from_waker(std::task::Waker::noop());
            assert!(evaluation.as_mut().poll(&mut context).is_pending());
        }
        assert_eq!(runtime.context.realm(), &parent);
        assert_eq!(
            runtime
                .eval("[typeof childOnly, childWindow.childOnly, !!document.getElementById('parent')].join('|')")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "undefined|1|true"
        );
    }

    #[test]
    fn child_parse_failure_restores_parent_realm() {
        let (mut runtime, document_id) = child_runtime();
        let parent = runtime.context.realm().clone();
        {
            let mut evaluation =
                Box::pin(runtime.eval_async_for_document("const = ;", document_id));
            let mut context = TaskContext::from_waker(std::task::Waker::noop());
            assert!(matches!(
                evaluation.as_mut().poll(&mut context),
                Poll::Ready(Err(_))
            ));
        }
        assert_eq!(runtime.context.realm(), &parent);
        assert_eq!(
            runtime
                .eval("!!document.getElementById('parent')")
                .unwrap()
                .as_boolean(),
            Some(true)
        );
    }
}
