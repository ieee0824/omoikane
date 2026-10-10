//! Private, Realm-scoped delivery of Boa promise rejection tracker changes.
use super::*;
#[cfg(test)]
use boa_gc::Rooted;

/// Owned UTF-16 error fields that can cross a Worker runtime boundary without
/// retaining the thrown object or its Realm.
#[derive(Debug, Clone, Default)]
pub(crate) struct WorkerErrorReport {
    pub(super) message: JsString,
    pub(super) filename: JsString,
    pub(super) line: u32,
    pub(super) column: u32,
}

impl From<String> for WorkerErrorReport {
    fn from(message: String) -> Self {
        Self {
            message: JsString::from(message),
            ..Self::default()
        }
    }
}

/// Distinguishes script execution exceptions from failure to load a Worker.
#[derive(Debug, Clone)]
pub(crate) enum WorkerErrorNotification {
    Exception(WorkerErrorReport),
    LoadFailure,
}

impl From<WorkerErrorReport> for WorkerErrorNotification {
    fn from(report: WorkerErrorReport) -> Self {
        Self::Exception(report)
    }
}

impl From<String> for WorkerErrorNotification {
    fn from(message: String) -> Self {
        Self::Exception(message.into())
    }
}

/// The stage reached by an initial classic Worker script.
pub(super) enum WorkerScriptOutcome {
    Completed,
    ParseFailure,
    Exception(JsError),
}

/// Keeps parse failures distinct from runtime-thrown SyntaxError values.
/// Host execution aborts propagate without becoming author error events.
pub(super) fn evaluate_worker_initial_script(
    context: &mut Context,
    source: &str,
    url: &str,
) -> JsResult<WorkerScriptOutcome> {
    let input = Source::from_reader(source.as_bytes(), Some(Path::new(url)));
    let script = match Script::parse(input, None, context) {
        Ok(script) => script,
        Err(error) => {
            if error
                .as_native()
                .is_some_and(|error| error.is_runtime_limit())
            {
                return Err(error);
            }
            return Ok(WorkerScriptOutcome::ParseFailure);
        }
    };
    match script.evaluate(context) {
        Ok(_) => Ok(WorkerScriptOutcome::Completed),
        Err(error) => {
            if error
                .as_native()
                .is_some_and(|error| error.is_runtime_limit())
            {
                return Err(error);
            }
            Ok(WorkerScriptOutcome::Exception(error))
        }
    }
}

struct ExceptionReport {
    not_canceled: bool,
    details: WorkerErrorReport,
}

/// Fetch provenance owned by a classic script, including its later callbacks.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub(super) struct ClassicScriptMetadata {
    pub(super) muted_errors: bool,
}

/// Builds the single provenance snapshot used by active and unwound scripts.
pub(super) fn classic_script_metadata(muted_errors: bool) -> boa_engine::HostDefined {
    let mut metadata = boa_engine::HostDefined::default();
    metadata.insert(boa_engine::error::ScriptErrorMetadata::new(
        ClassicScriptMetadata { muted_errors },
    ));
    metadata
}

pub(super) fn active_script_errors_are_muted(context: &Context) -> bool {
    context.active_script().is_some_and(|script| {
        script
            .host_defined()
            .get::<boa_engine::error::ScriptErrorMetadata>()
            .and_then(|metadata| metadata.get::<ClassicScriptMetadata>())
            .is_some_and(|metadata| metadata.muted_errors)
    })
}

/// Reads the immutable security origin, including inherited worker/srcdoc origins.
pub(super) fn document_fetch_origin(state: &HostState, document: usize) -> CorsOrigin {
    match state.document_security_origins.get(&document) {
        Some(DocumentSecurityOrigin::Tuple(origin)) => origin
            .serialize()
            .parse::<crate::http::Url>()
            .ok()
            .map(|url| CorsOrigin::from_url(&url))
            .unwrap_or_else(CorsOrigin::opaque),
        _ => CorsOrigin::opaque(),
    }
}

/// Fetches a classic script without discarding its CORS response provenance.
pub(super) fn fetch_classic_source(
    src: &str,
    base: Option<&crate::http::Url>,
    origin: &CorsOrigin,
    crossorigin: Option<&str>,
    client: &mut Client,
) -> Option<(String, String, usize, bool)> {
    let resource = resolve_resource_ref(src, base)?;
    let ResolvedResource::Url(url) = resource else {
        return fetch_script_resource_with_client(src, base, client)
            .map(|(url, source, redirects)| (url, source, redirects, false));
    };
    let url = url.parse::<crate::http::Url>().ok()?;
    let mut request = HttpRequest::new(Method::Get, url.clone());
    if requires_public_fetch(&url, base) {
        request.require_public_ip();
    }
    if let Some(site) = base {
        request.set_cookie_context(site.clone(), false);
    }
    let mode = if crossorigin.is_some() {
        RequestMode::Cors
    } else {
        RequestMode::NoCors
    };
    let credentials = match crossorigin {
        Some(value) if value.eq_ignore_ascii_case("use-credentials") => CredentialsMode::Include,
        Some(_) => CredentialsMode::SameOrigin,
        None => CredentialsMode::Include,
    };
    let fetched = crate::http::cors::fetch(
        client,
        request,
        origin,
        mode,
        credentials,
        RedirectMode::Follow,
        &mut PreflightCache::default(),
    )
    .ok()?;
    let muted = fetched.response_type == ResponseType::Opaque;
    let response = fetched.response;
    if response.status_code() != 200 {
        client.report_resource_failure();
        return None;
    }
    let filename = response
        .effective_url()
        .map(ToString::to_string)
        .unwrap_or_else(|| url.to_string());
    let source = match std::str::from_utf8(response.body()) {
        Ok(source) => source.to_owned(),
        Err(_) => {
            client.report_resource_failure();
            return None;
        }
    };
    Some((filename, source, response.redirect_count(), muted))
}

struct PromiseReport {
    promise: JsObject,
    operation: OperationType,
    document: usize,
    delivered: bool,
    sequence: u64,
}

struct PromiseCheckpoint {
    pending: Vec<PromiseReport>,
    handled_rejections: HashMap<JsObject, u64>,
    reporters: HashMap<usize, (JsObject, Realm)>,
}

impl Finalize for PromiseCheckpoint {}

unsafe impl Trace for PromiseCheckpoint {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for report in &self.pending {
            unsafe { report.promise.trace(tracer) };
        }
        for promise in self.handled_rejections.keys() {
            unsafe { promise.trace(tracer) };
        }
        for (callback, _) in self.reporters.values() {
            unsafe { callback.trace(tracer) };
        }
    }

    fn run_finalizer(&self) {}
}

#[derive(Default)]
pub(super) struct PromiseReports {
    pending: Vec<PromiseReport>,
    handled_rejections: HashMap<JsObject, u64>,
    next_sequence: u64,
    reporters: HashMap<usize, (JsObject, Realm)>,
    error_reporters: HashMap<usize, (JsObject, Realm)>,
}

impl PromiseReports {
    fn mark_handled(&mut self, promise: &JsObject) {
        let cutoff = self.next_sequence;
        self.handled_rejections
            .entry(promise.clone())
            .and_modify(|existing| *existing = (*existing).max(cutoff))
            .or_insert(cutoff);
    }

    fn push(
        &mut self,
        promise: JsObject,
        operation: OperationType,
        document: usize,
        delivered: bool,
    ) {
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .expect("promise report sequence exhausted before checkpoint");
        self.pending.push(PromiseReport {
            promise,
            operation,
            document,
            delivered,
            sequence,
        });
    }

    fn take_checkpoint(&mut self) -> (Vec<PromiseReport>, HashMap<JsObject, u64>) {
        let pending = std::mem::take(&mut self.pending);
        let handled_rejections = std::mem::take(&mut self.handled_rejections);
        self.next_sequence = 0;
        (pending, handled_rejections)
    }

    /// Releases notification callbacks when their document stops being active.
    /// Retained DOM wrappers must not keep browser notification roots alive.
    pub(super) fn retire_document(&mut self, document: usize) {
        self.reporters.remove(&document);
        self.error_reporters.remove(&document);
        self.pending.retain(|report| report.document != document);

        // A handler may belong to the retired document while suppressing a
        // rejection owned by another document. Preserve only cutoffs that
        // still suppress at least one retained rejection.
        let old_cutoffs = std::mem::take(&mut self.handled_rejections);
        for report in &self.pending {
            if report.operation != OperationType::Reject {
                continue;
            }
            let Some(&cutoff) = old_cutoffs.get(&report.promise) else {
                continue;
            };
            if report.sequence < cutoff {
                self.handled_rejections
                    .entry(report.promise.clone())
                    .and_modify(|existing| *existing = (*existing).max(cutoff))
                    .or_insert(cutoff);
            }
        }
    }

    pub(super) unsafe fn trace(&self, tracer: &mut Tracer) {
        for report in &self.pending {
            unsafe { report.promise.trace(tracer) };
        }
        for promise in self.handled_rejections.keys() {
            unsafe { promise.trace(tracer) };
        }
        for (callback, _) in self.reporters.values().chain(self.error_reporters.values()) {
            unsafe { callback.trace(tracer) };
        }
    }
}

fn rejection_was_handled(
    report: &PromiseReport,
    handled_rejections: &HashMap<JsObject, u64>,
) -> bool {
    report.operation == OperationType::Reject
        && handled_rejections
            .get(&report.promise)
            .is_some_and(|cutoff| report.sequence < *cutoff)
}

pub(super) fn track(promise: &JsObject, operation: OperationType, context: &mut Context) {
    // HTML HostPromiseRejectionTracker suppresses both operations according to
    // the running classic script, including handlers on another script's promise.
    if active_script_errors_are_muted(context) {
        return;
    }
    let reporter = if operation == OperationType::Handle {
        with_host_state(|host| {
            let mut state = host.borrow_mut();
            let document = context_document_id(context, &state);
            // A rejection not yet delivered at a checkpoint is no longer a
            // candidate, including when a parent Realm installs the handler.
            state.promise_reports.mark_handled(promise);
            Ok(state.promise_reports.reporters.get(&document).cloned())
        })
        .ok()
        .flatten()
    } else {
        None
    };
    // Handle notifications enqueue their task when the handler is attached,
    // before subsequent author tasks, rather than at the next checkpoint.
    let delivered = if operation == OperationType::Handle {
        reporter.is_some_and(|(callback, realm)| {
            let reason = match JsPromise::from_object(promise.clone()).map(|p| p.state()) {
                Ok(PromiseState::Rejected(reason)) => reason,
                _ => return false,
            };
            invoke(
                context,
                &callback,
                &realm,
                &[JsValue::from(1), promise.clone().into(), reason],
            )
            .is_ok()
        })
    } else {
        false
    };
    let _ = with_host_state(|host| {
        let mut state = host.borrow_mut();
        let document = context_document_id(context, &state);
        // Retain the operation until the checkpoint so a handler installed
        // during notification remains observable to that notification.
        state
            .promise_reports
            .push(promise.clone(), operation, document, delivered);
        Ok(())
    });
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_report_error"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            let value = args.first().ok_or_else(|| {
                JsNativeError::typ().with_message("reportError requires an argument")
            })?;
            report_exception(context, &JsError::from_opaque(value.clone()))?;
            Ok(JsValue::undefined())
        }),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_run_microtask_callback"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            let callback = args
                .first()
                .and_then(JsValue::as_callable)
                .ok_or_else(|| JsNativeError::typ().with_message("Callback must be callable"))?;
            if let Err(error) = callback.call(&JsValue::undefined(), &[], context) {
                if error.as_native().is_some_and(|error| {
                    matches!(error.kind, boa_engine::JsNativeErrorKind::RuntimeLimit)
                }) {
                    return Err(error);
                }
                report_exception(context, &error)?;
            }
            Ok(JsValue::undefined())
        }),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_register_error_reporter"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            let callback = args
                .first()
                .and_then(JsValue::as_callable)
                .ok_or_else(|| JsNativeError::typ().with_message("Reporter must be callable"))?;
            with_host_state(|host| {
                let mut state = host.borrow_mut();
                let document = context_document_id(context, &state);
                state
                    .promise_reports
                    .error_reporters
                    .insert(document, (callback.clone(), context.realm().clone()));
                Ok(JsValue::undefined())
            })
        }),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_register_promise_reporter"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            let callback = args
                .first()
                .and_then(JsValue::as_callable)
                .ok_or_else(|| JsNativeError::typ().with_message("Reporter must be callable"))?;
            with_host_state(|host| {
                let mut state = host.borrow_mut();
                let document = context_document_id(context, &state);
                state
                    .promise_reports
                    .reporters
                    .insert(document, (callback.clone(), context.realm().clone()));
                Ok(JsValue::undefined())
            })
        }),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_promise_handled_during_notification"),
        1,
        NativeFunction::from_copy_closure(|_, args, _| {
            let promise = args
                .first()
                .and_then(JsValue::as_object)
                .ok_or_else(|| JsNativeError::typ().with_message("Promise required"))?;
            Ok(JsValue::from(JsPromise::from_object(promise)?.is_handled()))
        }),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_mark_promise_handled"),
        1,
        NativeFunction::from_copy_closure(|_, args, _| {
            let promise = args
                .first()
                .and_then(JsValue::as_object)
                .ok_or_else(|| JsNativeError::typ().with_message("Promise required"))?;
            JsPromise::from_object(promise)?.mark_handled();
            Ok(JsValue::undefined())
        }),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_is_promise"),
        1,
        NativeFunction::from_copy_closure(|_, args, _| {
            Ok(JsValue::from(
                args.first()
                    .and_then(JsValue::as_object)
                    .is_some_and(|object| JsPromise::from_object(object.clone()).is_ok()),
            ))
        }),
    )
}

pub(super) fn flush(context: &mut Context) -> JsResult<()> {
    let checkpoint = Box::new(with_host_state(|host| {
        let mut state = host.borrow_mut();
        let (pending, handled_rejections) = state.promise_reports.take_checkpoint();
        Ok(PromiseCheckpoint {
            pending,
            handled_rejections,
            reporters: state.promise_reports.reporters.clone(),
        })
    })?);
    // SAFETY: The box keeps the checkpoint at a stable address, and the guard
    // is declared after it so registration ends before the box is dropped.
    // The checkpoint is only read while reporter callbacks may allocate.
    let _checkpoint_roots =
        unsafe { RootProvider::register(std::ptr::NonNull::from(&*checkpoint)) };
    for report in &checkpoint.pending {
        if report.delivered || rejection_was_handled(report, &checkpoint.handled_rejections) {
            continue;
        }
        let Some((callback, realm)) = checkpoint.reporters.get(&report.document) else {
            continue;
        };
        let reason = match JsPromise::from_object(report.promise.clone())?.state() {
            PromiseState::Rejected(reason) => reason,
            _ => continue,
        };
        invoke(
            context,
            callback,
            realm,
            &[
                JsValue::from(if report.operation == OperationType::Reject {
                    0
                } else {
                    1
                }),
                report.promise.clone().into(),
                reason,
            ],
        )?;
    }
    for (callback, realm) in checkpoint.reporters.values() {
        invoke(context, callback, realm, &[JsValue::from(2)])?;
    }
    Ok(())
}

fn invoke(
    context: &mut Context,
    callback: &JsObject,
    realm: &Realm,
    args: &[JsValue],
) -> JsResult<JsValue> {
    let old = context.enter_realm(realm.clone());
    let result = callback.call(&JsValue::undefined(), args, context);
    context.enter_realm(old);
    result
}

/// Queues one uncanceled Worker report, preserving the dispatched fields.
fn forward_worker_report(report: WorkerErrorReport) -> JsResult<()> {
    forward_worker_report_with_code(report, "WORKER_RUNTIME_FAILED")
}

fn forward_worker_report_with_code(report: WorkerErrorReport, code: &'static str) -> JsResult<()> {
    let owner = with_host_state(|host| {
        let state = host.borrow();
        Ok(state
            .worker_owner
            .as_ref()
            .zip(state.worker_id)
            .map(|(owner, id)| {
                (
                    Rc::clone(owner),
                    id,
                    state.worker_owner_realm.clone(),
                    state.worker_owner_object.clone(),
                )
            }))
    })?;
    if let Some((owner, id, realm, object)) = owner {
        report_safe_worker_or_module_failure(
            owner.borrow().error_reporter.clone(),
            ErrorCategory::Worker,
            code,
            "execute",
        );
        owner
            .borrow_mut()
            .event_loop
            .enqueue_worker_error(id, object, realm, report);
    } else {
        with_host_state(|host| {
            host.borrow()
                .shared_worker
                .record_failure(code == "WORKER_STARTUP_FAILED");
            Ok(())
        })?;
    }
    Ok(())
}

/// Dispatches a Worker owner error through its Realm's captured event interfaces.
pub(super) fn report_worker_owner_error(
    context: &mut Context,
    owner: JsValue,
    notification: WorkerErrorNotification,
) -> JsResult<()> {
    let (report, load_failure) = match notification {
        WorkerErrorNotification::Exception(report) => (report, false),
        WorkerErrorNotification::LoadFailure => (WorkerErrorReport::default(), true),
    };
    let reporter = with_host_state(|host| {
        let state = host.borrow();
        let document = context_document_id(context, &state);
        Ok(state
            .promise_reports
            .error_reporters
            .get(&document)
            .cloned())
    })?;
    if let Some((callback, realm)) = reporter {
        let fields = [
            report.message.into(),
            report.filename.into(),
            JsValue::from(report.line),
            JsValue::from(report.column),
            JsValue::from(false),
            owner,
            JsValue::from(load_failure),
            JsValue::from(true),
        ];
        let result = invoke(context, &callback, &realm, &fields)?;
        if !load_failure
            && let Some(report) = decode_exception_report(result, context)?
            && report.not_canceled
        {
            // Reuse the owned snapshot, not fields that author listeners may mutate.
            let mut global_fields = fields;
            global_fields[5] = JsValue::null();
            let result = invoke(context, &callback, &realm, &global_fields)?;
            if let Some(report) = decode_exception_report(result, context)?
                && report.not_canceled
            {
                forward_worker_report(report.details)?;
            }
        }
    }
    Ok(())
}

/// Reports an exception to the current document's private global dispatcher.
pub(super) fn report_exception(context: &mut Context, error: &JsError) -> JsResult<bool> {
    let document = with_host_state(|host| {
        let state = host.borrow();
        Ok(context_document_id(context, &state))
    })?;
    report_exception_for_document(context, error, document)
}

/// Reports a Worker initial-script exception without terminating its event loop.
pub(super) fn report_worker_startup_exception(
    context: &mut Context,
    error: &JsError,
) -> JsResult<()> {
    let document = with_host_state(|host| {
        let state = host.borrow();
        Ok(context_document_id(context, &state))
    })?;
    if let Some(report) = dispatch_exception_report(context, error, document, false)?
        && report.not_canceled
    {
        forward_worker_report_with_code(report.details, "WORKER_STARTUP_FAILED")?;
    }
    Ok(())
}

/// Uses the callback's document even after Boa restores its calling Realm.
pub(super) fn report_exception_for_document(
    context: &mut Context,
    error: &JsError,
    document: usize,
) -> JsResult<bool> {
    report_exception_with_muting(
        context,
        error,
        document,
        error.source_script_metadata().map_or_else(
            || active_script_errors_are_muted(context),
            |metadata| {
                metadata
                    .get::<ClassicScriptMetadata>()
                    .is_some_and(|metadata| metadata.muted_errors)
            },
        ),
    )
}

pub(super) fn report_exception_with_muting(
    context: &mut Context,
    error: &JsError,
    document: usize,
    muted: bool,
) -> JsResult<bool> {
    if let Some(report) = dispatch_exception_report(context, error, document, muted)? {
        if report.not_canceled {
            forward_worker_report(report.details)?;
        }
        return Ok(report.not_canceled);
    }
    Ok(true)
}

fn dispatch_exception_report(
    context: &mut Context,
    error: &JsError,
    document: usize,
    muted: bool,
) -> JsResult<Option<ExceptionReport>> {
    // Host execution limits are aborts, not page-thrown JavaScript values.
    // Preserve them before converting an error or invoking author handlers.
    if error
        .as_native()
        .is_some_and(|error| error.is_runtime_limit())
    {
        return Err(error.clone());
    }
    let reporter = with_host_state(|host| {
        let state = host.borrow();
        Ok(state
            .promise_reports
            .error_reporters
            .get(&document)
            .cloned()
            .map(|reporter| {
                (
                    reporter,
                    state
                        .document_urls
                        .get(&document)
                        .cloned()
                        .unwrap_or_default(),
                )
            }))
    })?;
    if let Some(((callback, realm), filename)) = reporter {
        let (source, line, column) = error.source_location().unwrap_or((None, 0, 0));
        let value = if muted {
            JsValue::null()
        } else {
            error.to_opaque(context)
        };
        let report = invoke(
            context,
            &callback,
            &realm,
            &[
                value,
                JsString::from(source.unwrap_or(filename)).into(),
                JsValue::from(line),
                JsValue::from(column),
                JsValue::from(muted),
            ],
        )?;
        return decode_exception_report(report, context);
    }
    Ok(None)
}

fn decode_exception_report(
    value: JsValue,
    context: &mut Context,
) -> JsResult<Option<ExceptionReport>> {
    let Some(report) = value.as_object() else {
        // The private dispatcher suppresses recursive error reporting.
        return Ok(None);
    };
    Ok(Some(ExceptionReport {
        not_canceled: report
            .get(js_string!("notCanceled"), context)?
            .as_boolean()
            .unwrap_or(true),
        details: WorkerErrorReport {
            message: report
                .get(js_string!("message"), context)?
                .as_string()
                .unwrap_or_default(),
            filename: report
                .get(js_string!("filename"), context)?
                .as_string()
                .unwrap_or_default(),
            line: report
                .get(js_string!("lineno"), context)?
                .as_number()
                .unwrap_or(0.0) as u32,
            column: report
                .get(js_string!("colno"), context)?
                .as_number()
                .unwrap_or(0.0) as u32,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handled_cutoff_suppresses_only_earlier_rejections() {
        let promise = Rooted::new(JsObject::with_null_proto());
        let other = Rooted::new(JsObject::with_null_proto());
        let mut reports = PromiseReports::default();
        reports.push((*promise).clone(), OperationType::Reject, 1, false);
        reports.push((*other).clone(), OperationType::Reject, 1, false);
        reports.mark_handled(&promise);
        reports.push((*promise).clone(), OperationType::Handle, 1, true);
        reports.push((*promise).clone(), OperationType::Reject, 1, false);

        let (pending, cutoffs) = reports.take_checkpoint();
        let rejected = pending
            .iter()
            .filter(|report| {
                report.operation == OperationType::Reject
                    && JsObject::equals(&report.promise, &promise)
            })
            .collect::<Vec<_>>();
        assert_eq!(rejected.len(), 2);
        assert!(rejection_was_handled(rejected[0], &cutoffs));
        assert!(!rejection_was_handled(rejected[1], &cutoffs));
    }

    #[test]
    fn retiring_handler_document_keeps_cross_document_cutoff() {
        let promise = Rooted::new(JsObject::with_null_proto());
        let mut reports = PromiseReports::default();
        reports.push((*promise).clone(), OperationType::Reject, 2, false);
        reports.mark_handled(&promise);
        reports.push((*promise).clone(), OperationType::Handle, 1, true);

        reports.retire_document(1);
        assert_eq!(reports.pending.len(), 1);
        assert_eq!(reports.handled_rejections.get(&*promise), Some(&1));

        reports.retire_document(2);
        assert!(reports.pending.is_empty());
        assert!(reports.handled_rejections.is_empty());
    }

    #[test]
    fn checkpoint_roots_later_promises_across_reporter_collection() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .with_active_host(|context| {
                context.register_global_builtin_callable(
                    js_string!("forcePromiseReportCollection"),
                    0,
                    NativeFunction::from_fn_ptr(|_, _, _| {
                        boa_gc::force_collect();
                        Ok(JsValue::undefined())
                    }),
                )?;
                let callback_value = context.eval(Source::from_bytes(
                    "globalThis.promiseReportReasons = [];\
                         (operation, promise, reason) => {\
                           if (operation === 0) {\
                             promiseReportReasons.push(reason);\
                             forcePromiseReportCollection();\
                           }\
                         }",
                ))?;
                let callback = Rooted::new(callback_value.as_callable().ok_or_else(|| {
                    JsNativeError::typ().with_message("reporter must be callable")
                })?);
                with_host_state(|host| {
                    let mut state = host.borrow_mut();
                    let document = context_document_id(context, &state);
                    state
                        .promise_reports
                        .reporters
                        .insert(document, ((*callback).clone(), context.realm().clone()));
                    Ok(())
                })?;
                context.eval(Source::from_bytes(
                    "Promise.reject('first'); Promise.reject('second');",
                ))?;
                flush(context)?;
                let reasons = context.eval(Source::from_bytes("promiseReportReasons.join(',')"))?;
                assert_eq!(
                    reasons
                        .as_string()
                        .map(|value| value.to_std_string_escaped()),
                    Some("first,second".to_owned())
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn muted_script_tracker_suppresses_reject_and_handle_in_retained_callbacks() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.eval("globalThis.reports=[];addEventListener('unhandledrejection',e=>{reports.push('reject');e.preventDefault()});addEventListener('rejectionhandled',()=>reports.push('handle')); ").unwrap();
        runtime.with_active_host(|context| {
            let metadata = classic_script_metadata(true);
            let script = Script::parse_with_host_defined(
                Source::from_bytes("globalThis.mutedReject=()=>Promise.reject(42);globalThis.mutedHandle=p=>p.catch(()=>{});"),
                None, metadata, context)?;
            script.evaluate(context)?;
            Ok(())
        }).unwrap();
        runtime.eval("mutedReject()").unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(
            runtime.eval("reports.length").unwrap().as_number(),
            Some(0.0)
        );
        runtime
            .eval("globalThis.visiblePromise=Promise.reject(43)")
            .unwrap();
        runtime.run_until_idle().unwrap();
        runtime.eval("mutedHandle(visiblePromise)").unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(
            runtime
                .eval("reports.join(',')")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "reject"
        );
    }
}
