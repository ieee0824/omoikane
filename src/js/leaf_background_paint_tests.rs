use super::*;

fn leaf_fixture(attributes: &str, extra_css: &str) -> String {
    format!(
        "<html><head><style>body{{margin:0;background:white}}\
        #target{{position:absolute;left:20px;top:20px;width:80px;height:40px;background-color:#e9e9e9}}\
        #target[style]{{background-color:#000;border-radius:1px}}{extra_css}</style></head>\
        <body><div id='target' {attributes}></div></body></html>"
    )
}

fn leaf_runtime(html: &str) -> JsRuntime {
    JsRuntime::with_document(crate::html::TreeBuilder::parse(html).document()).unwrap()
}

#[test]
fn leaf_background_and_data_loaded_batch_repaints_without_rebuilding_geometry() {
    let mut runtime = leaf_runtime(&leaf_fixture("", ""));
    let before = runtime.paint_current_document().unwrap();
    assert_eq!(
        before.pixel(40, 40),
        Some(crate::paint::Color::rgb(233, 233, 233))
    );
    let generation = runtime.render_generations().layout;
    runtime.eval("const target=document.getElementById('target');target.style.backgroundImage='linear-gradient(transparent, transparent)';target.dataset.loaded='true'").unwrap();
    let after = runtime.paint_current_document().unwrap();
    let mut reference = leaf_runtime(&leaf_fixture(
        "style='background-image:linear-gradient(transparent, transparent)' data-loaded='true'",
        "",
    ));
    let expected = reference.paint_current_document().unwrap();
    assert_eq!(
        after.pixels(),
        expected.pixels(),
        "independent fresh/full-layout reference"
    );
    assert_ne!(before.pixels(), after.pixels(), "actual paint must change");
    assert_eq!(after.pixel(40, 40), Some(crate::paint::Color::rgb(0, 0, 0)));
    // Existing API baseline: current background+dataset invalidation rebuilds.
    assert_eq!(runtime.render_generations().layout, generation);
}

const LEAF_BACKGROUND_SET: &str = "document.getElementById('target').style.backgroundImage='linear-gradient(transparent, transparent)'";
const LEAF_FINAL_STYLE: &str = "style='background-image:linear-gradient(transparent, transparent)'";

fn assert_leaf_fallback(css: &str, initial_attrs: &str, mutation: &str, final_attrs: &str) {
    let mut runtime = leaf_runtime(&leaf_fixture(initial_attrs, css));
    runtime.paint_current_document().unwrap();
    let generation = runtime.render_generations().layout;
    runtime.eval(mutation).unwrap();
    let actual = runtime.paint_current_document().unwrap();
    let mut reference = leaf_runtime(&leaf_fixture(final_attrs, css));
    let expected = reference.paint_current_document().unwrap();
    assert_eq!(
        actual.pixels(),
        expected.pixels(),
        "fallback must equal a fresh full layout: {css}"
    );
    assert!(
        runtime.render_generations().layout > generation,
        "conservative geometry fallback: {css}"
    );
}

#[test]
fn leaf_background_raw_style_width_change_falls_back() {
    assert_leaf_fallback(
        "#target[style]{width:120px}",
        "",
        LEAF_BACKGROUND_SET,
        LEAF_FINAL_STYLE,
    );
}

#[test]
fn leaf_background_new_generated_pseudo_falls_back() {
    assert_leaf_fallback(
        "#target[style]::before{content:'NEW';display:block;background:red;width:25px;height:15px}",
        "",
        LEAF_BACKGROUND_SET,
        LEAF_FINAL_STYLE,
    );
}

#[test]
fn leaf_background_conservative_shape_and_attr_dependencies_fall_back() {
    for css in [
        "#target{shape-outside:circle(30%)}",
        "#target{--future-attribute:attr(data-loaded)}",
        "#target{float:left}",
    ] {
        assert_leaf_fallback(css, "", LEAF_BACKGROUND_SET, LEAF_FINAL_STYLE);
    }
}

#[test]
fn leaf_data_loaded_with_a_selector_change_falls_back() {
    assert_leaf_fallback(
        "#target[data-loaded]{width:120px}",
        "",
        "document.getElementById('target').dataset.loaded='true'",
        "data-loaded='true'",
    );
}

#[test]
fn leaf_other_unobserved_data_attributes_keep_clean_geometry() {
    let mut runtime = leaf_runtime(&leaf_fixture("", ""));
    let before = runtime.paint_current_document().unwrap();
    let generation = runtime.render_generations().layout;
    runtime
        .eval("document.getElementById('target').dataset.other='true'")
        .unwrap();
    let after = runtime.paint_current_document().unwrap();
    assert_eq!(after.pixels(), before.pixels());
    assert_eq!(runtime.render_generations().layout, generation);
}

#[test]
fn leaf_data_loaded_without_css_dependency_keeps_clean_geometry() {
    let mut runtime = leaf_runtime(&leaf_fixture("", ""));
    let before = runtime.paint_current_document().unwrap();
    let generation = runtime.render_generations().layout;
    runtime
        .eval("document.getElementById('target').dataset.loaded='true'")
        .unwrap();
    let after = runtime.paint_current_document().unwrap();
    assert_eq!(after.pixels(), before.pixels());
    assert_eq!(runtime.render_generations().layout, generation);
}

#[test]
fn leaf_background_keeps_radius_paint_current_under_unchanged_transform() {
    let css = "#target{transform:scale(1.01)}#target[style]{border-radius:12px}";
    let mut runtime = leaf_runtime(&leaf_fixture("", css));
    let before = runtime.paint_current_document().unwrap();
    let generation = runtime.render_generations().layout;
    runtime.eval(LEAF_BACKGROUND_SET).unwrap();
    let actual = runtime.paint_current_document().unwrap();
    let mut reference = leaf_runtime(&leaf_fixture(LEAF_FINAL_STYLE, css));
    let expected = reference.paint_current_document().unwrap();
    assert_ne!(before.pixels(), actual.pixels());
    assert_eq!(actual.pixels(), expected.pixels());
    assert!(
        runtime
            .eval(
                "getComputedStyle(document.getElementById('target')).borderTopLeftRadius==='12px'"
            )
            .unwrap()
            .to_boolean()
    );
    assert_eq!(runtime.render_generations().layout, generation);
}

#[test]
fn leaf_background_does_not_resurrect_preexisting_layout_dirty_state() {
    assert_leaf_fallback(
        "",
        "",
        "const t=document.getElementById('target');t.style.width='120px';t.style.backgroundImage='linear-gradient(transparent, transparent)';t.dataset.loaded='true'",
        "style='width:120px;background-image:linear-gradient(transparent, transparent)' data-loaded='true'",
    );
}

#[test]
fn leaf_background_preserves_style_dirty_and_full_sampling_flags() {
    let mut runtime = leaf_runtime(&leaf_fixture("", ""));
    runtime.paint_current_document().unwrap();
    let generation = runtime.render_generations().layout;
    {
        let mut state = runtime.host_state.borrow_mut();
        let document = state.document.clone();
        state.mark_document_style_dirty(&document);
    }
    runtime.eval(LEAF_BACKGROUND_SET).unwrap();
    {
        let state = runtime.host_state.borrow();
        let entry = state
            .document_styles
            .get(&state.document.identity())
            .unwrap();
        assert!(entry.dirty);
        assert!(entry.needs_full_sample);
        assert!(state.layout_root.is_none());
    }
    let actual = runtime.paint_current_document().unwrap();
    let mut reference = leaf_runtime(&leaf_fixture(LEAF_FINAL_STYLE, ""));
    assert_eq!(
        actual.pixels(),
        reference.paint_current_document().unwrap().pixels()
    );
    assert!(runtime.render_generations().layout > generation);
}

#[test]
fn leaf_background_does_not_clear_existing_needs_full_sample() {
    let mut runtime = leaf_runtime(&leaf_fixture("", ""));
    runtime.paint_current_document().unwrap();
    let generation = runtime.render_generations().layout;
    {
        let mut state = runtime.host_state.borrow_mut();
        let document_id = state.document.identity();
        state
            .document_styles
            .get_mut(&document_id)
            .unwrap()
            .needs_full_sample = true;
    }
    runtime.eval(LEAF_BACKGROUND_SET).unwrap();
    {
        let state = runtime.host_state.borrow();
        assert!(
            state
                .document_styles
                .get(&state.document.identity())
                .unwrap()
                .needs_full_sample
        );
    }
    runtime.paint_current_document().unwrap();
    assert_eq!(runtime.render_generations().layout, generation);
}

#[test]
fn leaf_background_nonempty_target_and_inherited_child_rebuild() {
    let css = "#target span{background-image:inherit;background-color:inherit;display:block;width:10px;height:10px}";
    let original = leaf_fixture("", css).replace(
        "id='target' ></div>",
        "id='target' ><span>child</span></div>",
    );
    let final_html =
        leaf_fixture(LEAF_FINAL_STYLE, css).replace("'></div>", "'><span>child</span></div>");
    let mut runtime = leaf_runtime(&original);
    runtime.paint_current_document().unwrap();
    let generation = runtime.render_generations().layout;
    runtime.eval(LEAF_BACKGROUND_SET).unwrap();
    let actual = runtime.paint_current_document().unwrap();
    let mut reference = leaf_runtime(&final_html);
    assert_eq!(
        actual.pixels(),
        reference.paint_current_document().unwrap().pixels()
    );
    assert!(runtime.render_generations().layout > generation);
}

#[test]
fn leaf_background_nonlocal_subject_rule_keeps_sibling_geometry_current() {
    let css = "#target[style]+aside{width:120px}aside{width:20px;height:20px;background:red}";
    let original = leaf_fixture("", css).replace("</body>", "<aside></aside></body>");
    let final_html =
        leaf_fixture(LEAF_FINAL_STYLE, css).replace("</body>", "<aside></aside></body>");
    let mut runtime = leaf_runtime(&original);
    runtime.paint_current_document().unwrap();
    let generation = runtime.render_generations().layout;
    runtime.eval(LEAF_BACKGROUND_SET).unwrap();
    let actual = runtime.paint_current_document().unwrap();
    let mut reference = leaf_runtime(&final_html);
    assert_eq!(
        actual.pixels(),
        reference.paint_current_document().unwrap().pixels()
    );
    assert!(runtime.render_generations().layout > generation);
}

#[test]
fn leaf_background_clock_declarations_use_the_original_fallback_path() {
    for css in [
        "#target{transition:background-color 1s}",
        "#target[style]{transition:background-color 1s}",
        "@keyframes pulse{from{opacity:0.8}to{opacity:1}}#target[style]{animation:pulse 1s infinite}",
        "#target[style]::before{transition:opacity 1s}",
    ] {
        let html = leaf_fixture("", css);
        let mut actual = leaf_runtime(&html);
        let mut reference = leaf_runtime(&html);
        actual.paint_current_document().unwrap();
        reference.paint_current_document().unwrap();
        let generation = actual.render_generations().layout;
        {
            let mut state = reference.host_state.borrow_mut();
            let document = state.document.clone();
            state.mark_document_layout_dirty(&document, "leaf_reference_force_full_layout");
        }
        actual.eval(LEAF_BACKGROUND_SET).unwrap();
        reference.eval(LEAF_BACKGROUND_SET).unwrap();
        assert_eq!(
            actual.paint_current_document().unwrap().pixels(),
            reference.paint_current_document().unwrap().pixels(),
            "same-time forced-layout reference: {css}"
        );
        assert!(
            actual.render_generations().layout > generation,
            "clock fallback: {css}"
        );
    }
}

#[test]
fn leaf_background_nonabsolute_inline_and_inflow_targets_use_geometry_fallback() {
    for css in [
        "#target{position:static}",
        "#target{position:relative}",
        "#target{position:fixed}",
    ] {
        assert_leaf_fallback(css, "", LEAF_BACKGROUND_SET, LEAF_FINAL_STYLE);
    }
}

#[test]
fn leaf_background_raw_style_counter_effect_keeps_outside_generated_content_current() {
    let css = "body{counter-reset:outside 0}#target[style]{counter-increment:outside 7}aside::before{content:counter(outside)}";
    let original = leaf_fixture("", css).replace("</body>", "<aside></aside></body>");
    let final_html =
        leaf_fixture(LEAF_FINAL_STYLE, css).replace("</body>", "<aside></aside></body>");
    let mut runtime = leaf_runtime(&original);
    runtime.paint_current_document().unwrap();
    let generation = runtime.render_generations().layout;
    runtime.eval(LEAF_BACKGROUND_SET).unwrap();
    let actual = runtime.paint_current_document().unwrap();
    let mut reference = leaf_runtime(&final_html);
    assert_eq!(
        actual.pixels(),
        reference.paint_current_document().unwrap().pixels()
    );
    assert!(runtime.render_generations().layout > generation);
}

#[test]
fn leaf_background_viewport_invalidation_is_never_cancelled() {
    let mut runtime = leaf_runtime(&leaf_fixture("", ""));
    runtime.paint_current_document().unwrap();
    let generation = runtime.render_generations().layout;
    runtime.set_viewport(320.0, 200.0);
    runtime.eval(LEAF_BACKGROUND_SET).unwrap();
    let actual = runtime.paint_current_document().unwrap();
    let mut reference = leaf_runtime(&leaf_fixture(LEAF_FINAL_STYLE, ""));
    reference.set_viewport(320.0, 200.0);
    assert_eq!(
        actual.pixels(),
        reference.paint_current_document().unwrap().pixels()
    );
    assert!(runtime.render_generations().layout > generation);
}

#[test]
fn leaf_background_csp_hash_change_keeps_blocked_width_and_paint_current() {
    use base64::Engine as _;
    use sha2::Digest as _;

    let previous = "width:90px";
    let current = "width:90px;background-image:linear-gradient(transparent, transparent)";
    let digest =
        base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(previous.as_bytes()));
    let headers = [(
        "Content-Security-Policy".into(),
        format!("style-src-attr 'unsafe-hashes' 'sha256-{digest}'"),
    )];
    let mut actual = leaf_runtime(&leaf_fixture(&format!("style='{previous}'"), ""));
    actual.install_response_security_headers(&headers);
    actual.paint_current_document().unwrap();
    assert_eq!(
        actual
            .eval("document.getElementById('target').getBoundingClientRect().width")
            .unwrap()
            .as_number(),
        Some(90.0)
    );
    let generation = actual.render_generations().layout;
    actual
        .eval(&format!(
            "document.getElementById('target').setAttribute('style','{current}')"
        ))
        .unwrap();
    let image = actual.paint_current_document().unwrap();
    let mut reference = leaf_runtime(&leaf_fixture(&format!("style='{current}'"), ""));
    reference.install_response_security_headers(&headers);
    assert_eq!(
        image.pixels(),
        reference.paint_current_document().unwrap().pixels()
    );
    assert_eq!(
        actual
            .eval("document.getElementById('target').getBoundingClientRect().width")
            .unwrap()
            .as_number(),
        Some(80.0)
    );
    assert!(actual.render_generations().layout > generation);
}

fn assert_leaf_setup_fallback(attributes: &str, setup: &str, proof: &str) {
    let html = leaf_fixture(attributes, "");
    let mut actual = leaf_runtime(&html);
    let mut reference = leaf_runtime(&html);
    actual.eval(setup).unwrap();
    reference.eval(setup).unwrap();
    assert!(
        actual.eval(proof).unwrap().to_boolean(),
        "fixture must establish the excluded context"
    );
    actual.paint_current_document().unwrap();
    reference.paint_current_document().unwrap();
    let generation = actual.render_generations().layout;
    {
        let mut state = reference.host_state.borrow_mut();
        let document = state.document.clone();
        state.mark_document_layout_dirty(&document, "leaf_reference_force_full_layout");
    }
    actual.eval(LEAF_BACKGROUND_SET).unwrap();
    reference.eval(LEAF_BACKGROUND_SET).unwrap();
    assert_eq!(
        actual.paint_current_document().unwrap().pixels(),
        reference.paint_current_document().unwrap().pixels()
    );
    assert!(actual.render_generations().layout > generation);
}

#[test]
fn leaf_background_empty_shadow_host_uses_existing_geometry_fallback() {
    assert_leaf_setup_fallback(
        "",
        "document.getElementById('target').attachShadow({mode:'open'})",
        "document.getElementById('target').shadowRoot!==null",
    );
}

#[test]
fn leaf_background_slotted_target_uses_existing_geometry_fallback() {
    assert_leaf_setup_fallback(
        "",
        "const host=document.createElement('section');document.body.appendChild(host);host.appendChild(document.getElementById('target'));host.attachShadow({mode:'open'}).innerHTML='<slot></slot>'",
        "document.getElementById('target').assignedSlot!==null",
    );
}

#[test]
fn leaf_background_open_popover_uses_existing_geometry_fallback() {
    assert_leaf_setup_fallback(
        "popover='manual'",
        "document.getElementById('target').showPopover()",
        "document.getElementById('target').matches(':popover-open')",
    );
}

#[test]
fn leaf_background_mixed_raw_style_geometry_and_other_paint_values_fall_back() {
    for css in [
        "#target[style]{padding:4px;border:2px solid red}",
        "#target[style]{overflow:hidden}",
        "#target[style]{container-type:inline-size}",
        "#target[style]{opacity:0.5}",
        "#target[style]{border-start-start-radius:12px;writing-mode:vertical-rl}",
    ] {
        assert_leaf_fallback(css, "", LEAF_BACKGROUND_SET, LEAF_FINAL_STYLE);
    }
}

fn assert_leaf_running_clock_fallback(css: &str, setup: &str, transition: bool) {
    let html = leaf_fixture("", css).replace("</body>", "<aside id='clock'></aside></body>");
    let mut actual = leaf_runtime(&html);
    let mut reference = leaf_runtime(&html);
    for runtime in [&mut actual, &mut reference] {
        runtime.enable_live_css_animations();
        runtime.eval("globalThis.clockEvents=[];for(const name of ['transitionrun','transitionstart','transitionend','transitioncancel','animationstart','animationend','animationiteration','animationcancel'])document.getElementById('clock').addEventListener(name,event=>clockEvents.push(event.type))").unwrap();
        runtime.paint_current_document().unwrap();
        runtime.eval(setup).unwrap();
        runtime.paint_current_document().unwrap();
    }
    {
        let state = actual.host_state.borrow();
        let resolver = state
            .document_styles
            .get(&state.document.identity())
            .unwrap()
            .resolver
            .as_ref()
            .unwrap();
        assert!(
            if transition {
                resolver.has_running_transitions()
            } else {
                !resolver.animation_node_ids().is_empty()
            },
            "fixture must have a real active clock before the BG mutation"
        );
    }
    let generation = actual.render_generations().layout;
    {
        let mut state = reference.host_state.borrow_mut();
        let document = state.document.clone();
        state.mark_document_layout_dirty(&document, "leaf_reference_force_full_layout");
    }
    for runtime in [&mut actual, &mut reference] {
        runtime
            .eval(&format!(
                "{LEAF_BACKGROUND_SET};document.getElementById('target').dataset.loaded='true'"
            ))
            .unwrap();
    }
    assert_eq!(
        actual.paint_current_document().unwrap().pixels(),
        reference.paint_current_document().unwrap().pixels()
    );
    assert!(actual.render_generations().layout > generation);
    for elapsed in [500, 600] {
        actual.run_animation_frame(elapsed).unwrap();
        reference.run_animation_frame(elapsed).unwrap();
        assert_eq!(
            actual.paint_current_document().unwrap().pixels(),
            reference.paint_current_document().unwrap().pixels()
        );
        assert_eq!(
            actual
                .eval("JSON.stringify(clockEvents)")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            reference
                .eval("JSON.stringify(clockEvents)")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped()
        );
    }
}

#[test]
fn leaf_background_with_unrelated_running_opacity_animation_keeps_clock_events() {
    assert_leaf_running_clock_fallback(
        "@keyframes fade{from{opacity:0}to{opacity:1}}#clock{width:10px;height:10px;background:red;animation:fade 1s linear forwards}",
        "void 0",
        false,
    );
}

#[test]
fn leaf_background_with_unrelated_running_transition_keeps_clock_events() {
    assert_leaf_running_clock_fallback(
        "#clock{width:10px;height:10px;background:red;opacity:1;transition:opacity 1s linear}",
        "document.getElementById('clock').style.opacity='0';getComputedStyle(document.getElementById('clock')).opacity",
        true,
    );
}
