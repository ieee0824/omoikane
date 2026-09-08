//! Reentrant document.write parsing and parser-inserted script execution.

use super::*;
use crate::html::WriteParser;

const MAX_WRITE_DEPTH: usize = 32;

/// Script preparation captures the owner and source before page code can
/// remove the element or change its attributes while a fetch is pending.
#[derive(Debug)]
pub(super) struct WrittenScript {
    pub(super) node: NodeHandle,
    pub(super) document: NodeHandle,
    kind: ScriptKind,
    src: Option<String>,
    inline_source: String,
    deferred: bool,
    async_script: bool,
}

impl WrittenScript {
    fn new(document: &NodeHandle, node: &NodeHandle) -> Self {
        let kind = ScriptKind::from_type_attribute(node.get_attribute("type").as_deref());
        Self {
            node: node.clone(),
            document: document.clone(),
            kind,
            src: node.get_attribute("src"),
            inline_source: collect_text_content(node),
            deferred: kind == ScriptKind::Module
                || node.get_attribute("async").is_some()
                || node.get_attribute("defer").is_some(),
            async_script: node.get_attribute("async").is_some(),
        }
    }
}

#[derive(Debug)]
pub(super) struct WriteState {
    parser: WriteParser,
    anchor_id: Option<usize>,
    explicit_open: bool,
    blocking_script: Option<usize>,
    finish_requested: bool,
    active_scripts: usize,
}

impl WriteState {
    pub(super) fn new(
        document: NodeHandle,
        anchor: Option<NodeHandle>,
        explicit_open: bool,
    ) -> Self {
        Self {
            anchor_id: anchor.as_ref().map(NodeHandle::identity),
            parser: WriteParser::new(document, anchor),
            explicit_open,
            blocking_script: None,
            finish_requested: false,
            active_scripts: 0,
        }
    }

    pub(super) fn is_executing(&self) -> bool {
        self.active_scripts > 0
    }
}

struct ScriptExecution(Rc<RefCell<WriteState>>);

impl ScriptExecution {
    fn new(parser: Rc<RefCell<WriteState>>) -> Self {
        parser.borrow_mut().active_scripts += 1;
        Self(parser)
    }
}

impl Drop for ScriptExecution {
    fn drop(&mut self) {
        self.0.borrow_mut().active_scripts -= 1;
    }
}

struct WriteDepth(Rc<RefCell<HostState>>);

impl Drop for WriteDepth {
    fn drop(&mut self) {
        self.0.borrow_mut().document_write_depth -= 1;
    }
}

pub(super) fn write(
    state: &Rc<RefCell<HostState>>,
    document_id: usize,
    input: &str,
    eof: bool,
    context: &mut Context,
) -> JsResult<JsValue> {
    let (document, parser) = {
        let mut host = state.borrow_mut();
        if host.document_write_depth >= MAX_WRITE_DEPTH {
            return Err(JsNativeError::range()
                .with_message("document.write recursion limit exceeded")
                .into());
        }
        // close() during a reentrant script must not finish the outer stream.
        if eof
            && host
                .write_parsers
                .get(&document_id)
                .is_none_or(|parser| parser.borrow().is_executing())
        {
            return Ok(JsValue::undefined());
        }
        let document = host
            .get_node(document_id)
            .filter(|node| node.node_type() == NodeType::Document)
            .ok_or_else(|| {
                JsNativeError::reference().with_message("document is no longer available")
            })?;
        let anchor = host
            .write_insertion_ref
            .clone()
            .filter(|node| document_root_for_node(node).as_ref() == Some(&document));
        let anchor_id = anchor.as_ref().map(NodeHandle::identity);
        let replace = host.write_parsers.get(&document_id).is_none_or(|parser| {
            let parser = parser.borrow();
            host.document_write_depth == 0 && !parser.explicit_open && parser.anchor_id != anchor_id
        });
        if replace {
            host.write_parsers.insert(
                document_id,
                Rc::new(RefCell::new(WriteState::new(
                    document.clone(),
                    anchor,
                    false,
                ))),
            );
        }
        let parser = host.write_parsers[&document_id].clone();
        host.document_write_depth += 1;
        (document, parser)
    };
    let _depth = WriteDepth(state.clone());
    parser.borrow_mut().parser.push_input(input);
    loop {
        if !state
            .borrow()
            .write_parsers
            .get(&document_id)
            .is_some_and(|current| Rc::ptr_eq(current, &parser))
        {
            break;
        }
        if parser.borrow().blocking_script.is_some() {
            parser.borrow_mut().finish_requested |= eof;
            return Ok(JsValue::undefined());
        }
        let script = parser.borrow_mut().parser.advance(eof);
        let created = parser.borrow_mut().parser.take_created_nodes();
        {
            let mut host = state.borrow_mut();
            for node in &created {
                host.nodes.insert(node.identity(), node.clone());
                if let Some(content) = node.template_content() {
                    host.nodes.insert(content.identity(), content);
                }
                if matches!(node.tag_name().as_deref(), Some("iframe" | "object")) {
                    host.schedule_connected_resource_loads(node, false);
                }
            }
            host.mark_document_style_dirty(&document);
        }
        let Some(script) = script else {
            break;
        };
        let Some(prepared) = prepare_script(state, &document, &script, &parser) else {
            continue;
        };
        let tail = parser.borrow_mut().parser.take_pending_input();
        let result = {
            let _execution = ScriptExecution::new(parser.clone());
            execute_classic(state, &prepared, context)
        };
        parser.borrow_mut().parser.push_input(&tail);
        if let Err(error) = result {
            if is_wall_clock_timeout(&error) {
                return Err(error);
            }
            record_error(state, &error);
        }
    }
    if eof {
        state.borrow_mut().write_parsers.remove(&document_id);
    }
    Ok(JsValue::undefined())
}

fn record_error(state: &Rc<RefCell<HostState>>, error: &JsError) {
    let mut host = state.borrow_mut();
    if host.task_errors.len() < MAX_TASK_ERRORS {
        host.task_errors
            .push(format!("[document.write script] {error}"));
    } else {
        host.suppressed_task_errors = host.suppressed_task_errors.saturating_add(1);
    }
}

fn prepare_script(
    state: &Rc<RefCell<HostState>>,
    document: &NodeHandle,
    script: &NodeHandle,
    parser: &Rc<RefCell<WriteState>>,
) -> Option<WrittenScript> {
    // Template contents and inert documents do not execute page scripts.
    if document_root_for_node(script).as_ref() != Some(document) {
        return None;
    }
    let prepared = WrittenScript::new(document, script);
    let kind = prepared.kind;
    if kind == ScriptKind::NotExecutable {
        return None;
    }
    {
        let mut host = state.borrow_mut();
        if !host.parser_inserted_scripts.insert(script.identity()) {
            return None;
        }
        if document != &host.document
            && !host
                .iframe_documents
                .values()
                .any(|entry| entry.document == *document)
        {
            return None;
        }
        if !host.sandbox_allows_scripts_for_node(script) {
            return None;
        }
        if kind == ScriptKind::Module || script.get_attribute("src").is_some() {
            if kind != ScriptKind::Module
                && script.get_attribute("async").is_none()
                && script.get_attribute("defer").is_none()
            {
                parser.borrow_mut().blocking_script = Some(script.identity());
            }
            host.written_script_queue.push_back(prepared);
            if host.pending_resource_loads.insert(script.identity()) {
                host.event_loop.enqueue_timer(TimerPayload::ResourceLoad {
                    node_id: script.identity(),
                });
            }
            return None;
        }
    }
    Some(prepared)
}

fn execute_classic(
    state: &Rc<RefCell<HostState>>,
    prepared: &WrittenScript,
    context: &mut Context,
) -> JsResult<()> {
    let document = &prepared.document;
    let script = &prepared.node;
    let started = std::time::Instant::now();
    let fetched = script_source(state, prepared);
    let elapsed = started.elapsed().as_secs_f64() * 1_000.0;
    let Some(realm) = document_realm(state, document.identity(), context)? else {
        return Ok(());
    };
    let previous_realm = context.enter_realm(realm);
    let Some((source, url)) = fetched else {
        let result = if let Some(src) = prepared.src.as_ref() {
            dispatch_script_event(context, script, "error", &src, false, elapsed)
        } else {
            Ok(())
        };
        context.enter_realm(previous_realm);
        return result;
    };
    let old_current = context.eval(Source::from_bytes(
        "document.currentScript && document.currentScript.__id",
    ));
    let result = (|| {
        context.eval(Source::from_bytes(&format!(
            "__omoikane_set_current_script({})",
            script.identity()
        )))?;
        context.eval(Source::from_reader(
            source.as_bytes(),
            url.as_deref().map(Path::new),
        ))
    })();
    state.borrow_mut().pending_javascript_dialog = None;
    let restore = old_current
        .ok()
        .and_then(|value| value.as_number())
        .map(|id| id.to_string())
        .unwrap_or_else(|| "null".to_string());
    let _ = context.eval(Source::from_bytes(&format!(
        "__omoikane_set_current_script({restore})"
    )));
    let dispatched = if let Some(url) = url {
        let base = state.borrow().base_url_for_document(document.identity());
        let redirected = resource_reference_was_redirected(
            prepared.src.as_deref().unwrap_or_default(),
            &url,
            base.as_ref(),
        );
        dispatch_script_event(context, script, "load", &url, redirected, elapsed)
    } else {
        Ok(())
    };
    context.enter_realm(previous_realm);
    result.map(|_| ()).and(dispatched)
}

fn dispatch_script_event(
    context: &mut Context,
    script: &NodeHandle,
    event: &str,
    url: &str,
    redirected: bool,
    elapsed: f64,
) -> JsResult<()> {
    context.eval(Source::from_bytes("__omoikane_wire_inline_handlers()"))?;
    context.eval(Source::from_bytes(&dispatch_resource_timing_script(
        event,
        script.identity(),
        url,
        redirected,
        elapsed,
    )))?;
    Ok(())
}

impl JsRuntime {
    /// Execute queued written scripts at a parser checkpoint. Modules wait
    /// until parsing is complete; blocking classic scripts resume their stream.
    pub(super) fn run_written_scripts(&mut self, include_deferred: bool) -> JsResult<()> {
        loop {
            let next = self
                .host_state
                .borrow()
                .written_script_queue
                .iter()
                .find(|script| (include_deferred && !script.async_script) || !script.deferred)
                .map(|script| script.node.identity());
            let Some(id) = next else {
                break;
            };
            self.run_written_script(id)?;
        }
        Ok(())
    }

    pub(super) fn run_written_scripts_before(&mut self, reference: &NodeHandle) -> JsResult<()> {
        let Some(document) = document_root_for_node(reference) else {
            return Ok(());
        };
        let earlier: HashSet<_> = collect_script_elements(&document)
            .into_iter()
            .take_while(|script| script != reference)
            .map(|script| script.identity())
            .collect();
        loop {
            let next = self
                .host_state
                .borrow()
                .written_script_queue
                .iter()
                .find(|script| !script.async_script && earlier.contains(&script.node.identity()))
                .map(|script| script.node.identity());
            let Some(id) = next else {
                break;
            };
            self.run_written_script(id)?;
        }
        Ok(())
    }

    fn run_written_module(&mut self, prepared: &WrittenScript) -> JsResult<()> {
        let document = &prepared.document;
        let script = &prepared.node;
        let document_id = document.identity();
        let state = self.host_state.clone();
        let realm =
            self.with_active_host(|context| document_realm(&state, document_id, context))?;
        let Some(realm) = realm else {
            return Ok(());
        };
        let started = std::time::Instant::now();
        let fetched = script_source(&state, prepared);
        let elapsed = started.elapsed().as_secs_f64() * 1_000.0;
        let previous = self.context.enter_realm(realm);
        let result = if let Some((source, fetched_url)) = fetched {
            let base = state.borrow().base_url_for_document(document_id);
            let url = fetched_url.unwrap_or_else(|| {
                module_script_url(
                    &format!("written-{}", script.identity()),
                    base.as_ref(),
                    true,
                )
            });
            let redirected = prepared
                .src
                .as_ref()
                .is_some_and(|src| resource_reference_was_redirected(src, &url, base.as_ref()));
            let _ = self.eval("__omoikane_set_current_script(null)");
            let (result, _, _) = self.eval_module_timed(&source, &url, document.clone());
            let dispatched = self.with_active_host(|context| {
                dispatch_script_event(
                    context,
                    script,
                    if result.is_ok() { "load" } else { "error" },
                    &url,
                    redirected,
                    elapsed,
                )
            });
            result
                .map(|_| ())
                .map_err(|message| JsError::from(JsNativeError::error().with_message(message)))
                .and(dispatched)
        } else {
            let url = prepared.src.clone().unwrap_or_default();
            self.with_active_host(|context| {
                dispatch_script_event(context, script, "error", &url, false, elapsed)
            })
        };
        self.context.enter_realm(previous);
        result
    }

    pub(super) fn run_written_script(&mut self, id: usize) -> JsResult<()> {
        let prepared = {
            let mut state = self.host_state.borrow_mut();
            state.pending_resource_loads.remove(&id);
            let Some(index) = state
                .written_script_queue
                .iter()
                .position(|script| script.node.identity() == id)
            else {
                return Ok(());
            };
            state.written_script_queue.remove(index).unwrap()
        };
        let document = &prepared.document;
        let document_id = document.identity();
        if document != &self.document()
            && !self
                .host_state
                .borrow()
                .iframe_documents
                .values()
                .any(|entry| entry.document == *document)
        {
            return Ok(());
        }
        let parser = self
            .host_state
            .borrow()
            .write_parsers
            .get(&document_id)
            .cloned();
        let resume = parser.filter(|parser| parser.borrow().blocking_script == Some(id));
        let tail = resume.as_ref().map(|parser| {
            let mut parser = parser.borrow_mut();
            parser.blocking_script = None;
            parser.parser.take_pending_input()
        });
        let kind = prepared.kind;
        let result = if kind == ScriptKind::Module {
            self.run_written_module(&prepared)
        } else {
            let state = self.host_state.clone();
            // Reentrant writes belong to this parser even though the calling
            // top-level script has returned and no longer has an insertion ref.
            let _depth = resume.as_ref().map(|_| {
                state.borrow_mut().document_write_depth += 1;
                WriteDepth(state.clone())
            });
            let _execution = resume
                .as_ref()
                .map(|parser| ScriptExecution::new(parser.clone()));
            self.with_active_host(|context| execute_classic(&state, &prepared, context))
        };
        // The script has returned: run its microtasks before parsing its tail
        // or starting the next parser-blocking script.
        let jobs = self.with_active_host(|context| context.run_jobs());
        if let Some(parser) = resume {
            parser
                .borrow_mut()
                .parser
                .push_input(tail.as_deref().unwrap_or_default());
            let eof = parser.borrow().finish_requested;
            // Do not let the checkpoint's absent insertion ref replace this stream.
            let state = self.host_state.clone();
            state.borrow_mut().document_write_depth += 1;
            let depth = WriteDepth(state.clone());
            // Finishing is delayed until after the resume so close's reentrant
            // no-op rule does not discard an earlier close request.
            let resumed =
                self.with_active_host(|context| write(&state, document_id, "", false, context));
            drop(depth);
            resumed?;
            if eof {
                self.with_active_host(|context| write(&state, document_id, "", true, context))?;
            }
        }
        if let Err(error) = result {
            if is_wall_clock_timeout(&error) {
                return Err(error);
            }
            record_error(&self.host_state, &error);
        }
        jobs
    }
}

pub(super) fn document_realm(
    state: &Rc<RefCell<HostState>>,
    document_id: usize,
    context: &mut Context,
) -> JsResult<Option<Realm>> {
    let iframe_id = {
        let host = state.borrow();
        if host.document.identity() == document_id {
            return Ok(host.main_realm.clone());
        }
        host.iframe_documents
            .iter()
            .find(|(_, entry)| entry.document.identity() == document_id)
            .map(|(id, _)| *id)
    };
    iframe_id
        .map(|id| ensure_iframe_realm(context, state, id, document_id))
        .transpose()
}

/// Share the existing request and redirect CSP checks with every written script.
fn script_source(
    state: &Rc<RefCell<HostState>>,
    prepared: &WrittenScript,
) -> Option<(String, Option<String>)> {
    let document = &prepared.document;
    let mut host = state.borrow_mut();
    if !host.sandbox_allows_scripts_for_node(document) {
        return None;
    }
    let policy = host.csp_policy_for_document(document);
    if let Some(src) = prepared.src.as_ref() {
        if !policy.allows_reference(ResourceType::Script, &src) {
            host.record_csp_violation_for_node(document, ResourceType::Script, src);
            return None;
        }
        let base = host.base_url_for_document(document.identity());
        let Some((url, source, redirects)) =
            fetch_script_resource_with_client(&src, base.as_ref(), &mut host.http_client)
        else {
            return None;
        };
        if !policy.allows_reference_after_redirects(ResourceType::Script, &url, redirects) {
            host.record_csp_violation_for_node(document, ResourceType::Script, url);
            return None;
        }
        Some((source, Some(url)))
    } else if policy.allows_inline(ResourceType::Script) {
        Some((prepared.inline_source.clone(), None))
    } else {
        host.record_csp_violation_for_node(document, ResourceType::Script, "inline");
        None
    }
}
