//! Inline samples are captured at the check and disclosed per violating policy.
use super::*;
use serde_json::{Value, json};

fn runtime_with_samples(html: &str, headers: &[(&str, &str)]) -> JsRuntime {
    let document = crate::html::TreeBuilder::parse(html).document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
    runtime.install_response_security_headers(
        &headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect::<Vec<_>>(),
    );
    runtime
        .eval(
            r#"
        globalThis.sampleEvents = [], globalThis.sampleReports = [];
        document.addEventListener('securitypolicyviolation', event => sampleEvents.push({
            sample: event.sample, policy: event.originalPolicy,
            disposition: event.disposition, target: event.target.id || '',
            sourceFile: event.sourceFile, lineNumber: event.lineNumber,
            columnNumber: event.columnNumber
        }));
        new ReportingObserver(batch => sampleReports.push(...batch.map(report => ({
            sample: report.body.sample, policy: report.body.originalPolicy,
            disposition: report.body.disposition,
            sourceFile: report.body.sourceFile, lineNumber: report.body.lineNumber,
            columnNumber: report.body.columnNumber
        }))), {types: ['csp-violation']}).observe();
    "#,
        )
        .unwrap();
    runtime
}

fn observations(runtime: &mut JsRuntime) -> Value {
    runtime.run_timers(20, 1, 100);
    let text = runtime
        .eval("JSON.stringify({events: sampleEvents, reports: sampleReports})")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    serde_json::from_str(&text).unwrap()
}

fn assert_samples(runtime: &mut JsRuntime, expected: &[(&str, &str, &str)]) -> Value {
    assert_sample_channels(runtime, expected, expected)
}

fn assert_sample_channels(
    runtime: &mut JsRuntime,
    expected_events: &[(&str, &str, &str)],
    expected_reports: &[(&str, &str, &str)],
) -> Value {
    let observed = observations(runtime);
    for (channel, expected) in [("events", expected_events), ("reports", expected_reports)] {
        let mut expected: Vec<_> = expected
            .iter()
            .map(|(disposition, policy, sample)| json!([disposition, policy, sample]))
            .collect();
        expected.sort_by_key(Value::to_string);
        let mut rows: Vec<_> = observed[channel]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| json!([item["disposition"], item["policy"], item["sample"]]))
            .collect();
        rows.sort_by_key(Value::to_string);
        assert_eq!(rows, expected, "{channel}: {observed}");
        for item in observed[channel].as_array().unwrap() {
            let source = json!([item["sourceFile"], item["lineNumber"], item["columnNumber"]]);
            assert_eq!(
                source,
                if channel == "events" {
                    json!(["", 0, 0])
                } else {
                    json!([null, null, null])
                },
                "sample support must not invent source locations: {observed}"
            );
        }
    }
    observed
}

#[test]
fn each_enforced_and_monitoring_policy_controls_its_own_inline_sample() {
    let opted_in = "script-src 'report-sample'; report-to samples";
    let opted_out = "script-src 'none'; report-to samples";
    let mut runtime = runtime_with_samples(
        "<body><script id=source>globalThis.executed = true;</script>",
        &[
            ("Content-Security-Policy", opted_in),
            ("Content-Security-Policy", opted_out),
            ("Content-Security-Policy-Report-Only", opted_in),
            ("Content-Security-Policy-Report-Only", opted_out),
        ],
    );
    assert!(runtime.execute_document_scripts(None).is_empty());
    assert_samples(
        &mut runtime,
        &[
            ("enforce", opted_in, "globalThis.executed = true;"),
            ("enforce", opted_out, ""),
            ("report", opted_in, "globalThis.executed = true;"),
            ("report", opted_out, ""),
        ],
    );
    assert_eq!(
        runtime
            .eval("typeof executed")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "undefined"
    );
}

#[test]
fn observer_opt_in_is_independent_of_sample_opt_in_for_each_policy() {
    let event_only = "script-src 'report-sample'";
    let observable = "script-src 'report-sample'; report-to samples";
    // An endpoint is unnecessary for local observation. Both policies still
    // produce a violation event, while only report-to queues an observer report.
    let mut runtime = runtime_with_samples(
        "<body><script>globalThis.executed = true;</script>",
        &[
            ("Content-Security-Policy", event_only),
            ("Content-Security-Policy", observable),
            ("Content-Security-Policy-Report-Only", event_only),
            ("Content-Security-Policy-Report-Only", observable),
        ],
    );
    assert!(runtime.execute_document_scripts(None).is_empty());
    assert_sample_channels(
        &mut runtime,
        &[
            ("enforce", event_only, "globalThis.executed = true;"),
            ("enforce", observable, "globalThis.executed = true;"),
            ("report", event_only, "globalThis.executed = true;"),
            ("report", observable, "globalThis.executed = true;"),
        ],
        &[
            ("enforce", observable, "globalThis.executed = true;"),
            ("report", observable, "globalThis.executed = true;"),
        ],
    );
    assert_eq!(
        runtime
            .eval("typeof executed === 'undefined'")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn sample_opt_in_uses_the_selected_element_directive_and_its_fallback() {
    for (policy, sample) in [
        (
            "default-src 'report-sample'; report-to samples",
            Some("globalThis.executed = true;"),
        ),
        (
            "default-src 'report-sample'; script-src 'none'; report-to samples",
            Some(""),
        ),
        (
            "script-src 'none'; script-src-elem 'report-sample'; report-to samples",
            Some("globalThis.executed = true;"),
        ),
        (
            "script-src 'report-sample'; script-src-elem 'none'; report-to samples",
            Some(""),
        ),
        (
            "script-src 'none'; style-src 'report-sample'; report-to samples",
            Some(""),
        ),
        (
            "script-src 'report-sample'; script-src-elem 'unsafe-inline'; report-to samples",
            None,
        ),
    ] {
        let mut runtime = runtime_with_samples(
            "<body><script>globalThis.executed = true;</script>",
            &[("Content-Security-Policy-Report-Only", policy)],
        );
        assert!(
            runtime.execute_document_scripts(None).is_empty(),
            "{policy}"
        );
        let expected = sample
            .map(|sample| vec![("report", policy, sample)])
            .unwrap_or_default();
        assert_samples(&mut runtime, &expected);
        assert_eq!(
            runtime.eval("executed").unwrap().as_boolean(),
            Some(true),
            "{policy}"
        );
    }
}

#[test]
fn inline_samples_keep_leading_whitespace_and_clip_at_forty_unicode_scalars() {
    let policy = "script-src 'report-sample'; report-to samples";
    // Scalar counting is the engine's clipping choice; these expectations do
    // not derive the sample by calling the implementation's clipping function.
    for (source, expected) in [
        (
            " \n\tglobalThis.executed = true;",
            " \n\tglobalThis.executed = true;",
        ),
        (
            "012345678901234567890123456789012345678",
            "012345678901234567890123456789012345678",
        ),
        (
            "0123456789012345678901234567890123456789",
            "0123456789012345678901234567890123456789",
        ),
        (
            "01234567890123456789012345678901234567890",
            "0123456789012345678901234567890123456789",
        ),
        (
            "あいうえおあいうえおあいうえおあいうえおあいうえおあいうえおあいうえおかきくけ😀終",
            "あいうえおあいうえおあいうえおあいうえおあいうえおあいうえおあいうえおかきくけ😀",
        ),
    ] {
        let mut runtime = runtime_with_samples(
            &format!("<body><script>{source}</script>"),
            &[("Content-Security-Policy", policy)],
        );
        assert!(runtime.execute_document_scripts(None).is_empty());
        assert_samples(&mut runtime, &[("enforce", policy, expected)]);
    }
}

#[test]
fn dynamic_script_samples_keep_the_source_captured_before_text_changes() {
    let policy = "script-src 'report-sample'; report-to samples";
    let mut runtime = runtime_with_samples("<body>", &[("Content-Security-Policy", policy)]);
    runtime
        .eval(
            r#"
        globalThis.source = document.createElement('script');
        source.id = 'dynamic'; source.textContent = 'globalThis.originalRan = true;';
        document.body.appendChild(source);
        source.textContent = 'globalThis.replacementRan = true;';
    "#,
        )
        .unwrap();
    let observed = assert_samples(
        &mut runtime,
        &[("enforce", policy, "globalThis.originalRan = true;")],
    );
    assert_eq!(observed["events"][0]["target"], "dynamic");
    assert_eq!(
        runtime
            .eval("typeof originalRan === 'undefined' && typeof replacementRan === 'undefined'")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn written_inline_script_samples_belong_to_the_written_element() {
    let policy = "script-src 'report-sample'; report-to samples";
    let mut runtime = runtime_with_samples("<body>", &[("Content-Security-Policy", policy)]);
    runtime
        .eval(r#"document.write('<script id="written">globalThis.writtenRan = true;<\/script>');"#)
        .unwrap();
    let observed = assert_samples(
        &mut runtime,
        &[("enforce", policy, "globalThis.writtenRan = true;")],
    );
    assert_eq!(observed["events"][0]["target"], "written");
    assert_eq!(
        runtime
            .eval("typeof writtenRan === 'undefined'")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

fn queue_written_module_then_change_source(runtime: &mut JsRuntime, mutation: &str) {
    runtime
        .eval(&format!(
            r#"
        globalThis.ownerTargets = [];
        document.addEventListener('securitypolicyviolation', event =>
            ownerTargets.push([event.target === document, event.documentURI]));
        document.write('<script type="module" id="written">globalThis.originalRan = true;<\/script>');
        globalThis.written = document.getElementById('written');
        {mutation}
        written.textContent = 'globalThis.replacementRan = true;';
    "#
        ))
        .unwrap();
}

fn assert_written_module_uses_original_owner(runtime: &mut JsRuntime, disposition: &str) {
    assert_eq!(runtime.host_state.borrow().written_script_queue.len(), 1);
    // Parser-inserted modules wait for the parsing-complete checkpoint;
    // draining only ordinary event-loop tasks does not finish this queue.
    runtime.fire_dom_content_loaded().unwrap();
    runtime.run_until_idle().unwrap();
    let expected_execution = if disposition == "enforce" {
        "typeof originalRan === 'undefined'"
    } else {
        "originalRan === true"
    };
    assert_eq!(
        runtime.eval(expected_execution).unwrap().as_boolean(),
        Some(true)
    );
    let observed = assert_samples(
        runtime,
        &[(
            disposition,
            "script-src 'report-sample'; report-to samples",
            "globalThis.originalRan = true;",
        )],
    );
    assert_eq!(observed["events"][0]["target"], "");
    assert_eq!(
        runtime
            .eval("JSON.stringify(ownerTargets)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        r#"[[true,"https://example.test/document"]]"#
    );
    assert_eq!(
        runtime
            .eval("typeof replacementRan === 'undefined'")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    assert!(runtime.take_task_errors().is_empty());
}

#[test]
fn detached_written_module_retains_original_policy_and_sample() {
    for (header, disposition) in [
        ("Content-Security-Policy", "enforce"),
        ("Content-Security-Policy-Report-Only", "report"),
    ] {
        let mut runtime = runtime_with_samples(
            "<body>",
            &[(header, "script-src 'report-sample'; report-to samples")],
        );
        queue_written_module_then_change_source(&mut runtime, "written.remove();");
        assert_written_module_uses_original_owner(&mut runtime, disposition);
    }
}

#[test]
fn adopted_written_module_reports_to_original_owner_and_keeps_original_source() {
    for (header, disposition) in [
        ("Content-Security-Policy", "enforce"),
        ("Content-Security-Policy-Report-Only", "report"),
    ] {
        // Materialize the destination before installing the original owner's
        // response policy, so adoption cannot accidentally preserve the same
        // policy by inheriting it into the destination's initial document.
        let mut runtime = runtime_with_samples("<body><iframe></iframe>", &[]);
        runtime.run_until_idle().unwrap();
        runtime
            .eval(
                r#"
            globalThis.destinationWindow = document.querySelector('iframe').contentWindow;
            globalThis.destinationDocument = destinationWindow.document;
            globalThis.destinationEvents = [], globalThis.destinationReports = [];
            destinationDocument.addEventListener('securitypolicyviolation', event =>
                destinationEvents.push(event));
            new destinationWindow.ReportingObserver(batch =>
                destinationReports.push(...batch)).observe();
        "#,
            )
            .unwrap();
        runtime.install_response_security_headers(&[(
            header.to_owned(),
            "script-src 'report-sample'; report-to samples".to_owned(),
        )]);
        queue_written_module_then_change_source(
            &mut runtime,
            "destinationDocument.body.appendChild(destinationDocument.adoptNode(written));",
        );
        assert_eq!(
            runtime
                .eval("document.querySelector('iframe') !== null && document.querySelector('iframe').contentDocument === destinationDocument && document.querySelector('iframe').contentWindow === destinationWindow")
                .unwrap()
                .as_boolean(),
            Some(true),
            "document.write must retain the live destination browsing context"
        );
        assert_written_module_uses_original_owner(&mut runtime, disposition);
        assert_eq!(
            runtime
                .eval("written.ownerDocument === destinationDocument && destinationEvents.length === 0 && destinationReports.length === 0 && typeof destinationWindow.originalRan === 'undefined' && typeof destinationWindow.replacementRan === 'undefined'")
                .unwrap()
                .as_boolean(),
            Some(true)
        );
    }
}

#[test]
fn style_samples_keep_original_text_and_do_not_repeat_for_resolver_rebuilds() {
    let policy = "style-src 'report-sample'; report-to samples";
    let mut runtime = runtime_with_samples(
        "<style id=sheet> \n#target { color: rgb(1, 2, 3); }</style><body><p id=target>x</p>",
        &[("Content-Security-Policy-Report-Only", policy)],
    );
    assert_eq!(
        runtime
            .eval("getComputedStyle(document.getElementById('target')).color")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "rgb(1, 2, 3)"
    );
    let observed = assert_samples(
        &mut runtime,
        &[("report", policy, " \n#target { color: rgb(1, 2, 3); }")],
    );
    assert_eq!(observed["events"][0]["target"], "sheet");
    runtime.set_viewport(420.0, 320.0);
    runtime
        .eval("getComputedStyle(document.getElementById('target')).color")
        .unwrap();
    assert_samples(
        &mut runtime,
        &[("report", policy, " \n#target { color: rgb(1, 2, 3); }")],
    );
}

#[test]
fn external_scripts_links_and_imports_never_receive_inline_samples() {
    let policy =
        "script-src 'report-sample'; style-src 'unsafe-inline' 'report-sample'; report-to samples";
    let mut runtime = runtime_with_samples(
        r#"
        <style>@import url('data:text/css,%23target%7Bcolor:red%7D');</style>
        <link rel=stylesheet href='data:text/css,%23target%7Bwidth:19px%7D'>
        <body><p id=target>x</p>
        <script src='data:text/javascript,globalThis.externalRan%3Dtrue%3B'></script>
    "#,
        &[("Content-Security-Policy-Report-Only", policy)],
    );
    assert!(runtime.execute_document_scripts(None).is_empty());
    assert_eq!(
        runtime.eval("externalRan").unwrap().as_boolean(),
        Some(true)
    );
    assert_eq!(
        runtime
            .eval("getComputedStyle(document.getElementById('target')).width")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "19px"
    );
    // Each external resource produces a pre-request and post-response check.
    assert_samples(&mut runtime, &[("report", policy, ""); 6]);
}

#[test]
fn element_directive_overrides_do_not_change_style_attribute_enforcement() {
    for (policy, initial, mutated) in [
        (
            "style-src-elem 'none'; style-src-attr 'unsafe-inline'",
            "rgb(1, 2, 3)",
            "rgb(7, 8, 9)",
        ),
        (
            "style-src-elem 'unsafe-inline'; style-src-attr 'none'",
            "rgb(4, 5, 6)",
            "rgb(4, 5, 6)",
        ),
    ] {
        let mut runtime = runtime_with_samples(
            "<style>#target { color: rgb(4, 5, 6) }</style><body><p id=target style='color: rgb(1, 2, 3)'>x</p>",
            &[("Content-Security-Policy", policy)],
        );
        for (mutation, expected) in [(false, initial), (true, mutated)] {
            if mutation {
                runtime
                    .eval("document.getElementById('target').setAttribute('style', 'color: rgb(7, 8, 9)')")
                    .unwrap();
            }
            let color = runtime
                .eval("getComputedStyle(document.getElementById('target')).color")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped();
            assert_eq!(color, expected, "{policy}, mutation={mutation}");
        }
    }
}

#[test]
fn inline_sample_is_identical_in_events_observers_and_both_report_transports() {
    type Delivery = (String, String, Value);
    struct Transport(Arc<Mutex<Vec<Delivery>>>);
    impl crate::http::HttpTransport for Transport {
        fn send(
            &self,
            request: &HttpRequest,
            _: Option<Duration>,
        ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
            self.0.lock().unwrap().push((
                request.url().to_string(),
                request.header("Content-Type").unwrap_or("").to_owned(),
                serde_json::from_slice(request.body().unwrap()).unwrap(),
            ));
            crate::http::HttpResponse::parse(
                &mut &b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"[..],
            )
        }
    }
    let legacy = "script-src 'report-sample'; report-uri /legacy";
    let modern = "script-src 'report-sample'; report-to samples";
    for (header, disposition) in [
        ("Content-Security-Policy", "enforce"),
        ("Content-Security-Policy-Report-Only", "report"),
    ] {
        let mut runtime = runtime_with_samples(
            "<body><script>\n globalThis.endpointRan = true;</script>",
            &[
                (header, legacy),
                (header, modern),
                ("Reporting-Endpoints", "samples=\"/modern\""),
            ],
        );
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport(sent.clone()))));
        assert!(runtime.execute_document_scripts(None).is_empty());
        let observed = assert_sample_channels(
            &mut runtime,
            &[
                (disposition, legacy, "\n globalThis.endpointRan = true;"),
                (disposition, modern, "\n globalThis.endpointRan = true;"),
            ],
            &[(disposition, modern, "\n globalThis.endpointRan = true;")],
        );
        wait_for_report_delivery(&mut runtime);
        let legacy_event = observed["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["policy"] == legacy)
            .unwrap();
        let modern_observer_report = &observed["reports"][0];
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        let legacy_report = sent
            .iter()
            .find(|entry| entry.0.ends_with("/legacy"))
            .unwrap();
        assert_eq!(legacy_report.1, "application/csp-report");
        assert_eq!(
            legacy_report.2["csp-report"]["script-sample"],
            legacy_event["sample"]
        );
        assert_eq!(legacy_report.2["csp-report"]["disposition"], disposition);
        let modern_report = sent
            .iter()
            .find(|entry| entry.0.ends_with("/modern"))
            .unwrap();
        assert_eq!(modern_report.1, "application/reports+json");
        assert_eq!(
            modern_report.2[0]["body"]["sample"],
            modern_observer_report["sample"]
        );
        assert_eq!(modern_report.2[0]["body"]["disposition"], disposition);
    }
}
