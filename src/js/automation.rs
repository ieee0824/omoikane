//! Opt-in host-driven automation shared by active Document realms.
use super::*;
use boa_engine::native_function::NativeFunctionPointer;
use boa_gc::Rooted;

#[derive(Default)]
pub(super) struct State {
    enabled: bool,
    drivers: HashMap<usize, JsValue>,
}

impl State {
    pub(super) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(super) unsafe fn trace(&self, tracer: &mut Tracer) {
        for driver in self.drivers.values() {
            unsafe { driver.trace(tracer) };
        }
    }

    pub(super) fn retire_document(&mut self, document: usize) {
        self.drivers.remove(&document);
    }
}

impl JsRuntime {
    /// Enables test-driver bindings in this runtime and future Document realms.
    /// Ordinary page runtimes expose none of these privileged commands.
    pub fn enable_test_automation(&mut self) -> JsResult<()> {
        self.host_state.borrow_mut().input_bridge.automation.enabled = true;
        self.with_active_host(|context| register(context, true))
    }

    /// Advances one queued input tick in each active Document's test driver.
    /// Activation-producing input and its microtasks use the trusted-input scope;
    /// polling and non-activation input do not confer Storage Access activation.
    /// Returns the longest requested tick duration, or `None` when all are idle.
    pub fn advance_test_automation(&mut self) -> JsResult<Option<u64>> {
        let mut documents: Vec<_> = self
            .host_state
            .borrow()
            .input_bridge
            .automation
            .drivers
            .keys()
            .copied()
            .collect();
        documents.sort_unstable();
        let mut duration = None;
        for document in documents {
            let driver = {
                let state = self.host_state.borrow();
                state
                    .document_is_active(document)
                    .then(|| {
                        state
                            .input_bridge
                            .automation
                            .drivers
                            .get(&document)
                            .cloned()
                    })
                    .flatten()
            };
            let Some(driver) = driver else {
                continue;
            };
            // Polling can replace the registry entry before queued jobs collect.
            // Keep this tick's callback rooted through both phases and their jobs.
            let driver = Rooted::new(driver);
            let poll = self.call_test_driver(document, &driver, false)?;
            self.run_jobs()?;
            if poll.is_null() || poll.is_undefined() {
                continue;
            }
            let activation = poll.as_boolean().ok_or_else(|| {
                JsNativeError::typ().with_message("Invalid automation input poll")
            })?;
            if !self.host_state.borrow().document_is_active(document) {
                continue;
            }
            let execute = |runtime: &mut Self| {
                let value = runtime.call_test_driver(document, &driver, true)?;
                runtime.run_jobs()?;
                Ok(value)
            };
            let value = if activation {
                self.with_trusted_storage_access_input(execute)?
            } else {
                execute(self)?
            };
            if let Some(ms) = value.as_number() {
                if !ms.is_finite() || ms < 0.0 || ms.fract() != 0.0 || ms > 9_007_199_254_740_991.0
                {
                    return Err(JsNativeError::typ()
                        .with_message("Invalid automation tick duration")
                        .into());
                }
                duration = Some(duration.unwrap_or(0).max(ms as u64));
            } else if !value.is_null() && !value.is_undefined() {
                return Err(JsNativeError::typ()
                    .with_message("Invalid automation tick result")
                    .into());
            }
        }
        Ok(duration)
    }

    fn call_test_driver(
        &mut self,
        document: usize,
        driver: &JsValue,
        execute: bool,
    ) -> JsResult<JsValue> {
        let realm = if document == self.document().identity() {
            self.host_state.borrow().main_realm.clone()
        } else {
            self.existing_realm_for_document(document)
        };
        let Some(realm) = realm else {
            return Ok(JsValue::null());
        };
        let previous = self.context.enter_realm(realm);
        let _document = activate_module_document(&self.host_state, document);
        let result = self.with_active_host(|context| {
            driver.as_callable().expect("registered test driver").call(
                &JsValue::undefined(),
                &[execute.into()],
                context,
            )
        });
        self.context.enter_realm(previous);
        result
    }
}

pub(super) fn register(context: &mut Context, enabled: bool) -> JsResult<()> {
    if !enabled {
        return Ok(());
    }
    for (name, length, callback) in [
        (
            "__omoikane_automation_register_driver",
            1,
            register_driver as NativeFunctionPointer,
        ),
        (
            "__omoikane_automation_route_input",
            4,
            route_input as NativeFunctionPointer,
        ),
        (
            "__omoikane_automation_validate_input",
            1,
            validate_input as NativeFunctionPointer,
        ),
        (
            "__omoikane_automation_set_permission",
            2,
            set_permission as NativeFunctionPointer,
        ),
        (
            "__omoikane_automation_delete_all_cookies",
            0,
            delete_cookies as NativeFunctionPointer,
        ),
    ] {
        context.register_global_builtin_callable(
            js_string!(name),
            length,
            NativeFunction::from_fn_ptr(callback),
        )?;
    }
    Ok(())
}

fn active_document(context: &Context) -> JsResult<usize> {
    let document = caller_document_id(context)
        .ok_or_else(|| JsNativeError::typ().with_message("No automation Document"))?;
    with_host_state(|host| {
        let state = host.borrow();
        if !state.input_bridge.automation.enabled || !state.document_is_active(document) {
            return Err(JsNativeError::typ()
                .with_message("Automation Document is inactive")
                .into());
        }
        Ok(document)
    })
}

fn register_driver(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let document = active_document(context)?;
    let callback = args
        .first()
        .filter(|value| value.is_callable())
        .cloned()
        .ok_or_else(|| JsNativeError::typ().with_message("Test driver callback required"))?;
    with_host_state(|host| {
        host.borrow_mut()
            .input_bridge
            .automation
            .drivers
            .insert(document, callback);
        Ok(JsValue::undefined())
    })
}

// Resolve a genuine wrapper before selecting its Document. Author-visible
// numeric node IDs alone are not authority to route trusted input.
fn input_document(element: &JsValue, context: &mut Context) -> JsResult<usize> {
    let resolver =
        with_host_state(|host| Ok(host.borrow().canonical_node_identity_resolver.clone()))?
            .ok_or_else(|| JsNativeError::typ().with_message("No node identity resolver"))?;
    let identity = resolver
        .as_callable()
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid node identity resolver"))?
        .call(&JsValue::undefined(), &[element.clone()], context)?
        .as_number()
        .and_then(JsRuntime::accessibility_node_identity)
        .ok_or_else(|| JsNativeError::typ().with_message("Input target is not a DOM element"))?;
    let document = with_host_state(|host| {
        let state = host.borrow();
        let node = state
            .get_node(identity)
            .filter(|node| node.node_type() == NodeType::Element)
            .filter(|node| state.node_is_in_active_document(node))
            .ok_or_else(|| JsNativeError::typ().with_message("Input target is stale"))?;
        document_root_for_node(&node)
            .map(|document| document.identity())
            .ok_or_else(|| {
                JsNativeError::typ()
                    .with_message("Input target has no Document")
                    .into()
            })
    })?;
    ensure_same_origin_document(context, document)?;
    Ok(document)
}

fn validate_input(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let caller = active_document(context)?;
    let element = args.first().cloned().unwrap_or_default();
    if input_document(&element, context)? != caller {
        return Err(JsNativeError::typ()
            .with_message("Input target changed Document")
            .into());
    }
    Ok(JsValue::undefined())
}

fn check_input_document(element: &JsValue, expected: usize, context: &mut Context) -> JsResult<()> {
    active_document(context)?;
    if input_document(element, context)? != expected {
        return Err(JsNativeError::typ()
            .with_message("Input target changed Document")
            .into());
    }
    Ok(())
}

fn route_input(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    active_document(context)?;
    let element = args.first().cloned().unwrap_or_default();
    let method = string_argument(args.get(1), "", context)?;
    if !matches!(method.as_str(), "click" | "send_keys") {
        return Err(JsNativeError::typ()
            .with_message("Unsupported input command")
            .into());
    }
    let installer = string_argument(args.get(3), "", context)?;
    let document = input_document(&element, context)?;
    let (host, realm) = with_host_state(|host| {
        let realm = active_document_realm(&host.borrow(), document).ok_or_else(|| {
            JsNativeError::typ().with_message("Input Document has no active Realm")
        })?;
        Ok((host.clone(), realm))
    })?;
    let previous = context.enter_realm(realm);
    let _document = activate_module_document(&host, document);
    let result = (|| {
        let installed = host
            .borrow()
            .input_bridge
            .automation
            .drivers
            .contains_key(&document);
        if !installed {
            // The opt-in runner supplies its own installer, including for a
            // scriptless about:blank child. No author eval permission is used.
            context.eval(Source::from_bytes(&format!("({installer})()")))?;
        }
        check_input_document(&element, document, context)?;
        let driver = context
            .global_object()
            .get(js_string!("__wpt_enqueue_element_input"), context)?;
        check_input_document(&element, document, context)?;
        driver
            .as_callable()
            .ok_or_else(|| JsNativeError::typ().with_message("Input driver is unavailable"))?
            .call(
                &JsValue::undefined(),
                &[
                    element,
                    js_string!(method.as_str()).into(),
                    args.get(2).cloned().unwrap_or_default(),
                ],
                context,
            )
    })();
    context.enter_realm(previous);
    result
}

fn set_permission(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let document = active_document(context)?;
    let name = string_argument(args.first(), "", context)?;
    if matches!(name.as_str(), "notifications" | "push") {
        let permission = match string_argument(args.get(1), "", context)?.as_str() {
            "prompt" => "default",
            "granted" => "granted",
            "denied" => "denied",
            _ => {
                return Err(JsNativeError::typ()
                    .with_message("Invalid permission state")
                    .into());
            }
        };
        with_host_state(|host| {
            host.borrow_mut().notification_permission = permission.to_owned();
            Ok(())
        })?;
        return Ok(JsValue::undefined());
    }
    if name == "screen-wake-lock" {
        let granted = match string_argument(args.get(1), "", context)?.as_str() {
            "granted" => true,
            "prompt" | "denied" => false,
            _ => {
                return Err(JsNativeError::typ()
                    .with_message("Invalid permission state")
                    .into());
            }
        };
        with_host_state(|host| {
            host.borrow_mut().wake_lock.set_permission(granted);
            Ok(())
        })?;
        return Ok(JsValue::undefined());
    }
    if matches!(name.as_str(), "camera" | "microphone") {
        let granted = match string_argument(args.get(1), "", context)?.as_str() {
            "granted" => true,
            "prompt" | "denied" => false,
            _ => {
                return Err(JsNativeError::typ()
                    .with_message("Invalid permission state")
                    .into());
            }
        };
        with_host_state(|host| {
            host.borrow_mut().media_capture.set_permission(granted);
            Ok(())
        })?;
        return Ok(JsValue::undefined());
    }
    if name != "storage-access" {
        return Err(JsNativeError::typ()
            .with_message("Unsupported automation permission")
            .into());
    }
    let permission = match string_argument(args.get(1), "", context)?.as_str() {
        "prompt" => storage_access::StorageAccessPermission::Prompt,
        "granted" => storage_access::StorageAccessPermission::Granted,
        "denied" => storage_access::StorageAccessPermission::Denied,
        _ => {
            return Err(JsNativeError::typ()
                .with_message("Invalid permission state")
                .into());
        }
    };
    with_host_state(|host| {
        host.borrow_mut()
            .set_storage_access_permission(document, permission)?;
        Ok(())
    })?;
    storage_access::notify_permission_changes(context);
    Ok(JsValue::undefined())
}

fn delete_cookies(_: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let document = active_document(context)?;
    with_host_state(|host| {
        let state = host.borrow();
        let url = state
            .base_url_for_document(document)
            .ok_or_else(|| JsNativeError::typ().with_message("Document has no cookie URL"))?;
        state
            .cookie_store
            .lock()
            .unwrap()
            .delete_associated_cookies(&url, std::time::SystemTime::now());
        Ok(JsValue::undefined())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_pages_have_no_input_routing_binding() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_eq!(
            runtime
                .eval("typeof __omoikane_automation_route_input")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "undefined"
        );
    }

    #[test]
    fn opaque_child_element_cannot_be_routed_from_its_parent() {
        let document = crate::html::TreeBuilder::parse("<body></body>").document();
        let mut runtime = JsRuntime::with_document_and_url(document, "https://top.test/").unwrap();
        runtime.enable_test_automation().unwrap();
        runtime.eval("var frame = document.createElement('iframe'); frame.sandbox = 'allow-scripts'; document.body.appendChild(frame);").unwrap();
        runtime.tick(0).unwrap();
        let (iframe, document) = {
            let host = runtime.host_state.borrow();
            let (iframe, entry) = host.iframe_documents.iter().next().unwrap();
            (*iframe, entry.document.identity())
        };
        let realm = runtime.ensure_iframe_realm(iframe, document).unwrap();
        let previous = runtime.context.enter_realm(realm);
        // Obtain the genuine foreign wrapper through the host only: ordinary
        // parent script cannot read this opaque Document in the first place.
        let element = runtime
            .with_active_host(|context| {
                context.eval(Source::from_bytes(
                    "document.body.appendChild(document.createElement('button'))",
                ))
            })
            .unwrap();
        runtime.context.enter_realm(previous);
        assert!(
            runtime
                .with_active_host(|context| input_document(&element, context))
                .is_err()
        );
        assert_eq!(runtime.advance_test_automation().unwrap(), None);
    }
}
