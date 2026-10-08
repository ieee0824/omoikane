//! `BrowserSession`: the CDP endpoint that serves one `CdpSession` to WebSocket
//! clients, including suspended evaluations, page navigations, and JavaScript
//! dialogs.

use super::*;

type SessionEvaluation = Pin<Box<dyn Future<Output = (CdpSession, Result<Value, JsonRpcError>)>>>;

pub(super) struct PendingSessionEvaluation {
    token: DeferredResponseToken,
    controller: JavaScriptDialogController,
    cancelled: Rc<Cell<bool>>,
    page_url: String,
    opened: Option<JavaScriptDialog>,
    timeout_deadline: Instant,
    future: SessionEvaluation,
}

pub(super) struct PendingPageNavigation {
    pub(super) token: DeferredResponseToken,
    pub(super) controller: JavaScriptDialogController,
    pub(super) page_url: String,
    pub(super) opened: Option<JavaScriptDialog>,
    pub(super) timeout_deadline: Instant,
    pub(super) task: Pin<Box<OwnedPageTask>>,
    pub(super) commit: PendingDocumentCommit,
    pub(super) response: Value,
}

pub(super) enum BrowserSessionAction {
    Notify(&'static str, Value),
    Complete(DeferredResponseToken, Result<Value, JsonRpcError>),
}

pub(super) struct BrowserSessionState {
    pub(super) session: Option<CdpSession>,
    pub(super) pending: Option<PendingSessionEvaluation>,
    pub(super) pending_page: Option<PendingPageNavigation>,
    pub(super) actions: Vec<BrowserSessionAction>,
}

pub(super) fn deadline_after(timeout: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(timeout).unwrap_or(now)
}

pub(super) fn session_evaluation(
    mut session: CdpSession,
    expression: String,
    return_by_value: bool,
    cancelled: Rc<Cell<bool>>,
    timeout_deadline: Instant,
) -> SessionEvaluation {
    Box::pin(async move {
        let result = {
            let mut evaluation =
                Box::pin(session.evaluate_expression_async(&expression, return_by_value));
            std::future::poll_fn(|context| {
                if cancelled.get() {
                    return Poll::Ready(Err(JsonRpcError {
                        code: -32000,
                        message: "JavaScript evaluation cancelled by page teardown".to_string(),
                    }));
                }
                // The request deadline is created before this future is first
                // polled. The runtime starts its own clock later, so relying
                // on it alone can leave a suspended request pending when
                // wait_for_outgoing wakes at the earlier request deadline.
                if Instant::now() >= timeout_deadline {
                    return Poll::Ready(Err(evaluation_timeout_error()));
                }
                let result = evaluation.as_mut().poll(context);
                if Instant::now() >= timeout_deadline {
                    Poll::Ready(Err(evaluation_timeout_error()))
                } else {
                    result
                }
            })
            .await
        };
        (session, result)
    })
}

fn evaluation_timeout_error() -> JsonRpcError {
    js_error(
        boa_engine::JsNativeError::runtime_limit()
            .with_message(boa_engine::vm::WALL_CLOCK_TIMEOUT_MESSAGE)
            .into(),
    )
}

pub(super) fn page_task_script_error_lines(
    result: &Result<Vec<String>, PageTaskError>,
) -> Vec<String> {
    result
        .as_ref()
        .map(|errors| {
            errors
                .iter()
                .map(|error| format!("[omoikane][js-error] {error}"))
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn take_page_task_script_error_lines(completed: &mut CompletedPageTask) -> Vec<String> {
    let mut lines = page_task_script_error_lines(&completed.result);
    lines.extend(
        completed
            .runtime
            .take_task_errors()
            .into_iter()
            .map(|error| format!("[omoikane][js-error] {error}")),
    );
    lines
}

pub(super) fn pending_evaluation_busy_message(
    has_pending_evaluation: bool,
    has_pending_page: bool,
) -> Option<&'static str> {
    if has_pending_evaluation {
        Some("A Runtime.evaluate request is already pending")
    } else if has_pending_page {
        Some("Browser session is busy with a page navigation")
    } else {
        None
    }
}

impl BrowserSessionState {
    fn begin_evaluation(
        &mut self,
        token: DeferredResponseToken,
        params: &Value,
    ) -> Result<CdpMethodResult, JsonRpcError> {
        if let Some(message) =
            pending_evaluation_busy_message(self.pending.is_some(), self.pending_page.is_some())
        {
            return Err(JsonRpcError {
                code: -32000,
                message: message.to_string(),
            });
        }
        let expression = require_string(params, "expression")?;
        let return_by_value = params
            .get("returnByValue")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let session = self.session.take().ok_or(JsonRpcError {
            code: -32000,
            message: "Browser session is busy".to_string(),
        })?;
        // The controller belongs only to this in-flight evaluation. It is
        // discarded together with the evaluation before a replacement Runtime
        // is installed, so dialog ids reused after navigation cannot resolve an
        // older suspension.
        let controller = session.runtime.javascript_dialog_controller();
        let page_url = session.current_url.clone();
        let timeout_deadline = deadline_after(session.runtime_timeout());
        let cancelled = Rc::new(Cell::new(false));
        let future = session_evaluation(
            session,
            expression,
            return_by_value,
            Rc::clone(&cancelled),
            timeout_deadline,
        );
        self.pending = Some(PendingSessionEvaluation {
            token,
            controller,
            cancelled,
            page_url,
            opened: None,
            timeout_deadline,
            future,
        });
        self.poll_evaluation();
        // Even immediate completion is flushed out-of-band so CdpServer can
        // first record the original client and request id for this token.
        Ok(CdpMethodResult::Deferred)
    }

    pub(super) fn begin_page_navigation(
        &mut self,
        token: DeferredResponseToken,
        method: &str,
        params: &Value,
    ) -> Result<CdpMethodResult, JsonRpcError> {
        self.cancel_pending_dialog();
        if self.pending.is_some() || self.pending_page.is_some() {
            return Err(JsonRpcError {
                code: -32000,
                message: "Browser session is busy".to_string(),
            });
        }
        let session = self.session.as_mut().ok_or(JsonRpcError {
            code: -32000,
            message: "Browser session is unavailable".to_string(),
        })?;
        let timeout_deadline = deadline_after(session.runtime_timeout());
        let prepared = if method == "Page.reload" {
            session.prepare_page_reload()
        } else {
            session.prepare_page_navigate(params)
        };
        if prepared.is_err() {
            session.report_cdp_failure(CdpFailure::Navigation);
        }
        let prepared = prepared?;
        let (task, commit, response) = match prepared {
            PreparedPageNavigation::Complete(response) => {
                return Ok(CdpMethodResult::Complete(response));
            }
            PreparedPageNavigation::Pending {
                task,
                commit,
                response,
            } => (task, commit, response),
        };
        let controller = task.dialog_controller();
        let page_url = commit.url.clone();
        self.pending_page = Some(PendingPageNavigation {
            token,
            controller,
            page_url,
            opened: None,
            timeout_deadline,
            task: Box::pin(task),
            commit,
            response,
        });
        self.poll_page_navigation();
        Ok(CdpMethodResult::Deferred)
    }

    pub(super) fn poll_page_navigation(&mut self) {
        for _ in 0..64 {
            let Some(mut pending) = self.pending_page.take() else {
                return;
            };
            let waker: &'static Waker = Waker::noop();
            let mut context = TaskContext::from_waker(waker);
            match pending.task.as_mut().poll(&mut context) {
                Poll::Ready(completed) => {
                    if pending.opened.take().is_some() {
                        self.actions.push(BrowserSessionAction::Notify(
                            "Page.javascriptDialogClosed",
                            json!({ "result": false, "userInput": "" }),
                        ));
                    }
                    let result = self
                        .session
                        .as_mut()
                        .ok_or(CdpSessionError::Unavailable)
                        .and_then(|session| {
                            session.commit_document_page_task(completed, pending.commit)
                        })
                        .map(|()| pending.response)
                        .map_err(|error| JsonRpcError {
                            code: -32000,
                            message: error.to_string(),
                        });
                    self.actions
                        .push(BrowserSessionAction::Complete(pending.token, result));
                    return;
                }
                Poll::Pending => {
                    if pending.opened.is_none()
                        && let Some(dialog) = pending.controller.pending()
                    {
                        self.actions.push(BrowserSessionAction::Notify(
                            "Page.javascriptDialogOpening",
                            dialog_opening_params(&dialog, &pending.page_url),
                        ));
                        pending.opened = Some(dialog);
                        self.pending_page = Some(pending);
                        return;
                    }
                    self.pending_page = Some(pending);
                }
            }
        }
    }

    fn poll_evaluation(&mut self) {
        let Some(mut pending) = self.pending.take() else {
            return;
        };
        let waker: &'static Waker = Waker::noop();
        let mut context = TaskContext::from_waker(waker);
        match pending.future.as_mut().poll(&mut context) {
            Poll::Ready((session, result)) => {
                if result.is_err() {
                    session.report_cdp_failure(CdpFailure::Request);
                }
                self.session = Some(session);
                if pending.opened.take().is_some() {
                    self.actions.push(BrowserSessionAction::Notify(
                        "Page.javascriptDialogClosed",
                        json!({ "result": false, "userInput": "" }),
                    ));
                }
                self.actions
                    .push(BrowserSessionAction::Complete(pending.token, result));
            }
            Poll::Pending => {
                if pending.opened.is_none()
                    && let Some(dialog) = pending.controller.pending()
                {
                    self.actions.push(BrowserSessionAction::Notify(
                        "Page.javascriptDialogOpening",
                        dialog_opening_params(&dialog, &pending.page_url),
                    ));
                    pending.opened = Some(dialog);
                }
                self.pending = Some(pending);
            }
        }
    }

    fn next_pending_timeout_deadline(&self) -> Option<Instant> {
        self.pending
            .as_ref()
            .map(|pending| pending.timeout_deadline)
            .into_iter()
            .chain(
                self.pending_page
                    .as_ref()
                    .map(|pending| pending.timeout_deadline),
            )
            .min()
    }

    fn handle_dialog(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let accept = params
            .get("accept")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                invalid_params("Missing or invalid boolean parameter: accept".to_string())
            })?;
        let prompt_text = params
            .get("promptText")
            .map(|value| {
                value.as_str().map(ToString::to_string).ok_or_else(|| {
                    invalid_params("Invalid string parameter: promptText".to_string())
                })
            })
            .transpose()?;
        let controller = self
            .pending
            .as_ref()
            .map(|pending| pending.controller.clone())
            .or_else(|| {
                self.pending_page
                    .as_ref()
                    .map(|pending| pending.controller.clone())
            })
            .ok_or(JsonRpcError {
                code: -32000,
                message: "No JavaScript dialog is open".to_string(),
            })?;
        let dialog = controller.pending().ok_or(JsonRpcError {
            code: -32000,
            message: "No JavaScript dialog is open".to_string(),
        })?;
        controller
            .handle(dialog.id, accept, prompt_text.clone())
            .map_err(dialog_error)?;
        let user_input = if dialog.kind == JavaScriptDialogKind::Prompt && accept {
            prompt_text
                .or(dialog.default_prompt.clone())
                .unwrap_or_default()
        } else {
            String::new()
        };
        self.actions.push(BrowserSessionAction::Notify(
            "Page.javascriptDialogClosed",
            json!({ "result": accept, "userInput": user_input }),
        ));
        if let Some(pending) = self.pending.as_mut() {
            pending.opened = None;
            self.poll_evaluation();
        } else if let Some(pending) = self.pending_page.as_mut() {
            pending.opened = None;
            self.poll_page_navigation();
        }
        Ok(json!({}))
    }

    fn cancel_pending_page(&mut self) {
        let Some(pending) = self.pending_page.as_mut() else {
            return;
        };
        if pending.controller.pending().is_some() {
            self.actions.push(BrowserSessionAction::Notify(
                "Page.javascriptDialogClosed",
                json!({ "result": false, "userInput": "" }),
            ));
        }
        pending.opened = None;
        pending.task.cancel();
        self.poll_page_navigation();
    }

    fn cancel_pending_dialog(&mut self) {
        self.cancel_pending_page();
        let Some(pending) = self.pending.as_mut() else {
            return;
        };
        if pending.controller.pending().is_some() {
            self.actions.push(BrowserSessionAction::Notify(
                "Page.javascriptDialogClosed",
                json!({ "result": false, "userInput": "" }),
            ));
        }
        pending.cancelled.set(true);
        self.poll_evaluation();
    }
}

/// CDP transport and page-session coordinator with deferred modal-dialog support.
///
/// `Runtime.evaluate` remains pending while `alert`, `confirm`, or `prompt`
/// blocks JavaScript. Other transport commands continue to be routed, and
/// navigation, reload, or owner-client disconnect dismisses the active dialog
/// before the old evaluation state is discarded.
pub struct BrowserSession {
    server: CdpServer,
    pub(super) state: Rc<RefCell<BrowserSessionState>>,
    /// The first attached CDP client owns the page runtime. A secondary
    /// observer disconnect must not cancel the owner's dialogs or workers.
    owner_client_id: Option<u64>,
}

impl BrowserSession {
    pub fn new() -> Result<Self, CdpSessionError> {
        let state = Rc::new(RefCell::new(BrowserSessionState {
            session: Some(CdpSession::new()?),
            pending: None,
            pending_page: None,
            actions: Vec::new(),
        }));
        let mut server = CdpServer::new();

        let evaluation_state = Rc::clone(&state);
        server.register_deferred_method("Runtime.evaluate", move |token, params| {
            evaluation_state
                .borrow_mut()
                .begin_evaluation(token, params)
        });
        let dialog_state = Rc::clone(&state);
        server.register_method("Page.handleJavaScriptDialog", move |params| {
            dialog_state.borrow_mut().handle_dialog(params)
        });
        for method in ["Page.navigate", "Page.reload"] {
            let method_state = Rc::clone(&state);
            server.register_deferred_method(method, move |token, params| {
                method_state
                    .borrow_mut()
                    .begin_page_navigation(token, method, params)
            });
        }
        for method in [
            "Page.getFrameTree",
            "DOM.getDocument",
            "DOM.getAttributes",
            "DOM.querySelector",
            "DOM.getOuterHTML",
            "Accessibility.enable",
            "Accessibility.disable",
            "Accessibility.getFullAXTree",
            "Accessibility.getRootAXNode",
            "Accessibility.getPartialAXTree",
            "Accessibility.getAXNodeAndAncestors",
            "Accessibility.getChildAXNodes",
            "Accessibility.queryAXTree",
            "Emulation.setEmulatedMedia",
            "Runtime.callFunctionOn",
            "Target.createBrowserContext",
            "Target.getBrowserContexts",
            "Target.disposeBrowserContext",
            "Input.dispatchKeyEvent",
            "Input.dispatchMouseEvent",
            "Input.dispatchTouchEvent",
            "Input.imeSetComposition",
            "Input.insertText",
        ] {
            let method_state = Rc::clone(&state);
            server.register_method(method, move |params| {
                let mut state = method_state.borrow_mut();
                let session = state.session.as_mut().ok_or(JsonRpcError {
                    code: -32000,
                    message: "Page state is suspended by a JavaScript dialog".to_string(),
                })?;
                session.dispatch(method, params.clone())
            });
        }
        server.register_method("Browser.getVersion", |_| {
            Ok(json!({ "product": "Omoikane/0.1", "protocolVersion": "1.3" }))
        });

        Ok(Self {
            server,
            state,
            owner_client_id: None,
        })
    }

    pub fn accept_upgrade(&mut self, request: &str) -> Result<WebSocketUpgrade, CdpError> {
        let upgrade = self.server.accept_upgrade(request)?;
        if self.owner_client_id.is_none() {
            self.owner_client_id = Some(upgrade.client_id);
        }
        Ok(upgrade)
    }

    pub fn receive(&mut self, client_id: u64, bytes: &[u8]) -> Result<(), CdpError> {
        let owner_disconnect =
            WebSocketFrame::decode_client(bytes)
                .ok()
                .is_some_and(|(frame, _)| {
                    frame.opcode == WebSocketOpcode::Close
                        && self.owner_client_id == Some(client_id)
                });
        if owner_disconnect {
            let mut state = self.state.borrow_mut();
            state.cancel_pending_dialog();
            if let Some(session) = state.session.as_mut() {
                session.runtime.terminate_workers();
                let _ = session.runtime.eval(
                    "if (typeof globalThis.__omoikane_permission_teardown === 'function') \
                     globalThis.__omoikane_permission_teardown();",
                );
            }
        }
        self.server.receive(client_id, bytes)?;
        if owner_disconnect {
            self.owner_client_id = None;
        }
        self.flush_actions()
    }

    pub fn drain_outgoing(&mut self, client_id: u64) -> Result<Vec<WebSocketFrame>, CdpError> {
        self.flush_actions()?;
        self.server.drain_outgoing(client_id)
    }

    /// Waits for queued output or the next pending timeout deadline, then drains
    /// the current outbound frames for a client.
    pub fn wait_for_outgoing(&mut self, client_id: u64) -> Result<Vec<WebSocketFrame>, CdpError> {
        self.flush_actions()?;
        let mut outgoing = self.server.drain_outgoing(client_id)?;
        if !outgoing.is_empty() || self.server.pending_response_count() == 0 {
            return Ok(outgoing);
        }
        let deadline = { self.state.borrow().next_pending_timeout_deadline() };
        if let Some(deadline) = deadline {
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            self.flush_actions()?;
            outgoing.extend(self.server.drain_outgoing(client_id)?);
        }
        Ok(outgoing)
    }

    pub fn pending_response_count(&self) -> usize {
        self.server.pending_response_count()
    }

    fn flush_actions(&mut self) -> Result<(), CdpError> {
        self.state.borrow_mut().poll_evaluation();
        self.state.borrow_mut().poll_page_navigation();
        let actions = std::mem::take(&mut self.state.borrow_mut().actions);
        for action in actions {
            match action {
                BrowserSessionAction::Notify(method, params) => {
                    self.server.notify(method, params)?;
                }
                BrowserSessionAction::Complete(token, result) => {
                    // Disconnect already removed the response routing record.
                    if self.server.deferred_response_client(token).is_some() {
                        self.server.complete_deferred_response(token, result)?;
                    }
                }
            }
        }
        if let Some(session) = self.state.borrow_mut().session.as_mut() {
            for event in session.drain_events() {
                self.server.notify(&event.method, event.params)?;
            }
        }
        Ok(())
    }
}

fn dialog_opening_params(dialog: &JavaScriptDialog, page_url: &str) -> Value {
    let kind = match dialog.kind {
        JavaScriptDialogKind::Alert => "alert",
        JavaScriptDialogKind::Confirm => "confirm",
        JavaScriptDialogKind::Prompt => "prompt",
    };
    json!({
        "url": page_url,
        "type": kind,
        "message": dialog.message,
        "hasBrowserHandler": true,
        "defaultPrompt": dialog.default_prompt.clone().unwrap_or_default(),
    })
}

fn dialog_error(error: JavaScriptDialogError) -> JsonRpcError {
    JsonRpcError {
        code: -32000,
        message: error.to_string(),
    }
}
