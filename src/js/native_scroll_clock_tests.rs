use super::*;

fn runtime_with_animation(style: &str) -> JsRuntime {
    let html = format!(
        "<style>body{{height:1000px}}#box{{width:20px;height:20px;{style}}}\
         @keyframes pulse{{from{{opacity:1}}to{{opacity:.5}}}}</style><div id=box></div>"
    );
    let document = crate::html::TreeBuilder::parse(&html).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.set_viewport(200.0, 100.0);
    runtime.enable_live_css_animations();
    runtime
        .eval("globalThis.box=document.querySelector('#box');getComputedStyle(box).opacity")
        .unwrap();
    runtime
}

fn has_running_clock(runtime: &JsRuntime, animation: bool) -> bool {
    let state = runtime.host_state.borrow();
    let resolver = state.document_styles[&state.document.identity()]
        .resolver
        .as_ref()
        .unwrap();
    if animation {
        resolver.has_running_animations()
    } else {
        resolver.has_running_transitions()
    }
}

fn assert_same_clock_keeps_epoch(runtime: &mut JsRuntime) {
    let handle = runtime.native_scroll_handle();
    let before = handle.publication_gate();
    assert!(handle.authorize_policy(before.key, before.policy_epoch));
    runtime
        .eval("for(let i=0;i<3;i++){getComputedStyle(box).opacity}")
        .unwrap();
    assert_eq!(handle.publication_gate().policy_epoch, before.policy_epoch);
    assert!(handle.authorize_policy(before.key, before.policy_epoch));
}

#[test]
fn animation_cssom_reads_at_the_same_clock_keep_native_authority() {
    let mut runtime = runtime_with_animation("animation:pulse 1s linear infinite");
    assert!(has_running_clock(&runtime, true));
    assert_same_clock_keeps_epoch(&mut runtime);

    let before = runtime.native_scroll_handle().publication_gate();
    runtime.advance_clock_without_tasks(100);
    let opacity = runtime
        .eval("Number(getComputedStyle(box).opacity)")
        .unwrap()
        .as_number()
        .unwrap();
    assert!(opacity < 1.0 && opacity > 0.5);
    let handle = runtime.native_scroll_handle();
    assert_eq!(handle.publication_gate().policy_epoch, before.policy_epoch);
    assert!(handle.authorize_policy(before.key, before.policy_epoch));
    assert_same_clock_keeps_epoch(&mut runtime);
}

#[test]
fn transition_cssom_reads_at_the_same_clock_keep_native_authority() {
    let mut runtime = runtime_with_animation("opacity:1;transition:opacity 1s linear");
    runtime
        .eval("box.style.opacity='.5';getComputedStyle(box).opacity")
        .unwrap();
    assert!(has_running_clock(&runtime, false));
    assert_same_clock_keeps_epoch(&mut runtime);

    let before = runtime.native_scroll_handle().publication_gate();
    runtime.advance_clock_without_tasks(100);
    let opacity = runtime
        .eval("Number(getComputedStyle(box).opacity)")
        .unwrap()
        .as_number()
        .unwrap();
    assert!(opacity < 1.0 && opacity > 0.5);
    let handle = runtime.native_scroll_handle();
    assert_eq!(handle.publication_gate().policy_epoch, before.policy_epoch);
    assert!(handle.authorize_policy(before.key, before.policy_epoch));
    assert_same_clock_keeps_epoch(&mut runtime);
}

fn install_clock_test_scene(runtime: &mut JsRuntime) -> native_scroll::NativeScrollHandle {
    use native_scroll::*;
    let _ = runtime.hit_test(1.0, 1.0);
    let (offset, extent) = runtime.host_state.borrow_mut().native_scroll_geometry();
    assert!(extent.max.y >= 10.0);
    let key = TargetKey {
        document: 91,
        owner_generation: 1,
        viewport_epoch: 1,
        scroll_node: 1,
    };
    let handle = runtime.activate_native_scroll(key);
    let gate = handle.publication_gate();
    assert!(handle.authorize_policy(key, gate.policy_epoch));
    assert!(handle.install_scene(Scene {
        key,
        publication: PublicationSequence::new(1),
        policy_epoch: gate.policy_epoch,
        acceptance_epoch: gate.acceptance_epoch,
        acknowledged: 0,
        completed_fallback: FallbackTicket::default(),
        offset,
        extent,
        coverage: extent,
        eligible: true,
    }));
    handle
}

#[test]
fn render_clock_keeps_committed_scene_while_new_animation_values_are_sampled() {
    use native_scroll::{Offset, ScrollDecision};
    let mut runtime = runtime_with_animation("animation:pulse 1s linear infinite");
    let handle = install_clock_test_scene(&mut runtime);
    let gate = handle.publication_gate();
    let generations = runtime.render_generations();
    runtime.advance_clock_without_tasks(100);
    let opacity = runtime
        .eval("Number(getComputedStyle(box).opacity)")
        .unwrap()
        .as_number()
        .unwrap();
    assert!(
        opacity < 1.0 && opacity > 0.5,
        "new animation value must be sampled"
    );
    assert_ne!(
        runtime.render_generations(),
        generations,
        "candidate render state must advance"
    );
    assert_eq!(
        handle.publication_gate(),
        gate,
        "render work must not revoke committed pixels"
    );
    let accepted = match handle.scroll(Offset { x: 0.0, y: 10.0 }) {
        ScrollDecision::Accepted(accepted) => accepted,
        other => panic!("committed scene must remain scrollable: {other:?}"),
    };
    assert!(handle.validate_for_presentation(accepted));
    assert_eq!(runtime.eval("scrollY").unwrap().as_number(), Some(10.0));
    assert_eq!(runtime.eval("scrollY").unwrap().as_number(), Some(10.0));
    assert_eq!(handle.owner_sequences(gate.key).unwrap(), (1, 0));
}

#[test]
fn render_mutations_keep_the_scene_but_input_contract_changes_revoke_it() {
    for (mutation, revokes) in [
        (
            "window.addEventListener('wheel',()=>{}, {passive:false})",
            true,
        ),
        ("box.style.height='80px'", false),
        ("window.scrollTo(0, 30)", true),
    ] {
        let mut runtime = runtime_with_animation("animation:pulse 1s linear infinite");
        let handle = install_clock_test_scene(&mut runtime);
        let gate = handle.publication_gate();
        runtime.advance_clock_without_tasks(100);
        runtime.eval("getComputedStyle(box).opacity").unwrap();
        assert_eq!(handle.publication_gate(), gate);
        runtime.eval(mutation).unwrap();
        let after = handle.publication_gate();
        if revokes {
            assert!(
                after.policy_epoch > gate.policy_epoch,
                "mutation: {mutation}"
            );
            assert!(!after.eligible, "mutation: {mutation}");
            assert!(!handle.authorize_policy(gate.key, gate.policy_epoch));
        } else {
            assert_eq!(after, gate, "mutation: {mutation}");
            assert!(handle.authorize_policy(gate.key, gate.policy_epoch));
        }
    }
}
