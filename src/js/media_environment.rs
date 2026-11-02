//! Presentation settings shared by script-visible media query lists.
use super::{BootstrapBindings, HostState, JsRuntime};
use crate::css::MediaEnvironment;
use crate::dom::{Node, NodeHandle, NodeType};
use boa_engine::{Context, JsResult, JsString, JsValue, NativeFunction, js_string};
use std::{cell::RefCell, rc::Rc};

impl JsRuntime {
    /// Returns the top-level layout size to preserve across document replacement.
    pub(crate) fn presentation_viewport(&self) -> (f32, f32) {
        let state = self.host_state.borrow();
        (state.viewport.width, state.viewport.height)
    }

    /// Returns an owned snapshot of the runtime's current media settings.
    pub fn media_environment(&self) -> MediaEnvironment {
        self.host_state.borrow().media_environment.clone()
    }

    /// Installs settings immediately and queues change reporting for rendering.
    /// CSS and `matches` observe the new snapshot before the next opportunity;
    /// change events are coalesced and reported before animation-frame callbacks.
    /// Non-finite or non-positive resolution is normalized to one.
    /// Invalid device dimensions fall back to the host virtual display.
    pub fn set_media_environment(&mut self, mut environment: MediaEnvironment) -> JsResult<()> {
        if !environment.resolution_dppx.is_finite() || environment.resolution_dppx <= 0.0 {
            environment.resolution_dppx = 1.0;
        }
        environment.device_width = environment
            .device_width
            .filter(|value| value.is_finite() && *value >= 0.0);
        environment.device_height = environment
            .device_height
            .filter(|value| value.is_finite() && *value >= 0.0);
        let resolution = environment.resolution_dppx;
        let native_scroll = {
            let state = self.host_state.borrow();
            if state.media_environment == environment {
                return Ok(());
            }
            state.native_scroll.clone()
        };
        // Media features can replace computed style, layout, and paint without
        // changing the DOM. Revoke before the new snapshot is observable, and
        // without holding a HostState borrow across the protocol lock.
        native_scroll.revoke_policy();
        {
            let mut state = self.host_state.borrow_mut();
            state.media_environment = environment;
            state.pending_media_query_report = true;
            state.mark_all_document_styles_dirty();
        }
        for document_id in self.media_document_ids() {
            self.eval_in_document_realm(
                document_id,
                &format!("globalThis.devicePixelRatio = {resolution};"),
            )?;
        }
        Ok(())
    }

    pub(super) fn media_document_ids(&self) -> Vec<usize> {
        let state = self.host_state.borrow();
        let mut ids = Vec::new();
        collect_media_documents(&state, state.document.clone(), &mut ids);
        let mut auxiliary: Vec<_> = state
            .auxiliary_contexts
            .iter()
            .filter(|(_, entry)| entry.realm.is_some())
            .map(|(id, entry)| (*id, entry.document.clone()))
            .collect();
        auxiliary.sort_unstable_by_key(|(id, _)| *id);
        for (_, document) in auxiliary {
            collect_media_documents(&state, document, &mut ids);
        }
        ids
    }

    pub(super) fn report_media_query_changes(&mut self) -> JsResult<()> {
        if !std::mem::take(&mut self.host_state.borrow_mut().pending_media_query_report) {
            return Ok(());
        }
        for document_id in self.media_document_ids() {
            if let Err(error) = self.eval_in_document_realm(
                document_id,
                "globalThis.__omoikane_media_query_viewport_changed();",
            ) {
                self.host_state.borrow_mut().pending_media_query_report = true;
                return Err(error);
            }
        }
        Ok(())
    }
}

impl HostState {
    /// A dirty scene can change descendant iframe viewports. Keep a rendering
    /// opportunity even when the mutation occurs after this frame's reports.
    /// This is conservative for paint-only changes; unchanged scenes stay idle.
    pub(super) fn invalidate_child_media_query_inputs(&mut self) {
        self.pending_media_query_report |= !self.iframe_documents.is_empty();
    }

    /// Resolves one display snapshot for every document in this presentation.
    /// The host's virtual display uses the top-level viewport when no native
    /// screen size is provided, independently of iframe viewport dimensions.
    pub(super) fn presentation_media_environment(&self) -> MediaEnvironment {
        let mut environment = self.media_environment.clone();
        environment.device_width = Some(environment.device_width.unwrap_or(self.viewport.width));
        environment.device_height = Some(environment.device_height.unwrap_or(self.viewport.height));
        environment
    }
}

fn screen_metrics_native(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    super::with_host_state(|state| {
        let environment = state.borrow().presentation_media_environment();
        let depth = if environment.color_bits_per_component != 0 {
            environment.color_bits_per_component.saturating_mul(3)
        } else {
            environment.monochrome_bits_per_pixel
        };
        Ok(JsValue::from(JsString::from(
            serde_json::json!({
                "width": environment.device_width,
                "height": environment.device_height,
                "colorDepth": depth,
                "pixelDepth": depth,
            })
            .to_string(),
        )))
    })
}

/// Visits parent documents before their containers' shadow-including children.
fn collect_media_documents(state: &HostState, root: NodeHandle, ids: &mut Vec<usize>) {
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if node.node_type() == NodeType::Document {
            if ids.contains(&node.identity()) {
                continue;
            }
            ids.push(node.identity());
        }
        pending.extend(node.child_nodes().into_iter().rev());
        if let Some(shadow) = node.shadow_root() {
            pending.push(shadow);
        }
        if let Some(entry) = state.iframe_documents.get(&node.identity())
            && entry.realm.is_some()
        {
            pending.push(entry.document.clone());
        }
    }
}

/// Supplies the initial presentation snapshot before a Window runs scripts.
pub(super) fn register(
    context: &mut Context,
    host_state: &Rc<RefCell<HostState>>,
    bindings: &mut BootstrapBindings,
) -> JsResult<()> {
    bindings.callable(
        js_string!("__omoikane_screen_metrics"),
        0,
        NativeFunction::from_fn_ptr(screen_metrics_native),
        false,
        context,
    )?;
    let resolution = host_state.borrow().media_environment.resolution_dppx;
    bindings.value(
        js_string!("__omoikane_initial_device_pixel_ratio"),
        f64::from(resolution),
        context,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscribed_media_query_survives_collection_without_page_reference() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.eval("globalThis.mediaChanges = 0; (() => { const list = matchMedia('(prefers-reduced-motion: reduce)'); list.addEventListener('change', () => mediaChanges++); })();").unwrap();
        runtime.context.clear_kept_objects();
        boa_gc::force_collect();
        let mut environment = runtime.media_environment();
        environment.set_feature("prefers-reduced-motion", "reduce");
        runtime.set_media_environment(environment).unwrap();
        runtime.run_animation_frame(16).unwrap();
        assert_eq!(runtime.eval("mediaChanges").unwrap().as_number(), Some(1.0));
    }

    #[test]
    fn unsubscribed_media_queries_are_released_after_listener_removal() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"
            globalThis.mediaReferences = [];
            for (const cleanup of ['remove', 'onchange', 'abort', 'once', 'other', 'prototype']) {
                const list = matchMedia('(prefers-reduced-motion: reduce)');
                const callback = () => {};
                mediaReferences.push(new WeakRef(list));
                if (cleanup === 'remove') {
                    list.addEventListener('change', callback);
                    list.removeEventListener('change', callback);
                } else if (cleanup === 'onchange') {
                    list.onchange = callback; list.onchange = null;
                } else if (cleanup === 'abort') {
                    const controller = new AbortController();
                    list.addEventListener('change', callback, {signal: controller.signal});
                    controller.abort();
                } else if (cleanup === 'once') {
                    list.addEventListener('change', callback, {once: true});
                    list.dispatchEvent(new Event('change'));
                } else if (cleanup === 'prototype') {
                    EventTarget.prototype.addEventListener.call(list, 'change', callback);
                    EventTarget.prototype.removeEventListener.call(list, 'change', callback);
                } else {
                    list.addEventListener('other', callback);
                }
            }
        "#,
            )
            .unwrap();
        runtime.context.clear_kept_objects();
        boa_gc::force_collect();
        assert_eq!(
            runtime
                .eval("mediaReferences.every(reference => reference.deref() === undefined)")
                .unwrap()
                .as_boolean(),
            Some(true)
        );
    }

    #[test]
    fn pending_media_changes_request_one_rendering_opportunity() {
        use crate::cdp::NextRendering;
        let mut runtime = JsRuntime::new().unwrap();
        assert_eq!(runtime.next_rendering(), NextRendering::Idle);
        let mut environment = runtime.media_environment();
        environment.set_feature("prefers-reduced-motion", "reduce");
        runtime.set_media_environment(environment).unwrap();
        assert_eq!(runtime.next_rendering(), NextRendering::EveryFrame);
        assert_eq!(runtime.next_rendering(), NextRendering::EveryFrame);
        runtime.run_animation_frame(16).unwrap();
        assert_eq!(runtime.next_rendering(), NextRendering::Idle);
    }
    #[test]
    fn iframe_resize_after_raf_keeps_a_reporting_opportunity() {
        use crate::cdp::NextRendering;
        let document = crate::html::TreeBuilder::parse(
            "<body><div id=container style='width:200px'><iframe id=child style='width:100%;border:0'></iframe></div></body>",
        ).document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime.set_viewport(800.0, 600.0);
        runtime.eval(r#"
          globalThis.mediaChanges=[];
          globalThis.child=document.getElementById('child').contentWindow;
          child.eval("globalThis.query=matchMedia('(max-width:200px)');query.onchange=e=>parent.mediaChanges.push(e.matches)");
        "#).unwrap();
        for _ in 0..3 {
            runtime.run_animation_frame(16).unwrap();
        }
        assert_eq!(runtime.next_rendering(), NextRendering::Idle);
        runtime.eval("requestAnimationFrame(()=>document.getElementById('container').style.width='400px')").unwrap();
        runtime.run_animation_frame(16).unwrap();
        runtime.paint_current_document().unwrap();
        assert_eq!(runtime.next_rendering(), NextRendering::EveryFrame);
        for _ in 0..3 {
            runtime.run_animation_frame(16).unwrap();
        }
        assert!(
            runtime
                .eval("JSON.stringify(mediaChanges)==='[false]' && !child.query.matches")
                .unwrap()
                .to_boolean()
        );
        assert_eq!(runtime.next_rendering(), NextRendering::Idle);
    }

    #[test]
    fn media_lists_created_during_reporting_do_not_compact_the_active_iteration() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"
          globalThis.mediaOrder=[];
          globalThis.createdChanges=0;
          globalThis.first=matchMedia('(prefers-reduced-motion: reduce)');
          first.onchange=()=>{
            mediaOrder.push('first');
            for(let i=0;i<1;i++){
              matchMedia('(prefers-reduced-motion: reduce)').onchange=()=>createdChanges++;
            }
          };
          for(let i=0;i<126;i++)matchMedia('all');
          globalThis.last=matchMedia('(prefers-reduced-motion: reduce)');
          last.onchange=()=>mediaOrder.push('last');
        "#,
            )
            .unwrap();
        runtime.context.clear_kept_objects();
        boa_gc::force_collect();
        let baseline = runtime.media_environment();
        let mut reduced = baseline.clone();
        reduced.set_feature("prefers-reduced-motion", "reduce");
        runtime.set_media_environment(reduced).unwrap();
        runtime.run_animation_frame(16).unwrap();
        assert!(
            runtime
                .eval("JSON.stringify(mediaOrder)==='[\"first\",\"last\"]' && createdChanges===0")
                .unwrap()
                .to_boolean()
        );
        runtime.set_media_environment(baseline).unwrap();
        runtime.run_animation_frame(16).unwrap();
        assert!(runtime.eval("JSON.stringify(mediaOrder)==='[\"first\",\"last\",\"first\",\"last\"]' && createdChanges===1").unwrap().to_boolean());
    }
}
