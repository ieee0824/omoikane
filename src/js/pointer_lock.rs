//! Pointer-lock ownership and the asynchronous presentation-host handshake.

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

/// A pointer-lock operation for the presentation host to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerLockTransition {
    /// Attempt cursor confinement/hiding and report the result with this ID.
    Acquire {
        /// Unique request ID, including across document navigation.
        request_id: u64,
        /// Whether the page requires movement without OS acceleration.
        unadjusted_movement: bool,
    },
    /// Restore the visible, unconstrained cursor.
    Release,
}

#[derive(Clone)]
struct Completion {
    callback: JsValue,
    realm: Option<(Realm, usize)>,
    document: usize,
}

struct Request {
    id: u64,
    element: NodeHandle,
    document: usize,
    unadjusted: bool,
    callback: Completion,
    sent: bool,
}

pub(super) struct State {
    active: Option<Request>,
    queue: VecDeque<Request>,
    notifications: VecDeque<(Completion, &'static str)>,
    released_documents: HashSet<usize>,
    deferred: bool,
    focused: bool,
    release_pending: bool,
    activation: bool,
    activation_generation: u64,
    cursor: (f64, f64),
    locked_cursor: (f64, f64),
}

impl Default for State {
    fn default() -> Self {
        Self {
            active: None,
            queue: VecDeque::new(),
            notifications: VecDeque::new(),
            released_documents: HashSet::new(),
            deferred: false,
            focused: true,
            release_pending: false,
            activation: false,
            activation_generation: 0,
            cursor: (0.0, 0.0),
            locked_cursor: (0.0, 0.0),
        }
    }
}

impl State {
    pub(super) unsafe fn trace(&self, tracer: &mut Tracer) {
        for request in self.active.iter().chain(self.queue.iter()) {
            unsafe { request.callback.callback.trace(tracer) };
        }
        for (callback, _) in &self.notifications {
            unsafe { callback.callback.trace(tracer) };
        }
    }

    fn exit(&mut self, explicit: bool) {
        if let Some(request) = self.active.take() {
            if explicit {
                self.released_documents.insert(request.document);
            } else {
                self.released_documents.remove(&request.document);
            }
            self.notifications.push_back((request.callback, ""));
            self.release_pending = true;
        }
    }

    fn cancel_requests(&mut self) {
        for request in self.queue.drain(..) {
            self.release_pending |= request.sent;
            self.notifications
                .push_back((request.callback, "AbortError"));
        }
    }

    pub(super) fn retire_document(&mut self, document: usize) {
        if self.active.as_ref().is_some_and(|r| r.document == document) {
            self.exit(false);
        }
        let mut kept = VecDeque::new();
        for request in self.queue.drain(..) {
            if request.document == document {
                self.release_pending |= request.sent;
                self.notifications
                    .push_back((request.callback, "WrongDocumentError"));
            } else {
                kept.push_back(request);
            }
        }
        self.queue = kept;
        self.released_documents.remove(&document);
    }

    pub(super) fn subtree_removed(&mut self, root: &NodeHandle) {
        if self
            .active
            .as_ref()
            .is_some_and(|r| inclusive_descendant(&r.element, root))
        {
            self.exit(false);
        }
    }
}

fn inclusive_descendant(node: &NodeHandle, root: &NodeHandle) -> bool {
    let mut current = Some(node.clone());
    while let Some(node) = current {
        if node == *root {
            return true;
        }
        current = node.parent_node().or_else(|| node.shadow_host());
    }
    false
}

impl HostState {
    fn pointer_lock_sandbox_allowed(&self, mut document: usize) -> bool {
        loop {
            if self
                .document_sandbox
                .get(&document)
                .is_some_and(|s| !s.allow_pointer_lock)
            {
                return false;
            }
            if document == self.document.identity() {
                return true;
            }
            let Some((&frame_id, _)) = self
                .iframe_documents
                .iter()
                .find(|(_, frame)| frame.document.identity() == document)
            else {
                return false;
            };
            let Some(owner) = self
                .get_node(frame_id)
                .and_then(|frame| owner_document_for_node(&frame))
            else {
                return false;
            };
            document = owner.identity();
        }
    }

    fn pointer_lock_valid(&self, request: &Request) -> bool {
        self.pointer_lock.focused
            && self.document_is_active(request.document)
            && document_root_for_node(&request.element)
                .is_some_and(|doc| doc.identity() == request.document)
    }

    fn complete_pointer_lock(&mut self, id: u64, accepted: bool) -> bool {
        if !self.pointer_lock.queue.front().is_some_and(|r| r.id == id) {
            return false;
        }
        let request = self.pointer_lock.queue.pop_front().unwrap();
        let error = if !self.pointer_lock_valid(&request) {
            "WrongDocumentError"
        } else if self
            .pointer_lock
            .active
            .as_ref()
            .is_some_and(|r| r.document != request.document)
        {
            "InvalidStateError"
        } else if !accepted {
            "NotSupportedError"
        } else {
            ""
        };
        self.pointer_lock
            .notifications
            .push_back((request.callback.clone(), error));
        if error.is_empty() {
            if self.pointer_lock.active.is_none() {
                self.pointer_lock.locked_cursor = self.pointer_lock.cursor;
            }
            self.pointer_lock.active = Some(request);
        } else if accepted && request.sent {
            self.pointer_lock.release_pending = true;
        }
        true
    }
}

impl JsRuntime {
    /// Requires explicit presentation-host responses instead of the virtual
    /// headless host. Configure this before accepting requests from a page.
    pub fn set_pointer_lock_deferred(&mut self, deferred: bool) {
        self.host_state.borrow_mut().pointer_lock.deferred = deferred;
    }

    /// Takes the next native operation. Each acquisition must be acknowledged
    /// with [`Self::complete_pointer_lock_request`]; stale IDs are ignored.
    pub fn take_pointer_lock_transition(&mut self) -> Option<PointerLockTransition> {
        let mut state = self.host_state.borrow_mut();
        if std::mem::take(&mut state.pointer_lock.release_pending) {
            return Some(PointerLockTransition::Release);
        }
        let request = state.pointer_lock.queue.front_mut()?;
        if request.sent {
            return None;
        }
        request.sent = true;
        Some(PointerLockTransition::Acquire {
            request_id: request.id,
            unadjusted_movement: request.unadjusted,
        })
    }

    /// Applies the actual cursor-grab result and queues the page's event and
    /// Promise settlement. Returns false for canceled or obsolete requests.
    pub fn complete_pointer_lock_request(&mut self, id: u64, accepted: bool) -> JsResult<bool> {
        let applied = self
            .host_state
            .borrow_mut()
            .complete_pointer_lock(id, accepted);
        self.flush_pointer_lock_notifications()?;
        Ok(applied)
    }

    /// Releases lock and pending requests after Escape, focus loss, or navigation.
    pub fn release_pointer_lock_from_host(&mut self) -> JsResult<()> {
        {
            let mut state = self.host_state.borrow_mut();
            state.pointer_lock.exit(false);
            state.pointer_lock.cancel_requests();
            state.pointer_lock.released_documents.clear();
            state.pointer_lock.activation = false;
        }
        self.flush_pointer_lock_notifications()
    }

    /// Updates native window focus; losing focus always releases pointer lock.
    pub fn set_pointer_lock_focus(&mut self, focused: bool) -> JsResult<()> {
        self.host_state.borrow_mut().pointer_lock.focused = focused;
        if !focused {
            self.release_pointer_lock_from_host()?;
        }
        Ok(())
    }

    /// Returns the actual locked element, without Shadow DOM retargeting.
    pub fn pointer_lock_target(&self) -> Option<NodeHandle> {
        self.host_state
            .borrow()
            .pointer_lock
            .active
            .as_ref()
            .map(|r| r.element.clone())
    }

    /// Returns the frozen cursor coordinates while pointer lock is active.
    pub fn pointer_lock_position(&self) -> Option<(f64, f64)> {
        let state = self.host_state.borrow();
        state
            .pointer_lock
            .active
            .as_ref()
            .map(|_| state.pointer_lock.locked_cursor)
    }

    pub(crate) fn update_pointer_position(&mut self, x: f64, y: f64) {
        let mut state = self.host_state.borrow_mut();
        if state.pointer_lock.active.is_none() {
            state.pointer_lock.cursor = (x, y);
        }
    }

    pub(super) fn flush_pointer_lock_notifications(&mut self) -> JsResult<()> {
        loop {
            let notification = {
                let mut state = self.host_state.borrow_mut();
                // The virtual host accepts ordinary movement only. Native
                // hosts acknowledge their actual platform capabilities.
                if let Some(request) = state.pointer_lock.queue.front() {
                    let needs_host = state.pointer_lock_valid(request)
                        && state.pointer_lock.active.as_ref().is_none_or(|active| {
                            active.document == request.document
                                && active.unadjusted != request.unadjusted
                        });
                    if !state.pointer_lock.deferred || !needs_host {
                        let (id, accepted) = (
                            request.id,
                            needs_host.then_some(!request.unadjusted).unwrap_or(true),
                        );
                        state.complete_pointer_lock(id, accepted);
                    }
                }
                state.pointer_lock.notifications.pop_front()
            };
            let Some((callback, error)) = notification else {
                return Ok(());
            };
            let mut payload = TimerPayload::Callback {
                callback: callback.callback,
                args: vec![js_string!(error).into()],
            };
            if let Some((realm, document_id)) = callback.realm {
                payload = TimerPayload::Realm {
                    payload: Box::new(payload),
                    realm,
                    document_id,
                };
            }
            self.host_state
                .borrow_mut()
                .event_loop
                .enqueue_user_interaction(payload, callback.document);
        }
    }
}

pub(super) fn register(context: &mut Context) -> JsResult<()> {
    context.register_global_builtin_callable(
        js_string!("__omoikane_pointer_lock_target"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            let doc = args
                .first()
                .filter(|v| !v.is_null_or_undefined())
                .map(|v| v.to_number(context))
                .transpose()?
                .map(|v| v as usize);
            with_host_state(|state| {
                Ok(state
                    .borrow()
                    .pointer_lock
                    .active
                    .as_ref()
                    .filter(|request| doc.is_none_or(|doc| doc == request.document))
                    .map_or(JsValue::null(), |request| {
                        JsValue::from(request.element.identity() as f64)
                    }))
            })
        }),
    )?;
    context.register_global_builtin_callable(
        js_string!("__omoikane_pointer_lock_activation"),
        2,
        NativeFunction::from_copy_closure(|_, args, context| {
            let grant = args.first().is_some_and(JsValue::to_boolean);
            let generation = args
                .get(1)
                .cloned()
                .unwrap_or_default()
                .to_number(context)? as u64;
            with_host_state(|state| {
                let mut state = state.borrow_mut();
                if grant {
                    state.pointer_lock.activation_generation += 1;
                    state.pointer_lock.activation = true;
                } else if generation == 0 || generation == state.pointer_lock.activation_generation
                {
                    state.pointer_lock.activation = false;
                }
                Ok(JsValue::from(
                    state.pointer_lock.activation_generation as f64,
                ))
            })
        }),
    )?;
    context.register_global_builtin_callable(
        js_string!("__omoikane_request_pointer_lock"),
        4,
        NativeFunction::from_copy_closure(|_, args, context| {
            let id = parse_node_id(args.first(), context)?;
            let doc = parse_node_id(args.get(1), context)?;
            let unadjusted = args.get(2).is_some_and(JsValue::to_boolean);
            let callback = args
                .get(3)
                .filter(|v| v.is_callable())
                .cloned()
                .ok_or_else(|| {
                    JsNativeError::typ().with_message("Invalid pointer-lock callback")
                })?;
            let callback = Completion {
                callback,
                realm: current_iframe_realm(context),
                document: doc,
            };
            with_host_state(|state| {
                let mut state = state.borrow_mut();
                let element = state
                    .get_node(id)
                    .ok_or_else(|| JsNativeError::typ().with_message("Illegal invocation"))?;
                let error = if !state.pointer_lock.focused
                    || !state.document_is_active(doc)
                    || !document_root_for_node(&element).is_some_and(|d| d.identity() == doc)
                {
                    "WrongDocumentError"
                } else if !state.pointer_lock.activation
                    && !state.pointer_lock.released_documents.contains(&doc)
                {
                    "NotAllowedError"
                } else if !state.pointer_lock_sandbox_allowed(doc) {
                    "SecurityError"
                } else {
                    ""
                };
                if !error.is_empty() {
                    state
                        .pointer_lock
                        .notifications
                        .push_back((callback, error));
                } else {
                    let id = NEXT_REQUEST
                        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                        .map_err(|_| {
                            JsNativeError::error().with_message("Pointer lock request ID exhausted")
                        })?;
                    state.pointer_lock.queue.push_back(Request {
                        id,
                        element,
                        document: doc,
                        unadjusted,
                        callback,
                        sent: false,
                    });
                }
                Ok(JsValue::undefined())
            })
        }),
    )?;
    context.register_global_builtin_callable(
        js_string!("__omoikane_exit_pointer_lock"),
        2,
        NativeFunction::from_copy_closure(|_, args, context| {
            let doc = args
                .first()
                .filter(|v| !v.is_null_or_undefined())
                .map(|v| v.to_number(context))
                .transpose()?
                .map(|v| v as usize);
            let explicit = args.get(1).is_some_and(JsValue::to_boolean);
            with_host_state(|state| {
                let mut state = state.borrow_mut();
                if state
                    .pointer_lock
                    .active
                    .as_ref()
                    .is_some_and(|r| doc.is_none_or(|d| d == r.document))
                {
                    state.pointer_lock.exit(explicit);
                }
                if !explicit {
                    state.pointer_lock.cancel_requests();
                    state.pointer_lock.released_documents.clear();
                    state.pointer_lock.activation = false;
                }
                Ok(JsValue::undefined())
            })
        }),
    )?;
    context.register_global_builtin_callable(
        js_string!("__omoikane_pointer_lock_removing"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            let id = parse_node_id(args.first(), context)?;
            with_host_state(|state| {
                let mut state = state.borrow_mut();
                if let Some(root) = state.get_node(id) {
                    state.pointer_lock.subtree_removed(&root);
                    // Pending requests are revalidated on host completion.
                }
                Ok(JsValue::undefined())
            })
        }),
    )?;
    Ok(())
}
