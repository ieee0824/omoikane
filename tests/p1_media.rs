use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

#[test]
fn desktop_media_features_have_consistent_defaults() {
    let mut runtime =
        JsRuntime::with_document(TreeBuilder::parse("<div></div>").document()).unwrap();
    for query in [
        "(hover: hover)",
        "(pointer: fine)",
        "(any-hover: hover)",
        "(any-pointer: fine)",
        "(resolution: 1dppx)",
        "(min-resolution: 1dppx)",
        "(prefers-reduced-motion: no-preference)",
        "(scripting: enabled)",
        "(prefers-contrast: no-preference)",
        "(forced-colors: none)",
        "(update: fast)",
        "(color-gamut: srgb)",
        "(dynamic-range: standard)",
        "(display-mode: browser)",
    ] {
        let script = format!("matchMedia({query:?}).matches");
        assert_eq!(
            runtime.eval(&script).unwrap().as_boolean(),
            Some(true),
            "{query}"
        );
    }
}

#[test]
fn negating_an_unknown_media_feature_still_does_not_match() {
    let mut runtime =
        JsRuntime::with_document(TreeBuilder::parse("<div></div>").document()).unwrap();
    for query in [
        "not (omoikane-unknown-feature)",
        "not all and (omoikane-unknown-feature: yes)",
    ] {
        let script = format!("matchMedia({query:?}).matches");
        assert_eq!(
            runtime.eval(&script).unwrap().as_boolean(),
            Some(false),
            "{query}"
        );
    }
}

fn matches_environment(query: &str, environment: &omoikane::css::MediaEnvironment) -> bool {
    let queries = omoikane::css::parse_media_query_list(query).unwrap();
    omoikane::css::evaluate_media_query_with_environment(&queries[0], 800.0, 600.0, environment)
}

#[test]
fn explicit_media_snapshots_configure_preferences_and_all_pointer_devices() {
    let mut environment = omoikane::css::MediaEnvironment::default();
    let previous = environment.clone();
    assert!(environment.set_feature("prefers-reduced-motion", "reduce"));
    assert!(environment.set_feature("color-gamut", "p3"));
    environment.set_available_pointers(true, true);
    assert!(matches_environment(
        "(prefers-reduced-motion: reduce)",
        &environment
    ));
    assert!(!matches_environment(
        "(prefers-reduced-motion: reduce)",
        &previous
    ));
    for query in [
        "(any-pointer: fine)",
        "(any-pointer: coarse)",
        "(color-gamut: srgb)",
        "(color-gamut: p3)",
    ] {
        assert!(matches_environment(query, &environment), "{query}");
    }
    assert!(!matches_environment("(any-pointer: none)", &environment));
    let unchanged = environment.clone();
    assert!(!environment.set_feature("pointer", "invalid"));
    assert_eq!(environment, unchanged);
    assert!(!matches_environment("not (pointer: invalid)", &environment));
}

#[test]
fn resolution_units_ranges_and_boolean_syntax_use_the_snapshot() {
    let mut environment = omoikane::css::MediaEnvironment::default();
    environment.resolution_dppx = 2.0;
    for query in [
        "(resolution: 2dppx)",
        "(resolution: 192dpi)",
        "(min-resolution: 1dppx)",
        "(resolution > 1dppx)",
        "(1dppx < resolution)",
        "(resolution)",
    ] {
        assert!(matches_environment(query, &environment), "{query}");
    }
    assert!(!matches_environment(
        "(max-resolution: 1dppx)",
        &environment
    ));
    assert!(!matches_environment(
        "not (resolution: nonsense)",
        &environment
    ));
}

#[test]
fn nested_media_conditions_use_three_valued_logic() {
    let environment = omoikane::css::MediaEnvironment::default();
    for (query, expected) in [
        ("(unknown-feature) or (width > 1px)", true),
        ("not ((unknown-feature) and (width: 0px))", true),
        ("not ((unknown-feature) or (width: 0px))", false),
        ("(width > 1px) and (not (pointer: coarse))", true),
        ("(unknown-feature) and (width > 1px)", false),
    ] {
        assert_eq!(
            matches_environment(query, &environment),
            expected,
            "{query}"
        );
    }
}

#[test]
fn malformed_queries_recover_independently_at_commas() {
    let environment = omoikane::css::MediaEnvironment::default();
    for query in [
        "(width > 1px) and (height > 1px) or (hover)",
        "screen and (width > 1px) or (hover)",
        "(width > 1px) (hover)",
        "(width > 1px) and",
        "only (hover)",
        "or",
    ] {
        assert!(!matches_environment(query, &environment), "{query}");
        let list =
            omoikane::css::parse_media_query_list(&format!("{query}, (hover: hover)")).unwrap();
        assert_eq!(list.len(), 2);
        assert!(omoikane::css::evaluate_media_query_with_environment(
            &list[1],
            800.0,
            600.0,
            &environment,
        ));
    }
    assert!(matches_environment("not unknown-type", &environment));
    assert!(!matches_environment("日本語 and (hover)", &environment));
}

#[test]
fn runtime_media_changes_update_existing_lists_once() {
    let mut runtime =
        JsRuntime::with_document(TreeBuilder::parse("<div></div>").document()).unwrap();
    runtime.eval("globalThis.events = []; globalThis.list = matchMedia('(prefers-reduced-motion: reduce)'); list.addEventListener('change', e => events.push([e.matches, e.media]));").unwrap();
    let mut environment = runtime.media_environment();
    assert!(environment.set_feature("prefers-reduced-motion", "reduce"));
    runtime.set_media_environment(environment.clone()).unwrap();
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(runtime.eval("list.matches && events.length === 1 && events[0][0] === true && events[0][1] === list.media").unwrap().as_boolean(), Some(true));
    runtime.set_media_environment(environment).unwrap();
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime.eval("events.length").unwrap().as_number(),
        Some(1.0)
    );
    let mut environment = runtime.media_environment();
    assert!(environment.set_feature("prefers-reduced-motion", "no-preference"));
    environment.color_scheme_dark = true;
    environment.media_type = omoikane::css::MediaType::Print;
    runtime.set_media_environment(environment).unwrap();
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(runtime.eval("!list.matches && events.length === 2 && events[1][0] === false && matchMedia('print').matches && matchMedia('(prefers-color-scheme: dark)').matches").unwrap().as_boolean(), Some(true));
}

#[test]
fn environment_changes_apply_to_css_cascade() {
    let document = TreeBuilder::parse("<style>#target { width: 10px; } @media (prefers-reduced-motion: reduce) { #target { width: 40px; } }</style><div id=target></div>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    assert_eq!(
        runtime
            .eval("getComputedStyle(document.getElementById('target')).width")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "10px"
    );
    let mut environment = runtime.media_environment();
    environment.set_feature("prefers-reduced-motion", "reduce");
    runtime.set_media_environment(environment).unwrap();
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime
            .eval("getComputedStyle(document.getElementById('target')).width")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "40px"
    );
}

#[test]
fn runtime_resolution_updates_script_metrics_and_queries() {
    let mut runtime = JsRuntime::new().unwrap();
    let mut environment = runtime.media_environment();
    environment.resolution_dppx = 2.0;
    runtime.set_media_environment(environment).unwrap();
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime
            .eval("devicePixelRatio === 2 && matchMedia('(resolution: 2dppx)').matches")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn resolution_equality_and_chained_ranges_preserve_direction() {
    let mut environment = omoikane::css::MediaEnvironment::default();
    environment.resolution_dppx = 2.0;
    for (query, expected) in [
        ("(resolution = 2dppx)", true),
        ("(192dpi = resolution)", true),
        ("(1dppx < resolution <= 2dppx)", true),
        ("(2dppx >= resolution > 1dppx)", true),
        ("(1dppx < resolution < 2dppx)", false),
        ("(3dppx > resolution >= 2dppx)", true),
        ("(1dppx < resolution > 2dppx)", false),
        ("(2dppx = resolution = 2dppx)", false),
        ("not (1dppx < resolution > 2dppx)", false),
    ] {
        assert_eq!(
            matches_environment(query, &environment),
            expected,
            "{query}"
        );
    }
}

#[test]
fn native_accessibility_snapshot_updates_existing_query_lists() {
    use omoikane::platform_media::PlatformMediaPreferences;
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval("globalThis.hostChanges = []; globalThis.hostLists = ['(prefers-reduced-transparency: reduce)', '(inverted-colors: inverted)', '(prefers-contrast: more)'].map(q => { const list = matchMedia(q); list.addEventListener('change', e => hostChanges.push(e.matches)); return list; });").unwrap();
    let preference = PlatformMediaPreferences {
        reduced_transparency: Some(true),
        inverted_colors: Some(true),
        contrast: Some(omoikane::platform_media::ContrastPreference::More),
        ..PlatformMediaPreferences::default()
    };
    let mut environment = runtime.media_environment();
    preference.apply_to(&mut environment);
    runtime.set_media_environment(environment.clone()).unwrap();
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(runtime.eval("hostLists.every(list => list.matches) && hostChanges.length === 3 && hostChanges.every(Boolean)").unwrap().as_boolean(), Some(true));
    runtime.set_media_environment(environment).unwrap();
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime
            .eval("hostChanges.length === 3")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    let reset = PlatformMediaPreferences {
        reduced_transparency: Some(false),
        inverted_colors: Some(false),
        contrast: Some(omoikane::platform_media::ContrastPreference::NoPreference),
        ..PlatformMediaPreferences::default()
    };
    let mut environment = runtime.media_environment();
    reset.apply_to(&mut environment);
    runtime.set_media_environment(environment).unwrap();
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(runtime.eval("hostLists.every(list => !list.matches) && hostChanges.length === 6 && hostChanges.slice(3).every(value => value === false)").unwrap().as_boolean(), Some(true));
}

#[test]
fn media_snapshot_owns_color_scheme_and_output_type() {
    let mut environment = omoikane::css::MediaEnvironment::default();
    environment.color_scheme_dark = true;
    environment.media_type = omoikane::css::MediaType::Print;
    assert!(matches_environment(
        "print and (prefers-color-scheme: dark)",
        &environment
    ));
    assert!(!matches_environment("screen", &environment));
    assert!(!matches_environment(
        "(prefers-color-scheme: light)",
        &environment
    ));
    let query =
        omoikane::css::parse_media_query_list("print and (prefers-color-scheme: dark)").unwrap();
    assert!(omoikane::css::evaluate_media_query_for_type(
        &query[0],
        800.0,
        600.0,
        true,
        omoikane::css::MediaType::Print
    ));
}

#[test]
fn resolution_math_checks_dimensions_and_normalizes_mixed_units() {
    let mut environment = omoikane::css::MediaEnvironment::default();
    environment.resolution_dppx = 2.0;
    for query in [
        "(resolution: calc(3x - 96dpi))",
        "(resolution: calc(-1x + 3x))",
        "(resolution: abs(-2x))",
        "(resolution: calc(sign(-1x) * -2x))",
        "(resolution = calc(1dppx + 96dpi))",
        "(resolution: calc(4dppx / 2))",
        "(resolution: min(2dppx, 288dpi))",
        "(resolution: max(1dppx, 192dpi))",
        "(resolution: clamp(1dppx, 2x, 288dpi))",
        "(calc(96dpi / 1dppx * 1x) < resolution <= calc(2 * 1dppx))",
    ] {
        assert!(matches_environment(query, &environment), "{query}");
    }
    for query in [
        "(resolution: calc(2px))",
        "(resolution: calc(2))",
        "(resolution: calc(50%))",
        "(resolution: calc(2x / 0))",
        "(resolution: calc(1x + 1))",
        "(resolution: calc(1x+1x))",
        "(resolution: calc(2x);color:red)",
        "(resolution: min(2x,))",
    ] {
        assert!(!matches_environment(query, &environment), "{query}");
        assert!(
            !matches_environment(&format!("not {query}"), &environment),
            "negated {query}"
        );
    }
}

#[test]
fn media_change_handler_uses_listener_order_and_exception_reporting() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval("globalThis.changes = []; globalThis.errors = []; addEventListener('error', e => { errors.push(e.message); e.preventDefault(); }); globalThis.firstList = matchMedia('(prefers-reduced-motion: reduce)'); globalThis.secondList = matchMedia('(prefers-reduced-motion: reduce)'); firstList.addEventListener('change', e => { changes.push('before'); globalThis.peerCurrent = secondList.matches; globalThis.trustedChange = e.isTrusted; }); firstList.onchange = () => changes.push('old'); firstList.addEventListener('change', () => changes.push('after')); firstList.onchange = () => { changes.push('new'); throw new Error('media-handler-error'); }; secondList.addEventListener('change', () => changes.push('second'));").unwrap();
    let mut environment = runtime.media_environment();
    environment.set_feature("prefers-reduced-motion", "reduce");
    runtime.set_media_environment(environment).unwrap();
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(changes)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[\"before\",\"new\",\"after\",\"second\"]"
    );
    assert_eq!(runtime.eval("errors.length === 1 && errors[0] === 'media-handler-error' && firstList.matches && secondList.matches && peerCurrent && trustedChange").unwrap().as_boolean(), Some(true));
}

#[test]
fn media_change_handler_supports_manual_dispatch_cancellation_and_removal() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval("(() => { const list = matchMedia('all'); const calls = []; list.addEventListener('change', () => calls.push('before')); list.onchange = function(e) { calls.push(this === list ? 'handler' : 'wrong'); return false; }; list.addEventListener('change', () => calls.push('after')); const canceled = !list.dispatchEvent(new MediaQueryListEvent('change', {cancelable: true})); list.onchange = null; list.onchange = () => calls.push('readded'); list.dispatchEvent(new MediaQueryListEvent('change')); list.onchange = {}; return canceled && JSON.stringify(calls) === '[\"before\",\"handler\",\"after\",\"before\",\"after\",\"readded\"]' && list.onchange === null; })()").unwrap().as_boolean(), Some(true));
    assert_eq!(runtime.eval("(() => { const list = matchMedia('all'); let called = false; list.addEventListener('change', e => e.stopImmediatePropagation()); list.onchange = () => called = true; list.dispatchEvent(new MediaQueryListEvent('change')); return !called; })()").unwrap().as_boolean(), Some(true));
}

#[test]
fn iframe_media_query_uses_its_own_viewport() {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse(
            "<body><iframe id=frame width=200 height=200 style='border:none'></iframe></body>",
        )
        .document(),
    )
    .unwrap();
    runtime.set_viewport(800.0, 600.0);
    let result = runtime.eval(r#"(() => {
        const frame = document.createElement('iframe');
        frame.width = '200'; frame.height = '200'; frame.style.border = 'none';
        document.body.appendChild(frame);
        const child = frame.contentWindow;
        return JSON.stringify([child.visualViewport.width, child.matchMedia('(max-width: 200px)').matches,
            child.eval("matchMedia('(max-width: 200px)').matches"), matchMedia('(max-width: 200px)').matches]);
    })()"#).unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(result, "[200,true,true,false]");
    runtime.eval(r#"
        globalThis.mediaFrame = document.body.lastElementChild;
        globalThis.mediaChild = mediaFrame.contentWindow;
        mediaChild.eval(`
            globalThis.mediaChanges = [];
            globalThis.mediaList = matchMedia('(max-width: 200px)');
            mediaList.addEventListener('change', event => mediaChanges.push([event.matches, event.isTrusted]));
        `);
        mediaFrame.width = '250'; void mediaFrame.offsetWidth;
    "#).unwrap();
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(mediaChild.mediaChanges)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[[false,true]]"
    );
    assert_eq!(
        runtime
            .eval("mediaChild.mediaList.matches")
            .unwrap()
            .as_boolean(),
        Some(false)
    );
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime
            .eval("mediaChild.mediaChanges.length")
            .unwrap()
            .as_number(),
        Some(1.0)
    );
}

#[test]
fn environment_change_notifies_existing_child_query_lists() {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse("<body><iframe id=frame></iframe></body>").document(),
    )
    .unwrap();
    runtime
        .eval(
            r#"
        globalThis.child = document.getElementById('frame').contentWindow;
        globalThis.popup = open('', 'media-preferences');
        popup.eval(`
            globalThis.environmentChanges = [];
            globalThis.motionQuery = matchMedia('(prefers-reduced-motion: reduce)');
            motionQuery.onchange = event => environmentChanges.push(event.matches);
        `);
        child.eval(`
            globalThis.environmentChanges = [];
            globalThis.motionQuery = matchMedia('(prefers-reduced-motion: reduce)');
            motionQuery.onchange = event => environmentChanges.push(event.matches);
        `);
    "#,
        )
        .unwrap();
    let mut environment = runtime.media_environment();
    environment.set_feature("prefers-reduced-motion", "reduce");
    environment.resolution_dppx = 2.0;
    runtime.set_media_environment(environment.clone()).unwrap();
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(popup.environmentChanges)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[true]"
    );
    assert_eq!(
        runtime
            .eval("popup.devicePixelRatio === 2 && popup.motionQuery.matches")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    assert_eq!(
        runtime
            .eval("JSON.stringify(child.environmentChanges)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[true]"
    );
    assert_eq!(
        runtime
            .eval("child.devicePixelRatio === 2 && child.motionQuery.matches")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    runtime.set_media_environment(environment.clone()).unwrap();
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime
            .eval("popup.environmentChanges.length")
            .unwrap()
            .as_number(),
        Some(1.0)
    );
    assert_eq!(
        runtime
            .eval("child.environmentChanges.length")
            .unwrap()
            .as_number(),
        Some(1.0)
    );
    environment.set_feature("prefers-reduced-motion", "no-preference");
    runtime.set_media_environment(environment).unwrap();
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(popup.environmentChanges)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[true,false]"
    );
    assert_eq!(
        runtime
            .eval("JSON.stringify(child.environmentChanges)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[true,false]"
    );
}

#[test]
fn new_child_windows_inherit_current_resolution_before_scripts() {
    let mut runtime =
        JsRuntime::with_document(TreeBuilder::parse("<body></body>").document()).unwrap();
    let mut environment = runtime.media_environment();
    environment.resolution_dppx = 2.0;
    runtime.set_media_environment(environment.clone()).unwrap();
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const frame = document.createElement('iframe');
        frame.srcdoc = '<script>globalThis.initialResolution = devicePixelRatio === 2 && matchMedia("(resolution: 2dppx)").matches;<\/script>';
        document.body.appendChild(frame);
        globalThis.resolutionChild = frame.contentWindow;
        globalThis.resolutionPopup = open('', 'resolution-popup');
        return resolutionChild.devicePixelRatio === 2 && resolutionPopup.devicePixelRatio === 2 &&
            resolutionChild.matchMedia('(resolution: 2dppx)').matches &&
            resolutionPopup.matchMedia('(resolution: 2dppx)').matches;
    })()"#).unwrap().as_boolean(), Some(true));
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("resolutionChild.initialResolution")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    runtime.set_media_environment(environment).unwrap();
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime
            .eval(
                "resolutionChild.devicePixelRatio === 2 && resolutionPopup.devicePixelRatio === 2"
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn media_query_list_preserves_its_query_and_rejects_direct_construction() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        let constructorsRejected = false;
        try { new MediaQueryList('all'); } catch (error) { constructorsRejected = error instanceof TypeError; }
        const list = matchMedia('all');
        let writeRejected = false;
        try { (() => { 'use strict'; list.media = 'print'; })(); }
        catch (error) { writeRejected = error instanceof TypeError; }
        const original = list.media;
        Object.defineProperty(list, 'media', {value: 'print'});
        return constructorsRejected && writeRejected && original === 'all' && list.matches;
    })()"#).unwrap().as_boolean(), Some(true));
}

#[test]
fn media_changes_are_coalesced_before_animation_frame_callbacks() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval("globalThis.mediaOrder = []; globalThis.motion = matchMedia('(prefers-reduced-motion: reduce)'); motion.onchange = () => mediaOrder.push('change'); requestAnimationFrame(() => mediaOrder.push('frame'));").unwrap();
    let baseline = runtime.media_environment();
    let mut reduced = baseline.clone();
    reduced.set_feature("prefers-reduced-motion", "reduce");
    runtime.set_media_environment(reduced.clone()).unwrap();
    assert_eq!(
        runtime
            .eval("motion.matches && mediaOrder.length === 0")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    runtime.set_media_environment(baseline).unwrap();
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(mediaOrder)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[\"frame\"]"
    );
    runtime
        .eval("mediaOrder.length = 0; requestAnimationFrame(() => mediaOrder.push('frame'))")
        .unwrap();
    runtime.set_media_environment(reduced).unwrap();
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(mediaOrder)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[\"change\",\"frame\"]"
    );
}

#[test]
fn media_changes_follow_container_tree_order() {
    let mut runtime =
        JsRuntime::with_document(TreeBuilder::parse("<body></body>").document()).unwrap();
    runtime.eval(r#"
        globalThis.mediaTreeOrder = [];
        const a = document.createElement('iframe');
        const b = document.createElement('iframe');
        document.body.appendChild(b); document.body.appendChild(a);
        a.contentWindow.eval("matchMedia('(prefers-reduced-motion: reduce)').onchange = () => parent.mediaTreeOrder.push('a')");
        b.contentWindow.eval("matchMedia('(prefers-reduced-motion: reduce)').onchange = () => parent.mediaTreeOrder.push('b')");
        matchMedia('(prefers-reduced-motion: reduce)').onchange = () => mediaTreeOrder.push('root');
    "#).unwrap();
    let mut environment = runtime.media_environment();
    environment.set_feature("prefers-reduced-motion", "reduce");
    runtime.set_media_environment(environment).unwrap();
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(mediaTreeOrder)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[\"root\",\"b\",\"a\"]"
    );
}

#[test]
fn hidden_documents_defer_media_notifications_until_renderable() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval("globalThis.hiddenChanges = 0; globalThis.hiddenMotion = matchMedia('(prefers-reduced-motion: reduce)'); hiddenMotion.onchange = () => hiddenChanges++;").unwrap();
    runtime.set_page_visibility(true);
    let mut environment = runtime.media_environment();
    environment.set_feature("prefers-reduced-motion", "reduce");
    runtime.set_media_environment(environment).unwrap();
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime
            .eval("hiddenMotion.matches && hiddenChanges === 0")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    runtime.set_page_visibility(false);
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime.eval("hiddenChanges").unwrap().as_number(),
        Some(1.0)
    );
}

#[test]
fn pointer_inventory_changes_and_removal_notify_existing_media_queries() {
    use omoikane::platform_media::{PointerCapabilities, PointerDeviceTracker};
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval("globalThis.pointerChanges = []; globalThis.pointerLists = ['(pointer: fine)', '(pointer: coarse)', '(any-pointer: none)', '(hover: hover)'].map(q => { const list = matchMedia(q); list.addEventListener('change', e => pointerChanges.push([e.media, e.matches])); return list; });").unwrap();
    let touch = PointerCapabilities {
        coarse: true,
        ..Default::default()
    };
    let mut devices = PointerDeviceTracker::new(None);
    devices.observe(1, touch);
    let mut environment = runtime.media_environment();
    devices
        .snapshot(PointerCapabilities::default())
        .apply_to(&mut environment);
    runtime.set_media_environment(environment).unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(pointerLists.map(list => list.matches))")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[false,true,false,false]"
    );
    assert_eq!(
        runtime.eval("pointerChanges.length").unwrap().as_number(),
        Some(0.0)
    );
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime.eval("pointerChanges.length").unwrap().as_number(),
        Some(3.0)
    );
    assert!(devices.remove(&1));
    let mut environment = runtime.media_environment();
    devices
        .snapshot(PointerCapabilities::default())
        .apply_to(&mut environment);
    runtime.set_media_environment(environment).unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(pointerLists.map(list => list.matches))")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[false,false,true,false]"
    );
    runtime.run_animation_frame(32).unwrap();
    assert_eq!(
        runtime.eval("pointerChanges.length").unwrap().as_number(),
        Some(5.0)
    );
}

#[test]
fn negative_resolution_values_are_known_comparisons() {
    let environment = omoikane::css::MediaEnvironment::default();
    for query in [
        "not (resolution: -300dpi)",
        "(resolution > -1dppx)",
        "(-1x < resolution)",
        "(min-resolution: -2dppx)",
        "not (max-resolution: -2dppx)",
        "(resolution > calc(-1x))",
        "not (resolution: calc(-1x))",
    ] {
        assert!(matches_environment(query, &environment), "{query}");
    }
    for query in [
        "(resolution: -300dpi)",
        "(resolution <= -1dppx)",
        "(max-resolution: -2dppx)",
    ] {
        assert!(!matches_environment(query, &environment), "{query}");
    }
}

#[test]
fn integer_display_ranges_accept_negative_values_and_use_owned_capabilities() {
    let mut environment = omoikane::css::MediaEnvironment::default();
    for feature in ["color", "color-index", "monochrome"] {
        for query in [
            format!("(min-{feature}: -10)"),
            format!("not (max-{feature}: -10)"),
            format!("not ({feature}: -10)"),
            format!("(-10 < {feature})"),
        ] {
            assert!(matches_environment(&query, &environment), "{query}");
        }
        for query in [
            format!("({feature}: -10)"),
            format!("({feature} <= -10)"),
            format!("not ({feature}: invalid)"),
        ] {
            assert!(!matches_environment(&query, &environment), "{query}");
        }
    }
    environment.color_bits_per_component = 0;
    environment.color_index = 16;
    environment.monochrome_bits_per_pixel = 4;
    for query in [
        "not (color)",
        "(color: 0)",
        "(color-index)",
        "(color-index: 16)",
        "(8 < color-index <= 16)",
        "(monochrome)",
        "(monochrome: 4)",
        "(min-monochrome: 4)",
    ] {
        assert!(matches_environment(query, &environment), "{query}");
    }
    for query in ["(color)", "(color-index: 0)", "(monochrome > 4)"] {
        assert!(!matches_environment(query, &environment), "{query}");
    }
}

#[test]
fn device_dimensions_use_display_snapshot_independently_of_viewport() {
    let mut environment = omoikane::css::MediaEnvironment::default();
    environment.device_width = Some(1920.0);
    environment.device_height = Some(1080.0);
    for query in [
        "(device-width: 1920px)",
        "(device-height: 1080px)",
        "(1000px < device-width <= 1920px)",
        "(device-height > -1px)",
        "(min-device-width: -100px)",
        "not (max-device-height: -100px)",
    ] {
        assert!(matches_environment(query, &environment), "{query}");
    }
    for query in [
        "(device-width: 800px)",
        "(device-height: 600px)",
        "not (device-width: invalid)",
        "(device-width > 1 px)",
    ] {
        assert!(!matches_environment(query, &environment), "{query}");
    }
    assert!(matches_environment("(-1px < width < 900px)", &environment));
    assert!(!matches_environment("(0px < width > 100px)", &environment));
}

#[test]
fn child_device_queries_use_the_shared_virtual_display() {
    let mut runtime =
        JsRuntime::with_document(TreeBuilder::parse("<body></body>").document()).unwrap();
    runtime.set_viewport(800.0, 600.0);
    runtime.eval("globalThis.deviceFrame = document.createElement('iframe'); deviceFrame.width = '200'; deviceFrame.height = '100'; deviceFrame.srcdoc = `<style>#probe {color:red} @media (min-device-width:750px) {#probe {color:green}}</style><div id=probe>device</div>`; document.body.appendChild(deviceFrame); globalThis.deviceChild = deviceFrame.contentWindow;").unwrap();
    assert_eq!(runtime.eval("deviceChild.matchMedia('(device-width:800px)').matches && deviceChild.matchMedia('(width:200px)').matches").unwrap().as_boolean(),Some(true));
    assert_eq!(
        runtime
            .eval(
                "deviceChild.getComputedStyle(deviceChild.document.getElementById('probe')).color"
            )
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "rgb(0, 128, 0)"
    );
    assert_eq!(runtime.eval("screen.width === 800 && deviceChild.screen.width === 800 && screen.height === 600 && deviceChild.screen.height === 600").unwrap().as_boolean(),Some(true));
    runtime.eval("deviceChild.eval(`globalThis.deviceChanges=0;globalThis.deviceList=matchMedia('(device-width:800px)');deviceList.onchange=()=>deviceChanges++`)").unwrap();
    runtime.set_viewport(700.0, 500.0);
    assert_eq!(runtime.eval("!deviceChild.deviceList.matches && deviceChild.screen.width === 700 && deviceChild.deviceChanges === 0").unwrap().as_boolean(), Some(true));
    assert_eq!(
        runtime
            .eval(
                "deviceChild.getComputedStyle(deviceChild.document.getElementById('probe')).color"
            )
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "rgb(255, 0, 0)"
    );
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime
            .eval("deviceChild.deviceChanges")
            .unwrap()
            .as_number(),
        Some(1.0)
    );
}

#[test]
fn native_display_snapshot_updates_screen_in_existing_and_new_windows() {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse("<body><iframe id=display-frame></iframe></body>").document(),
    )
    .unwrap();
    runtime.eval("globalThis.displayChild = document.getElementById('display-frame').contentWindow; globalThis.displayPopup = open('', 'display-screen'); globalThis.displayScreen = screen;").unwrap();
    let mut environment = runtime.media_environment();
    environment.device_width = Some(1920.0);
    environment.device_height = Some(1080.0);
    environment.color_bits_per_component = 10;
    runtime.set_media_environment(environment).unwrap();
    assert_eq!(runtime.eval("[window, displayChild, displayPopup].every(w => w.screen.width === 1920 && w.screen.height === 1080 && w.screen.colorDepth === 30 && w.matchMedia('(device-width:1920px)').matches) && screen === displayScreen").unwrap().as_boolean(),Some(true));
    runtime.eval("globalThis.laterDisplay = document.createElement('iframe'); document.body.appendChild(laterDisplay);").unwrap();
    assert_eq!(runtime.eval("laterDisplay.contentWindow.screen.width === 1920 && laterDisplay.contentWindow.screen.height === 1080 && laterDisplay.contentWindow.matchMedia('(device-width:1920px)').matches").unwrap().as_boolean(),Some(true));
    runtime.set_viewport(640.0, 400.0);
    assert_eq!(runtime.eval("screen.width === 1920 && screen.height === 1080 && displayChild.screen.width === 1920 && innerWidth === 640").unwrap().as_boolean(), Some(true));
}

#[test]
fn invalid_native_dimensions_restore_the_shared_virtual_display() {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse("<body><iframe id=invalid-screen></iframe></body>").document(),
    )
    .unwrap();
    runtime.set_viewport(800.0, 600.0);
    let mut environment = runtime.media_environment();
    environment.device_width = Some(f32::NAN);
    environment.device_height = Some(-1.0);
    runtime.set_media_environment(environment).unwrap();
    assert_eq!(runtime.eval("const invalidChild=document.getElementById('invalid-screen').contentWindow; screen.width === 800 && screen.height === 600 && invalidChild.screen.width === 800 && invalidChild.matchMedia('(device-width:800px)').matches").unwrap().as_boolean(),Some(true));
}

#[test]
fn infinite_resolution_is_a_known_keyword_for_finite_displays() {
    let environment = omoikane::css::MediaEnvironment::default();
    for query in [
        "not (resolution: infinite)",
        "(resolution < infinite)",
        "(resolution <= infinite)",
        "(infinite > resolution)",
        "(max-resolution: infinite)",
        "(0dppx < resolution < infinite)",
    ] {
        assert!(matches_environment(query, &environment), "{query}");
    }
    for query in [
        "(resolution: infinite)",
        "(min-resolution: infinite)",
        "(resolution > infinite)",
        "not (resolution: infinity)",
        "not (resolution: -infinite)",
    ] {
        assert!(!matches_environment(query, &environment), "{query}");
    }
}

#[test]
fn viewport_and_device_aspect_ratios_use_separate_snapshot_dimensions() {
    let mut environment = omoikane::css::MediaEnvironment::default();
    environment.device_width = Some(1920.0);
    environment.device_height = Some(1080.0);
    for query in [
        "(aspect-ratio)",
        "(aspect-ratio: 4/3)",
        "(min-aspect-ratio: 1.25)",
        "(max-aspect-ratio: 1.5)",
        "(1/1 < aspect-ratio < 3/2)",
        "(3/2 > aspect-ratio >= 4/3)",
        "(device-aspect-ratio: 16/9)",
        "(min-device-aspect-ratio: 1.7)",
        "(device-aspect-ratio > aspect-ratio-unknown) or (aspect-ratio: 4/3)",
        "not (aspect-ratio: 16/9)",
    ] {
        assert!(matches_environment(query, &environment), "{query}");
    }
    for query in [
        "(aspect-ratio: 16/9)",
        "(device-aspect-ratio: 4/3)",
        "not (aspect-ratio: 1px/2)",
        "not (aspect-ratio: -1/2)",
        "not (aspect-ratio: 1/2/3)",
        "not (1 < aspect-ratio > 2)",
    ] {
        assert!(!matches_environment(query, &environment), "{query}");
    }
}

#[test]
fn degenerate_ratio_queries_follow_fixed_wpt_zero_denominator_contract() {
    let environment = omoikane::css::MediaEnvironment::default();
    for query in [
        "(max-aspect-ratio: 0/0)",
        "(max-aspect-ratio: 1/0)",
        "not (aspect-ratio: 0/0)",
        "(aspect-ratio > 0/1)",
    ] {
        assert!(matches_environment(query, &environment), "{query}");
    }
    for query in ["(min-aspect-ratio: 0/0)", "(aspect-ratio: 0/1)"] {
        assert!(!matches_environment(query, &environment), "{query}");
    }
}

#[test]
fn child_aspect_ratio_uses_local_viewport_and_shared_display_updates() {
    let mut runtime =
        JsRuntime::with_document(TreeBuilder::parse("<body></body>").document()).unwrap();
    runtime.set_viewport(800.0, 600.0);
    runtime.eval("globalThis.ratioFrame = document.createElement('iframe'); ratioFrame.width = '200'; ratioFrame.height = '100'; ratioFrame.srcdoc = `<style>#probe {color:red} @media (aspect-ratio:2) {#probe {color:green}}</style><div id=probe>ratio</div>`; document.body.appendChild(ratioFrame); globalThis.ratioChild = ratioFrame.contentWindow;").unwrap();
    assert_eq!(runtime.eval("matchMedia('(aspect-ratio:4/3)').matches && ratioChild.matchMedia('(aspect-ratio:2)').matches && ratioChild.matchMedia('(device-aspect-ratio:4/3)').matches && ratioChild.getComputedStyle(ratioChild.document.getElementById('probe')).color === 'rgb(0, 128, 0)'").unwrap().as_boolean(), Some(true));
    runtime.eval("globalThis.displayRatio = ratioChild.matchMedia('(device-aspect-ratio:4/3)'); globalThis.ratioChanges = 0; displayRatio.onchange = () => ratioChanges++;").unwrap();
    let mut environment = runtime.media_environment();
    environment.device_width = Some(1920.0);
    environment.device_height = Some(1080.0);
    runtime.set_media_environment(environment).unwrap();
    assert_eq!(runtime.eval("!displayRatio.matches && ratioChild.matchMedia('(device-aspect-ratio:16/9)').matches && ratioChild.matchMedia('(aspect-ratio:2)').matches && ratioChanges === 0").unwrap().as_boolean(),Some(true));
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(runtime.eval("ratioChanges").unwrap().as_number(), Some(1.0));
}

#[test]
fn empty_colon_values_are_unknown_instead_of_boolean_features() {
    let environment = omoikane::css::MediaEnvironment::default();
    for feature in [
        "hover",
        "pointer",
        "resolution",
        "width",
        "height",
        "color",
        "aspect-ratio",
        "device-aspect-ratio",
        "prefers-color-scheme",
    ] {
        for query in [
            format!("({feature}:)"),
            format!("not ({feature}:)"),
            format!("({feature}:   )"),
        ] {
            assert!(!matches_environment(&query, &environment), "{query}");
        }
        assert!(
            matches_environment(&format!("({feature}:) or (width > 0px)"), &environment),
            "unknown OR true: {feature}"
        );
    }
}

#[test]
fn numeric_math_queries_preserve_dimensions_and_comparison_semantics() {
    let environment = omoikane::css::MediaEnvironment::default();
    for query in [
        "(width: calc(400px * 2))",
        "(width >= calc(700px + 6.25rem))",
        "(height: calc(10in - 360px))",
        "(max-width: min(900px, 1000px))",
        "(device-width: calc(8in + 32px))",
        "(aspect-ratio: calc(4 / 3))",
        "(1 < aspect-ratio < calc(3 / 2))",
        "(resolution: calc(192dpi / 2))",
    ] {
        assert!(matches_environment(query, &environment), "{query}");
    }
    for query in [
        "(width: calc(400px * 3))",
        "not (width: calc(1px + 1dppx))",
        "not (width: calc(1px * 1px))",
        "not (width: calc(100%))",
        "not (aspect-ratio: calc(1px))",
        "not (resolution: calc(1px))",
    ] {
        assert!(!matches_environment(query, &environment), "{query}");
    }
}

#[test]
fn media_comments_preserve_token_boundaries() {
    let environment = omoikane::css::MediaEnvironment::default();
    for query in [
        "/* lead */ (hover: /* value */ hover)",
        "screen/**/and/**/(pointer: fine)",
        "(width/**/:/**/800px)",
        "(hover: hover) /* comma , ) */ , (pointer: none)",
        "(hover: hover) /* unclosed",
    ] {
        assert!(matches_environment(query, &environment), "{query}");
    }
    for query in [
        "(pointer: f/**/ine)",
        "not (pointer: f/**/ine)",
        "(width: 800/**/px)",
    ] {
        assert!(!matches_environment(query, &environment), "{query}");
    }
}

#[test]
fn comments_between_comparison_delimiters_do_not_insert_whitespace() {
    let environment = omoikane::css::MediaEnvironment::default();
    for query in [
        "(width >/**/= 800px)",
        "(width >/**//**/= 800px)",
        "(height </**/= 600px)",
        "(resolution >/**/= 1dppx)",
        "(1dppx </**/= resolution)",
        "(400px </**/= width </**/= 800px)",
    ] {
        assert!(matches_environment(query, &environment), "{query}");
    }
    for query in [
        "(width > /* comment */ = 800px)",
        "(resolution < /**/= 1dppx)",
    ] {
        assert!(!matches_environment(query, &environment), "{query}");
    }
}

#[test]
fn media_changes_visit_nested_shadow_frames_and_skip_detached_documents() {
    let mut runtime =
        JsRuntime::with_document(TreeBuilder::parse("<body></body>").document()).unwrap();
    runtime.eval(r#"
        globalThis.mediaOrder = [];
        const host = document.createElement('div');
        document.body.appendChild(host);
        const shadow = host.attachShadow({mode: 'open'});
        const shadowFrame = document.createElement('iframe');
        shadow.appendChild(shadowFrame);
        const lightFrame = document.createElement('iframe');
        document.body.appendChild(lightFrame);
        const detachedFrame = document.createElement('iframe');
        document.body.appendChild(detachedFrame);
        const detachedWindow = detachedFrame.contentWindow;
        detachedWindow.eval("matchMedia('(prefers-reduced-motion: reduce)').onchange = () => parent.mediaOrder.push('detached')");
        detachedFrame.remove();
        shadowFrame.contentWindow.eval(`
            const nestedFrame = document.createElement('iframe');
            document.body.appendChild(nestedFrame);
            matchMedia('(prefers-reduced-motion: reduce)').onchange = () => parent.mediaOrder.push('shadow');
            nestedFrame.contentWindow.eval("matchMedia('(prefers-reduced-motion: reduce)').onchange = () => parent.parent.mediaOrder.push('nested')");
        `);
        lightFrame.contentWindow.eval("matchMedia('(prefers-reduced-motion: reduce)').onchange = () => parent.mediaOrder.push('light')");
        matchMedia('(prefers-reduced-motion: reduce)').onchange = () => mediaOrder.push('root');
    "#).unwrap();
    let mut environment = runtime.media_environment();
    environment.set_feature("prefers-reduced-motion", "reduce");
    runtime.set_media_environment(environment).unwrap();
    assert_eq!(
        runtime.eval("mediaOrder.length").unwrap().as_number(),
        Some(0.0)
    );
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(mediaOrder)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[\"root\",\"shadow\",\"nested\",\"light\"]"
    );
    runtime.run_animation_frame(32).unwrap();
    assert_eq!(
        runtime.eval("mediaOrder.length").unwrap().as_number(),
        Some(4.0)
    );
}

#[test]
fn forced_palette_lightness_controls_queries_without_losing_host_preference() {
    use omoikane::css::{ForcedColorPalette, MediaEnvironment};
    for host_dark in [false, true] {
        for (rgb, expected) in [
            ([0, 0, 0], true),
            ([255, 255, 255], false),
            ([128, 128, 128], host_dark),
            ([77, 77, 77], true),
            ([78, 78, 78], host_dark),
            ([163, 163, 163], host_dark),
            ([164, 164, 164], false),
        ] {
            let mut environment = MediaEnvironment::default();
            environment.color_scheme_dark = host_dark;
            environment.set_feature("forced-colors", "active");
            let mut palette = ForcedColorPalette::for_color_scheme(false);
            palette.canvas = rgb;
            environment.forced_color_palette = Some(palette);
            assert_eq!(
                matches_environment("(prefers-color-scheme: dark)", &environment),
                expected
            );
            environment.set_feature("forced-colors", "none");
            assert_eq!(
                matches_environment("(prefers-color-scheme: dark)", &environment),
                host_dark
            );
        }
    }
}

#[test]
fn forced_palette_scheme_updates_css_and_existing_query_lists() {
    use omoikane::css::ForcedColorPalette;
    let document = TreeBuilder::parse("<style>#probe { width:10px } @media (prefers-color-scheme:dark) { #probe { width:20px } }</style><div id='probe'></div>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.eval("globalThis.schemeEvents=[];globalThis.schemeList=matchMedia('(prefers-color-scheme:dark)');schemeList.onchange=e=>schemeEvents.push(e.matches)").unwrap();
    let mut environment = runtime.media_environment();
    environment.set_feature("forced-colors", "active");
    environment.forced_color_palette = Some(ForcedColorPalette::for_color_scheme(true));
    runtime.set_media_environment(environment.clone()).unwrap();
    assert_eq!(runtime.eval("schemeList.matches && schemeEvents.length===0 && getComputedStyle(document.getElementById('probe')).width==='20px'").unwrap().as_boolean(), Some(true));
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(schemeEvents)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[true]"
    );
    environment.set_feature("forced-colors", "none");
    runtime.set_media_environment(environment).unwrap();
    assert_eq!(runtime.eval("!schemeList.matches && getComputedStyle(document.getElementById('probe')).width==='10px'").unwrap().as_boolean(), Some(true));
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(schemeEvents)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[true,false]"
    );
}

#[test]
fn media_query_event_identity_does_not_depend_on_page_reflect_apply() {
    let document = TreeBuilder::parse("<body></body>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                    const list = matchMedia('(hover: hover)');
                    let calls = 0;
                    list.addEventListener('change', () => calls++);
                    const original = Reflect.apply;
                    Reflect.apply = () => { throw new Error('page Reflect.apply must not be used'); };
                    try {
                        return list.dispatchEvent(new Event('change')) && calls === 1;
                    } finally {
                        Reflect.apply = original;
                    }
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}
