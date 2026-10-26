//! Shares form-validation results with native selector matching and painting.
use super::*;

mod tree;

#[derive(Default)]
pub(super) struct State {
    callbacks: HashMap<usize, JsValue>,
    fresh_roots: HashSet<usize>,
    revision: u64,
    syncing: bool,
    #[cfg(test)]
    completed_scans: usize,
}

impl State {
    pub(super) unsafe fn trace(&self, tracer: &mut Tracer) {
        for callback in self.callbacks.values() {
            unsafe { callback.trace(tracer) };
        }
    }
    pub(super) fn invalidate(&mut self) {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("validation revision exhausted");
        self.fresh_roots.clear();
    }
    pub(super) fn retire_document(&mut self, document: usize) {
        self.callbacks.remove(&document);
        self.invalidate();
    }
}

pub(super) fn selector_uses_validation(selectors: &[Selector]) -> bool {
    use crate::css::SimpleSelector;
    selectors.iter().any(|selector| {
        selector.parts.iter().any(|part| {
            part.simples.iter().any(|simple| match simple {
                SimpleSelector::PseudoClass(name) => {
                    ["valid", "invalid", "user-valid", "user-invalid"]
                        .iter()
                        .any(|candidate| name.eq_ignore_ascii_case(candidate))
                }
                SimpleSelector::Is(list)
                | SimpleSelector::Where(list)
                | SimpleSelector::Not(list)
                | SimpleSelector::NthChild {
                    selectors: list, ..
                } => selector_uses_validation(list),
                SimpleSelector::Has(list) => list.iter().any(|relative| {
                    selector_uses_validation(std::slice::from_ref(&relative.selector))
                }),
                _ => false,
            })
        })
    })
}

pub(super) fn flush(node_id: usize, context: &mut Context) -> JsResult<()> {
    with_host_state(|host| {
        let Some((root, callback, revision)) = ({
            let state = host.borrow();
            let node = state.get_node(node_id);
            node.and_then(|node| {
                let mut root = node.clone();
                while let Some(parent) = root.parent_node() {
                    root = parent;
                }
                if state.form_validation.syncing
                    || state.form_validation.fresh_roots.contains(&root.identity())
                {
                    return None;
                }
                // A Document has no ownerDocument, but its validation callback
                // belongs to that Document's Realm, including inside an iframe.
                let document =
                    document_root_for_node(&node).or_else(|| state.node_lifetime_owner(node_id))?;
                let callback = state
                    .form_validation
                    .callbacks
                    .get(&document.identity())?
                    .clone();
                Some((root, callback, state.form_validation.revision))
            })
        }) else {
            return Ok(());
        };
        host.borrow_mut().form_validation.syncing = true;
        let result = callback.as_callable().expect("validation callback").call(
            &JsValue::undefined(),
            &[JsValue::from(root.identity() as f64)],
            context,
        );
        host.borrow_mut().form_validation.syncing = false;
        let value = result?;
        let json = value
            .as_string()
            .ok_or_else(|| JsNativeError::typ().with_message("Invalid validation snapshot"))?
            .to_std_string_escaped();
        let values: Vec<(usize, Option<bool>)> = serde_json::from_str(&json)
            .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
        // Derived selector state is render work: keep committed root pixels
        // available until their replacement is published. Author code above
        // still revokes through the dedicated listener, navigation and scroll
        // boundaries if it changes the input contract.
        let mut state = host.borrow_mut();
        #[cfg(test)]
        {
            state.form_validation.completed_scans += 1;
        }
        let stable = revision == state.form_validation.revision;
        let mut changed_documents = HashMap::new();
        for (id, value) in values {
            if let Some(node) = state.get_node(id) {
                if node.set_css_validity(value) {
                    if let Some(document) = document_root_for_node(&node) {
                        changed_documents.insert(document.identity(), document);
                    }
                }
            }
        }
        for document in changed_documents.values() {
            state.invalidate_document_style_cache(document);
        }
        if stable {
            // Only identities are cached: this never retains detached nodes.
            // Bound the cache for scripts querying many unrelated roots.
            if state.form_validation.fresh_roots.len() >= 128 {
                state.form_validation.fresh_roots.clear();
            }
            state.form_validation.fresh_roots.insert(root.identity());
        }
        Ok(())
    })
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    tree::register(context, bindings)?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_set_user_validity"),
        2,
        NativeFunction::from_copy_closure(|_, args, context| {
            let id = parse_node_id(args.first(), context)?;
            ensure_same_origin_node(context, id)?;
            let value = args.get(1).is_some_and(JsValue::to_boolean);
            with_host_state(|host| {
                let mut state = host.borrow_mut();
                if let Some(node) = state.get_node(id) {
                    if node.set_user_validity(value) {
                        state.invalidate_style_cache_for_node(&node);
                    }
                }
                Ok(JsValue::undefined())
            })
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_register_validation"),
        2,
        NativeFunction::from_copy_closure(|_, args, context| {
            let document = parse_node_id(args.first(), context)?;
            ensure_same_origin_document(context, document)?;
            let callback = args
                .get(1)
                .filter(|value| value.is_callable())
                .cloned()
                .ok_or_else(|| JsNativeError::typ().with_message("Validation callback required"))?;
            with_host_state(|state| {
                state
                    .borrow_mut()
                    .form_validation
                    .callbacks
                    .insert(document, callback);
                Ok(JsValue::undefined())
            })
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_flush_validation"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            let id = parse_node_id(args.first(), context)?;
            ensure_same_origin_node(context, id)?;
            flush(id, context)?;
            Ok(JsValue::undefined())
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_invalidate_validation"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            let id = parse_node_id(args.first(), context)?;
            ensure_same_origin_node(context, id)?;
            with_host_state(|state| {
                let mut state = state.borrow_mut();
                if let Some(node) = state.get_node(id) {
                    state.invalidate_style_cache_for_node(&node);
                }
                Ok(JsValue::undefined())
            })
        }),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene_runtime() -> JsRuntime {
        let document = crate::html::TreeBuilder::parse(
            "<style>body{height:1000px}input:valid{width:40px}\
             input:invalid{width:20px}</style><form><input required value=filled></form>\
             <iframe></iframe>",
        )
        .document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime.set_viewport(200.0, 100.0);
        runtime
            .eval("globalThis.control=document.querySelector('input');getComputedStyle(control).width")
            .unwrap();
        runtime
    }

    #[test]
    fn validation_scene_survives_validity_style_changes() {
        use native_scroll::{Offset, ScrollDecision};
        let mut runtime = scene_runtime();
        let handle =
            super::super::native_scroll_clock_tests::install_clock_test_scene(&mut runtime);
        let gate = handle.publication_gate();
        let generations = runtime.render_generations();
        for (message, width, valid) in [("invalid", "20px", false), ("", "40px", true)] {
            let result = runtime
                .eval(&format!(
                    "control.setCustomValidity('{}');getComputedStyle(control).width",
                    message
                ))
                .unwrap();
            assert_eq!(result.as_string().unwrap().to_std_string_escaped(), width);
            assert_eq!(
                runtime
                    .document()
                    .query_selector("input")
                    .unwrap()
                    .css_validity(),
                Some(valid)
            );
            assert_eq!(handle.publication_gate(), gate);
        }
        assert_ne!(runtime.render_generations(), generations);
        let accepted = match handle.scroll(Offset { x: 0.0, y: 10.0 }) {
            ScrollDecision::Accepted(accepted) => accepted,
            other => panic!("validity styling retired committed pixels: {other:?}"),
        };
        assert!(handle.validate_for_presentation(accepted));
        assert_eq!(runtime.eval("scrollY").unwrap().as_number(), Some(10.0));
    }

    #[test]
    fn validation_scene_survives_child_validity_changes() {
        use native_scroll::{Offset, ScrollDecision};
        let mut runtime = scene_runtime();
        runtime
            .eval(
                "globalThis.childWindow=document.querySelector('iframe').contentWindow;\
             globalThis.childControl=childWindow.document.createElement('input');\
             childControl.required=true;childControl.value='filled';\
             childWindow.document.body.appendChild(childControl);\
             childControl.matches(':valid')",
            )
            .unwrap();
        let handle =
            super::super::native_scroll_clock_tests::install_clock_test_scene(&mut runtime);
        let gate = handle.publication_gate();
        assert_eq!(
            runtime
                .eval("childControl.setCustomValidity('invalid');childControl.matches(':invalid')")
                .unwrap()
                .as_boolean(),
            Some(true)
        );
        assert_eq!(handle.publication_gate(), gate);
        assert!(matches!(
            handle.scroll(Offset { x: 0.0, y: 10.0 }),
            ScrollDecision::Accepted(_)
        ));
        assert_eq!(runtime.eval("scrollY").unwrap().as_number(), Some(10.0));
    }

    #[test]
    fn validation_scene_still_rejects_author_input_contract_changes() {
        use native_scroll::{Offset, ScrollDecision};
        for operation in [
            "window.addEventListener('wheel',()=>{}, {passive:false})",
            "window.scrollTo(0,30)",
        ] {
            let mut runtime = scene_runtime();
            let handle =
                super::super::native_scroll_clock_tests::install_clock_test_scene(&mut runtime);
            let gate = handle.publication_gate();
            let callback = runtime
                .eval(&format!(
                    "(function(){{{};return JSON.stringify([[control.__id,false]])}})",
                    operation
                ))
                .unwrap();
            {
                let mut state = runtime.host_state.borrow_mut();
                let id = state.document.identity();
                state.form_validation.callbacks.insert(id, callback);
                state.form_validation.invalidate();
            }
            runtime.eval("control.matches(':invalid')").unwrap();
            assert!(
                handle.publication_gate().policy_epoch > gate.policy_epoch,
                "{operation}"
            );
            assert!(
                !handle.authorize_policy(gate.key, gate.policy_epoch),
                "{operation}"
            );
            assert!(
                matches!(
                    handle.scroll(Offset { x: 0.0, y: 10.0 }),
                    ScrollDecision::Fallback(_)
                ),
                "{operation}"
            );
        }
    }

    #[test]
    fn computed_style_without_validity_changes_preserves_native_policy_epoch() {
        let document = crate::html::TreeBuilder::parse("<div>content</div>").document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        let handle = runtime.native_scroll_handle();
        let before = handle.publication_gate().policy_epoch;
        runtime
            .eval("getComputedStyle(document.querySelector('div')).overscrollBehaviorX")
            .unwrap();
        assert_eq!(handle.publication_gate().policy_epoch, before);
        assert_eq!(
            runtime.host_state.borrow().form_validation.completed_scans,
            1
        );
    }

    #[test]
    fn validity_rescans_update_selectors_without_retiring_committed_policy() {
        let document = crate::html::TreeBuilder::parse("<form><input required></form>").document();
        let input = document.query_selector("input").unwrap();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        let handle = runtime.native_scroll_handle();
        let before = handle.publication_gate().policy_epoch;
        runtime
            .eval("globalThis.control=document.querySelector('input');control.matches(':invalid')")
            .unwrap();
        assert_eq!(input.css_validity(), Some(false));
        assert_eq!(handle.publication_gate().policy_epoch, before);

        runtime.host_state.borrow_mut().form_validation.invalidate();
        let unchanged = handle.publication_gate().policy_epoch;
        runtime.eval("control.matches(':invalid')").unwrap();
        assert_eq!(handle.publication_gate().policy_epoch, unchanged);
        assert_eq!(
            runtime.host_state.borrow().form_validation.completed_scans,
            2
        );

        runtime.eval("control.value='filled'").unwrap();
        let before_change = handle.publication_gate();
        assert!(handle.authorize_policy(before_change.key, before_change.policy_epoch));
        runtime.eval("control.matches(':valid')").unwrap();
        assert_eq!(input.css_validity(), Some(true));
        assert_eq!(
            handle.publication_gate().policy_epoch,
            before_change.policy_epoch
        );
        assert!(handle.authorize_policy(before_change.key, before_change.policy_epoch));
        assert_eq!(
            runtime.host_state.borrow().form_validation.completed_scans,
            3
        );
    }

    #[test]
    fn repeated_css_queries_share_one_validation_scan_until_a_mutation() {
        let document = crate::html::TreeBuilder::parse("<form><input required></form>").document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime.eval("globalThis.input=document.querySelector('input'); for(let i=0;i<100;i++) { if(!input.matches(':invalid')) throw Error('invalid expected'); }").unwrap();
        assert_eq!(
            runtime.host_state.borrow().form_validation.completed_scans,
            1
        );
        runtime.eval("input.value='filled'; for(let i=0;i<100;i++) { if(!input.matches(':valid')) throw Error('valid expected'); }").unwrap();
        assert_eq!(
            runtime.host_state.borrow().form_validation.completed_scans,
            2
        );
    }

    #[test]
    fn inline_style_changes_keep_the_validation_snapshot_fresh() {
        let document = crate::html::TreeBuilder::parse("<form><input required></form>").document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        let scans =
            |runtime: &JsRuntime| runtime.host_state.borrow().form_validation.completed_scans;
        runtime.eval("globalThis.input=document.querySelector('input'); if(!input.matches(':invalid')) throw Error('invalid expected');").unwrap();
        assert_eq!(scans(&runtime), 1);
        runtime.eval("input.style.color='red'; input.setAttribute('style','color: blue'); input.setAttributeNS(null,'style','color: green'); if(!input.matches(':invalid')) throw Error('still invalid'); input.removeAttribute('style'); if(!input.matches(':invalid')) throw Error('still invalid');").unwrap();
        assert_eq!(
            scans(&runtime),
            1,
            "inline style cannot change constraint validity"
        );
        runtime.eval("input.removeAttribute('required'); if(!input.matches(':valid')) throw Error('valid expected');").unwrap();
        assert_eq!(scans(&runtime), 2, "a validity attribute change rescans");
    }
    #[test]
    fn ordinary_data_attribute_changes_keep_constraint_validation_snapshot_fresh() {
        let document = crate::html::TreeBuilder::parse("<form><input required></form>").document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime.eval("globalThis.input=document.querySelector('input'); if(!input.matches(':invalid')) throw Error('invalid expected');").unwrap();
        let initial = runtime.host_state.borrow().form_validation.completed_scans;
        runtime.eval("for(let i=0;i<100;i++){input.dataset.probe=String(i); if(!input.matches(':invalid')) throw Error('must stay invalid')} input.setAttribute('data-other','yes');input.removeAttribute('data-other');if(!input.matches(':invalid')) throw Error('must stay invalid')").unwrap();
        assert_eq!(
            runtime.host_state.borrow().form_validation.completed_scans,
            initial
        );
        runtime.eval("input.removeAttribute('required');if(!input.matches(':valid'))throw Error('must become valid')").unwrap();
        assert_eq!(
            runtime.host_state.borrow().form_validation.completed_scans,
            initial + 1
        );
    }

    #[test]
    fn css_observed_data_attribute_restyles_without_changing_constraint_validity() {
        let document = crate::html::TreeBuilder::parse(
        "<style>input{width:20px;height:10px;padding:0;border:0}input[data-probe]{width:40px}</style><input required>",
    )
    .document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        assert!(runtime.eval("globalThis.input=document.querySelector('input');input.matches(':invalid') && input.getBoundingClientRect().width===20").unwrap().to_boolean());
        let initial = runtime.host_state.borrow().form_validation.completed_scans;
        assert!(runtime.eval("input.dataset.probe='yes';input.matches(':invalid') && input.getBoundingClientRect().width===40").unwrap().to_boolean());
        assert_eq!(
            runtime.host_state.borrow().form_validation.completed_scans,
            initial
        );
    }

    #[test]
    fn data_attribute_author_callback_can_change_custom_constraint_validity() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.eval("customElements.define('validity-probe',class extends HTMLElement{static formAssociated=true;static get observedAttributes(){return ['data-valid']}constructor(){super();this._internals=this.attachInternals()}attributeChangedCallback(){this._internals.setValidity(this.dataset.valid==='no'?{customError:true}:{},this.dataset.valid==='no'?'invalid':'')}});globalThis.probe=document.createElement('validity-probe');document.body.appendChild(probe)").unwrap();
        assert!(
            runtime
                .eval("probe.matches(':valid')")
                .unwrap()
                .to_boolean()
        );
        assert!(
            runtime
                .eval("probe.dataset.valid='no';probe.matches(':invalid')")
                .unwrap()
                .to_boolean()
        );
        assert!(
            runtime
                .eval("probe.dataset.valid='yes';probe.matches(':valid')")
                .unwrap()
                .to_boolean()
        );
    }
}
