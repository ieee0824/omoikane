//! CSP violation event conversion and the reporting integration.
use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

#[path = "support/http_fixture.rs"]
mod http_fixture;

fn check(script: &str) {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse("<!doctype html><html><body></body></html>").document(),
    )
    .unwrap();
    assert_eq!(runtime.eval(script).unwrap().as_boolean(), Some(true));
}

#[test]
fn csp_event_dictionary_preserves_payload_and_converts_webidl_values() {
    check(
        r#"(() => {
        const event = new SecurityPolicyViolationEvent('securitypolicyviolation', {
            blockedURI: '\ud800', documentURI: '\ud800', referrer: '\ud800',
            sourceFile: '\ud800', sample: '\ud800', originalPolicy: "default-src 'none'",
            effectiveDirective: 'script-src', violatedDirective: 'default-src',
            statusCode: 65537, lineNumber: -1, columnNumber: 4294967298,
            disposition: 'report', bubbles: true, composed: true
        });
        event.effectiveDirective = 'changed';
        return event instanceof Event && event.bubbles && event.composed &&
            event.blockedURI === '\ufffd' && event.documentURI === '\ufffd' &&
            event.sourceFile === '\ufffd' && event.referrer === '\ufffd' &&
            event.sample === '\ud800' && event.originalPolicy === "default-src 'none'" &&
            event.effectiveDirective === 'script-src' && event.violatedDirective === 'default-src' &&
            event.statusCode === 1 && event.lineNumber === 4294967295 && event.columnNumber === 2 &&
            event.disposition === 'report';
        })()"#,
    );
}

#[test]
fn csp_event_requires_arguments_valid_enum_and_private_brand() {
    check(
        r#"(() => {
        const event = new SecurityPolicyViolationEvent('');
        if (event.documentURI !== '' || event.sourceFile !== '' || event.referrer !== '' ||
            event.sample !== '' || event.lineNumber !== 0 || event.columnNumber !== 0 ||
            event.statusCode !== 0 || event.disposition !== 'enforce') return false;
        const explicitNull = new SecurityPolicyViolationEvent('', {sourceFile: null});
        if (explicitNull.sourceFile !== 'null') return false;
        const getter = Object.getOwnPropertyDescriptor(SecurityPolicyViolationEvent.prototype, 'blockedURI').get;
        for (const create of [() => new SecurityPolicyViolationEvent(),
            () => new SecurityPolicyViolationEvent(Symbol()),
            () => new SecurityPolicyViolationEvent('x', {disposition: 'invalid'}),
            () => new SecurityPolicyViolationEvent('x', {blockedURI: Symbol()}),
            () => getter.call(Object.create(SecurityPolicyViolationEvent.prototype)),
            () => getter.call(new Event('x'))]) {
            try { create(); return false; } catch (error) { if (!(error instanceof TypeError)) throw error; }
        }
        return true;
        })()"#,
    );
}

#[test]
fn reporting_observer_converts_options_and_checks_private_receivers() {
    check(
        r#"(() => {
        const order = [];
        const observer = new ReportingObserver(() => {}, {
            get buffered() { order.push('buffered'); return false; },
            get types() { order.push('types'); return { [Symbol.iterator]: function*() { yield 'csp-violation'; } }; }
        });
        if (order.join(',') !== 'buffered,types' || observer.takeRecords().length !== 0) return false;
        observer.observe(); observer.observe(); observer.disconnect();
        for (const create of [() => new ReportingObserver(), () => new ReportingObserver(null),
            () => new ReportingObserver(() => {}, {types: null}),
            () => new ReportingObserver(() => {}, {types: 'csp-violation'}),
            () => new ReportingObserver(() => {}, {types: [Symbol()]}),
            () => ReportingObserver.prototype.observe.call({}),
            () => ReportingObserver.prototype.takeRecords.call(Object.create(ReportingObserver.prototype))]) {
            try { create(); return false; } catch (error) { if (!(error instanceof TypeError)) throw error; }
        }
        return Object.prototype.toString.call(observer) === '[object ReportingObserver]';
    })()"#,
    );
}

#[test]
fn blocked_fetch_preserves_empty_report_strings_and_absent_source_metadata() {
    use omoikane::cdp::CdpSession;
    use serde_json::json;

    // Navigation installs the actual Document CSP through the public API.
    // The request is blocked before transport, so this test needs no server.
    let html = r#"<!doctype html><meta http-equiv="Content-Security-Policy" content="connect-src 'none'; report-to reports"><body>"#;
    let url = format!(
        "data:text/html,{}",
        html.bytes()
            .map(|byte| format!("%{byte:02X}"))
            .collect::<String>()
    );
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch("Page.navigate", json!({"url": url}))
        .unwrap();
    session
        .dispatch(
            "Runtime.evaluate",
            json!({"expression": r#"
                globalThis.events = [];
                globalThis.reports = [];
                globalThis.fetchRejected = false;
                document.addEventListener('securitypolicyviolation', event => events.push({
                    referrer: event.referrer, sample: event.sample,
                    sourceFile: event.sourceFile, lineNumber: event.lineNumber,
                    columnNumber: event.columnNumber, trusted: event.isTrusted,
                    targetsDocument: event.target === document
                }));
                new ReportingObserver(batch => reports.push(...batch), {
                    types: ['csp-violation']
                }).observe();
                void fetch('https://blocked.test/resource').catch(error => {
                    fetchRejected = error instanceof TypeError;
                });
            "#}),
        )
        .unwrap();
    let observed = session
        .dispatch(
            "Runtime.evaluate",
            json!({"expression": r#"({fetchRejected, events, reports: reports.map(report => ({
                type: report.type, blockedURL: report.body.blockedURL,
                disposition: report.body.disposition, referrer: report.body.referrer,
                sample: report.body.sample, sourceFile: report.body.sourceFile,
                lineNumber: report.body.lineNumber, columnNumber: report.body.columnNumber
            }))})"#}),
        )
        .unwrap();
    assert_eq!(
        observed["result"]["value"],
        json!({
            "fetchRejected": true,
            "events": [{
                "referrer": "", "sample": "", "sourceFile": "",
                "lineNumber": 0, "columnNumber": 0,
                "trusted": true, "targetsDocument": true
            }],
            "reports": [{
                "type": "csp-violation", "blockedURL": "https://blocked.test/resource",
                "disposition": "enforce", "referrer": "", "sample": "",
                "sourceFile": null, "lineNumber": null, "columnNumber": null
            }]
        })
    );
}

fn serve_redirected_error_document() -> (String, http_fixture::FixtureWorker<Vec<String>>) {
    use http_fixture::{
        ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
        read_request_headers,
    };
    use std::io::Write;

    let listener = bind_loopback().unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let worker = FixtureWorker::spawn(move || {
        let mut paths = Vec::new();
        for expected in ["/redirect", "/error-document"] {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let headers = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let path = headers
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap();
            assert_eq!(path, expected);
            paths.push(path.to_owned());
            if path == "/redirect" {
                stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /error-document\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                continue;
            }
            let body = r#"<!doctype html><script>
                globalThis.events = [];
                globalThis.reports = [];
                globalThis.fetchRejected = false;
                document.addEventListener('securitypolicyviolation', event => events.push({
                    statusCode: event.statusCode, documentURI: event.documentURI,
                    blockedURI: event.blockedURI, disposition: event.disposition
                }));
                new ReportingObserver(batch => reports.push(...batch), {
                    types: ['csp-violation']
                }).observe();
                void fetch('/blocked-resource').catch(error => {
                    fetchRejected = error instanceof TypeError;
                });
            </script>"#;
            write!(stream, "HTTP/1.1 404 Not Found\r\nContent-Type: text/html\r\nContent-Security-Policy: script-src 'unsafe-inline'; connect-src 'none'; report-to reports\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
        paths
    });
    (origin, worker)
}

#[test]
fn csp_reports_use_final_document_response_status_after_navigation_redirect() {
    use omoikane::cdp::CdpSession;
    use serde_json::json;

    let (origin, worker) = serve_redirected_error_document();
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({"url": format!("{origin}/redirect")}),
        )
        .unwrap();
    let observed = session
        .dispatch(
            "Runtime.evaluate",
            json!({"expression": r#"({fetchRejected, events, reports: reports.map(report => ({
                type: report.type, url: report.url, statusCode: report.body.statusCode,
                documentURL: report.body.documentURL, blockedURL: report.body.blockedURL,
                disposition: report.body.disposition
            }))})"#}),
        )
        .unwrap();
    assert_eq!(worker.join(), ["/redirect", "/error-document"]);
    // The blocked resource has no response: 404 belongs to the final
    // Document, rather than the redirect or the request that violated CSP.
    assert_eq!(
        observed["result"]["value"],
        json!({
            "fetchRejected": true,
            "events": [{
                "statusCode": 404, "documentURI": format!("{origin}/error-document"),
                "blockedURI": format!("{origin}/blocked-resource"), "disposition": "enforce"
            }],
            "reports": [{
                "type": "csp-violation", "url": format!("{origin}/error-document"),
                "statusCode": 404, "documentURL": format!("{origin}/error-document"),
                "blockedURL": format!("{origin}/blocked-resource"), "disposition": "enforce"
            }]
        })
    );
}

#[test]
fn reporting_automation_generates_before_page_microtasks_and_requires_opt_in() {
    let document = TreeBuilder::parse("<!doctype html><body></body>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://example.test/reporting#fragment")
            .unwrap();
    assert_eq!(
        runtime
            .eval("typeof __omoikane_automation_generate_test_report")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "undefined"
    );
    runtime.enable_reporting_automation().unwrap();
    runtime
        .eval("globalThis.test_driver_internal = {}; globalThis.add_completion_callback = callback => globalThis.finishAutomation = callback;")
        .unwrap();
    runtime
        .eval(include_str!("wpt_smoke/testdriver_vendor.js"))
        .unwrap();
    runtime
        .eval(
            r#"
            globalThis.notifications = 0;
            globalThis.reportObserver = new ReportingObserver(() => notifications++);
            reportObserver.observe();
            globalThis.commandResolved = false;
            test_driver_internal.generate_test_report('quoted "message"')
                .then(() => commandResolved = true);
            globalThis.recordsBeforeHostTick = null;
            Promise.resolve().then(() => recordsBeforeHostTick = reportObserver.takeRecords());
            "#,
        )
        .unwrap();
    assert_eq!(
        runtime.eval("notifications").unwrap().as_number(),
        Some(0.0)
    );
    runtime.run_jobs().unwrap();
    assert_eq!(
        runtime.eval(r#"commandResolved && recordsBeforeHostTick.length === 1 && recordsBeforeHostTick[0].type === 'test' && recordsBeforeHostTick[0].url === 'https://example.test/reporting' && recordsBeforeHostTick[0].body.message === 'quoted "message"' && notifications === 0"#).unwrap().as_boolean(),
        Some(true)
    );
    runtime.eval("finishAutomation(); globalThis.lateCommandResolved = false; test_driver_internal.generate_test_report('after completion').then(() => lateCommandResolved = true);").unwrap();
    runtime.run_jobs().unwrap();
    assert_eq!(
        runtime
            .eval("reportObserver.takeRecords().length === 0 && !lateCommandResolved")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn reporting_task_pump_yields_without_losing_self_scheduled_notifications() {
    let document = TreeBuilder::parse("<!doctype html><body></body>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.enable_reporting_automation().unwrap();
    runtime.set_task_pump_limit(std::num::NonZeroUsize::new(3));
    runtime
        .eval(
            r#"
        globalThis.reportsSeen = 0;
        globalThis.keepReporting = true;
        new ReportingObserver(() => {
            reportsSeen++;
            if (keepReporting) __omoikane_automation_generate_test_report('next');
        }).observe();
        __omoikane_automation_generate_test_report('first');
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.eval("reportsSeen").unwrap().as_number(), Some(3.0));
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.eval("reportsSeen").unwrap().as_number(), Some(6.0));
    runtime.eval("keepReporting = false;").unwrap();
    runtime.set_task_pump_limit(None);
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.eval("reportsSeen").unwrap().as_number(), Some(7.0));
}
