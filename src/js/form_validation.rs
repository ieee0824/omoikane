//! Shares form-validation results with native selector matching and painting.
use super::*;

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
                    name.eq_ignore_ascii_case("valid") || name.eq_ignore_ascii_case("invalid")
                }
                SimpleSelector::Is(list)
                | SimpleSelector::Where(list)
                | SimpleSelector::Not(list) => selector_uses_validation(list),
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
                let document =
                    owner_document_for_node(&node).unwrap_or_else(|| state.document.clone());
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

pub(super) fn register(context: &mut Context) -> JsResult<()> {
    context.register_global_builtin_callable(
        js_string!("__omoikane_register_validation"),
        2,
        NativeFunction::from_copy_closure(|_, args, context| {
            let document = parse_node_id(args.first(), context)?;
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
    context.register_global_builtin_callable(
        js_string!("__omoikane_flush_validation"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            flush(parse_node_id(args.first(), context)?, context)?;
            Ok(JsValue::undefined())
        }),
    )?;
    context.register_global_builtin_callable(
        js_string!("__omoikane_invalidate_validation"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            let id = parse_node_id(args.first(), context)?;
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
}
