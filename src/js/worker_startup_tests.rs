//! Rejected Dedicated Worker endpoints: runtime-free state and existing delivery rules.
use super::*;

fn runtime_with_denied_workers() -> JsRuntime {
    let mut runtime = JsRuntime::with_document_and_url(
        crate::html::TreeBuilder::parse("<html><body></body></html>").document(),
        "https://worker-startup.test/page",
    )
    .unwrap();
    runtime.install_csp_policy(&["worker-src 'none'".into()]);
    runtime
}

fn id_value(id: u64) -> JsValue {
    JsValue::from(JsString::from(id.to_string()))
}

fn create_unbound_failure(runtime: &mut JsRuntime, document_id: usize) -> u64 {
    let owner = Rc::clone(&runtime.host_state);
    runtime
        .with_active_host(|_| {
            create_worker_for_owner_state(owner, "/blocked.js", document_id, None)
        })
        .unwrap()
}

fn assert_pending(runtime: &JsRuntime, id: u64, document_id: usize) {
    let state = runtime.host_state.borrow();
    assert!(
        !state.workers.contains_key(&id),
        "a rejected endpoint needs no runtime"
    );
    assert_eq!(
        state
            .failed_worker_startups
            .get(&id)
            .map(|entry| entry.owner_document_id),
        Some(document_id)
    );
}

fn terminate_native(runtime: &mut JsRuntime, id: u64) {
    runtime
        .with_active_host(|context| {
            terminate_worker_native(&JsValue::undefined(), &[id_value(id)], context)
        })
        .unwrap();
}

fn assert_no_worker_state(runtime: &JsRuntime) {
    let state = runtime.host_state.borrow();
    assert!(state.workers.is_empty());
    assert!(state.failed_worker_startups.is_empty());
    assert!(state.worker_owner_objects.is_empty());
}

#[test]
fn rejected_load_has_only_an_owned_marker_before_owner_binding() {
    let mut runtime = runtime_with_denied_workers();
    let document_id = runtime.document().identity();
    let id = create_unbound_failure(&mut runtime, document_id);
    assert_pending(&runtime, id, document_id);
    terminate_native(&mut runtime, id);
    assert_no_worker_state(&runtime);
}

#[test]
fn binding_consumes_the_marker_and_reports_load_failure_once() {
    let mut runtime = runtime_with_denied_workers();
    let document_id = runtime.document().identity();
    let id = create_unbound_failure(&mut runtime, document_id);
    assert_pending(&runtime, id, document_id);
    runtime
        .with_active_host(|context| {
            let owner = context.eval(Source::from_bytes(
                "globalThis.failures=0; globalThis.failureOwner=new EventTarget(); \
                 failureOwner.__terminated=false; \
                 failureOwner.addEventListener('error',()=>failures++); failureOwner",
            ))?;
            let args = [id_value(id), owner];
            bind_worker_owner_native(&JsValue::undefined(), &args, context)?;
            bind_worker_owner_native(&JsValue::undefined(), &args, context)?;
            Ok(())
        })
        .unwrap();
    assert_no_worker_state(&runtime);
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.eval("failures").unwrap().as_number(), Some(1.0));
    assert!(runtime.take_task_errors().is_empty());
}

#[test]
fn explicit_termination_removes_an_unbound_marker_without_a_notification() {
    let mut runtime = runtime_with_denied_workers();
    let document_id = runtime.document().identity();
    let id = create_unbound_failure(&mut runtime, document_id);
    assert_pending(&runtime, id, document_id);
    terminate_native(&mut runtime, id);
    terminate_native(&mut runtime, id);
    assert_no_worker_state(&runtime);
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
}

#[test]
fn whole_global_termination_clears_unbound_markers() {
    let mut runtime = runtime_with_denied_workers();
    let document_id = runtime.document().identity();
    let id = create_unbound_failure(&mut runtime, document_id);
    assert_pending(&runtime, id, document_id);
    runtime.terminate_workers();
    assert_no_worker_state(&runtime);
}

#[test]
fn constructor_setter_exception_retains_a_scalar_marker_until_termination() {
    let mut runtime = runtime_with_denied_workers();
    runtime
        .eval(
            "globalThis.abandonedId=null; globalThis.constructorError=''; \
             class UnboundWorker extends Worker { \
               set __id(value) { abandonedId=value; throw new Error('bind aborted'); } \
             } \
             try { new UnboundWorker('/blocked.js'); } \
             catch(error) { constructorError=error.message; }",
        )
        .unwrap();
    assert_eq!(
        runtime
            .eval("constructorError")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "bind aborted"
    );
    let id = runtime
        .eval("abandonedId")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped()
        .parse::<u64>()
        .unwrap();
    assert_pending(&runtime, id, runtime.document().identity());
    terminate_native(&mut runtime, id);
    assert_no_worker_state(&runtime);
}

#[test]
fn an_author_id_getter_does_not_rebind_a_consumed_failure_to_another_owner() {
    let mut runtime = runtime_with_denied_workers();
    runtime
        .eval(
            "globalThis.allocated=[]; globalThis.firstErrors=0; globalThis.secondErrors=0; \
             class MisdirectedWorker extends Worker { \
               set __id(value) { allocated.push(value); } \
               get __id() { return allocated[0]; } \
             } \
             globalThis.first=new MisdirectedWorker('/blocked.js'); \
             first.onerror=()=>firstErrors++; \
             globalThis.second=new MisdirectedWorker('/blocked.js'); \
             second.onerror=()=>secondErrors++;",
        )
        .unwrap();
    let id = runtime
        .eval("allocated[1]")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped()
        .parse::<u64>()
        .unwrap();
    assert_pending(&runtime, id, runtime.document().identity());
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.eval("firstErrors").unwrap().as_number(), Some(1.0));
    assert_eq!(runtime.eval("secondErrors").unwrap().as_number(), Some(0.0));
    assert_no_worker_state(&runtime);
    terminate_native(&mut runtime, id);
    assert_no_worker_state(&runtime);
}

#[test]
fn failed_worker_post_message_keeps_clone_and_native_string_conversion_order() {
    let mut runtime = runtime_with_denied_workers();
    runtime
        .eval(
            "globalThis.calls=[]; globalThis.cloneError=''; \
             globalThis.failedWorker=new Worker('/blocked.js'); \
             try { failedWorker.postMessage(()=>{}); } \
             catch(error) { cloneError=error.name; }",
        )
        .unwrap();
    assert_eq!(
        runtime
            .eval("cloneError")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "DataCloneError"
    );
    runtime
        .with_active_host(|context| {
            let id = context.eval(Source::from_bytes("failedWorker.__id"))?;
            let data = boa_gc::Rooted::new(context.eval(Source::from_bytes(
                "({toString(){ calls.push('converted'); return 'null'; }})",
            ))?);
            worker_post_message_native(
                &JsValue::undefined(),
                &[id, (*data).clone(), JsValue::undefined()],
                context,
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        runtime
            .eval("calls.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "converted"
    );
    assert_no_worker_state(&runtime);
}

#[test]
fn document_tree_retirement_removes_its_unbound_failure_marker() {
    let mut runtime = runtime_with_denied_workers();
    let document = runtime.document();
    let id = create_unbound_failure(&mut runtime, document.identity());
    assert_pending(&runtime, id, document.identity());
    runtime
        .host_state
        .borrow_mut()
        .retire_document_tree(&document);
    assert_no_worker_state(&runtime);
}

#[test]
fn iframe_retirement_preserves_another_documents_failed_endpoint() {
    let mut runtime = runtime_with_denied_workers();
    runtime
        .eval("const frame=document.createElement('iframe'); document.body.appendChild(frame);")
        .unwrap();
    runtime.tick(0).unwrap();
    let (iframe_id, child_id) = {
        let state = runtime.host_state.borrow();
        let (id, frame) = state.iframe_documents.iter().next().unwrap();
        (*id, frame.document.identity())
    };
    let document_id = runtime.document().identity();
    let policy = runtime.host_state.borrow().document_csp[&document_id].clone();
    runtime
        .host_state
        .borrow_mut()
        .document_csp
        .insert(child_id, policy);
    let owner_id = create_unbound_failure(&mut runtime, document_id);
    let child_worker_id = create_unbound_failure(&mut runtime, child_id);
    assert_pending(&runtime, owner_id, document_id);
    assert_pending(&runtime, child_worker_id, child_id);
    runtime
        .host_state
        .borrow_mut()
        .retire_iframe_document(iframe_id);
    assert_pending(&runtime, owner_id, document_id);
    assert!(
        !runtime
            .host_state
            .borrow()
            .failed_worker_startups
            .contains_key(&child_worker_id)
    );
    terminate_native(&mut runtime, owner_id);
    assert_no_worker_state(&runtime);
}

#[test]
fn background_collection_prevents_late_binding_of_an_unbound_rejected_load() {
    let mut runtime = runtime_with_denied_workers();
    let document_id = runtime.document().identity();
    let id = create_unbound_failure(&mut runtime, document_id);
    {
        let state = runtime.host_state.borrow();
        assert_ne!(
            state.workers.contains_key(&id),
            state.failed_worker_startups.contains_key(&id)
        );
    }
    runtime.run_until_idle().unwrap();
    assert_no_worker_state(&runtime);
    runtime
        .with_active_host(|context| {
            let owner = context.eval(Source::from_bytes(
                "globalThis.lateErrors=0; globalThis.lateOwner=new EventTarget(); \
                 lateOwner.__terminated=false; \
                 lateOwner.addEventListener('error',()=>lateErrors++); lateOwner",
            ))?;
            bind_worker_owner_native(&JsValue::undefined(), &[id_value(id), owner], context)?;
            Ok(())
        })
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.eval("lateErrors").unwrap().as_number(), Some(0.0));
    assert_no_worker_state(&runtime);
}

/// A test observation never keeps the old HostState or its JS roots alive.
pub(crate) struct FailedStartupProbe {
    host: std::rc::Weak<RefCell<HostState>>,
    id: u64,
    document_id: usize,
}

/// Destruction and an observed empty live host are distinct outcomes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FailedStartupSnapshot {
    HostDropped,
    Alive {
        marker_document_id: Option<usize>,
        runtime_present: bool,
    },
}

impl FailedStartupProbe {
    pub(crate) fn document_id(&self) -> usize {
        self.document_id
    }

    pub(crate) fn snapshot(&self) -> FailedStartupSnapshot {
        let Some(host) = self.host.upgrade() else {
            return FailedStartupSnapshot::HostDropped;
        };
        let state = host.borrow();
        FailedStartupSnapshot::Alive {
            marker_document_id: state
                .failed_worker_startups
                .get(&self.id)
                .map(|entry| entry.owner_document_id),
            runtime_present: state.workers.contains_key(&self.id),
        }
    }
}

/// Creates one denied, unbound endpoint without HTTP or an owning host probe.
pub(crate) fn denied_data_worker_probe(runtime: &mut JsRuntime) -> FailedStartupProbe {
    runtime.install_csp_policy(&["worker-src 'none'".into()]);
    let document_id = runtime.document().identity();
    let host = Rc::clone(&runtime.host_state);
    let id = runtime
        .with_active_host(|_| {
            create_worker_for_owner_state(
                Rc::clone(&host),
                "data:text/javascript,void%200",
                document_id,
                None,
            )
        })
        .unwrap();
    assert_pending(runtime, id, document_id);
    FailedStartupProbe {
        host: Rc::downgrade(&host),
        id,
        document_id,
    }
}

pub(crate) fn failed_startup_count(runtime: &JsRuntime) -> usize {
    runtime.host_state.borrow().failed_worker_startups.len()
}

fn rejected_endpoint_after_rust_unwind(runtime: &mut JsRuntime) -> u64 {
    let mut outer = runtime_with_denied_workers();
    let outer_host = Rc::clone(&outer.host_state);
    let owner = Rc::clone(&runtime.host_state);
    let document_id = runtime.document().identity();
    let id = Cell::new(None);
    let previous = ACTIVE_HOST_STATE.with(|slot| slot.borrow().as_ref().map(Rc::as_ptr));
    outer
        .with_active_host(|_| {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                runtime.with_active_host(|_| -> JsResult<()> {
                    id.set(Some(create_worker_for_owner_state(
                        Rc::clone(&owner),
                        "/blocked.js",
                        document_id,
                        None,
                    )?));
                    std::panic::panic_any(0x1733_u64);
                })
            }));
            let payload = result.expect_err("the known Rust panic must escape the inner scope");
            assert_eq!(payload.downcast_ref::<u64>(), Some(&0x1733));
            ACTIVE_HOST_STATE.with(|slot| {
                assert!(Rc::ptr_eq(slot.borrow().as_ref().unwrap(), &outer_host));
            });
            Ok(())
        })
        .unwrap();
    assert_eq!(
        ACTIVE_HOST_STATE.with(|slot| slot.borrow().as_ref().map(Rc::as_ptr)),
        previous
    );
    let id = id
        .get()
        .expect("the rejected endpoint existed before unwinding");
    assert_pending(runtime, id, document_id);
    id
}

#[test]
fn rust_unwind_preserves_payload_and_outer_host_until_failed_endpoint_binding() {
    let mut runtime = runtime_with_denied_workers();
    let id = rejected_endpoint_after_rust_unwind(&mut runtime);
    runtime
        .with_active_host(|context| {
            let owner = context.eval(Source::from_bytes(
                "globalThis.unwindErrors=0; const owner=new EventTarget(); \
                 owner.__terminated=false; \
                 owner.addEventListener('error',()=>unwindErrors++); owner",
            ))?;
            bind_worker_owner_native(&JsValue::undefined(), &[id_value(id), owner], context)
        })
        .unwrap();
    assert_no_worker_state(&runtime);
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.eval("unwindErrors").unwrap().as_number(), Some(1.0));
    assert!(runtime.take_task_errors().is_empty());
}

#[test]
fn rust_unwind_leaves_unbound_failure_for_the_next_background_collection() {
    let mut runtime = runtime_with_denied_workers();
    let _id = rejected_endpoint_after_rust_unwind(&mut runtime);
    runtime.run_worker_background_tasks();
    assert_no_worker_state(&runtime);
    assert!(runtime.take_task_errors().is_empty());
}
