//! Delivery of encoded forms to an existing browsing context.

use super::*;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct Submission {
    pub url: String,
    pub method: String,
    pub body: Option<Vec<u8>>,
    pub content_type: Option<String>,
}

pub(super) enum NamedLinkTarget {
    NotFound,
    Blocked,
    Frame(NodeHandle),
}

/// Resolution does not enqueue navigation or alter pending resource loads.
enum FormTarget {
    TopLevel,
    Frame(NodeHandle),
    Blocked,
}

impl HostState {
    pub(super) fn refresh_iframe_context_name(&mut self, frame: &NodeHandle) {
        if self.iframe_context_ids.contains_key(&frame.identity()) {
            self.browsing_context_names.insert(
                frame.identity(),
                frame.get_attribute("name").unwrap_or_default(),
            );
        }
    }

    fn context_key_for_document(&self, document: usize) -> Option<usize> {
        if document == self.document.identity() {
            Some(0)
        } else {
            self.frame_for_document(document)
                .map(|frame| frame.identity())
        }
    }

    fn frame_for_document(&self, document: usize) -> Option<NodeHandle> {
        self.iframe_documents.iter().find_map(|(id, frame)| {
            (frame.document.identity() == document)
                .then(|| self.get_node(*id))
                .flatten()
        })
    }

    fn named_browsing_context_frame(&self, root: &NodeHandle, name: &str) -> Option<NodeHandle> {
        if matches!(root.tag_name().as_deref(), Some("iframe" | "frame")) {
            let current_name = self
                .browsing_context_names
                .get(&root.identity())
                .cloned()
                .unwrap_or_else(|| root.get_attribute("name").unwrap_or_default());
            if current_name == name {
                return Some(root.clone());
            }
            if let Some(frame) = self.iframe_documents.get(&root.identity()) {
                if let Some(found) = self.named_browsing_context_frame(&frame.document, name) {
                    return Some(found);
                }
            }
        }
        if let Some(shadow) = root.shadow_root() {
            if let Some(found) = self.named_browsing_context_frame(&shadow, name) {
                return Some(found);
            }
        }
        root.child_nodes()
            .iter()
            .find_map(|child| self.named_browsing_context_frame(child, name))
    }

    /// Whether the target frame belongs to a descendant context of `source`.
    fn frame_is_descendant_of_document(&self, frame: &NodeHandle, source: &NodeHandle) -> bool {
        let mut ancestor = owner_document_for_node(frame);
        while let Some(document) = ancestor {
            if document == *source {
                return true;
            }
            ancestor = self
                .frame_for_document(document.identity())
                .as_ref()
                .and_then(owner_document_for_node);
        }
        false
    }

    pub(super) fn resolve_named_link_target(
        &self,
        link: &NodeHandle,
        target: &str,
    ) -> NamedLinkTarget {
        if !self.node_is_in_active_document(link)
            || !matches!(link.tag_name().as_deref(), Some("a" | "area"))
            || link.get_attribute("href").is_none()
        {
            return NamedLinkTarget::Blocked;
        }
        let Some(source) = owner_document_for_node(link) else {
            return NamedLinkTarget::Blocked;
        };
        let Some(frame) = self.named_browsing_context_frame(&self.document, target) else {
            return NamedLinkTarget::NotFound;
        };
        if !self.node_is_in_active_document(&frame) {
            return NamedLinkTarget::Blocked;
        }
        let source_frame = self.frame_for_document(source.identity());
        let sandbox = self
            .document_sandbox
            .get(&source.identity())
            .copied()
            .unwrap_or_default();
        if sandbox.active
            && source_frame.as_ref() != Some(&frame)
            && !self.frame_is_descendant_of_document(&frame, &source)
        {
            return NamedLinkTarget::Blocked;
        }
        NamedLinkTarget::Frame(frame)
    }

    /// Queues an allowed submission. `Some(id)` identifies a frame whose
    /// navigation callback the caller must dispatch. `None` means either a
    /// blocked/unsupported target or successfully queued top-level navigation.
    pub(super) fn queue_form_submission(
        &mut self,
        form: &NodeHandle,
        target: &str,
        request: Submission,
    ) -> JsResult<Option<usize>> {
        if !self.node_is_in_active_document(form) {
            return Ok(None);
        }
        let source = owner_document_for_node(form).unwrap_or_else(|| self.document.clone());
        let destination = self.resolve_form_target(&source, target);
        if !self.form_target_is_allowed(&source, &destination) {
            return Ok(None);
        }
        Ok(self.deliver_form_submission(&source, destination, request))
    }

    fn resolve_form_target(&self, source: &NodeHandle, target: &str) -> FormTarget {
        let source_frame = self.frame_for_document(source.identity());
        let target_frame = if target.is_empty() || target.eq_ignore_ascii_case("_self") {
            source_frame
        } else if target.eq_ignore_ascii_case("_top") {
            None
        } else if target.eq_ignore_ascii_case("_parent") {
            source_frame
                .as_ref()
                .and_then(owner_document_for_node)
                .and_then(|parent| self.frame_for_document(parent.identity()))
        } else if target.eq_ignore_ascii_case("_blank") {
            return FormTarget::Blocked;
        } else if self
            .context_key_for_document(source.identity())
            .and_then(|key| self.browsing_context_names.get(&key))
            .is_some_and(|name| name == target)
        {
            source_frame
        } else if self
            .browsing_context_names
            .get(&0)
            .is_some_and(|name| name == target)
        {
            None
        } else {
            // This delivery path cannot create an auxiliary host window.
            let Some(frame) = self.named_browsing_context_frame(&self.document, target) else {
                return FormTarget::Blocked;
            };
            Some(frame)
        };
        target_frame.map_or(FormTarget::TopLevel, FormTarget::Frame)
    }

    /// Form permission is separate from the named-link sandbox policy.
    fn form_target_is_allowed(&self, source: &NodeHandle, target: &FormTarget) -> bool {
        let sandbox = self
            .document_sandbox
            .get(&source.identity())
            .copied()
            .unwrap_or_default();
        if sandbox.active && !sandbox.allow_forms {
            return false;
        }
        let source_frame = self.frame_for_document(source.identity());
        match target {
            FormTarget::Blocked => false,
            FormTarget::TopLevel => {
                !sandbox.active || source_frame.is_none() || sandbox.allow_top_navigation
            }
            FormTarget::Frame(frame) => {
                !sandbox.active
                    || source_frame.as_ref() == Some(frame)
                    || self.frame_is_descendant_of_document(frame, source)
            }
        }
    }

    /// Updates pending loads and dispatches only a previously permitted target.
    fn deliver_form_submission(
        &mut self,
        source: &NodeHandle,
        target: FormTarget,
        request: Submission,
    ) -> Option<usize> {
        if let FormTarget::Frame(frame) = target {
            // A planned navigation supersedes an initial about:blank load or
            // an earlier submission that has not started yet.
            self.event_loop
                .cancel_resource_loads_for_nodes(&HashSet::from([frame.identity()]));
            self.pending_resource_loads.remove(&frame.identity());
            let frame_id = frame.identity();
            self.pending_iframe_visits.remove(&frame_id);
            let visit_source = self.visit_source_for_document(source.identity());
            self.event_loop.enqueue_timer_for_document(
                TimerPayload::FormSubmission {
                    node_id: frame.identity(),
                    request,
                    visit_source,
                },
                Some(source.identity()),
            );
            return Some(frame_id);
        } else if matches!(target, FormTarget::TopLevel) {
            let visit_source = self.visit_source_for_document(source.identity());
            self.event_loop.enqueue_navigation_from_source(
                NavigationRequest::FormSubmit {
                    url: request.url,
                    method: request.method,
                    body: request.body,
                    content_type: request.content_type,
                },
                visit_source,
            );
        }
        None
    }

    /// Loads a form submission's response into a child browsing context.
    ///
    /// GET submissions share [`HostState::load_iframe_document`], including its
    /// `about:blank` fallback on fetch failure. A POST propagates request and
    /// transport errors instead, always commits the response's effective URL
    /// (or the submission URL), and keeps its CSP headers even when the content
    /// type is not rendered.
    pub(super) fn load_iframe_form_submission(
        &mut self,
        submission: &Submission,
    ) -> Result<LoadedChildDocument, JsHostError> {
        if submission.method.eq_ignore_ascii_case("GET") {
            let site = self.location_href.parse::<crate::http::Url>().ok();
            return Ok(self.load_iframe_document(&submission.url, site.as_ref()));
        }
        let url: crate::http::Url = submission.url.parse()?;
        let mut request = crate::http::HttpRequest::new(crate::http::Method::Post, url);
        if let Ok(site) = self.location_href.parse::<crate::http::Url>() {
            request.set_cookie_context(site, false);
        }
        if let Some(body) = &submission.body {
            request.set_body(body.clone());
        }
        if let Some(content_type) = &submission.content_type {
            request.set_header("Content-Type", content_type);
        }
        let response = self.http_client.send(request)?;
        let mime_type = response.header("Content-Type").unwrap_or("");
        let document = child_document::parse_child_document(mime_type, response.body())
            .unwrap_or_else(blank_html_document);
        let url = response
            .effective_url()
            .map_or_else(|| submission.url.clone(), ToString::to_string);
        Ok(LoadedChildDocument {
            document,
            csp_headers: child_document::response_csp_headers(&response),
            url: Some(url),
        })
    }
}

/// A text response displays its bytes as text, never as parsed markup.
pub(super) fn plain_text_document(bytes: &[u8]) -> NodeHandle {
    let document = blank_html_document();
    let body = document.query_selector("body").expect("blank HTML body");
    let pre = NodeHandle::element("pre");
    pre.append_child(NodeHandle::text(
        String::from_utf8_lossy(bytes).into_owned(),
    ));
    body.append_child(pre);
    document
}

/// Private history transport: serialized requests live only in the facade's
/// closure, so truncating history releases request bodies without retaining DOMs.
pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_resolve_form_action"),
        2,
        NativeFunction::from_copy_closure(|_, args, context| {
            let form_id = parse_node_id(args.first(), context)?;
            ensure_same_origin_node(context, form_id)?;
            let reference = args
                .get(1)
                .unwrap_or(&JsValue::undefined())
                .to_string(context)?
                .to_std_string_escaped();
            with_host_state(|host| {
                let state = host.borrow();
                let form = state
                    .get_node(form_id)
                    .ok_or_else(|| JsNativeError::typ().with_message("form not found"))?;
                let document = owner_document_for_node(&form).ok_or_else(|| {
                    JsNativeError::typ().with_message("form has no owner document")
                })?;
                let base = crate::paint::stylesheet::extract_document_base_url(
                    &document,
                    state.base_url_for_document(document.identity()).as_ref(),
                );
                Ok(js_string!(resolve_url_reference(&reference, base.as_ref())).into())
            })
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_window_name"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            let document = parse_node_id(args.first(), context)?;
            ensure_same_origin_document(context, document)?;
            let next = if args.len() > 1 {
                Some(args[1].to_string(context)?.to_std_string_escaped())
            } else {
                None
            };
            with_host_state(|host| {
                let mut state = host.borrow_mut();
                let Some(key) = state.context_key_for_document(document) else {
                    return Ok(js_string!("").into());
                };
                if let Some(name) = next {
                    state.browsing_context_names.insert(key, name);
                }
                Ok(js_string!(
                    state
                        .browsing_context_names
                        .get(&key)
                        .map(String::as_str)
                        .unwrap_or("")
                )
                .into())
            })
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_iframe_submission_snapshot"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            let id = parse_node_id(args.first(), context)?;
            ensure_same_origin_node(context, id)?;
            with_host_state(|host| {
                let state = host.borrow();
                let Some(request) = state
                    .iframe_documents
                    .get(&id)
                    .and_then(|entry| entry.submission.as_ref())
                else {
                    return Ok(JsValue::null());
                };
                let json = serde_json::to_string(request)
                    .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
                Ok(js_string!(json).into())
            })
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_iframe_replay_submission"),
        3,
        NativeFunction::from_copy_closure(|_, args, context| {
            let id = parse_node_id(args.first(), context)?;
            ensure_same_origin_node(context, id)?;
            let json = args
                .get(1)
                .unwrap_or(&JsValue::undefined())
                .to_string(context)?
                .to_std_string_escaped();
            let mut request: Submission = serde_json::from_str(&json)
                .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
            request.url = args
                .get(2)
                .unwrap_or(&JsValue::undefined())
                .to_string(context)?
                .to_std_string_escaped();
            with_host_state(|host| {
                let mut state = host.borrow_mut();
                let Some(frame) = state.get_node(id) else {
                    return Ok(JsValue::undefined());
                };
                if !state.node_is_in_active_document(&frame) {
                    return Ok(JsValue::undefined());
                }
                state
                    .event_loop
                    .cancel_resource_loads_for_nodes(&HashSet::from([id]));
                state.pending_resource_loads.remove(&id);
                state
                    .iframe_document_with_submission(&frame, Some(&request))
                    .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
                state.schedule_connected_resource_loads(&frame, true);
                Ok(JsValue::undefined())
            })
        }),
    )?;
    Ok(())
}
