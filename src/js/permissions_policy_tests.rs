use super::{GeolocationPositionData, JsRuntime};
use crate::html::TreeBuilder;
use serde_json::{Value, json};

fn json_result(runtime: &mut JsRuntime, source: &str) -> Value {
    let result = runtime.eval(source).expect("evaluate policy assertion");
    serde_json::from_str(&result.as_string().unwrap().to_std_string_escaped())
        .expect("parse JSON result")
}

#[test]
fn document_policy_object_reports_the_same_header_rules_as_fullscreen() {
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse("<body><button>go</button></body>").document(),
        "https://owner.test/page",
    )
    .unwrap();
    runtime.install_response_security_headers(&[(
        "Permissions-Policy".into(),
        "fullscreen=(self \"https://child.test\"), geolocation=(), cross-origin-isolated=()".into(),
    )]);
    assert_eq!(
        json_result(
            &mut runtime,
            "JSON.stringify({same:document.permissionsPolicy===document.permissionsPolicy && \
             document.featurePolicy===document.permissionsPolicy, \
             features:document.permissionsPolicy.features(), \
             allowed:document.permissionsPolicy.allowedFeatures(), \
             self:document.permissionsPolicy.allowsFeature('fullscreen'), \
             child:document.permissionsPolicy.allowsFeature('fullscreen','https://child.test'), \
             other:document.permissionsPolicy.allowsFeature('fullscreen','https://other.test'), \
             disabled:document.permissionsPolicy.allowsFeature('geolocation'), \
             isolation:document.permissionsPolicy.allowsFeature('cross-origin-isolated'), \
             unknown:document.permissionsPolicy.allowsFeature('made-up'), \
             list:document.permissionsPolicy.getAllowlistForFeature('fullscreen'), \
             empty:document.permissionsPolicy.getAllowlistForFeature('geolocation'), \
             privateHook:typeof globalThis.__omoikane_permissions_policy_query})"
        ),
        json!({
            "same": true,
            "features": ["fullscreen", "geolocation", "gamepad", "screen-wake-lock", "cross-origin-isolated", "storage-access", "focus-without-user-activation"],
            "allowed": ["fullscreen", "gamepad", "screen-wake-lock", "storage-access", "focus-without-user-activation"],
            "self": true,
            "child": true,
            "other": false,
            "disabled": false,
            "isolation": false,
            "unknown": false,
            "list": ["https://owner.test", "https://child.test"],
            "empty": [],
            "privateHook": "undefined"
        })
    );
    assert_eq!(
        json_result(
            &mut runtime,
            "JSON.stringify([document.fullscreenEnabled, (()=>{try {\
             document.permissionsPolicy.allowsFeature.call({},'fullscreen'); return false \
             } catch(error) { return error.name==='TypeError' }})()])"
        ),
        json!([true, true])
    );

    runtime.install_response_security_headers(&[(
        "Permissions-Policy".into(),
        "fullscreen=(), geolocation=*".into(),
    )]);
    assert_eq!(
        json_result(
            &mut runtime,
            "JSON.stringify([document.permissionsPolicy.allowsFeature('fullscreen'), \
             document.fullscreenEnabled, document.permissionsPolicy.allowedFeatures(), \
             document.permissionsPolicy.getAllowlistForFeature('geolocation')])"
        ),
        json!([
            false,
            false,
            [
                "geolocation",
                "gamepad",
                "screen-wake-lock",
                "cross-origin-isolated",
                "storage-access",
                "focus-without-user-activation"
            ],
            ["*"]
        ])
    );

    runtime.install_response_security_headers(&[(
        "Permissions-Policy".into(),
        "fullscreen=(\"https://child.test\")".into(),
    )]);
    assert_eq!(
        json_result(
            &mut runtime,
            "JSON.stringify([document.permissionsPolicy.allowsFeature('fullscreen'), \
             document.permissionsPolicy.allowsFeature('fullscreen','https://child.test'), \
             document.permissionsPolicy.allowedFeatures(), \
             document.permissionsPolicy.getAllowlistForFeature('fullscreen')])"
        ),
        json!([
            false,
            true,
            [
                "geolocation",
                "gamepad",
                "screen-wake-lock",
                "cross-origin-isolated",
                "storage-access",
                "focus-without-user-activation"
            ],
            []
        ])
    );

    runtime.install_response_security_headers(&[(
        "Permissions-Policy".into(),
        "fullscreen=(*;bad=?)".into(),
    )]);
    assert_eq!(
        json_result(
            &mut runtime,
            "JSON.stringify([document.permissionsPolicy.allowsFeature('fullscreen', \
             'https://child.test'), document.fullscreenEnabled])"
        ),
        json!([false, true])
    );
}

#[test]
fn document_policy_methods_require_a_feature_argument() {
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse("<body></body>").document(),
        "https://owner.test/page",
    )
    .unwrap();
    assert_eq!(
        json_result(
            &mut runtime,
            "JSON.stringify(['allowsFeature', 'getAllowlistForFeature'].map(name => { \
               try { document.permissionsPolicy[name](); return 'no-error'; } \
               catch (error) { return error.name; } \
             }))"
        ),
        json!(["TypeError", "TypeError"])
    );
    assert_eq!(
        json_result(
            &mut runtime,
            "JSON.stringify([document.permissionsPolicy.allowsFeature(undefined), \
             document.permissionsPolicy.getAllowlistForFeature(undefined)])"
        ),
        json!([false, []])
    );
}

#[test]
fn denied_geolocation_header_rejects_a_real_request_and_permission_query() {
    for header in [
        "geolocation=()",
        "geolocation=(bogus)",
        "geolocation=(bogus;note=ok)",
        "geolocation=(?0 42)",
        "geolocation=*, geolocation=()",
    ] {
        let mut runtime = JsRuntime::with_document_and_url(
            TreeBuilder::parse("<body></body>").document(),
            "https://owner.test/page",
        )
        .unwrap();
        runtime.set_geolocation_position(GeolocationPositionData::new(35.0, 139.0, 4.0));
        runtime.install_response_security_headers(&[("Permissions-Policy".into(), header.into())]);
        runtime
            .eval(
                "globalThis.geoResult='pending';globalThis.permissionResult='pending'; \
                 navigator.geolocation.getCurrentPosition(\
                   ()=>geoResult='success',error=>geoResult=error.code); \
                 navigator.permissions.query({name:'geolocation'}).then(\
                   status=>permissionResult=status.state);",
            )
            .unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(
            json_result(&mut runtime, "JSON.stringify([geoResult,permissionResult])"),
            json!([1, "denied"]),
            "{header}"
        );
    }
}

#[test]
fn iframe_allow_controls_child_geolocation_policy() {
    let document = TreeBuilder::parse(
        "<body><iframe allow='geolocation' srcdoc='<p>child</p>'></iframe></body>",
    )
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://owner.test/page").unwrap();
    let frame = runtime.document().query_selector("iframe").unwrap();
    let child = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    assert!(
        runtime
            .host_state
            .borrow()
            .feature_enabled_in_document(child.identity(), super::PolicyFeature::Geolocation)
    );
    runtime
        .eval("document.querySelector('iframe').allow=\"geolocation 'none'\"")
        .unwrap();
    assert!(
        runtime
            .host_state
            .borrow()
            .feature_enabled_in_document(child.identity(), super::PolicyFeature::Geolocation)
    );
    runtime
        .eval("document.querySelector('iframe').srcdoc='<p>next</p>'")
        .unwrap();
    let next_child = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    assert_ne!(child.identity(), next_child.identity());
    assert!(
        !runtime
            .host_state
            .borrow()
            .feature_enabled_in_document(next_child.identity(), super::PolicyFeature::Geolocation)
    );
}

#[test]
fn sandboxed_iframe_src_permission_does_not_follow_location_navigation() {
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse(
            "<body><iframe sandbox='allow-scripts' allow='fullscreen' \
             srcdoc='<script>0</script>'></iframe></body>",
        )
        .document(),
        "https://owner.test/page",
    )
    .unwrap();
    let frame = runtime.document().query_selector("iframe").unwrap();
    let initial = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .existing_realm_for_document(initial.identity())
            .is_some()
    );
    assert!(
        runtime
            .host_state
            .borrow()
            .fullscreen_allowed_for_document(initial.identity())
    );

    runtime
        .eval_in_document_realm(initial.identity(), "location.reload()")
        .unwrap();
    let reloaded_srcdoc = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    assert_ne!(initial.identity(), reloaded_srcdoc.identity());
    assert!(
        runtime
            .host_state
            .borrow()
            .fullscreen_allowed_for_document(reloaded_srcdoc.identity())
    );
    runtime.run_until_idle().unwrap();

    // The WindowProxy navigation implementation writes `iframe.src` as an
    // internal resource trigger. It must not redefine the embedding page's
    // original `'src'` permission for this child navigation.
    runtime
        .eval(
            "globalThis.frame=document.querySelector('iframe'); \
             frame.contentWindow.location='data:text/html,%3Cscript%3E0%3C/script%3E'",
        )
        .unwrap();
    let navigated = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .existing_realm_for_document(navigated.identity())
            .is_some()
    );
    assert_ne!(initial.identity(), navigated.identity());
    assert!(
        !runtime
            .host_state
            .borrow()
            .fullscreen_allowed_for_document(navigated.identity())
    );

    // Browser-triggered reload and history traversal must keep the embedding
    // element's declared origin from the original srcdoc navigation.
    runtime
        .eval_in_document_realm(navigated.identity(), "location.reload()")
        .unwrap();
    let reloaded = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    assert_ne!(navigated.identity(), reloaded.identity());
    assert!(
        !runtime
            .host_state
            .borrow()
            .fullscreen_allowed_for_document(reloaded.identity())
    );
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .existing_realm_for_document(reloaded.identity())
            .is_some()
    );

    runtime
        .eval_in_document_realm(reloaded.identity(), "history.back()")
        .unwrap();
    let restored_srcdoc = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    assert!(
        runtime
            .host_state
            .borrow()
            .fullscreen_allowed_for_document(restored_srcdoc.identity())
    );
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .existing_realm_for_document(restored_srcdoc.identity())
            .is_some()
    );

    runtime
        .eval_in_document_realm(restored_srcdoc.identity(), "history.forward()")
        .unwrap();
    let restored_data = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    assert_ne!(restored_srcdoc.identity(), restored_data.identity());
    assert!(
        !runtime
            .host_state
            .borrow()
            .fullscreen_allowed_for_document(restored_data.identity())
    );

    runtime.eval("frame.srcdoc='<p>C</p>'").unwrap();
    let embedding_navigation = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    assert_ne!(navigated.identity(), embedding_navigation.identity());
    assert!(
        runtime
            .host_state
            .borrow()
            .fullscreen_allowed_for_document(embedding_navigation.identity())
    );
}

#[test]
fn iframe_src_history_traversal_recomputes_declared_origin_for_embedding_navigation() {
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse(
            "<body><iframe allow='fullscreen' src='data:text/html,%3Cscript%3E0%3C/script%3E'></iframe></body>",
        )
        .document(),
        "https://owner.test/page",
    )
    .unwrap();
    let frame = runtime.document().query_selector("iframe").unwrap();
    runtime.run_until_idle().unwrap();
    let initial_data = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    runtime
        .eval(
            "globalThis.frame=document.querySelector('iframe'); \
               frame.src='data:text/html,%3Cscript%3E1%3C/script%3E'",
        )
        .unwrap();
    let first_data = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    assert_ne!(initial_data.identity(), first_data.identity());
    assert!(
        runtime
            .host_state
            .borrow()
            .fullscreen_allowed_for_document(first_data.identity())
    );
    runtime.run_until_idle().unwrap();

    runtime
        .eval("frame.src='data:text/html,%3Cscript%3E2%3C/script%3E'")
        .unwrap();
    let second_data = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    assert_ne!(first_data.identity(), second_data.identity());
    assert!(
        runtime
            .host_state
            .borrow()
            .fullscreen_allowed_for_document(second_data.identity())
    );
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .existing_realm_for_document(second_data.identity())
            .is_some()
    );

    runtime
        .eval_in_document_realm(second_data.identity(), "history.back()")
        .unwrap();
    let restored_first_data = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    assert_ne!(second_data.identity(), restored_first_data.identity());
    assert!(
        runtime
            .host_state
            .borrow()
            .fullscreen_allowed_for_document(restored_first_data.identity())
    );
}

#[test]
fn location_replace_then_reload_keeps_the_original_declared_origin() {
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse(
            "<body><iframe sandbox='allow-scripts' allow='fullscreen' \
             srcdoc='<script>0</script>'></iframe></body>",
        )
        .document(),
        "https://owner.test/page",
    )
    .unwrap();
    let frame = runtime.document().query_selector("iframe").unwrap();
    let initial = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .host_state
            .borrow()
            .fullscreen_allowed_for_document(initial.identity())
    );

    runtime
        .eval_in_document_realm(
            initial.identity(),
            "location.replace('data:text/html,%3Cscript%3E0%3C/script%3E')",
        )
        .unwrap();
    let replaced = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    assert!(
        !runtime
            .host_state
            .borrow()
            .fullscreen_allowed_for_document(replaced.identity())
    );
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .existing_realm_for_document(replaced.identity())
            .is_some()
    );

    runtime
        .eval_in_document_realm(replaced.identity(), "location.reload()")
        .unwrap();
    let reloaded = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    assert_ne!(replaced.identity(), reloaded.identity());
    assert!(
        !runtime
            .host_state
            .borrow()
            .fullscreen_allowed_for_document(reloaded.identity())
    );
}

#[test]
fn location_history_restores_its_declared_origin_after_an_embedding_navigation() {
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse(
            "<body><iframe allow='fullscreen' src='data:text/html,%3Cp%3Einitial%3C/p%3E'></iframe></body>",
        )
        .document(),
        "https://owner.test/page",
    )
    .unwrap();
    let frame = runtime.document().query_selector("iframe").unwrap();
    runtime.run_until_idle().unwrap();
    let initial = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    assert!(
        runtime
            .host_state
            .borrow()
            .fullscreen_allowed_for_document(initial.identity())
    );

    runtime
        .eval(
            "globalThis.frame=document.querySelector('iframe'); \
               frame.contentWindow.location='about:blank'",
        )
        .unwrap();
    let location_entry = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    assert!(
        !runtime
            .host_state
            .borrow()
            .fullscreen_allowed_for_document(location_entry.identity())
    );

    runtime.eval("frame.srcdoc='<p>from parent</p>'").unwrap();
    let embedding_entry = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    assert!(
        runtime
            .host_state
            .borrow()
            .fullscreen_allowed_for_document(embedding_entry.identity())
    );

    runtime.eval("frame.contentWindow.history.back()").unwrap();
    let restored_location = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    assert_ne!(embedding_entry.identity(), restored_location.identity());
    assert!(
        !runtime
            .host_state
            .borrow()
            .fullscreen_allowed_for_document(restored_location.identity())
    );
}

#[test]
fn iframe_allow_delegates_without_parent_header_but_explicit_header_can_restrict_it() {
    let markup = "<body><iframe allow='geolocation *; cross-origin-isolated *' \
                  src='data:text/html,%3Cp%3Echild%3C/p%3E'></iframe></body>";
    for (header, expected) in [(None, true), (Some("geolocation=(self)"), false)] {
        let mut runtime = JsRuntime::with_document_and_url(
            TreeBuilder::parse(markup).document(),
            "https://owner.test/page",
        )
        .unwrap();
        if let Some(header) = header {
            runtime
                .install_response_security_headers(&[("Permissions-Policy".into(), header.into())]);
        }
        let frame = runtime.document().query_selector("iframe").unwrap();
        runtime.run_until_idle().unwrap();
        let child = runtime
            .host_state
            .borrow_mut()
            .iframe_content_document(&frame)
            .unwrap();
        assert_eq!(
            runtime
                .host_state
                .borrow()
                .feature_enabled_in_document(child.identity(), super::PolicyFeature::Geolocation),
            expected,
            "{header:?}"
        );
        assert!(runtime.host_state.borrow().feature_enabled_in_document(
            child.identity(),
            super::PolicyFeature::CrossOriginIsolated
        ));
    }
}

#[test]
fn iframe_allow_src_matches_its_data_document_opaque_origin() {
    let runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse(
            "<body><iframe allow='fullscreen' src='data:text/html,%3Cp%3Echild%3C/p%3E'></iframe></body>",
        )
        .document(),
        "https://owner.test/page",
    )
    .unwrap();
    let frame = runtime.document().query_selector("iframe").unwrap();
    let child = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap();
    assert!(
        runtime
            .host_state
            .borrow()
            .fullscreen_allowed_for_document(child.identity())
    );
}

#[test]
fn iframe_denied_geolocation_blocks_its_own_realm_request() {
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse(
            "<body><iframe allow=\"geolocation 'none'\" srcdoc='<p>child</p>'></iframe></body>",
        )
        .document(),
        "https://owner.test/page",
    )
    .unwrap();
    runtime.set_geolocation_position(GeolocationPositionData::new(35.0, 139.0, 4.0));
    runtime
        .eval(
            "globalThis.child=document.querySelector('iframe').contentWindow; \
             child.eval(\"globalThis.geoResult='pending';globalThis.permissionResult='pending'; \
               navigator.geolocation.getCurrentPosition( \
                 ()=>geoResult='success',error=>geoResult=error.code); \
               navigator.permissions.query({name:'geolocation'}).then( \
                 status=>permissionResult=status.state);\");",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        json_result(
            &mut runtime,
            "JSON.stringify([child.geoResult,child.permissionResult])"
        ),
        json!([1, "denied"])
    );
}

#[test]
fn popup_initial_and_local_documents_have_fresh_top_level_permissions_policy() {
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse("<body></body>").document(),
        "https://owner.test/page",
    )
    .unwrap();
    runtime.install_response_security_headers(&[(
        "Permissions-Policy".into(),
        "fullscreen=(), geolocation=()".into(),
    )]);
    let mut state = runtime.host_state.borrow_mut();
    let top_document_id = state.document.identity();
    let popup_id = state
        .open_auxiliary_context(top_document_id, "policy-popup")
        .unwrap()
        .0;
    let initial_document_id = state.auxiliary_contexts[&popup_id].document.identity();
    assert!(!state.fullscreen_allowed_for_document(top_document_id));
    assert!(!state.feature_enabled_in_document(top_document_id, super::PolicyFeature::Geolocation));
    assert!(state.fullscreen_allowed_for_document(initial_document_id));
    assert!(
        state.feature_enabled_in_document(initial_document_id, super::PolicyFeature::Geolocation)
    );

    let source = state.navigation_referrer_source_for_document(top_document_id);
    let client = state.navigation_security_snapshot(top_document_id).unwrap();
    for url in ["about:blank", "data:text/html,<p>new</p>"] {
        let old_document_id = state.auxiliary_contexts[&popup_id].document.identity();
        let prepared = state
            .prepare_auxiliary_navigation(popup_id, url, &source, &client)
            .unwrap();
        let navigated_document_id = state.commit_auxiliary_navigation(prepared).unwrap();
        assert_ne!(old_document_id, navigated_document_id);
        assert!(
            state.fullscreen_allowed_for_document(navigated_document_id),
            "{url}"
        );
        assert!(
            state.feature_enabled_in_document(
                navigated_document_id,
                super::PolicyFeature::Geolocation
            ),
            "{url}"
        );
    }
}
