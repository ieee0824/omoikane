use super::*;
use crate::html::TreeBuilder;

fn make_runtime(markup: &str) -> JsRuntime {
    JsRuntime::with_document_and_url(
        TreeBuilder::parse(markup).document(),
        "https://example.test/page",
    )
    .unwrap()
}

fn assert_js_true(runtime: &mut JsRuntime, source: &str) {
    let observed = runtime.eval(source).unwrap().as_boolean();
    if observed != Some(true) {
        eprintln!("Failed JavaScript assertion: {source}");
        eprintln!("Task errors: {:?}", runtime.take_task_errors());
    }
    assert_eq!(observed, Some(true));
}

fn queued_javascript_url(runtime: &mut JsRuntime, assignment: &str) -> String {
    runtime.eval(assignment).unwrap();
    runtime.run_until_idle().unwrap();
    let requests = runtime.take_navigation_requests();
    let [NavigationRequest::Navigate { url, .. }] = requests.as_slice() else {
        panic!("expected one javascript: navigation, got {requests:?}");
    };
    assert!(url.starts_with("javascript:"), "{url}");
    url.clone()
}

#[test]
fn javascript_location_navigation_uses_default_policy_and_preserves_document() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.originalRan = false;
        globalThis.convertedRan = false;
        globalThis.policyArguments = null;
        trustedTypes.createPolicy('default', {createScript: (source, type, sink) => {
          policyArguments = [source, type, sink];
          return source.replace('originalRan', 'convertedRan');
        }});
        true
        "#,
    );
    let url = queued_javascript_url(
        &mut runtime,
        "location.href = 'javascript:globalThis.originalRan%20%3D%20true;';",
    );
    assert_eq!(runtime.evaluate_javascript_navigation(&url).unwrap(), None);
    assert_js_true(
        &mut runtime,
        "convertedRan && !originalRan && policyArguments[0] === 'globalThis.originalRan%20%3D%20true;' && policyArguments[1] === 'TrustedScript' && policyArguments[2] === 'Location href' && location.href === 'https://example.test/page'",
    );
}

#[test]
fn javascript_location_assignment_coerces_url_once() {
    let mut runtime = make_runtime("<body>");
    let url = queued_javascript_url(
        &mut runtime,
        r#"
        globalThis.coercions = 0;
        location.href = {toString() {
          coercions++;
          return 'javascript:globalThis.executed=true;';
        }};
        "#,
    );
    assert_js_true(&mut runtime, "coercions === 1");
    assert_eq!(runtime.evaluate_javascript_navigation(&url).unwrap(), None);
    assert_js_true(&mut runtime, "executed === true && coercions === 1");
}

#[test]
fn location_navigation_rejects_unsupported_schemes_without_queuing() {
    let mut runtime = make_runtime("<body>");
    assert_js_true(
        &mut runtime,
        r#"
        [
          'data:text/html,hello',
          'data://example.test/escape',
          'vbscript:msgbox(1)',
          'vbscript://example.test/escape',
          'ftp://example.test/escape'
        ].every(url => {
          try { location.href = url; return false; }
          catch (error) { return error.name === 'SecurityError'; }
        })
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_navigation_requests().is_empty());
    assert_js_true(
        &mut runtime,
        "location.href === 'https://example.test/page'",
    );
}

#[test]
fn javascript_location_navigation_reports_enforce_and_report_only_failures() {
    for report_only in [false, true] {
        let mut runtime = make_runtime("<body>");
        runtime.install_response_security_headers(&[(
            if report_only {
                "Content-Security-Policy-Report-Only"
            } else {
                "Content-Security-Policy"
            }
            .into(),
            "require-trusted-types-for 'script'".into(),
        )]);
        runtime
            .eval(
                r#"
                globalThis.scriptRan = false;
                globalThis.violations = [];
                document.addEventListener('securitypolicyviolation', e => violations.push(e));
                trustedTypes.createPolicy('default', {createScript: () => { throw Error('no'); }});
                "#,
            )
            .unwrap();
        let url = queued_javascript_url(
            &mut runtime,
            "location.assign('javascript:globalThis.scriptRan=true;');",
        );
        assert_eq!(runtime.evaluate_javascript_navigation(&url).unwrap(), None);
        runtime.run_until_idle().unwrap();
        assert_js_true(
            &mut runtime,
            if report_only {
                "scriptRan && violations.length === 1 && violations[0].disposition === 'report' && violations[0].sample === 'Location href|globalThis.scriptRan=true;'"
            } else {
                "!scriptRan && violations.length === 1 && violations[0].disposition === 'enforce' && violations[0].sample === 'Location href|globalThis.scriptRan=true;'"
            },
        );
    }
}

#[test]
fn javascript_location_navigation_rejects_missing_or_invalid_default_policy() {
    for invalid_policy in [false, true] {
        let mut runtime = make_runtime("<body>");
        runtime.install_response_security_headers(&[(
            "Content-Security-Policy".into(),
            "require-trusted-types-for 'script'".into(),
        )]);
        runtime
            .eval(
                r#"
                globalThis.executed = false;
                globalThis.violations = [];
                document.addEventListener('securitypolicyviolation', e => violations.push(e));
                "#,
            )
            .unwrap();
        if invalid_policy {
            runtime
                .eval(
                    "trustedTypes.createPolicy('default', {createScript: () => '//make:invalid/'});",
                )
                .unwrap();
        }
        let url = queued_javascript_url(
            &mut runtime,
            "location.href = 'javascript:globalThis.executed=true;';",
        );
        assert_eq!(runtime.evaluate_javascript_navigation(&url).unwrap(), None);
        runtime.run_until_idle().unwrap();
        assert_js_true(
            &mut runtime,
            "!executed && violations.length === 1 && violations[0].sample === 'Location href|globalThis.executed=true;'",
        );
    }
}

#[test]
fn javascript_location_navigation_obeys_inline_csp_and_string_completion() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "script-src 'none' 'report-sample'".into(),
    )]);
    runtime
        .eval("globalThis.scriptRan = false; globalThis.violations = []; document.addEventListener('securitypolicyviolation', event => violations.push(event));")
        .unwrap();
    let url = queued_javascript_url(
        &mut runtime,
        "location = 'javascript:globalThis.scriptRan%20%3D%20true;';",
    );
    assert_eq!(runtime.evaluate_javascript_navigation(&url).unwrap(), None);
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "!scriptRan && violations.length === 1 && violations[0].effectiveDirective === 'script-src-elem' && violations[0].sample === 'globalThis.scriptRan = true;'",
    );

    let mut runtime = make_runtime("<body>");
    let url = queued_javascript_url(
        &mut runtime,
        "location.replace('javascript:%27%3Cmain%3Eok%3C%2Fmain%3E%27');",
    );
    assert_eq!(
        runtime.evaluate_javascript_navigation(&url).unwrap(),
        Some("<main>ok</main>".into()),
    );
    assert_js_true(
        &mut runtime,
        "location.href === 'https://example.test/page'",
    );
}

#[test]
fn javascript_child_self_navigation_uses_child_policy_and_realm() {
    let mut runtime = make_runtime("<body><iframe id=frame></iframe>");
    runtime
        .eval(
            r#"
            const frame = document.getElementById('frame');
            frame.srcdoc = `<meta http-equiv="Content-Security-Policy" content="require-trusted-types-for 'script'"><script>
              globalThis.executed = false;
              globalThis.violations = [];
              document.addEventListener('securitypolicyviolation', e => violations.push(e));
              location.href = 'javascript:globalThis.executed=true;';
            <\/script>`;
            globalThis.child = frame.contentWindow;
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "child.executed === false && child.violations.length === 1 && child.violations[0].effectiveDirective === 'require-trusted-types-for'",
    );
}

#[test]
fn javascript_popup_navigation_uses_opener_policy_and_target_realm() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    runtime
        .eval(
            r#"
            globalThis.defaultCalls = [];
            trustedTypes.createPolicy('default', {createScript: (source, type, sink) => {
              defaultCalls.push([source, type, sink]);
              return source.replace('untrustedMarker', 'approvedMarker');
            }});
            globalThis.popup = window.open('javascript:globalThis.untrustedMarker=17;', 'trustedPopup');
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "defaultCalls.length === 1 && defaultCalls[0][1] === 'TrustedScript' && defaultCalls[0][2] === 'Location href' && popup.approvedMarker === 17 && popup.untrustedMarker === undefined",
    );
}

#[test]
fn javascript_popup_string_completion_replaces_document_at_existing_url() {
    let mut runtime = make_runtime("<body>");
    runtime.eval(r#"
        globalThis.popup = window.open(
          "javascript:%27%3Cmain%20id%3Dnew%3Enew%3C%2Fmain%3E%3Cscript%3Eopener.popupScriptRan%3Dtrue%3C%2Fscript%3E%27",
          "completionPopup");
        globalThis.oldPopupDocument = popup.document;
        globalThis.oldPopupHistoryLength = popup.history.length;
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "popup.document !== oldPopupDocument && popup.document.URL === 'about:blank' && popup.location.href === 'about:blank' && popup.history.length === oldPopupHistoryLength && popup.document.getElementById('new').textContent === 'new' && globalThis.popupScriptRan === true",
    );
}

#[test]
fn javascript_popup_string_completion_inherits_target_csp() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    runtime
        .eval(
            r#"
        trustedTypes.createPolicy('default', {createScript: source => source});
        globalThis.popup = window.open(
          "javascript:%27%3Cmain%20id%3Dnew%3Enew%3C%2Fmain%3E%27", "cspPopup");
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.popupViolations = [];
        popup.document.addEventListener('securitypolicyviolation', event => popupViolations.push(event));
        popup.document.getElementById('new') !== null &&
          (() => { try { popup.document.body.innerHTML = '<b>unsafe</b>'; return false; }
            catch (error) { return error.name === 'TypeError'; } })()
    "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "popupViolations.length === 1 && popupViolations[0].effectiveDirective === 'require-trusted-types-for' && popupViolations[0].disposition === 'enforce'",
    );
}

#[test]
fn javascript_iframe_src_uses_owner_policy_and_keeps_existing_document() {
    let mut runtime = make_runtime("<body><iframe id=frame></iframe>");
    runtime
        .eval(
            r#"
            globalThis.frame = document.getElementById('frame');
            frame.srcdoc = '<meta http-equiv="Content-Security-Policy" content="require-trusted-types-for \'script\'"><body></body>';
            globalThis.child = frame.contentWindow;
            child.executed = false;
            frame.removeAttribute('srcdoc');
            frame.src = 'javascript:globalThis.executed=true;';
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "child.executed === true && frame.contentWindow === child && frame.contentDocument.URL === 'about:srcdoc'",
    );
}

#[test]
fn javascript_iframe_string_completion_replaces_document_and_history_entry() {
    let mut runtime = make_runtime("<body><iframe id=frame></iframe>");
    runtime.eval(r#"
        globalThis.frame = document.getElementById('frame');
        frame.srcdoc = '<p id=old>old</p>';
        globalThis.oldFrameDocument = frame.contentDocument;
        globalThis.frameProxy = frame.contentWindow;
        globalThis.oldFrameHistoryLength = frameProxy.history.length;
        frame.removeAttribute('srcdoc');
        frame.src = "javascript:%27%3Cmain%20id%3Dnew%3Enew%3C%2Fmain%3E%3Cscript%3Eparent.iframeScriptRan%3Dtrue%3C%2Fscript%3E%27";
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "frame.contentDocument !== oldFrameDocument && frame.contentWindow === frameProxy && frame.contentDocument.URL === 'about:srcdoc' && frameProxy.location.href === 'about:srcdoc' && frameProxy.history.length === oldFrameHistoryLength && frame.contentDocument.getElementById('new').textContent === 'new' && globalThis.iframeScriptRan === true",
    );
}

#[test]
fn javascript_iframe_string_completion_inherits_target_csp() {
    let mut runtime = make_runtime("<body><iframe id=frame></iframe>");
    runtime.eval(r#"
        globalThis.frame = document.getElementById('frame');
        frame.srcdoc = `<meta http-equiv="Content-Security-Policy" content="require-trusted-types-for 'script'"><body></body>`;
        void frame.contentDocument;
        frame.removeAttribute('srcdoc');
        frame.src = "javascript:%27%3Cmain%20id%3Dnew%3Enew%3C%2Fmain%3E%27";
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.iframeViolations = [];
        frame.contentDocument.addEventListener('securitypolicyviolation', event => iframeViolations.push(event));
        frame.contentDocument.getElementById('new') !== null &&
          (() => { try { frame.contentDocument.body.innerHTML = '<b>unsafe</b>'; return false; }
            catch (error) { return error.name === 'TypeError'; } })()
    "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "iframeViolations.length === 1 && iframeViolations[0].effectiveDirective === 'require-trusted-types-for' && iframeViolations[0].disposition === 'enforce'",
    );
}

#[test]
fn javascript_child_self_string_completion_replaces_own_document() {
    let mut runtime = make_runtime("<body><iframe id=frame></iframe>");
    runtime.eval(r#"
        globalThis.frame = document.getElementById('frame');
        frame.srcdoc = `<script>location.href = "javascript:%27%3Cmain%20id%3Dnew%3Enew%3C%2Fmain%3E%27";<\/script>`;
        globalThis.oldChildDocument = frame.contentDocument;
        globalThis.childProxy = frame.contentWindow;
        globalThis.oldChildHistoryLength = childProxy.history.length;
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "frame.contentDocument !== oldChildDocument && frame.contentWindow === childProxy && frame.contentDocument.URL === 'about:srcdoc' && childProxy.location.href === 'about:srcdoc' && childProxy.history.length === oldChildHistoryLength && frame.contentDocument.getElementById('new').textContent === 'new'",
    );
}

#[test]
fn javascript_iframe_removal_during_default_policy_does_not_execute() {
    let mut runtime = make_runtime("<body><iframe id=frame></iframe>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    runtime
        .eval(
            r#"
            globalThis.frame = document.getElementById('frame');
            globalThis.executed = false;
            globalThis.policyCalls = 0;
            trustedTypes.createPolicy('default', {createScript: source => {
              policyCalls++;
              frame.remove();
              return source;
            }});
            frame.src = 'javascript:parent.executed=true;';
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "policyCalls === 1 && !frame.isConnected && !executed",
    );
}

#[test]
fn javascript_child_removal_during_own_default_policy_does_not_execute() {
    // Mirrors navigate-to-javascript-url-007-crash.html, which has no testharness.
    let mut runtime = make_runtime("<body><iframe id=frame></iframe>");
    runtime
        .eval(
            r#"
            globalThis.frame = document.getElementById('frame');
            globalThis.executed = false;
            globalThis.policyCalls = 0;
            frame.srcdoc = `<meta http-equiv="Content-Security-Policy" content="require-trusted-types-for 'script'"><script>
              window.addEventListener('load', () => {
                trustedTypes.createPolicy('default', {createScript: source => {
                  parent.policyCalls++;
                  window.frameElement.remove();
                  return source;
                }});
                location.href = 'javascript:parent.executed=true;';
              });
            <\/script>`;
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "policyCalls === 1 && !frame.isConnected && !executed",
    );
}

#[test]
fn javascript_named_iframe_link_uses_link_document_as_request_client() {
    let mut runtime =
        make_runtime("<body><iframe id=source></iframe><iframe id=target name=target></iframe>");
    runtime
        .eval(
            r#"
            const sourceFrame = document.getElementById('source');
            const targetFrame = document.getElementById('target');
            sourceFrame.srcdoc = `<meta http-equiv="Content-Security-Policy" content="require-trusted-types-for 'script'">
              <a id=link target=target href="javascript:globalThis.hit=true;">go</a>`;
            targetFrame.srcdoc = '<body></body>';
            globalThis.sourceWindow = sourceFrame.contentWindow;
            globalThis.targetWindow = targetFrame.contentWindow;
            targetWindow.hit = false;
            sourceWindow.violations = [];
            sourceWindow.document.addEventListener('securitypolicyviolation', e => sourceWindow.violations.push(e));
            sourceWindow.document.getElementById('link').click();
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "!targetWindow.hit && sourceWindow.violations.length === 1 && sourceWindow.violations[0].sample === 'Location href|globalThis.hit=true;'",
    );
}

#[test]
fn javascript_named_iframe_link_obeys_target_inline_csp() {
    let mut runtime =
        make_runtime("<body><iframe id=source></iframe><iframe id=target name=target></iframe>");
    runtime.eval(r#"
        const source = document.getElementById('source');
        const target = document.getElementById('target');
        source.srcdoc = '<a id=link target=target href="javascript:globalThis.hit=true;">go</a>';
        target.srcdoc = `<meta http-equiv="Content-Security-Policy" content="script-src 'none'"><body></body>`;
        globalThis.targetWindow = target.contentWindow;
        targetWindow.hit = false;
        targetWindow.violations = [];
        targetWindow.document.addEventListener('securitypolicyviolation', e => targetWindow.violations.push(e));
        source.contentDocument.getElementById('link').click();
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "targetWindow.hit === false && targetWindow.violations.length === 1 && targetWindow.violations[0].effectiveDirective === 'script-src-elem'",
    );
}

#[test]
fn javascript_named_iframe_link_string_completion_replaces_target_document() {
    let mut runtime =
        make_runtime("<body><iframe id=source></iframe><iframe id=target name=target></iframe>");
    runtime.eval(r#"
        const source = document.getElementById('source');
        globalThis.target = document.getElementById('target');
        source.srcdoc = '<a id=link target=target href="javascript:%27%3Cmain%20id%3Dnew%3Enew%3C%2Fmain%3E%27">go</a>';
        target.srcdoc = '<p id=old>old</p>';
        globalThis.oldTargetDocument = target.contentDocument;
        globalThis.targetProxy = target.contentWindow;
        globalThis.oldTargetHistoryLength = targetProxy.history.length;
        source.contentDocument.getElementById('link').click();
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "target.contentDocument !== oldTargetDocument && target.contentWindow === targetProxy && target.contentDocument.URL === 'about:srcdoc' && targetProxy.history.length === oldTargetHistoryLength && target.contentDocument.getElementById('new').textContent === 'new'",
    );
}

#[test]
fn javascript_form_target_string_completion_replaces_target_document() {
    let mut runtime = make_runtime(
        "<body><form id=form target=target action=\"javascript:%27%3Cmain%20id%3Dnew%3Enew%3C%2Fmain%3E%27\"><button id=submit type=submit>Go</button></form><iframe id=target name=target></iframe>",
    );
    runtime
        .eval(
            r#"
        globalThis.target = document.getElementById('target');
        target.srcdoc = '<p id=old>old</p>';
        globalThis.oldTargetDocument = target.contentDocument;
        globalThis.targetProxy = target.contentWindow;
        globalThis.oldTargetHistoryLength = targetProxy.history.length;
        document.getElementById('submit').click();
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "target.contentDocument !== oldTargetDocument && target.contentWindow === targetProxy && target.contentDocument.URL === 'about:srcdoc' && targetProxy.history.length === oldTargetHistoryLength && target.contentDocument.getElementById('new').textContent === 'new'",
    );
}

#[test]
fn javascript_formaction_override_queues_source_without_get_query() {
    let mut runtime =
        make_runtime("<body><form id=form><button id=submit type=submit>Go</button></form>");
    runtime
        .eval(
            r#"
            globalThis.formRan = false;
            const submit = document.getElementById('submit');
            submit.formAction = 'javascript:globalThis.formRan=true;';
            submit.click();
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    let requests = runtime.take_navigation_requests();
    let [NavigationRequest::FormSubmit { url, .. }] = requests.as_slice() else {
        panic!("expected one form navigation, got {requests:?}");
    };
    assert_eq!(url, "javascript:globalThis.formRan=true;");
    assert_eq!(runtime.evaluate_javascript_navigation(url).unwrap(), None);
    assert_js_true(&mut runtime, "formRan");
}

#[test]
fn main_document_meta_csp_enforces_trusted_types_before_page_scripts() {
    let mut runtime = make_runtime(
        r#"<head><meta http-equiv="Content-Security-Policy"
            content="require-trusted-types-for 'script'"></head>
            <body><div id="target"></div><script>
              globalThis.untrustedMarkupRejected = (() => {
                try { document.getElementById('target').innerHTML = '<b>unsafe</b>'; }
                catch (error) { return error instanceof TypeError; }
                return false;
              })();
            </script></body>"#,
    );
    let errors = runtime.execute_document_scripts(None);
    assert!(errors.is_empty(), "page script errors: {errors:?}");
    assert_js_true(&mut runtime, "untrustedMarkupRejected === true");
}

#[test]
fn parser_meta_csp_applies_only_after_its_position() {
    let mut runtime = make_runtime(
        r#"<head>
            <script>
              globalThis.beforeMeta = true;
              document.createElement('div').innerHTML = '<b>allowed</b>';
            </script>
            <meta http-equiv="Content-Security-Policy"
              content="require-trusted-types-for 'script'">
            <script>
              globalThis.betweenMetas = true;
              try { document.createElement('div').innerHTML = '<b>blocked</b>'; }
              catch (error) { globalThis.trustedTypesBlocked = error instanceof TypeError; }
            </script>
            <meta http-equiv="Content-Security-Policy" content="script-src 'nonce-ok'">
            <script nonce="ok">globalThis.afterMetas = true</script>
            <script>globalThis.unnoncedRan = true</script>
            </head><body></body>"#,
    );
    let errors = runtime.execute_document_scripts(None);
    assert!(errors.is_empty(), "page script errors: {errors:?}");
    assert_js_true(
        &mut runtime,
        "beforeMeta === true && betweenMetas === true && trustedTypesBlocked === true && afterMetas === true && globalThis.unnoncedRan === undefined",
    );
}

#[test]
fn window_brand_and_non_sink_reflection_match_trusted_types_wpt() {
    let mut runtime = make_runtime("<body><div id=target>old</div>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const target = document.getElementById('target');
        const anchor = document.createElement('a');
        const seen = [];
        trustedTypes.createPolicy('default', {
          createHTML: (value, type, sink) => {
            seen.push([value, type, sink]);
            return value;
          },
        });
        anchor.setAttribute('rel', 'next');
        target.outerHTML = null;
        globalThis.toString() === '[object Window]' &&
          anchor.rel === 'next' && target.parentNode === null &&
          document.body.textContent === '' &&
          seen.length === 1 && seen[0].join('|') === '|TrustedHTML|Element outerHTML'
        "#,
    );
}

#[test]
fn main_document_nonce_csp_runs_only_matching_script_elements() {
    let mut runtime = make_runtime(
        r#"<head><meta http-equiv="Content-Security-Policy"
            content="script-src 'nonce-abc' 'unsafe-inline'"></head><body>
            <script nonce="abc">globalThis.nonceAllowed = true</script>
            <script nonce="bad">globalThis.badNonceRan = true</script>
            <script>globalThis.unsafeInlineRan = true</script></body>"#,
    );
    let errors = runtime.execute_document_scripts(None);
    assert!(errors.is_empty(), "page script errors: {errors:?}");
    assert_js_true(
        &mut runtime,
        "globalThis.nonceAllowed === true && globalThis.badNonceRan === undefined && globalThis.unsafeInlineRan === undefined",
    );
}

#[test]
fn duplicate_policy_creation_reports_each_csp_violation() {
    let mut runtime = make_runtime(
        r#"<head><meta http-equiv="Content-Security-Policy"
            content="trusted-types Allowed; connect-src 'none'"></head><body>"#,
    );
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.policyViolations = [];
        document.addEventListener('securitypolicyviolation', event => {
          if (event.effectiveDirective === 'trusted-types') policyViolations.push(event);
        });
        trustedTypes.createPolicy('Allowed', {});
        let duplicateBlocked = false;
        let unlistedBlocked = false;
        try { trustedTypes.createPolicy('Allowed', {}); }
        catch (error) { duplicateBlocked = error instanceof TypeError; }
        try { trustedTypes.createPolicy('Other', {}); }
        catch (error) { unlistedBlocked = error instanceof TypeError; }
        duplicateBlocked && unlistedBlocked
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "policyViolations.length === 2 && policyViolations[0].sample === 'Allowed' && policyViolations[1].sample === 'Other'",
    );
}

#[test]
fn duplicate_policy_violation_precedes_async_connect_barrier() {
    let mut runtime = make_runtime(
        r#"<head><meta http-equiv="Content-Security-Policy"
            content="trusted-types SomeName AnotherName">
            <meta http-equiv="Content-Security-Policy"
            content="connect-src 'none'"></head><body>"#,
    );
    runtime
        .eval(
            r#"
            globalThis.violationCounts = [];
            function collectViolations(action) {
              return new Promise(async resolve => {
                const events = [];
                const handler = event => {
                  if (event.effectiveDirective === 'trusted-types') events.push(event);
                  if (event.effectiveDirective === 'connect-src') {
                    self.removeEventListener('securitypolicyviolation', handler);
                    resolve(events.length);
                  }
                };
                self.addEventListener('securitypolicyviolation', handler);
                try { await action(); } catch (_) {}
                try { new WebSocket('ws:/common/blank.html'); } catch (_) {}
              });
            }
            trustedTypes.createPolicy('SomeName', {});
            (async () => {
              violationCounts.push(await collectViolations(() => trustedTypes.createPolicy('Other', {})));
              trustedTypes.createPolicy('AnotherName', {});
              violationCounts.push(await collectViolations(() => trustedTypes.createPolicy('AnotherName', {})));
            })();
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_js_true(&mut runtime, "violationCounts.join(',') === '1,1'");
}

#[test]
fn inner_text_collapses_trailing_newline_but_preserves_preformatted_text() {
    let mut runtime = make_runtime("<body><div id=plain></div><pre id=pre></pre>");
    assert_js_true(
        &mut runtime,
        r#"
        const plain = document.getElementById('plain');
        const pre = document.getElementById('pre');
        plain.textContent = 'line\n';
        pre.textContent = 'line\n';
        plain.textContent === 'line\n' && plain.innerText === 'line' &&
          pre.textContent === 'line\n' && pre.innerText === 'line\n'
        "#,
    );
}

#[test]
fn trusted_values_have_unforgeable_brands_and_policy_callbacks() {
    let mut runtime = make_runtime("<body>");
    assert_js_true(
        &mut runtime,
        r#"
        const policy = trustedTypes.createPolicy('app', {
          createHTML: (s, suffix) => s + suffix,
          createScript: s => s,
          createScriptURL: s => s,
        });
        const html = policy.createHTML('<b>', '</b>');
        const script = policy.createScript('1 + 1');
        const url = policy.createScriptURL('/app.js');
        const forged = Object.create(TrustedHTML.prototype);
        forged.toString = () => '<script>forged</script>';
        const checks = [
          policy.name === 'app', String(html) === '<b></b>', html.toJSON() === '<b></b>',
          trustedTypes.isHTML(html), trustedTypes.isScript(script), trustedTypes.isScriptURL(url),
          !trustedTypes.isHTML(forged), !trustedTypes.isHTML({toString: () => '<b>fake</b>'}),
          !trustedTypes.isHTML(Object.create(html)),
          trustedTypes.isHTML(trustedTypes.emptyHTML),
          trustedTypes.isScript(trustedTypes.emptyScript),
          (() => { try { new TrustedHTML(); return false; } catch(e) { return e instanceof TypeError; } })(),
          (() => { try { new TrustedTypePolicyFactory(); return false; } catch(e) { return e instanceof TypeError; } })(),
          (() => { try { policy.createHTML.call(forged, 'x'); return false; } catch(e) { return e instanceof TypeError; } })(),
          (() => { try { trustedTypes.isHTML.call({}, html); return false; } catch(e) { return e instanceof TypeError; } })(),
          (() => { try { trustedTypes.createPolicy(Symbol(), {}); return false; } catch(e) { return e instanceof TypeError; } })(),
          (() => { try { policy.createHTML(Symbol()); return false; } catch(e) { return e instanceof TypeError; } })(),
          (() => { try { policy.createHTML(); return false; } catch(e) { return e instanceof TypeError; } })(),
          (() => { try { policy.createScript(); return false; } catch(e) { return e instanceof TypeError; } })(),
          (() => { try { policy.createScriptURL(); return false; } catch(e) { return e instanceof TypeError; } })(),
          trustedTypes.createPolicy('app', {}).name === 'app',
        ];
        checks.every(Boolean)
        "#,
    );
}

#[test]
fn borrowed_policy_methods_use_the_receiver_realm_and_reject_forged_factories() {
    let mut runtime = make_runtime("<body><iframe id=frame></iframe>");
    assert_js_true(
        &mut runtime,
        r#"
        const frame = document.getElementById('frame');
        frame.srcdoc = `<meta http-equiv="Content-Security-Policy" content="trusted-types childOnly default; require-trusted-types-for 'script'"><body></body>`;
        const child = frame.contentWindow;
        const borrowedCreate = trustedTypes.createPolicy;
        const parentPolicy = borrowedCreate.call(trustedTypes, 'parentOnly', {createHTML: s => s});
        let childRejected = false;
        try { borrowedCreate.call(child.trustedTypes, 'parentOnly', {createHTML: s => s}); }
        catch (e) { childRejected = e instanceof TypeError; }
        const policy = borrowedCreate.call(child.trustedTypes, 'childOnly', {createHTML: s => s});
        const borrowedCreateHTML = TrustedTypePolicy.prototype.createHTML;
        const html = borrowedCreateHTML.call(policy, '<b>safe</b>');
        const defaultPolicy = borrowedCreate.call(child.trustedTypes, 'default', {createHTML: s => s});
        const defaultGetter = Object.getOwnPropertyDescriptor(TrustedTypePolicyFactory.prototype, 'defaultPolicy').get;
        let forgedRejected = false;
        try { borrowedCreate.call(Object.create(child.trustedTypes), 'childOnly', {}); }
        catch (e) { forgedRejected = e instanceof TypeError; }
        parentPolicy.name === 'parentOnly' && childRejected && forgedRejected &&
          policy instanceof child.TrustedTypePolicy &&
          html instanceof child.TrustedHTML && child.trustedTypes.isHTML(html) &&
          trustedTypes.isHTML.call(child.trustedTypes, html) &&
          defaultPolicy === defaultGetter.call(child.trustedTypes)
        "#,
    );
}

#[test]
fn enforced_inner_html_rejects_before_dom_changes_and_reports() {
    let mut runtime = make_runtime("<body><div id=target><i id=original>old</i></div>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        const target = document.getElementById('target');
        const original = target.firstChild;
        let rejected = false;
        try { target.innerHTML = '<b id=blocked>bad</b>'; }
        catch (e) { rejected = e instanceof TypeError; }
        const preserved = target.firstChild === original && original.id === 'original' &&
          document.getElementById('blocked') === null;
        const policy = trustedTypes.createPolicy('app', {createHTML: s => s});
        target.innerHTML = policy.createHTML('<b id=allowed>safe</b>');
        rejected && preserved &&
          target.firstChild.id === 'allowed'
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 1 && violations[0].effectiveDirective === 'require-trusted-types-for' && violations[0].blockedURI === 'trusted-types-sink' && violations[0].sample === 'Element innerHTML|<b id=blocked>bad</b>' && violations[0].disposition === 'enforce'",
    );
}

#[test]
fn report_only_allows_strings_but_emits_a_sink_violation() {
    let mut runtime = make_runtime("<body><div id=target></div>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy-Report-Only".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        document.getElementById('target').innerHTML = '<b id=inserted>allowed</b>';
        document.getElementById('inserted').textContent === 'allowed'
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 1 && violations[0].disposition === 'report' && violations[0].blockedURI === 'trusted-types-sink'",
    );
}

#[test]
fn report_only_trusted_types_directive_reports_disallowed_policy_names() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy-Report-Only".into(),
        "trusted-types approved".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        const policy = trustedTypes.createPolicy('other', {createHTML: s => s});
        policy.createHTML('<b>allowed by report-only</b>').toString() ===
          '<b>allowed by report-only</b>'
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 1 && violations[0].disposition === 'report' && violations[0].blockedURI === 'trusted-types-policy' && violations[0].sample === 'other'",
    );
}

#[test]
fn default_policy_converts_plain_html_and_rejected_names_report() {
    let mut runtime = make_runtime("<body><div id=target></div>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "trusted-types default app; require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        let args;
        const fallback = trustedTypes.createPolicy('default', {
          createHTML: (input, type, sink) => {
            args = [input, type, sink];
            return input.replace('unsafe', 'safe');
          }
        });
        const target = document.getElementById('target');
        target.innerHTML = '<b>unsafe</b>';
        let duplicate = false, denied = false;
        try { trustedTypes.createPolicy('default', {}); }
        catch (e) { duplicate = e instanceof TypeError; }
        try { trustedTypes.createPolicy('other', {}); }
        catch (e) { denied = e instanceof TypeError; }
        fallback === trustedTypes.defaultPolicy && target.innerHTML === '<b>safe</b>' &&
          args.join('|') === '<b>unsafe</b>|TrustedHTML|Element innerHTML' &&
          duplicate && denied
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 2 && violations.every(e => e.effectiveDirective === 'trusted-types' && e.blockedURI === 'trusted-types-policy') && violations[1].sample === 'other'",
    );
}

#[test]
fn rejected_default_policy_preserves_dom_before_throwing() {
    let mut runtime = make_runtime("<body><div id=target><i id=original>old</i></div>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        trustedTypes.createPolicy('default', {createHTML: () => null});
        const target = document.getElementById('target');
        const old = target.firstChild;
        let rejected = false;
        try { target.innerHTML = '<b id=blocked>blocked</b>'; }
        catch (e) { rejected = e instanceof TypeError; }
        rejected && target.firstChild === old && document.getElementById('blocked') === null
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 1 && violations[0].blockedURI === 'trusted-types-sink'",
    );
}

#[test]
fn sink_uses_owning_document_policy_and_accepts_cross_realm_trusted_html() {
    let mut runtime = make_runtime("<body><iframe id=frame></iframe>");
    assert_js_true(
        &mut runtime,
        r#"
        const frame = document.getElementById('frame');
        frame.srcdoc = `<head><meta http-equiv="Content-Security-Policy" content="require-trusted-types-for 'script'"></head><body><div id="target"><i id="original">old</i></div>`;
        const child = frame.contentWindow;
        const target = frame.contentDocument.getElementById('target');
        const original = target.firstChild;
        const parentSetter = Object.getOwnPropertyDescriptor(Element.prototype, 'innerHTML').set;
        let rejected = false;
        try { parentSetter.call(target, '<b id="blocked">bad</b>'); }
        catch (e) { rejected = e instanceof TypeError; }
        const preserved = target.firstChild === original && original.id === 'original';
        const trusted = trustedTypes.createPolicy('parent', {createHTML: s => s}).createHTML('<b id="safe">safe</b>');
        const crossRealmBrand = child.trustedTypes.isHTML(trusted);
        parentSetter.call(target, trusted);
        rejected && preserved && crossRealmBrand &&
          target.firstChild.id === 'safe' && !frame.contentDocument.getElementById('blocked')
        "#,
    );
}

#[test]
fn retired_document_html_sink_keeps_its_original_trusted_types_policy() {
    let mut runtime = make_runtime("<body><iframe id=frame></iframe>");
    assert_js_true(
        &mut runtime,
        r#"
        const frame = document.getElementById('frame');
        frame.srcdoc = `<meta http-equiv="Content-Security-Policy" content="require-trusted-types-for 'script'"><body><div id=old></div></body>`;
        const oldDocument = frame.contentDocument;
        const oldTarget = oldDocument.getElementById('old');
        frame.srcdoc = '<body><div id=fresh></div></body>';
        const freshTarget = frame.contentDocument.getElementById('fresh');
        let rejected = false;
        try { oldTarget.innerHTML = '<b>blocked</b>'; }
        catch (error) { rejected = error.name === 'TypeError'; }
        freshTarget.innerHTML = '<b>allowed</b>';
        const inert = document.implementation.createHTMLDocument('inert');
        inert.body.innerHTML = '<b>inert allowed</b>';
        rejected && oldTarget.innerHTML === '' &&
          freshTarget.firstChild.textContent === 'allowed' &&
          inert.body.firstChild.textContent === 'inert allowed'
        "#,
    );
}

#[test]
fn default_policy_cannot_move_html_sink_between_documents_during_validation() {
    let mut runtime =
        make_runtime("<body><div id=target><i id=original>old</i></div><iframe id=frame></iframe>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const target = document.getElementById('target');
        const original = target.firstChild;
        const frame = document.getElementById('frame');
        trustedTypes.createPolicy('default', {createHTML: source => {
          frame.contentDocument.body.appendChild(target);
          return source;
        }});
        let rejected = false;
        try { target.innerHTML = '<b id=injected>bad</b>'; }
        catch (e) { rejected = e instanceof TypeError; }
        rejected && target.ownerDocument === frame.contentDocument &&
          target.firstChild === original && target.querySelector('#injected') === null
        "#,
    );
}

#[test]
fn default_policy_result_stringification_cannot_move_html_sink_between_documents() {
    let mut runtime =
        make_runtime("<body><div id=target><i id=original>old</i></div><iframe id=frame></iframe>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const target = document.getElementById('target');
        const original = target.firstChild;
        const frame = document.getElementById('frame');
        trustedTypes.createPolicy('default', {createHTML: source => ({toString() {
          frame.contentDocument.body.appendChild(target);
          return source;
        }})});
        let rejected = false;
        try { target.innerHTML = '<b id=injected>bad</b>'; }
        catch (e) { rejected = e instanceof TypeError; }
        rejected && target.ownerDocument === frame.contentDocument &&
          target.firstChild === original && target.querySelector('#injected') === null
        "#,
    );
}

#[test]
fn html_parser_option_getters_cannot_adopt_an_approved_sink_into_a_stricter_document() {
    let mut runtime = make_runtime(
        "<body><div id=run><i id=run-old>old</i></div><div id=sanitizer><i id=sanitizer-old>old</i></div><iframe id=frame></iframe>",
    );
    assert_js_true(
        &mut runtime,
        r#"
        const frame = document.getElementById('frame');
        frame.srcdoc = `<meta http-equiv="Content-Security-Policy" content="require-trusted-types-for 'script'"><body></body>`;
        const child = frame.contentDocument;
        const run = document.getElementById('run');
        const sanitizer = document.getElementById('sanitizer');
        let runRejected = false, sanitizerRejected = false;
        try {
          run.setHTMLUnsafe('<b id=run-bad>bad</b>', {
            get runScripts() { child.body.appendChild(run); return false; }
          });
        } catch (e) { runRejected = e instanceof TypeError; }
        try {
          sanitizer.setHTMLUnsafe('<b id=sanitizer-bad>bad</b>', {
            get sanitizer() { child.body.appendChild(sanitizer); return undefined; }
          });
        } catch (e) { sanitizerRejected = e instanceof TypeError; }
        runRejected && sanitizerRejected &&
          run.ownerDocument === child && sanitizer.ownerDocument === child &&
          run.firstChild.id === 'run-old' && sanitizer.firstChild.id === 'sanitizer-old' &&
          child.getElementById('run-bad') === null && child.getElementById('sanitizer-bad') === null
        "#,
    );
}

#[test]
fn traversal_callback_cannot_move_unsafe_html_sink_during_replacement() {
    let mut runtime =
        make_runtime("<body><div id=target><i id=old>old</i></div><iframe id=frame></iframe>");
    assert_js_true(
        &mut runtime,
        r#"
        const frame = document.getElementById('frame');
        frame.srcdoc = `<meta http-equiv="Content-Security-Policy" content="require-trusted-types-for 'script'"><body></body>`;
        const child = frame.contentDocument;
        const target = document.getElementById('target');
        const iterator = document.createNodeIterator(target);
        let called = false;
        iterator.__preRemove = () => {
          if (!called) { called = true; child.body.appendChild(target); }
        };
        let rejected = false;
        try { target.setHTMLUnsafe('<b id=bad>bad</b>'); }
        catch (e) { rejected = e instanceof TypeError; }
        iterator.detach();
        called && rejected && target.ownerDocument === child &&
          child.getElementById('bad') === null
        "#,
    );
}

#[test]
fn inner_html_document_parts_use_the_checked_source_despite_regexp_poisoning() {
    let mut runtime = make_runtime("<html><head></head><body></body></html>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const policy = trustedTypes.createPolicy('parts', {createHTML: source => source});
        const originalExec = RegExp.prototype.exec;
        const originalTest = RegExp.prototype.test;
        let called = false;
        RegExp.prototype.exec = function() {
          called = true;
          return ['forged', '<script id=untrusted>bad</script>'];
        };
        RegExp.prototype.test = function() { called = true; return true; };
        try {
          document.documentElement.innerHTML = policy.createHTML(
            '<head><title>safe</title></head><body><p id=safe>safe</p></body>');
        } finally {
          RegExp.prototype.exec = originalExec;
          RegExp.prototype.test = originalTest;
        }
        !called && document.getElementById('safe') !== null &&
          document.getElementById('untrusted') === null
        "#,
    );
}

#[test]
fn dom_parser_gates_html_in_the_calling_document_before_creating_a_document() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        const parser = new DOMParser();
        const implementationGetter = Object.getOwnPropertyDescriptor(Document.prototype, 'implementation').get;
        let documentCreations = 0;
        Object.defineProperty(document, 'implementation', { configurable: true, get() {
          documentCreations++;
          return implementationGetter.call(this);
        }});
        let rejected = false;
        try { parser.parseFromString('<p id="blocked">bad</p>', 'text/html'); }
        catch (e) { rejected = e instanceof TypeError; }
        const rejectedBeforeCreation = rejected && documentCreations === 0;
        const policy = trustedTypes.createPolicy('parser', {createHTML: s => s});
        const trustedDocument = parser.parseFromString(policy.createHTML('<b id="trusted">safe</b>'), 'text/html');
        const xmlDocument = parser.parseFromString('<root/>', 'text/xml');
        let callbackArguments;
        trustedTypes.createPolicy('default', {createHTML: (input, type, sink) => {
          callbackArguments = [input, type, sink];
          return input.replace('raw', 'safe');
        }});
        const convertedDocument = parser.parseFromString('<i>raw</i>', 'text/html');
        delete document.implementation;
        rejectedBeforeCreation && documentCreations === 2 &&
          trustedDocument.body.firstChild.id === 'trusted' &&
          xmlDocument.documentElement !== null &&
          convertedDocument.body.innerHTML === '<i>safe</i>' &&
          callbackArguments.join('|') === '<i>raw</i>|TrustedHTML|DOMParser parseFromString'
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 1 && violations[0].sample === 'DOMParser parseFromString|<p id=\"blocked\">bad</p>' && violations[0].blockedURI === 'trusted-types-sink'",
    );
}

#[test]
fn borrowed_dom_parser_method_uses_the_parsers_document_policy() {
    let mut runtime = make_runtime("<body><iframe id=frame></iframe>");
    assert_js_true(
        &mut runtime,
        r#"
        const frame = document.getElementById('frame');
        frame.srcdoc = `<head><meta http-equiv="Content-Security-Policy"
          content="require-trusted-types-for 'script'"></head><body></body>`;
        const childParser = new frame.contentWindow.DOMParser();
        const parentMethod = DOMParser.prototype.parseFromString;
        let rejected = false;
        try { parentMethod.call(childParser, '<p>raw</p>', 'text/html'); }
        catch (e) { rejected = e instanceof TypeError; }
        rejected
        "#,
    );
}

#[test]
fn dom_parser_report_only_allows_html_and_reports() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy-Report-Only".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        new DOMParser().parseFromString('<p id="allowed">plain</p>', 'text/html')
          .body.firstChild.id === 'allowed'
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 1 && violations[0].disposition === 'report' && violations[0].sample === 'DOMParser parseFromString|<p id=\"allowed\">plain</p>'",
    );
}

#[test]
fn script_and_frame_sinks_require_the_matching_native_brand() {
    let mut runtime = make_runtime("<body><script id=script></script><iframe id=frame></iframe>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const script = document.getElementById('script');
        const frame = document.getElementById('frame');
        const policy = trustedTypes.createPolicy('typed', {
          createHTML: s => s, createScript: s => s, createScriptURL: s => s,
        });
        const forgedURL = Object.create(TrustedScriptURL.prototype);
        forgedURL.toString = () => '/forged.js';
        const rejected = setter => {
          try { setter(); return false; } catch (e) { return e instanceof TypeError; }
        };
        const blocked = rejected(() => script.src = '/blocked.js') &&
          rejected(() => script.text = 'globalThis.blocked = true') &&
          rejected(() => script.textContent = 'globalThis.blocked = true') &&
          rejected(() => script.innerText = 'globalThis.blocked = true') &&
          rejected(() => frame.srcdoc = '<p>blocked</p>') &&
          rejected(() => script.setAttribute('src', '/blocked.js')) &&
          rejected(() => script.src = forgedURL) &&
          rejected(() => script.setAttribute('onclick', 'blocked()')) &&
          !script.hasAttribute('src') && script.textContent === '' &&
          !frame.hasAttribute('srcdoc');
        const trustedURL = policy.createScriptURL('/allowed.js');
        trustedURL.toString = () => '/forged.js';
        script.src = trustedURL;
        script.text = policy.createScript('globalThis.allowed = true');
        frame.srcdoc = policy.createHTML('<p>allowed</p>');
        blocked && script.getAttribute('src') === '/allowed.js' &&
          script.textContent === 'globalThis.allowed = true' &&
          frame.getAttribute('srcdoc') === '<p>allowed</p>'
        "#,
    );
}

#[test]
fn html_fragment_and_document_write_sinks_reject_before_mutation() {
    let mut runtime = make_runtime("<body><div id=target><i id=original>old</i></div>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const target = document.getElementById('target');
        const original = target.firstChild;
        const range = document.createRange();
        range.selectNodeContents(target);
        const rejected = operation => {
          try { operation(); return false; } catch (e) { return e instanceof TypeError; }
        };
        const blocked = rejected(() => target.insertAdjacentHTML('beforeend', '<b>bad</b>')) &&
          rejected(() => range.createContextualFragment('<b>bad</b>')) &&
          rejected(() => document.write('<b>bad</b>')) &&
          rejected(() => document.writeln('<b>bad</b>')) &&
          target.firstChild === original && target.childNodes.length === 1;
        const policy = trustedTypes.createPolicy('fragments', {createHTML: s => s});
        target.insertAdjacentHTML('beforeend', policy.createHTML('<b id=safe>safe</b>'));
        const fragment = range.createContextualFragment(policy.createHTML('<em>safe</em>'));
        blocked && target.lastChild.id === 'safe' && fragment.firstChild.localName === 'em'
        "#,
    );
}

#[test]
fn document_write_checks_concatenated_arguments_before_adding_newline() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const seen = [];
        trustedTypes.createPolicy('default', {createHTML: (value, kind, sink) => {
          seen.push([value, kind, sink]);
          return value.replace('defghi', 'zxcvbn');
        }});
        document.write('abcde', 'fghijkl');
        document.writeln('line', 'break');
        seen.length === 2 &&
          seen[0].join('|') === 'abcdefghijkl|TrustedHTML|Document write' &&
          seen[1].join('|') === 'linebreak|TrustedHTML|Document writeln'
        "#,
    );
}

#[test]
fn insert_adjacent_html_default_policy_receives_domstring_null() {
    let mut runtime = make_runtime("<body><div id=target></div>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.seen = [];
        trustedTypes.createPolicy('default', {createHTML: (value, kind, sink) => {
          seen.push([value, kind, sink]);
          return value;
        }});
        const target = document.getElementById('target');
        target.insertAdjacentHTML('beforeend', null);
        target.textContent === 'null' &&
          seen.length === 1 && seen[0].join('|') === 'null|TrustedHTML|Element insertAdjacentHTML'
        "#,
    );
}

#[test]
fn unsafe_html_parser_and_shadow_sinks_require_trusted_html() {
    let mut runtime = make_runtime("<body><div id=target><i id=old>old</i></div>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const target = document.getElementById('target');
        const old = target.firstChild;
        const shadow = target.attachShadow({mode: 'open'});
        let elementRejected = false, shadowRejected = false, parserRejected = false;
        try { target.setHTMLUnsafe('<b id=bad>bad</b>'); }
        catch (e) { elementRejected = e instanceof TypeError; }
        try { shadow.setHTMLUnsafe('<b id=bad>bad</b>'); }
        catch (e) { shadowRejected = e instanceof TypeError; }
        try { Document.parseHTMLUnsafe('<b id=bad>bad</b>'); }
        catch (e) { parserRejected = e instanceof TypeError; }
        const policy = trustedTypes.createPolicy('markup', {createHTML: s => s});
        target.setHTMLUnsafe(policy.createHTML('<b id=safe>safe</b>'));
        shadow.setHTMLUnsafe(policy.createHTML('<em id=shadow-safe>safe</em>'));
        const parsed = Document.parseHTMLUnsafe(policy.createHTML('<p id=parsed-safe>safe</p>'));
        elementRejected && shadowRejected && parserRejected && old.parentNode === null &&
          target.firstChild.id === 'safe' && shadow.firstChild.id === 'shadow-safe' &&
          parsed.getElementById('parsed-safe') !== null
        "#,
    );
}

#[test]
fn shadow_root_inner_html_uses_its_own_sink_name_and_null_conversion() {
    let mut runtime = make_runtime("<body><div id=host></div>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        const root = document.getElementById('host').attachShadow({mode: 'open'});
        let rejected = false;
        try { root.innerHTML = '<b>bad</b>'; }
        catch (e) { rejected = e instanceof TypeError; }
        const seen = [];
        trustedTypes.createPolicy('default', {createHTML: (value, kind, sink) => {
          seen.push([value, kind, sink]);
          return value;
        }});
        root.innerHTML = '<b>safe</b>';
        const created = root.firstChild.localName === 'b';
        root.innerHTML = null;
        rejected && created && root.innerHTML === '' &&
          seen.length === 2 &&
          seen[0].join('|') === '<b>safe</b>|TrustedHTML|ShadowRoot innerHTML' &&
          seen[1].join('|') === '|TrustedHTML|ShadowRoot innerHTML'
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 1 && violations[0].sample === 'ShadowRoot innerHTML|<b>bad</b>'",
    );
}

#[test]
fn default_policy_converts_each_trusted_type_with_the_sink_name() {
    let mut runtime = make_runtime("<body><script id=script></script><iframe id=frame></iframe>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const calls = [];
        trustedTypes.createPolicy('default', {
          createHTML: (s, type, sink) => { calls.push([s, type, sink]); return s.replace('bad', 'safe'); },
          createScript: (s, type, sink) => { calls.push([s, type, sink]); return s.replace('bad', 'safe'); },
          createScriptURL: (s, type, sink) => { calls.push([s, type, sink]); return s.replace('bad', 'safe'); },
        });
        const script = document.getElementById('script');
        const frame = document.getElementById('frame');
        script.src = '/bad.js';
        script.text = 'bad()';
        frame.srcdoc = '<b>bad</b>';
        script.getAttribute('src') === '/safe.js' && script.text === 'safe()' &&
          frame.srcdoc === '<b>safe</b>' &&
          calls.some(c => c.join('|') === '/bad.js|TrustedScriptURL|HTMLScriptElement src') &&
          calls.some(c => c.join('|') === 'bad()|TrustedScript|HTMLScriptElement text') &&
          calls.some(c => c.join('|') === '<b>bad</b>|TrustedHTML|HTMLIFrameElement srcdoc')
        "#,
    );
}

#[test]
fn report_only_script_url_allows_assignment_and_reports() {
    let mut runtime = make_runtime("<body><script id=script></script>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy-Report-Only".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        const script = document.getElementById('script');
        script.src = '/plain.js';
        script.getAttribute('src') === '/plain.js'
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 1 && violations[0].sample === 'HTMLScriptElement src|/plain.js' && violations[0].disposition === 'report'",
    );
}

#[test]
fn service_worker_registration_requires_trusted_script_url() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.registrationResults = [];
        navigator.serviceWorker.register('/blocked.js').then(
          () => registrationResults.push('unexpected'),
          error => registrationResults.push(error instanceof TypeError ? 'blocked' : error.name));
        const policy = trustedTypes.createPolicy('workers', {createScriptURL: s => s});
        navigator.serviceWorker.register(policy.createScriptURL('/allowed.js')).then(
          () => registrationResults.push('allowed'),
          error => registrationResults.push(error.name));
        true
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "registrationResults.includes('blocked') && registrationResults.includes('allowed') && registrationResults.length === 2",
    );
}

#[test]
fn borrowed_service_worker_register_uses_the_containers_document_policy() {
    let mut runtime = make_runtime("<body><iframe id=frame></iframe>");
    assert_js_true(
        &mut runtime,
        r#"
        const frame = document.getElementById('frame');
        frame.srcdoc = `<head><meta http-equiv="Content-Security-Policy"
          content="require-trusted-types-for 'script'"></head><body></body>`;
        const childContainer = frame.contentWindow.navigator.serviceWorker;
        const borrowedRegister = navigator.serviceWorker.register;
        globalThis.registrationResult = null;
        borrowedRegister.call(childContainer, '/untrusted.js').then(
          () => registrationResult = 'allowed',
          error => registrationResult = error instanceof TypeError ? 'blocked' : error.name);
        true
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(&mut runtime, "registrationResult === 'blocked'");
}

#[test]
fn property_and_attribute_type_introspection_matches_enforced_sinks() {
    let mut runtime = make_runtime("<body>");
    assert_js_true(
        &mut runtime,
        r#"
        const checks = [
          trustedTypes.getPropertyType('div', 'innerHTML') === 'TrustedHTML',
          trustedTypes.getPropertyType('div', 'outerHTML') === 'TrustedHTML',
          trustedTypes.getPropertyType('iframe', 'srcdoc') === 'TrustedHTML',
          trustedTypes.getPropertyType('script', 'src') === 'TrustedScriptURL',
          trustedTypes.getPropertyType('script', 'textContent') === 'TrustedScript',
          trustedTypes.getPropertyType('script', 'innerText') === 'TrustedScript',
          trustedTypes.getPropertyType('script', 'text') === 'TrustedScript',
          trustedTypes.getPropertyType('div', 'textContent') === null,
          trustedTypes.getAttributeType('script', 'src') === 'TrustedScriptURL',
          trustedTypes.getAttributeType('iframe', 'srcdoc') === 'TrustedHTML',
          trustedTypes.getAttributeType('div', 'onclick') === 'TrustedScript',
          trustedTypes.getAttributeType('script', 'href', 'http://www.w3.org/2000/svg',
            'http://www.w3.org/1999/xlink') === 'TrustedScriptURL',
          trustedTypes.getAttributeType('div', 'href') === null,
          trustedTypes.getAttributeType('div', 'onmadeupevent') === null,
        ];
        checks.every(Boolean)
        "#,
    );
}

#[test]
fn namespaced_attribute_setters_enforce_the_matching_type() {
    let mut runtime = make_runtime("<body><script id=script></script><button id=button></button>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const script = document.getElementById('script');
        const button = document.getElementById('button');
        const rejected = operation => {
          try { operation(); return false; } catch (e) { return e instanceof TypeError; }
        };
        const blocked = rejected(() => script.setAttributeNS(null, 'src', '/bad.js')) &&
          rejected(() => button.setAttributeNS(null, 'onclick', 'bad()')) &&
          !script.hasAttribute('src') && !button.hasAttribute('onclick');
        const policy = trustedTypes.createPolicy('attributes', {
          createScript: s => s, createScriptURL: s => s,
        });
        script.setAttributeNS(null, 'src', policy.createScriptURL('/good.js'));
        button.setAttributeNS(null, 'onclick', policy.createScript('good()'));
        script.setAttributeNS(null, 'SRC', '/case-sensitive.js');
        blocked && button.getAttribute('onclick') === 'good()' &&
          script.getAttribute('src') === '/good.js' &&
          script.getAttributeNS(null, 'SRC') === '/case-sensitive.js'
        "#,
    );
}

#[test]
fn attribute_node_mutations_enforce_trusted_types_before_changing_values() {
    let mut runtime = make_runtime(
        "<body><script id=script></script><iframe id=frame></iframe><button id=button></button>",
    );
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const script = document.getElementById('script');
        const frame = document.getElementById('frame');
        const button = document.getElementById('button');
        const policy = trustedTypes.createPolicy('attributes', {
          createHTML: s => s, createScript: s => s, createScriptURL: s => s,
        });
        script.setAttribute('src', policy.createScriptURL('/original.js'));
        frame.setAttribute('srcdoc', policy.createHTML('<p>original</p>'));
        button.setAttribute('onclick', policy.createScript('original()'));
        const scriptAttr = script.getAttributeNode('src');
        const frameAttr = frame.getAttributeNode('srcdoc');
        const buttonAttr = button.getAttributeNode('onclick');
        const typedURL = policy.createScriptURL('/typed-but-domstring.js');
        const rejected = operation => {
          try { operation(); return false; } catch (e) { return e instanceof TypeError; }
        };
        const blocked = rejected(() => scriptAttr.value = '/bad.js') &&
          rejected(() => scriptAttr.value = typedURL) &&
          rejected(() => frameAttr.nodeValue = '<p>bad</p>') &&
          rejected(() => buttonAttr.textContent = 'bad()') &&
          scriptAttr.value === '/original.js' && frameAttr.value === '<p>original</p>' &&
          buttonAttr.value === 'original()';
        const detached = document.createAttribute('src');
        detached.value = '/detached.js';
        const blockedTransfer = rejected(() => script.setAttributeNode(detached)) &&
          detached.ownerElement === null && script.getAttribute('src') === '/original.js';
        const seen = [];
        trustedTypes.createPolicy('default', {
          createScriptURL: (value, kind, sink) => {
            seen.push([value, kind, sink]);
            return '/approved.js';
          },
        });
        const old = script.setAttributeNode(detached);
        const movedWithConversion = old === scriptAttr &&
          detached.ownerElement === script && detached.value === '/approved.js' &&
          script.getAttribute('src') === '/approved.js';
        const deceptive = document.createAttribute('src');
        deceptive.value = '/still-untrusted.js';
        Object.defineProperty(deceptive, 'value', {configurable: true, get: () => typedURL});
        const previous = script.setAttributeNode(deceptive);
        const overriddenGetterIgnored = previous === detached &&
          script.getAttribute('src') === '/approved.js' &&
          seen[1].join('|') === '/still-untrusted.js|TrustedScriptURL|HTMLScriptElement src';
        delete deceptive.value;
        const ordinary = document.createAttribute('data-label');
        ordinary.value = 'allowed';
        script.attributes.setNamedItem(ordinary);
        blocked && blockedTransfer && movedWithConversion && overriddenGetterIgnored &&
          seen.length === 2 &&
          seen[0].join('|') === '/detached.js|TrustedScriptURL|HTMLScriptElement src' &&
          deceptive.ownerElement === script && deceptive.value === '/approved.js' &&
          script.getAttribute('data-label') === 'allowed'
        "#,
    );
}

#[test]
fn attribute_nodes_use_default_policy_for_values_and_node_transfer() {
    let mut runtime =
        make_runtime("<body><script id=script></script><iframe id=frame></iframe><embed id=embed>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const script = document.getElementById('script');
        const frame = document.getElementById('frame');
        const embed = document.getElementById('embed');
        const typed = trustedTypes.createPolicy('typed', {
          createScriptURL: s => s, createHTML: s => s,
        });
        script.setAttribute('src', typed.createScriptURL('/original.js'));
        frame.setAttribute('srcdoc', typed.createHTML('<p>original</p>'));
        const seen = [];
        trustedTypes.createPolicy('default', {
          createScriptURL: (value, kind, sink) => {
            seen.push([value, kind, sink]);
            return '/safe.js';
          },
          createHTML: (value, kind, sink) => {
            seen.push([value, kind, sink]);
            return '<p>safe</p>';
          },
        });
        script.getAttributeNode('src').value = '/raw.js';
        frame.getAttributeNode('srcdoc').nodeValue = '<p>raw</p>';
        embed.setAttribute('src', '/detached.js');
        const moved = embed.getAttributeNode('src');
        embed.removeAttributeNode(moved);
        script.attributes.setNamedItem(moved);
        script.getAttribute('src') === '/safe.js' && moved.value === '/safe.js' &&
          frame.getAttribute('srcdoc') === '<p>safe</p>' &&
          seen.length === 3 &&
          seen[0].join('|') === '/raw.js|TrustedScriptURL|HTMLScriptElement src' &&
          seen[1].join('|') === '<p>raw</p>|TrustedHTML|HTMLIFrameElement srcdoc' &&
          seen[2].join('|') === '/detached.js|TrustedScriptURL|HTMLScriptElement src'
        "#,
    );
}

#[test]
fn attribute_node_default_policy_cannot_retarget_an_attr_during_validation() {
    let mut runtime = make_runtime("<body><script id=script></script><embed id=embed>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const script = document.getElementById('script');
        const embed = document.getElementById('embed');
        const typed = trustedTypes.createPolicy('typed', {createScriptURL: s => s});
        script.setAttribute('src', typed.createScriptURL('/original.js'));
        const attr = script.getAttributeNode('src');
        trustedTypes.createPolicy('default', {
          createScriptURL: () => {
            script.removeAttributeNode(attr);
            embed.setAttributeNode(attr);
            return '/safe.js';
          },
        });
        let rejected = false;
        try { attr.value = '/untrusted.js'; }
        catch (e) { rejected = e instanceof TypeError; }
        rejected && script.getAttribute('src') === null &&
          embed.getAttributeNode('src') === attr && attr.value === '/original.js'
        "#,
    );
}

#[test]
fn outer_html_replaces_only_after_trusted_html_validation() {
    let mut runtime = make_runtime("<body><div id=target>old</div>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const target = document.getElementById('target');
        let blocked = false;
        try { target.outerHTML = '<p id=bad>bad</p>'; }
        catch (e) { blocked = e instanceof TypeError; }
        const preserved = document.getElementById('target') === target &&
          document.getElementById('bad') === null;
        const policy = trustedTypes.createPolicy('outer', {createHTML: s => s});
        target.outerHTML = policy.createHTML('<p id=safe>safe</p>');
        blocked && preserved && !document.getElementById('target') &&
          document.getElementById('safe').textContent === 'safe'
        "#,
    );
}

#[test]
fn internal_html_delegation_does_not_expose_a_minted_trusted_value() {
    let mut runtime = make_runtime(
        "<body><div id=outer>old</div><template id=template></template><div id=host></div>",
    );
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const policy = trustedTypes.createPolicy('delegate', {createHTML: source => source});
        const outer = document.getElementById('outer');
        const template = document.getElementById('template');
        const shadow = document.getElementById('host').attachShadow({mode: 'open'});
        const parser = new DOMParser();
        let parsed;
        let exposed = null;
        outer.insertAdjacentHTML = (_, value) => { exposed = value; };
        const descriptor = Object.getOwnPropertyDescriptor(Element.prototype, 'innerHTML');
        Object.defineProperty(Element.prototype, 'innerHTML', {
          configurable: true, get: descriptor.get,
          set(value) { exposed = value; },
        });
        try {
          outer.outerHTML = policy.createHTML('<p id=replaced>safe</p>');
          template.innerHTML = policy.createHTML('<b id=template-safe>safe</b>');
          shadow.innerHTML = policy.createHTML('<i id=shadow-safe>safe</i>');
          parsed = parser.parseFromString(policy.createHTML('<em id=parsed-safe>safe</em>'), 'text/html');
        } finally {
          Object.defineProperty(Element.prototype, 'innerHTML', descriptor);
        }
        exposed === null && document.getElementById('replaced') !== null &&
          template.content.firstChild.id === 'template-safe' &&
          shadow.firstChild.id === 'shadow-safe' &&
          parsed.body.firstChild.id === 'parsed-safe'
        "#,
    );
}

#[test]
fn delegated_attribute_sinks_do_not_expose_a_minted_trusted_value() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const policy = trustedTypes.createPolicy('attributes', {
          createHTML: source => source, createScriptURL: source => source,
        });
        const script = document.createElement('script');
        const frame = document.createElement('iframe');
        const originalSetAttribute = Element.prototype.setAttribute;
        let exposed = null;
        Element.prototype.setAttribute = (_, value) => { exposed = value; };
        try {
          script.setAttribute('src', policy.createScriptURL('/safe.js'));
          frame.setAttribute('srcdoc', policy.createHTML('<p>safe</p>'));
        } finally {
          Element.prototype.setAttribute = originalSetAttribute;
        }
        exposed === null && script.getAttribute('src') === '/safe.js' &&
          frame.getAttribute('srcdoc') === '<p>safe</p>'
        "#,
    );
}

#[test]
fn script_resource_hook_cannot_adopt_a_checked_attribute_into_a_stricter_document() {
    let mut runtime = make_runtime("<body><iframe id=frame></iframe>");
    assert_js_true(
        &mut runtime,
        r#"
        const frame = document.getElementById('frame');
        frame.srcdoc = `<meta http-equiv="Content-Security-Policy" content="require-trusted-types-for 'script'"><body></body>`;
        const child = frame.contentDocument;
        const script = document.createElement('script');
        const originalDefine = Object.defineProperty;
        let moved = false;
        Object.defineProperty = function(object, property, descriptor) {
          if (!moved && object === script && property === '__resourceTimingStart') {
            moved = true;
            Object.defineProperty = originalDefine;
            child.body.appendChild(script);
          }
          return originalDefine(object, property, descriptor);
        };
        let rejected = false;
        try { script.setAttribute('src', '/untrusted.js'); }
        catch (e) { rejected = e instanceof TypeError; }
        finally { Object.defineProperty = originalDefine; }
        moved && rejected && script.ownerDocument === child &&
          script.getAttribute('src') === null
        "#,
    );
}

#[test]
fn string_timers_require_trusted_script_and_use_default_policy() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const rejected = callback => {
          try { callback(); return false; } catch (e) { return e instanceof TypeError; }
        };
        const blocked = rejected(() => setTimeout('globalThis.bad = true', 0)) &&
          rejected(() => setInterval('globalThis.bad = true', 0));
        const policy = trustedTypes.createPolicy('timer', {createScript: s => s});
        setTimeout(policy.createScript('globalThis.trustedRan = true'), 0);
        const calls = [];
        trustedTypes.createPolicy('default', {createScript: (source, type, sink) => {
          calls.push([source, type, sink]);
          return 'globalThis.defaultRan = true';
        }});
        setTimeout('plain timer', 0);
        blocked && calls.length === 1 &&
          calls[0].join('|') === 'plain timer|TrustedScript|Window setTimeout'
        "#,
    );
    runtime.tick(0).unwrap();
    assert_js_true(
        &mut runtime,
        "globalThis.trustedRan === true && globalThis.defaultRan === true && globalThis.bad === undefined",
    );
}

#[test]
fn eval_and_function_constructor_require_native_trusted_script() {
    let mut runtime = make_runtime("<body><iframe id=frame></iframe>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "script-src 'unsafe-eval'; require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const policy = trustedTypes.createPolicy('code', {createScript: s => s});
        const trusted = policy.createScript('1 + 1');
        const forged = Object.create(TrustedScript.prototype);
        forged.toString = () => 'globalThis.forgedRan = true';
        const rejects = f => { try { f(); return false; } catch (e) { return e instanceof EvalError; } };
        const frame = document.getElementById('frame').contentWindow;
        rejects(() => eval('globalThis.plainRan = true')) &&
          rejects(() => new Function('globalThis.plainRan = true')) &&
          rejects(() => new Function(forged)) &&
          eval(forged) === forged &&
          eval(trusted) === 2 &&
          new Function(policy.createScript('value'), policy.createScript('return value + 1'))(1) === 2 &&
          frame.trustedTypes.isScript(trusted) &&
          eval(frame.trustedTypes.createPolicy('child', {createScript: s => s}).createScript('4 + 1')) === 5 &&
          globalThis.plainRan === undefined && globalThis.forgedRan === undefined
        "#,
    );
}

#[test]
fn trusted_inline_event_handler_uses_attribute_permission_not_dynamic_eval_permission() {
    let mut runtime = make_runtime("<body><button id=target>run</button>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "script-src 'unsafe-inline'; require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const target = document.getElementById('target');
        const policy = trustedTypes.createPolicy('events', {createScript: s => s});
        let plainRejected = false;
        try { target.setAttribute('onclick', 'globalThis.plainHandlerRan = true'); }
        catch (e) { plainRejected = e instanceof TypeError; }
        target.setAttribute('onclick', policy.createScript('globalThis.trustedHandlerRan = event.type'));
        target.click();
        plainRejected && globalThis.plainHandlerRan === undefined &&
          globalThis.trustedHandlerRan === 'click'
        "#,
    );
}

#[test]
fn inline_event_handler_obeys_script_src_attr_and_reports_without_running() {
    let mut runtime = make_runtime("<body><button id=target>run</button>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "script-src 'unsafe-eval'; script-src-attr 'none' 'report-sample'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        const target = document.getElementById('target');
        target.setAttribute('onclick', 'globalThis.blockedHandlerRan = true');
        target.click();
        globalThis.blockedHandlerRan === undefined
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        r#"
        violations.length === 1 &&
          violations[0].effectiveDirective === 'script-src-attr' &&
          violations[0].blockedURI === 'inline' &&
          violations[0].sample === 'globalThis.blockedHandlerRan = true' &&
          violations[0].target === document.getElementById('target')
        "#,
    );
}

#[test]
fn inline_event_handler_report_only_delivers_violation_and_still_runs() {
    let mut runtime = make_runtime("<body><button id=target>run</button>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy-Report-Only".into(),
        "script-src-attr 'none' 'report-sample'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        const target = document.getElementById('target');
        target.setAttribute('onclick', 'globalThis.reportedHandlerRan = event.type');
        target.click();
        globalThis.reportedHandlerRan === 'click'
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 1 && violations[0].effectiveDirective === 'script-src-attr' && violations[0].disposition === 'report'",
    );
}

#[test]
fn cross_realm_function_constructor_uses_its_own_realm_trusted_types_policy() {
    let mut runtime = make_runtime("<body><iframe id=frame></iframe>");
    assert_js_true(
        &mut runtime,
        r#"
        const frame = document.getElementById('frame');
        frame.srcdoc = '<meta http-equiv="Content-Security-Policy" content="script-src \'unsafe-eval\'; require-trusted-types-for \'script\'"><body></body>';
        const child = frame.contentWindow;
        let functionBlocked = false;
        let evalBlocked = false;
        try { new child.Function('globalThis.leaked = true'); }
        catch (e) { functionBlocked = e instanceof child.EvalError; }
        try { child.eval('globalThis.leaked = true'); }
        catch (e) { evalBlocked = e instanceof child.EvalError; }
        functionBlocked && evalBlocked && child.leaked === undefined
        "#,
    );
}

#[test]
fn retired_realm_function_keeps_its_original_script_policy() {
    let mut runtime = make_runtime("<body><iframe id=frame></iframe>");
    assert_js_true(
        &mut runtime,
        r#"
        const frame = document.getElementById('frame');
        frame.srcdoc = '<meta http-equiv="Content-Security-Policy" content="script-src \'none\'"><body></body>';
        const old = frame.contentWindow;
        const oldDocument = frame.contentDocument;
        const OldFunction = old.Function;
        const OldEvalError = old.EvalError;
        frame.srcdoc = '<body>replacement</body>';
        void frame.contentDocument;
        let blocked = false;
        try { OldFunction('return 1')(); }
        catch (error) { blocked = error instanceof OldEvalError; }
        blocked && oldDocument !== frame.contentDocument
        "#,
    );
}

#[test]
fn retired_realm_function_keeps_its_original_trusted_types_requirement() {
    let mut runtime = make_runtime("<body><iframe id=frame></iframe>");
    assert_js_true(
        &mut runtime,
        r#"
        const frame = document.getElementById('frame');
        frame.srcdoc = '<meta http-equiv="Content-Security-Policy" content="script-src \'unsafe-eval\'; require-trusted-types-for \'script\'"><body></body>';
        const old = frame.contentWindow;
        const oldDocument = frame.contentDocument;
        const OldFunction = old.Function;
        const OldEvalError = old.EvalError;
        frame.srcdoc = '<body>replacement</body>';
        void frame.contentDocument;
        let blocked = false;
        try { OldFunction('return 1')(); }
        catch (error) { blocked = error instanceof OldEvalError; }
        blocked && oldDocument !== frame.contentDocument
        "#,
    );
}

#[test]
fn dynamic_code_default_policy_sees_generated_source_and_cannot_change_it() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const calls = [];
        trustedTypes.createPolicy('default', {createScript: (code, type, sink) => {
          calls.push([code, type, sink]);
          return code.includes('changed') ? code + ' // mutation' : code;
        }});
        const rejects = f => { try { f(); return false; } catch (e) { return e instanceof EvalError; } };
        eval('1 + 1') === 2 && new Function('value', 'return value + 1')(2) === 3 &&
          rejects(() => eval('changed')) &&
          rejects(() => new Function('return "changed"')) &&
          calls.some(c => c.join('|') === '1 + 1|TrustedScript|eval') &&
          calls.some(c => c[0] === 'function anonymous(value\n) {\nreturn value + 1\n}' &&
                         c[1] === 'TrustedScript' && c[2] === 'Function')
        "#,
    );
}

#[test]
fn dynamic_code_policy_exception_is_eval_error_and_reports_function_parameter_sample() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', event => violations.push(event));
        let constructorRejected = false;
        try { new Function('return 1'); }
        catch (error) { constructorRejected = error instanceof EvalError; }
        trustedTypes.createPolicy('default', {createScript: () => { throw new RangeError('policy failure'); }});
        let callbackRejected = false;
        try { eval('2 + 2'); }
        catch (error) { callbackRejected = error instanceof EvalError; }
        constructorRejected && callbackRejected
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 1 && violations[0].sample === 'Function|(\\n) {\\nreturn 1\\n}'",
    );
}

#[test]
fn script_src_controls_eval_function_and_string_timers_after_trust_check() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "script-src 'self' 'report-sample'; require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        const policy = trustedTypes.createPolicy('code', {createScript: s => s});
        const rejects = f => { try { f(); return false; } catch (e) { return e instanceof EvalError; } };
        rejects(() => eval(policy.createScript('1 + 1'))) &&
          rejects(() => new Function(policy.createScript('return 2'))) &&
          rejects(() => setTimeout(policy.createScript('globalThis.blocked = true'), 0)) &&
          globalThis.blocked === undefined
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 3 && violations.every(e => e.effectiveDirective === 'script-src' && e.blockedURI === 'eval' && e.disposition === 'enforce')",
    );

    let mut allowed = make_runtime("<body>");
    allowed.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "script-src 'trusted-types-eval'; require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut allowed,
        r#"
        const policy = trustedTypes.createPolicy('code', {createScript: s => s});
        eval(policy.createScript('6 * 7')) === 42 &&
          new Function(policy.createScript('return 42'))() === 42
        "#,
    );
}

#[test]
fn report_only_dynamic_code_policy_reports_without_blocking() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy-Report-Only".into(),
        "script-src 'self'; require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        eval('1 + 1') === 2 && new Function('return 3')() === 3 &&
          setTimeout('globalThis.timerRan = true', 0) > 0
        "#,
    );
    runtime.tick(0).unwrap();
    assert_js_true(
        &mut runtime,
        "globalThis.timerRan === true && violations.filter(e => e.blockedURI === 'trusted-types-sink').length === 3 && violations.filter(e => e.blockedURI === 'eval').length === 3 && violations.every(e => e.disposition === 'report')",
    );
}

#[test]
fn worker_script_urls_require_trusted_script_url_before_worker_creation() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        const scriptURL = 'data:text/javascript,postMessage(42)';
        let rejected = false;
        try { new Worker(scriptURL); } catch (e) { rejected = e instanceof TypeError; }
        const policy = trustedTypes.createPolicy('workers', {createScriptURL: s => s});
        const worker = new Worker(policy.createScriptURL(scriptURL));
        const result = rejected && worker.url === scriptURL;
        worker.terminate();
        result
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 1 && violations[0].sample === 'Worker constructor|data:text/javascript,postMessage(42)'",
    );
}

#[test]
fn import_scripts_checks_original_urls_despite_array_prototype_poisoning() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.workerResults = null;
        const source = `
          const raw = 'data:text/javascript,globalThis.untrustedImportRan=true';
          const map = Array.prototype.map;
          Array.prototype.map = function() { return this; };
          let mapBlocked = false;
          try { importScripts(raw); }
          catch (error) { mapBlocked = error instanceof TypeError; }
          Array.prototype.map = map;

          const policy = trustedTypes.createPolicy('imports', {createScriptURL: value => value});
          const trusted = policy.createScriptURL('data:text/javascript,globalThis.trustedImportRan=true');
          const iterator = Array.prototype[Symbol.iterator];
          Array.prototype[Symbol.iterator] = function*() { yield raw; };
          importScripts(trusted);
          Array.prototype[Symbol.iterator] = iterator;
          postMessage([mapBlocked, globalThis.untrustedImportRan === undefined,
                       globalThis.trustedImportRan === true]);
        `;
        const policy = trustedTypes.createPolicy('worker', {createScriptURL: value => value});
        const worker = new Worker(policy.createScriptURL('data:text/javascript,' + encodeURIComponent(source)));
        worker.onmessage = event => { workerResults = event.data; worker.terminate(); };
        workerResults === null
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "JSON.stringify(workerResults) === '[true,true,true]'",
    );
}

#[test]
fn child_text_cannot_bypass_trusted_script_at_dynamic_script_execution() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "script-src 'unsafe-inline'; require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        const raw = document.createElement('script');
        raw.appendChild(document.createTextNode('globalThis.injected = true'));
        document.body.appendChild(raw);
        const policy = trustedTypes.createPolicy('code', {createScript: s => s});
        const approved = document.createElement('script');
        approved.textContent = policy.createScript('globalThis.approved = true');
        document.body.appendChild(approved);
        globalThis.injected === undefined && globalThis.approved === true
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    assert_js_true(
        &mut runtime,
        "violations.length === 1 && violations[0].sample === 'HTMLScriptElement text|globalThis.injected = true'",
    );
}

#[test]
fn trusted_script_text_survives_document_fragment_insertion() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "script-src 'unsafe-inline'; require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const policy = trustedTypes.createPolicy('fragment', {createScript: value => value});
        const script = document.createElement('script');
        script.textContent = policy.createScript('globalThis.fragmentScriptRan = true');
        const fragment = document.createDocumentFragment();
        fragment.appendChild(script);
        document.body.appendChild(fragment);
        const other = document.createDocumentFragment();
        const scriptAfterFragment = document.createElement('script');
        other.appendChild(scriptAfterFragment);
        scriptAfterFragment.textContent = policy.createScript('globalThis.secondFragmentScriptRan = true');
        document.body.appendChild(other);
        globalThis.fragmentScriptRan === true &&
          globalThis.secondFragmentScriptRan === true
        "#,
    );
}

#[test]
fn trusted_script_text_approval_expires_after_child_changes_even_if_bytes_are_restored() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "script-src 'unsafe-inline'; require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        const policy = trustedTypes.createPolicy('code', {createScript: s => s});
        const script = document.createElement('script');
        script.textContent = policy.createScript('globalThis.reused = true');
        script.firstChild.data = 'globalThis.modified = true';
        script.firstChild.data = 'globalThis.reused = true';
        document.body.appendChild(script);
        globalThis.reused === undefined && globalThis.modified === undefined
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 1 && violations[0].sample === 'HTMLScriptElement text|globalThis.reused = true'",
    );
}

#[test]
fn trusted_script_text_approval_expires_after_detach_and_reconnect() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "script-src 'unsafe-inline'; require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        const policy = trustedTypes.createPolicy('code', {createScript: s => s});
        const script = document.createElement('script');
        script.textContent = policy.createScript('globalThis.reconnected = true');
        const detached = document.createElement('div');
        detached.appendChild(script);
        detached.removeChild(script);
        document.body.appendChild(script);
        globalThis.reconnected === undefined
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(&mut runtime, "violations.length === 1");
}

#[test]
fn trusted_script_text_approval_expires_after_cross_document_adoption() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "script-src 'unsafe-inline'; require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        const frame = document.createElement('iframe');
        document.body.appendChild(frame);
        const policy = trustedTypes.createPolicy('code', {createScript: s => s});
        const script = document.createElement('script');
        script.textContent = policy.createScript('globalThis.adopted = true');
        frame.contentDocument.adoptNode(script);
        document.adoptNode(script);
        document.body.appendChild(script);
        globalThis.adopted === undefined
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(&mut runtime, "violations.length === 1");
}

#[test]
fn default_policy_cannot_move_prepared_inline_script_to_another_document_realm() {
    let mut runtime = make_runtime("<body><iframe id=frame></iframe>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "script-src 'unsafe-inline'; require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        const frame = document.getElementById('frame');
        const child = frame.contentWindow;
        const script = document.createElement('script');
        script.appendChild(document.createTextNode('globalThis.movedScriptRan = true'));
        trustedTypes.createPolicy('default', {createScript: source => {
          frame.contentDocument.adoptNode(script);
          frame.contentDocument.body.appendChild(script);
          return source;
        }});
        document.body.appendChild(script);
        globalThis.movedScriptRan === undefined && child.movedScriptRan === undefined &&
          script.ownerDocument === frame.contentDocument
        "#,
    );
}

#[test]
fn untrusted_script_text_does_not_gain_approval_before_policy_tightens() {
    let mut runtime = make_runtime("<body>");
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.scriptBeforePolicy = document.createElement('script');
        scriptBeforePolicy.textContent = 'globalThis.untrustedBeforePolicy = true';
        true
        "#,
    );
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "script-src 'unsafe-inline'; require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        document.body.appendChild(scriptBeforePolicy);
        globalThis.untrustedBeforePolicy === undefined
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(&mut runtime, "violations.length === 1");
}

#[test]
fn trusted_script_approval_bindings_are_not_page_callable() {
    let mut runtime = make_runtime("<body>");
    assert_js_true(
        &mut runtime,
        "typeof globalThis.__omoikane_trusted_remember_script_text === 'undefined' && \
         typeof globalThis.__omoikane_trusted_take_script_text_approval === 'undefined' && \
         typeof globalThis.__omoikane_compile_inline_handler === 'undefined'",
    );
}

#[cfg(feature = "baseline-jit")]
#[test]
fn trusted_dynamic_code_matches_interpreter_with_jit_call_sites() {
    const SOURCE: &str = r#"
      const policy = trustedTypes.createPolicy('code', {createScript: s => s});
      const expression = policy.createScript('1');
      const body = policy.createScript('return 2');
      function invoke() {
        let blocked = false;
        try { eval('0'); } catch (e) { blocked = e instanceof EvalError; }
        return blocked ? eval(expression) + new Function(body)() : -1;
      }
      let total = 0;
      for (let i = 0; i < 40; i++) total += invoke();
      total
    "#;
    for enabled in [false, true] {
        let mut runtime = make_runtime("<body>");
        runtime.install_response_security_headers(&[(
            "Content-Security-Policy".into(),
            "script-src 'unsafe-eval'; require-trusted-types-for 'script'".into(),
        )]);
        runtime.set_baseline_jit_enabled(enabled);
        assert_eq!(runtime.eval(SOURCE).unwrap().as_number(), Some(120.0));
        if enabled {
            assert!(runtime.baseline_jit_diagnostics().runtime_helper_entries > 0);
        }
    }
}

#[test]
fn inserted_script_default_policy_converts_the_prepared_source_snapshot() {
    let mut runtime = make_runtime("<body>");
    runtime.install_response_security_headers(&[
        (
            "Content-Security-Policy".into(),
            "script-src 'unsafe-inline'; require-trusted-types-for 'script'".into(),
        ),
        (
            "Content-Security-Policy-Report-Only".into(),
            "script-src 'none' 'report-sample'".into(),
        ),
    ]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.convertedViolations = [];
        document.addEventListener('securitypolicyviolation', e => convertedViolations.push(e));
        const script = document.createElement('script');
        const original = 'globalThis.originalSourceRan = true';
        script.appendChild(document.createTextNode(original));
        let observedSource;
        trustedTypes.createPolicy('default', {createScript: source => {
          observedSource = source;
          script.firstChild.data = 'globalThis.mutatedSourceRan = true';
          return 'globalThis.convertedSourceRan = true';
        }});
        document.body.appendChild(script);
        observedSource === original && globalThis.convertedSourceRan === true &&
          globalThis.originalSourceRan === undefined &&
          globalThis.mutatedSourceRan === undefined
        "#,
    );
    assert!(
        runtime
            .host_state
            .borrow()
            .prepared_inserted_inline_scripts
            .is_empty()
    );
}
