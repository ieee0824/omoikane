//! Page-visible exception reporting contracts for Issue #1289.
use omoikane::js::JsRuntime;

#[test]
fn child_storage_changes_notify_other_active_windows_with_owned_storage() {
    let document = omoikane::html::TreeBuilder::parse(
        "<html><body><iframe id='source'></iframe><iframe id='sibling'></iframe></body></html>",
    )
    .document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://example.com/").unwrap();
    let result = runtime.eval(r#"(() => {
        const source = document.querySelector('#source').contentWindow;
        const siblingFrame = document.querySelector('#sibling');
        const sibling = siblingFrame.contentWindow;
        const parentEvents = [], siblingEvents = [], sourceEvents = [];
        function observe(target, events) {
            target.addEventListener('storage', event => {
                const area = event.key === 'local-key' ? target.localStorage : target.sessionStorage;
                events.push([event.key, event.newValue, event.storageArea === area].join('|'));
            });
        }
        observe(window, parentEvents);
        observe(sibling, siblingEvents);
        observe(source, sourceEvents);
        source.localStorage.setItem('local-key', 'first');
        source.sessionStorage.setItem('session-key', 'first');
        const active = parentEvents.join(',') === 'local-key|first|true,session-key|first|true' &&
            siblingEvents.join(',') === parentEvents.join(',') && sourceEvents.length === 0;
        siblingFrame.remove();
        source.localStorage.setItem('local-key', 'second');
        return JSON.stringify({active, parent: parentEvents, sibling: siblingEvents, source: sourceEvents});
    })()"#).unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(
        result,
        r#"{"active":true,"parent":["local-key|first|true","session-key|first|true","local-key|second|true"],"sibling":["local-key|first|true","session-key|first|true"],"source":[]}"#
    );
}

#[test]
fn event_listener_runtime_limit_aborts_without_dispatching_a_page_error() {
    use omoikane::js::SandboxConfig;
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    for (asynchronous, listener) in [false, true].into_iter().flat_map(|asynchronous| {
        [
            "() => { while (true) {} }",
            "{ get handleEvent() { while (true) {} } }",
        ]
        .into_iter()
        .map(move |listener| (asynchronous, listener))
    }) {
        let document =
            omoikane::html::TreeBuilder::parse("<!doctype html><body></body>").document();
        let mut runtime = JsRuntime::with_document_and_sandbox(
            document,
            SandboxConfig {
                timeout: std::time::Duration::from_secs(1),
                max_loop_iterations: 16,
            },
        )
        .unwrap();
        runtime.eval(&format!(
            "globalThis.pageErrors=0; globalThis.caught=false; globalThis.beforeEvent=window.event; addEventListener('error',()=>pageErrors++); addEventListener('runaway', {listener});"
        )).unwrap();
        let source = "try { dispatchEvent(new Event('runaway')); } catch (_) { caught=true; }";
        let result = if asynchronous {
            let mut future = std::pin::pin!(runtime.eval_async(source));
            let mut result = None;
            for _ in 0..32 {
                if let Poll::Ready(value) = future
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                {
                    result = Some(value);
                    break;
                }
            }
            result.expect("finite listener dispatch must finish within bounded polls")
        } else {
            runtime.eval(source)
        };
        let error = result.expect_err("host execution limit must abort listener dispatch");
        assert!(
            error
                .as_native()
                .is_some_and(|error| error.is_runtime_limit())
        );
        let observed = runtime
            .eval("JSON.stringify([pageErrors, caught, window.event === beforeEvent])")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(
            observed, "[0,false,true]",
            "{listener}, asynchronous={asynchronous}"
        );
        assert_eq!(runtime.eval("2+3").unwrap().as_number(), Some(5.0));
    }
}

#[test]
fn catch_installed_during_unhandled_event_does_not_emit_handled_event() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        globalThis.log = [];
        window.addEventListener('unhandledrejection', event => {
            log.push('unhandled');
            event.promise.catch(() => {});
            event.preventDefault();
        });
        window.addEventListener('rejectionhandled', () => log.push('handled'));
        Promise.reject(42);
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("log.join(',') === 'unhandled'")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn promise_rejection_notifications_are_tasks_and_keep_promise_identity() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        globalThis.rejections = [];
        globalThis.reason = {message: 'rejected'};
        window.addEventListener('unhandledrejection', event => {
            rejections.push(event.promise === pending && event.reason === reason &&
                event instanceof PromiseRejectionEvent && event.cancelable ? 'unhandled' : 'wrong');
            event.preventDefault();
        });
        window.addEventListener('rejectionhandled', event => {
            rejections.push(event.promise === pending && event.reason === reason &&
                !event.cancelable ? 'handled' : 'wrong');
        });
        globalThis.pending = Promise.reject(reason);
    "#,
        )
        .unwrap();
    runtime.run_jobs().unwrap();
    assert_eq!(
        runtime
            .eval("rejections.length === 0")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("rejections.join(',') === 'unhandled'")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    runtime.eval("pending.catch(() => {});").unwrap();
    runtime.run_jobs().unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("rejections.join(',') === 'unhandled,handled'")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn rejection_handled_in_same_checkpoint_never_notifies() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        globalThis.notifications = 0;
        window.addEventListener('unhandledrejection', () => notifications++);
        window.addEventListener('rejectionhandled', () => notifications++);
        const promise = Promise.reject(123);
        queueMicrotask(() => promise.catch(() => {}));
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime.eval("notifications === 0").unwrap().as_boolean(),
        Some(true)
    );
}

#[test]
fn host_error_reporting_ignores_author_dispatch_override() {
    let mut runtime = JsRuntime::new().unwrap();
    let value = runtime
        .eval(
            r#"(() => {
        let observed = false;
        window.addEventListener('error', event => {
            observed = event.isTrusted && event.error === 42;
            event.preventDefault();
        });
        window.dispatchEvent = () => { throw new Error('author override'); };
        reportError(42);
        return observed;
    })()"#,
        )
        .unwrap();
    assert_eq!(value.as_boolean(), Some(true));
}

#[test]
fn hostile_error_message_does_not_prevent_reporting_original_value() {
    let mut runtime = JsRuntime::new().unwrap();
    let value = runtime
        .eval(
            r#"(() => {
        let observed = false;
        const error = {get message() {throw 99;}, toString() {throw 100;}};
        window.addEventListener('error', event => {
            observed = event.error === error;
            event.preventDefault();
        });
        reportError(error);
        return observed;
    })()"#,
        )
        .unwrap();
    assert_eq!(value.as_boolean(), Some(true));
}

#[test]
fn report_error_dispatches_synchronously_with_original_value_and_onerror_arguments() {
    let mut runtime = JsRuntime::new().unwrap();
    let value = runtime
        .eval(
            r#"(() => {
        const error = new Error('reported');
        const log = [];
        window.onerror = function(message, filename, line, column, value) {
            log.push(arguments.length === 5 && message === 'reported' && value === error);
            return true;
        };
        window.addEventListener('error', event => {
            log.push(event instanceof ErrorEvent && event.error === error && event.cancelable);
        });
        const result = reportError(error);
        return result === undefined && log.length === 2 && log.every(Boolean);
    })()"#,
        )
        .unwrap();
    assert_eq!(value.as_boolean(), Some(true));
}

#[test]
fn microtask_exception_reports_error_and_preserves_remaining_job_order() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        globalThis.errorLog = [];
        window.addEventListener('error', event => {
            errorLog.push(event.error === sentinel ? 'error' : 'wrong');
            event.preventDefault();
        });
        globalThis.sentinel = {message: 'microtask'};
        queueMicrotask(() => { errorLog.push('first'); throw sentinel; });
        Promise.resolve().then(() => errorLog.push('promise'));
        queueMicrotask(() => errorLog.push('last'));
    "#,
        )
        .unwrap();
    runtime.run_jobs().unwrap();
    let value = runtime
        .eval("errorLog.join(',') === 'first,error,promise,last'")
        .unwrap();
    assert_eq!(value.as_boolean(), Some(true));
}

#[test]
fn report_error_requires_an_argument_but_accepts_undefined() {
    let mut runtime = JsRuntime::new().unwrap();
    let value = runtime
        .eval(
            r#"(() => {
        let missing = false, observed = false;
        try { reportError(); } catch (error) { missing = error instanceof TypeError; }
        window.addEventListener('error', event => {
            observed = event.error === undefined;
            event.preventDefault();
        });
        reportError(undefined);
        return missing && observed;
    })()"#,
        )
        .unwrap();
    assert_eq!(value.as_boolean(), Some(true));
}

#[test]
fn parent_catch_suppresses_child_realm_rejection_at_the_same_checkpoint() {
    use omoikane::html::TreeBuilder;
    let document = TreeBuilder::parse("<iframe id='child'></iframe>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime
        .eval(
            r#"
        globalThis.notifications = [];
        const child = document.getElementById('child').contentWindow;
        child.addEventListener('unhandledrejection', () => notifications.push('child'));
        window.addEventListener('unhandledrejection', () => notifications.push('parent'));
        const promise = child.Promise.reject('reason');
        promise.catch(() => {});
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("notifications.length === 0")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn timer_callback_exception_reports_original_value_and_keeps_next_task() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        globalThis.log = [];
        globalThis.thrown = {message: 'timer'};
        window.addEventListener('error', event => {
            log.push(event.error === thrown && event.isTrusted ? 'error' : 'wrong');
            event.preventDefault();
        });
        setTimeout(() => { log.push('timer'); throw thrown; }, 0);
        setTimeout(() => log.push('next'), 0);
    "#,
        )
        .unwrap();
    runtime.run_timers(20, 1, 20);
    assert_eq!(
        runtime
            .eval("log.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "timer,error,next"
    );
}

#[test]
fn document_script_exception_reports_to_global_and_keeps_following_script() {
    use omoikane::html::TreeBuilder;
    let doc = TreeBuilder::parse(
        r#"<script>
        globalThis.log = [];
        globalThis.thrown = {message: 'script'};
        window.onerror = (message, filename, line, column, error) => {
            log.push(error === thrown ? 'error' : 'wrong');
            return true;
        };
        throw thrown;
    </script><script>log.push('next');</script>"#,
    )
    .document();
    let mut runtime = JsRuntime::with_document(doc).unwrap();
    runtime.execute_document_scripts(None);
    assert_eq!(
        runtime
            .eval("log.join(',') === 'error,next'")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn worker_report_error_and_microtask_exception_use_worker_global() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        globalThis.workerReports = [];
        const source = `
            const sentinel = {message: 'worker'};
            let phase = 'report';
            self.addEventListener('error', event => {
                postMessage(event.error === sentinel && event.isTrusted &&
                    typeof window === 'undefined' ? phase : 'wrong');
                event.preventDefault();
            });
            self.dispatchEvent = () => { throw 'author override'; };
            reportError(sentinel);
            phase = 'microtask';
            queueMicrotask(() => { throw sentinel; });
        `;
        const worker = new Worker('data:text/javascript,' + encodeURIComponent(source));
        worker.onmessage = event => workerReports.push(event.data);
    "#,
        )
        .unwrap();
    runtime.run_timers(20, 1, 40);
    assert_eq!(
        runtime
            .eval("workerReports.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "report,microtask"
    );
}

#[test]
fn listener_exception_reports_to_the_callbacks_realm_global() {
    use omoikane::html::TreeBuilder;
    let doc =
        TreeBuilder::parse("<button id='target'></button><iframe id='child'></iframe>").document();
    let mut runtime = JsRuntime::with_document(doc).unwrap();
    let value = runtime.eval(r#"(() => {
        const log = [];
        const child = document.getElementById('child').contentWindow;
        child.addEventListener('error', event => {log.push(event.error === 42 ? 'child' : 'wrong'); event.preventDefault();});
        window.addEventListener('error', event => {log.push('parent'); event.preventDefault();});
        const target = document.getElementById('target');
        target.addEventListener('probe', child.Function('throw 42;'));
        target.dispatchEvent(new Event('probe'));
        return log.join(',');
    })()"#).unwrap();
    assert_eq!(value.as_string().unwrap().to_std_string_escaped(), "child");
}

#[test]
fn timer_error_filename_uses_document_url_instead_of_base_element() {
    use omoikane::html::TreeBuilder;
    let doc = TreeBuilder::parse("<base href='https://other.example/base/'>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(doc, "https://page.example/index.html#fragment").unwrap();
    runtime
        .eval(
            r#"
        globalThis.filename = null;
        window.onerror = (message, source) => { filename = source; return true; };
        setTimeout(() => { throw 42; }, 0);
    "#,
        )
        .unwrap();
    runtime.run_timers(20, 1, 20);
    assert_eq!(
        runtime
            .eval("filename")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "https://page.example/index.html#fragment"
    );
}

#[test]
fn exception_source_location_uses_engine_metadata_without_reading_error_properties() {
    let mut context = boa_engine::Context::default();
    let source = "\nconst value = 42;\nthrow value;";
    let error = context
        .eval(boa_engine::Source::from_reader(
            source.as_bytes(),
            Some(std::path::Path::new("/owned/script.js")),
        ))
        .unwrap_err();
    let (filename, line, column) = error
        .source_location()
        .expect("throw has a source position");
    assert_eq!(filename.as_deref(), Some("/owned/script.js"));
    assert_eq!(line, 3);
    assert!(column > 0);
}

#[test]
fn timer_error_line_comes_from_the_throwing_callback() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval("globalThis.position = []; onerror = (message, file, line, column) => { position = [line, column]; return true; };\nsetTimeout(() => {\nthrow 42;\n}, 0);").unwrap();
    runtime.run_timers(20, 1, 20);
    assert_eq!(runtime.eval("position[0]").unwrap().as_number(), Some(3.0));
    assert!(runtime.eval("position[1]").unwrap().as_number().unwrap() > 0.0);
}

#[test]
fn reference_error_source_location_keeps_the_identifier_position() {
    let mut context = boa_engine::Context::default();
    let error = context
        .eval(boa_engine::Source::from_bytes("\n\nmissing_identifier;"))
        .unwrap_err();
    let (_, line, column) = error
        .source_location()
        .expect("reference error has source metadata");
    assert_eq!((line, column), (3, 1));
}

#[test]
fn compiled_callback_keeps_the_embedding_source_start_position() {
    let mut context = boa_engine::Context::default();
    let source = boa_engine::Source::from_bytes("globalThis.later = () => {\nthrow 42;\n};")
        .with_start_position(30, 7);
    context.eval(source).unwrap();
    let error = context
        .eval(boa_engine::Source::from_bytes("later();"))
        .unwrap_err();
    let (_, line, column) = error.source_location().unwrap();
    assert_eq!(line, 31);
    assert!(column > 0);
}

#[test]
fn inline_script_timer_error_keeps_absolute_html_line_and_document_url() {
    use omoikane::html::TreeBuilder;
    let html = "<!doctype html>\n<!-- <script>fake</script> -->\n<script>globalThis.observed = []; onerror = (message, file, line) => { observed = [file, line]; return true; };\nsetTimeout(() => {\nthrow 42;\n}, 0);\n</script>";
    let doc = TreeBuilder::parse(html).document();
    let mut runtime =
        JsRuntime::with_document_and_url(doc, "https://page.example/source.html").unwrap();
    runtime.execute_document_scripts(None);
    runtime.run_timers(20, 1, 20);
    assert_eq!(
        runtime
            .eval("observed[0]")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "https://page.example/source.html"
    );
    assert_eq!(runtime.eval("observed[1]").unwrap().as_number(), Some(5.0));
}

#[test]
fn details_dom_task_can_handle_rejection_before_notification_task() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval(r#"
        globalThis.log = [];
        const promise = Promise.reject(42);
        window.addEventListener('unhandledrejection', event => { log.push('unhandled'); event.preventDefault(); });
        const details = document.createElement('details');
        details.ontoggle = () => { log.push('toggle'); promise.catch(() => {}); };
        details.setAttribute('open', '');
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("log.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "toggle"
    );
}

#[test]
fn parsing_readiness_changes_surround_rejection_notification_before_load() {
    use omoikane::html::TreeBuilder;
    let document = TreeBuilder::parse(r#"<!doctype html><script>
        globalThis.log = [];
        document.addEventListener('readystatechange', () => log.push(document.readyState));
        addEventListener('unhandledrejection', event => { log.push('rejection'); event.preventDefault(); });
        addEventListener('load', () => log.push('load'));
        Promise.reject(42);
    </script>"#).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    assert!(runtime.execute_document_scripts(None).is_empty());
    runtime.fire_load().unwrap();
    assert_eq!(
        runtime
            .eval("log.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "interactive,rejection,complete,load"
    );
}

#[test]
fn rejection_reporting_uses_intrinsic_map_methods() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval(r#"
        globalThis.log = [];
        addEventListener('unhandledrejection', event => { log.push('unhandled'); event.preventDefault(); });
        globalThis.savedMapMethods = [Map.prototype.set, Map.prototype.delete, Map.prototype.clear, Map.prototype.forEach, Map.prototype[Symbol.iterator]];
        globalThis.pending = Promise.reject(42);
        Map.prototype.set = Map.prototype.delete = Map.prototype.clear = Map.prototype.forEach = Map.prototype[Symbol.iterator] = () => { throw new Error('author override'); };
    "#).unwrap();
    let result = runtime.run_until_idle();
    runtime.eval(r#"
        [Map.prototype.set, Map.prototype.delete, Map.prototype.clear, Map.prototype.forEach, Map.prototype[Symbol.iterator]] = savedMapMethods;
    "#).unwrap();
    result.unwrap();
    assert_eq!(
        runtime
            .eval("log.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "unhandled"
    );
}

#[test]
fn rejection_of_child_promise_notifies_child_global() {
    use omoikane::html::TreeBuilder;
    let document = TreeBuilder::parse("<iframe id='child'></iframe>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.eval(r#"
        globalThis.log = [];
        const child = document.getElementById('child').contentWindow;
        child.addEventListener('unhandledrejection', event => { log.push('child'); event.preventDefault(); });
        addEventListener('unhandledrejection', event => { log.push('parent'); event.preventDefault(); });
        child.Promise.reject(42);
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("log.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "child"
    );
}

#[test]
fn child_rejection_from_iframe_load_callback_keeps_active_document() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval(r#"
        globalThis.log = [];
        const iframe = document.createElement('iframe');
        iframe.onload = () => {
            log.push('load');
            const child = iframe.contentWindow;
            child.addEventListener('unhandledrejection', event => { log.push('child'); event.preventDefault(); });
            new child.Promise((resolve, reject) => setTimeout(() => reject(42), 1));
        };
        iframe.srcdoc = '';
        document.documentElement.appendChild(iframe);
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    runtime.run_timers(20, 1, 100);
    assert_eq!(
        runtime
            .eval("log.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "load,child"
    );
}

#[test]
fn worker_import_scripts_evaluates_synchronously_and_keeps_original_throw() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        globalThis.log = [];
        const source = `
            self.sentinel = {message: 'imported'};
            let original = false;
            try { importScripts('data:text/javascript,throw%20sentinel%3B'); }
            catch (error) { original = error === sentinel; }
            importScripts('data:text/javascript,self.imported%20%3D%2042%3B');
            postMessage({original, imported, noArgs: importScripts() === undefined});
        `;
        const worker = new Worker('data:text/javascript,' + encodeURIComponent(source));
        worker.onmessage = event => log.push(event.data);
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.eval("log.length === 1 && log[0].original && log[0].imported === 42 && log[0].noArgs && typeof importScripts === 'undefined'").unwrap().as_boolean(), Some(true));
}

#[test]
fn worker_global_event_methods_accept_unqualified_calls() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        globalThis.log = [];
        const source = `
            'use strict';
            let count = 0;
            const handler = () => count++;
            addEventListener('probe', handler);
            dispatchEvent(new Event('probe'));
            removeEventListener('probe', handler);
            dispatchEvent(new Event('probe'));
            postMessage(count);
        `;
        const worker = new Worker('data:text/javascript,' + encodeURIComponent(source));
        worker.onmessage = event => log.push(event.data);
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("log.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "1"
    );
}

#[test]
fn discarded_iframe_window_keeps_event_cleanup_and_document() {
    use omoikane::html::TreeBuilder;
    let document = TreeBuilder::parse("<iframe id='child'></iframe>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let value = runtime.eval(r#"(() => {
        const iframe = document.getElementById('child');
        const child = iframe.contentWindow;
        const oldDocument = child.document;
        const listener = () => {};
        child.addEventListener('probe', listener);
        iframe.remove();
        child.removeEventListener('probe', listener);
        return child.closed && child.document === oldDocument && child.frameElement === null &&
            typeof child.addEventListener === 'function' && typeof child.dispatchEvent === 'function';
    })()"#).unwrap();
    assert_eq!(value.as_boolean(), Some(true));
}

#[test]
fn worker_first_timer_survives_clearing_a_null_handle() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        globalThis.timerResult = '';
        const source = `
            const id = setTimeout(() => postMessage('fired'), 1);
            clearTimeout(null);
            postMessage(id > 0 ? 'positive' : 'zero');
        `;
        const worker = new Worker('data:text/javascript,' + encodeURIComponent(source));
        worker.onmessage = event => timerResult += event.data + ',';
    "#,
        )
        .unwrap();
    runtime.run_timers(20, 1, 40);
    assert_eq!(
        runtime
            .eval("timerResult")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "positive,fired,"
    );
}

#[path = "support/http_fixture.rs"]
mod http_fixture;

#[test]
fn cross_origin_non_cors_script_mutes_exception_and_promise_rejection() {
    use http_fixture::{
        ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
        read_request_headers,
    };
    use std::io::Write;
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
        read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
        let body = "Promise.reject('private rejection'); throw new Error('private exception');";
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    let document = omoikane::html::TreeBuilder::parse(&format!(
        "<!doctype html><script src='http://{address}/private.js'></script>"
    ))
    .document();
    let page = "http://127.0.0.1:1/page.html";
    let mut runtime = JsRuntime::with_document_and_url(document, page).unwrap();
    runtime.eval(r#"
        globalThis.reports = [];
        addEventListener('error', event => {
            reports.push([event.message, event.filename, event.lineno, event.colno, event.error === null].join('|'));
            event.preventDefault();
        });
        addEventListener('unhandledrejection', event => {
            reports.push('rejection leaked'); event.preventDefault();
        });
    "#).unwrap();
    // A cross-origin base URL allows this local fixture through the existing
    // transport policy; the document security origin remains `page`.
    let resource_base = format!("http://{address}/").parse().unwrap();
    let errors = runtime.execute_document_scripts(Some(&resource_base));
    runtime.run_until_idle().unwrap();
    server.join();
    assert!(
        !errors.iter().any(|error| error.contains("failed to fetch")),
        "{errors:?}"
    );
    assert_eq!(
        runtime
            .eval("reports.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "Script error.||0|0|true"
    );
}

#[test]
fn retained_callback_keeps_its_scripts_owned_host_metadata() {
    use boa_engine::HostDefined;
    use boa_engine::{Context, NativeFunction, Script, Source, js_string};
    #[derive(Debug, boa_gc::Trace, boa_gc::Finalize, boa_engine::JsData)]
    struct ScriptMetadata {
        muted: bool,
    }
    let mut context = Context::default();
    context
        .register_global_callable(
            js_string!("readMetadata"),
            0,
            NativeFunction::from_fn_ptr(|_, _, context| {
                let muted = context.active_script().and_then(|script| {
                    script
                        .host_defined()
                        .get::<ScriptMetadata>()
                        .map(|metadata| metadata.muted)
                });
                Ok(muted.unwrap_or(false).into())
            }),
        )
        .unwrap();
    let mut metadata = HostDefined::default();
    metadata.insert(ScriptMetadata { muted: true });
    let script = Script::parse_with_host_defined(
        Source::from_bytes("globalThis.retained = () => readMetadata();"),
        None,
        metadata,
        &mut context,
    )
    .unwrap();
    script.evaluate(&mut context).unwrap();
    drop(script);
    boa_gc::force_collect();
    assert_eq!(
        context
            .eval(Source::from_bytes("retained()"))
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    assert_eq!(
        context
            .eval(Source::from_bytes("readMetadata()"))
            .unwrap()
            .as_boolean(),
        Some(false)
    );
}

#[test]
fn same_script_url_keeps_distinct_cors_metadata_in_retained_callbacks() {
    use http_fixture::{
        ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
        read_request_headers,
    };
    use std::io::Write;
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        let mut requests = Vec::new();
        for _ in 0..2 {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            requests.push(read_request_headers(&mut stream, READ_TIMEOUT).unwrap());
            let body = "callbacks.push(() => Promise.reject('reason'));";
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        }
        requests
    });
    let document = omoikane::html::TreeBuilder::parse(&format!(
        "<!doctype html><script src='http://{address}/same.js'></script><script crossorigin='anonymous' src='http://{address}/same.js'></script>"
    )).document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://127.0.0.1:1/page").unwrap();
    runtime.eval("globalThis.callbacks=[];globalThis.reports=[];addEventListener('unhandledrejection',e=>{reports.push(e.reason);e.preventDefault()});").unwrap();
    let base = format!("http://{address}/").parse().unwrap();
    assert!(runtime.execute_document_scripts(Some(&base)).is_empty());
    let requests = server.join();
    assert!(!requests[0].to_ascii_lowercase().contains("origin:"));
    assert!(
        requests[1]
            .to_ascii_lowercase()
            .contains("origin: http://127.0.0.1:1")
    );
    assert_eq!(
        runtime.eval("callbacks.length").unwrap().as_number(),
        Some(2.0)
    );
    runtime.eval("callbacks[0]();callbacks[1]();").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("reports.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "reason"
    );
}

#[test]
fn cross_origin_cors_script_without_permission_does_not_execute() {
    use http_fixture::{
        ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
        read_request_headers,
    };
    use std::io::Write;
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
        read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
        let body = "globalThis.privateExecuted=true;";
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    let document = omoikane::html::TreeBuilder::parse(&format!(
        "<!doctype html><script crossorigin src='http://{address}/denied.js'></script>"
    ))
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://127.0.0.1:1/page").unwrap();
    let base = format!("http://{address}/").parse().unwrap();
    let errors = runtime.execute_document_scripts(Some(&base));
    server.join();
    assert!(
        errors.iter().any(|error| error.contains("failed to fetch")),
        "{errors:?}"
    );
    assert_eq!(
        runtime
            .eval("typeof privateExecuted")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "undefined"
    );
}

#[test]
fn cross_origin_non_cors_timer_exception_keeps_muted_provenance() {
    use http_fixture::{
        ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
        read_request_headers,
    };
    use std::io::Write;
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
        read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
        let body = "setTimeout(() => { throw new Error('private timer'); }, 1);";
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    let document = omoikane::html::TreeBuilder::parse(&format!(
        "<!doctype html><script src='http://{address}/private.js'></script>"
    ))
    .document();
    let page = "http://127.0.0.1:1/page.html";
    let mut runtime = JsRuntime::with_document_and_url(document, page).unwrap();
    runtime.eval(r#"
        globalThis.reports = [];
        addEventListener('error', event => {
            reports.push([event.message, event.filename, event.lineno, event.colno, event.error === null].join('|'));
            event.preventDefault();
        });
        addEventListener('unhandledrejection', event => {
            reports.push('rejection leaked'); event.preventDefault();
        });
    "#).unwrap();
    // A cross-origin base URL allows this local fixture through the existing
    // transport policy; the document security origin remains `page`.
    let resource_base = format!("http://{address}/").parse().unwrap();
    let errors = runtime.execute_document_scripts(Some(&resource_base));
    runtime.run_timers(20, 1, 40);
    server.join();
    assert!(
        !errors.iter().any(|error| error.contains("failed to fetch")),
        "{errors:?}"
    );
    assert_eq!(
        runtime
            .eval("reports.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "Script error.||0|0|true"
    );
}

#[test]
fn retained_native_error_keeps_source_metadata_after_script_collection() {
    use boa_engine::{Context, HostDefined, Script, Source};
    #[derive(Debug, boa_gc::Trace, boa_gc::Finalize, boa_engine::JsData)]
    struct Marker {
        value: bool,
    }
    let mut context = Context::default();
    let mut metadata = HostDefined::default();
    metadata.insert(boa_engine::error::ScriptErrorMetadata::new(Marker {
        value: true,
    }));
    let script = Script::parse_with_host_defined(
        Source::from_bytes("throw 42;"),
        None,
        metadata,
        &mut context,
    )
    .unwrap();
    let error = script.evaluate(&mut context).unwrap_err();
    drop(script);
    drop(context);
    boa_gc::force_collect();
    assert!(
        error
            .source_script_metadata()
            .unwrap()
            .get::<Marker>()
            .unwrap()
            .value
    );
}

#[test]
fn cross_origin_non_cors_deferred_script_reports_muted_exception() {
    use http_fixture::{
        ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
        read_request_headers,
    };
    use std::io::Write;
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
        read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
        let body = "Promise.reject('private rejection'); throw new Error('private exception');";
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    let document = omoikane::html::TreeBuilder::parse(&format!(
        "<!doctype html><script defer src='http://{address}/private.js'></script>"
    ))
    .document();
    let page = "http://127.0.0.1:1/page.html";
    let mut runtime = JsRuntime::with_document_and_url(document, page).unwrap();
    runtime.eval(r#"
        globalThis.reports = [];
        addEventListener('error', event => {
            reports.push([event.message, event.filename, event.lineno, event.colno, event.error === null].join('|'));
            event.preventDefault();
        });
        addEventListener('unhandledrejection', event => {
            reports.push('rejection leaked'); event.preventDefault();
        });
    "#).unwrap();
    // A cross-origin base URL allows this local fixture through the existing
    // transport policy; the document security origin remains `page`.
    let resource_base = format!("http://{address}/").parse().unwrap();
    let errors = runtime.execute_document_scripts(Some(&resource_base));
    runtime.run_until_idle().unwrap();
    server.join();
    assert!(
        !errors.iter().any(|error| error.contains("failed to fetch")),
        "{errors:?}"
    );
    assert_eq!(
        runtime
            .eval("reports.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "Script error.||0|0|true"
    );
}

#[test]
fn cross_origin_non_cors_listener_exception_keeps_muted_provenance() {
    use http_fixture::{
        ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
        read_request_headers,
    };
    use std::io::Write;
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
        read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
        let body = "addEventListener('probe', () => { throw new Error('private listener'); });";
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    let document = omoikane::html::TreeBuilder::parse(&format!(
        "<!doctype html><script src='http://{address}/private.js'></script>"
    ))
    .document();
    let page = "http://127.0.0.1:1/page.html";
    let mut runtime = JsRuntime::with_document_and_url(document, page).unwrap();
    runtime.eval(r#"
        globalThis.reports = [];
        addEventListener('error', event => {
            reports.push([event.message, event.filename, event.lineno, event.colno, event.error === null].join('|'));
            event.preventDefault();
        });
        addEventListener('unhandledrejection', event => {
            reports.push('rejection leaked'); event.preventDefault();
        });
    "#).unwrap();
    // A cross-origin base URL allows this local fixture through the existing
    // transport policy; the document security origin remains `page`.
    let resource_base = format!("http://{address}/").parse().unwrap();
    let errors = runtime.execute_document_scripts(Some(&resource_base));
    runtime.eval("dispatchEvent(new Event(\"probe\"))").unwrap();
    runtime.run_until_idle().unwrap();
    server.join();
    assert!(
        !errors.iter().any(|error| error.contains("failed to fetch")),
        "{errors:?}"
    );
    assert_eq!(
        runtime
            .eval("reports.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "Script error.||0|0|true"
    );
}

#[test]
fn cross_origin_non_cors_microtask_exception_keeps_muted_provenance() {
    use http_fixture::{
        ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
        read_request_headers,
    };
    use std::io::Write;
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
        read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
        let body = "queueMicrotask(() => { throw new Error('private microtask'); });";
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    let document = omoikane::html::TreeBuilder::parse(&format!(
        "<!doctype html><script src='http://{address}/private.js'></script>"
    ))
    .document();
    let page = "http://127.0.0.1:1/page.html";
    let mut runtime = JsRuntime::with_document_and_url(document, page).unwrap();
    runtime.eval(r#"
        globalThis.reports = [];
        addEventListener('error', event => {
            reports.push([event.message, event.filename, event.lineno, event.colno, event.error === null].join('|'));
            event.preventDefault();
        });
        addEventListener('unhandledrejection', event => {
            reports.push('rejection leaked'); event.preventDefault();
        });
    "#).unwrap();
    // A cross-origin base URL allows this local fixture through the existing
    // transport policy; the document security origin remains `page`.
    let resource_base = format!("http://{address}/").parse().unwrap();
    let errors = runtime.execute_document_scripts(Some(&resource_base));
    runtime.run_until_idle().unwrap();
    server.join();
    assert!(
        !errors.iter().any(|error| error.contains("failed to fetch")),
        "{errors:?}"
    );
    assert_eq!(
        runtime
            .eval("reports.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "Script error.||0|0|true"
    );
}

#[test]
fn cross_origin_non_cors_report_error_keeps_muted_provenance() {
    use http_fixture::{
        ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
        read_request_headers,
    };
    use std::io::Write;
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
        read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
        let body = "reportError(new Error('private report')); ";
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    let document = omoikane::html::TreeBuilder::parse(&format!(
        "<!doctype html><script src='http://{address}/private.js'></script>"
    ))
    .document();
    let page = "http://127.0.0.1:1/page.html";
    let mut runtime = JsRuntime::with_document_and_url(document, page).unwrap();
    runtime.eval(r#"
        globalThis.reports = [];
        addEventListener('error', event => {
            reports.push([event.message, event.filename, event.lineno, event.colno, event.error === null].join('|'));
            event.preventDefault();
        });
        addEventListener('unhandledrejection', event => {
            reports.push('rejection leaked'); event.preventDefault();
        });
    "#).unwrap();
    // A cross-origin base URL allows this local fixture through the existing
    // transport policy; the document security origin remains `page`.
    let resource_base = format!("http://{address}/").parse().unwrap();
    let errors = runtime.execute_document_scripts(Some(&resource_base));
    runtime.run_until_idle().unwrap();
    server.join();
    assert!(
        !errors.iter().any(|error| error.contains("failed to fetch")),
        "{errors:?}"
    );
    assert_eq!(
        runtime
            .eval("reports.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "Script error.||0|0|true"
    );
}

#[test]
fn cross_origin_non_cors_dynamic_script_mutes_exception_and_rejection() {
    use http_fixture::{
        ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
        read_request_headers,
    };
    use std::io::Write;
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
        read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
        let body = "Promise.reject('private rejection'); throw new Error('private exception');";
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    let document = omoikane::html::TreeBuilder::parse(&format!(
        "<!doctype html><base href='http://{address}/'><body></body>"
    ))
    .document();
    let page = "http://127.0.0.1:1/page.html";
    let mut runtime = JsRuntime::with_document_and_url(document, page).unwrap();
    runtime.eval(r#"
        globalThis.reports = [];
        addEventListener('error', event => {
            reports.push([event.message, event.filename, event.lineno, event.colno, event.error === null].join('|'));
            event.preventDefault();
        });
        addEventListener('unhandledrejection', event => {
            reports.push('rejection leaked'); event.preventDefault();
        });
    "#).unwrap();
    // A cross-origin base URL allows this local fixture through the existing
    // transport policy; the document security origin remains `page`.
    runtime.set_base_url(format!("http://{address}/").parse().unwrap());
    runtime.eval(&format!("const script=document.createElement('script');script.src='http://{address}/private.js';document.body.appendChild(script);")).unwrap();
    runtime.run_timers(20, 1, 40);
    server.join();
    assert_eq!(
        runtime
            .eval("reports.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "Script error.||0|0|true"
    );
}

#[test]
fn worker_onerror_cancellation_prevents_owner_error_forwarding() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval(r#"
        globalThis.localReports = [];
        globalThis.ownerReports = [];
        for (const cancel of [true, false]) {
            const source = `onerror = function() { postMessage('local'); return ${cancel}; }; setTimeout(() => { throw new Error('timer'); }, 1);`;
            const worker = new Worker('data:text/javascript,' + encodeURIComponent(source));
            worker.onmessage = event => localReports.push(event.data);
            worker.onerror = event => { ownerReports.push(cancel); event.preventDefault(); };
        }
    "#).unwrap();
    runtime.run_timers(20, 1, 80);
    assert_eq!(
        runtime.eval("localReports.length").unwrap().as_number(),
        Some(2.0)
    );
    assert_eq!(
        runtime
            .eval("ownerReports.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "false"
    );
}

#[test]
fn worker_report_error_and_microtask_forward_only_uncanceled_reports() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval(r#"
        globalThis.localReports = [];
        globalThis.ownerReports = [];
        for (const cancel of [true, false]) {
            const source = `onerror = function() { postMessage('local'); return ${cancel}; }; reportError(new Error('explicit')); queueMicrotask(() => { throw new Error('microtask'); });`;
            const worker = new Worker('data:text/javascript,' + encodeURIComponent(source));
            worker.onmessage = event => localReports.push(event.data);
            worker.onerror = event => { ownerReports.push(cancel); event.preventDefault(); };
        }
    "#).unwrap();
    runtime.run_timers(20, 1, 80);
    assert_eq!(
        runtime.eval("localReports.length").unwrap().as_number(),
        Some(4.0)
    );
    assert_eq!(
        runtime
            .eval("ownerReports.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "false,false"
    );
}

#[test]
fn microtask_runtime_limit_aborts_without_dispatching_a_page_error() {
    use omoikane::js::SandboxConfig;
    let doc = omoikane::html::TreeBuilder::parse("<!doctype html><body></body>").document();
    let mut runtime = JsRuntime::with_document_and_sandbox(
        doc,
        SandboxConfig {
            timeout: std::time::Duration::from_secs(1),
            max_loop_iterations: 16,
        },
    )
    .unwrap();
    runtime.eval("globalThis.pageErrors=0;addEventListener('error',()=>pageErrors++);queueMicrotask(()=>{while(true){}});").unwrap();
    let error = runtime
        .run_until_idle()
        .expect_err("runaway microtask must abort");
    assert!(matches!(
        error.as_native().unwrap().kind,
        boa_engine::JsNativeErrorKind::RuntimeLimit
    ));
    assert_eq!(runtime.eval("pageErrors").unwrap().as_number(), Some(0.0));
    assert_eq!(runtime.eval("2+3").unwrap().as_number(), Some(5.0));
}

#[test]
fn boa_runtime_abort_does_not_resume_stale_native_continuations() {
    use boa_engine::{
        Context, JsValue, NativeFunction, Source, js_string,
        native_function::NativeCallContinuation,
    };
    let mut context = Context::default();
    context.runtime_limits_mut().set_loop_iteration_limit(16);
    context
        .register_global_builtin_callable(
            js_string!("nativeBridge"),
            1,
            NativeFunction::from_copy_closure(|_, args, context| {
                let callback = args[0].as_callable().expect("callable fixture");
                context.call_with_native_continuation(
                    &callback,
                    &JsValue::undefined(),
                    &[],
                    NativeCallContinuation::from_copy_closure_with_captures(
                        |result, (), context| {
                            context.global_object().set(
                                js_string!("continued"),
                                true,
                                true,
                                context,
                            )?;
                            result
                        },
                        (),
                    ),
                )
            }),
        )
        .unwrap();
    let error = context
        .eval(Source::from_bytes(
            "globalThis.continued = false; nativeBridge(() => { while (true) {} });",
        ))
        .expect_err("runaway callback must abort");
    assert!(
        error
            .as_native()
            .is_some_and(|error| error.is_runtime_limit())
    );
    assert_eq!(
        context
            .eval(Source::from_bytes("(function ordinary() { return 42; })()"))
            .unwrap(),
        JsValue::from(42)
    );
    assert_eq!(
        context.eval(Source::from_bytes("continued")).unwrap(),
        JsValue::from(false),
        "an aborted callback's continuation must never run in a later script"
    );
    assert_eq!(
        context
            .eval(Source::from_bytes("nativeBridge(() => 5)"))
            .unwrap(),
        JsValue::from(5)
    );
    assert_eq!(
        context.eval(Source::from_bytes("continued")).unwrap(),
        JsValue::from(true)
    );
}

#[test]
fn boa_promise_reactions_preserve_runtime_abort_in_sync_and_async_jobs() {
    use boa_engine::{Context, JsValue, Source};
    use std::{
        future::Future,
        task::{Context as TaskContext, Poll, Waker},
    };

    fn run_jobs(context: &mut Context, asynchronous: bool) -> boa_engine::JsResult<()> {
        if asynchronous {
            let mut jobs = std::pin::pin!(context.run_jobs_async());
            // The executor yields between jobs even without author suspension.
            for _ in 0..32 {
                if let Poll::Ready(result) = jobs
                    .as_mut()
                    .poll(&mut TaskContext::from_waker(Waker::noop()))
                {
                    return result;
                }
            }
            panic!("the finite promise chain did not finish");
        } else {
            context.run_jobs()
        }
    }

    for (asynchronous, expression) in [false, true].into_iter().flat_map(|mode| {
        [
            "Promise.resolve().then(() => { while (true) {} })",
            "Promise.resolve({ then() { while (true) {} } })",
            "Promise.resolve({ get then() { while (true) {} } })",
            "new Promise(() => { while (true) {} })",
            "Promise.try(() => { while (true) {} })",
            "Promise.all({ [Symbol.iterator]() { return { next() { while (true) {} } }; } })",
            "Promise.race({ [Symbol.iterator]() { return { next() { while (true) {} } }; } })",
            "Promise.any({ [Symbol.iterator]() { return { next() { while (true) {} } }; } })",
            "Promise.allSettled({ [Symbol.iterator]() { return { next() { while (true) {} } }; } })",
        ]
        .into_iter()
        .map(move |source| (mode, source))
    }) {
        let mut context = Context::default();
        context.runtime_limits_mut().set_loop_iteration_limit(16);
        let source =
            format!("globalThis.caught = false; {expression}.catch(() => {{ caught = true; }});");
        let evaluation = context.eval(Source::from_bytes(&source));
        let result = evaluation.and_then(|_| run_jobs(&mut context, asynchronous));
        let error = result.expect_err("host execution limit must abort the job");
        assert!(
            error
                .as_native()
                .is_some_and(|error| error.is_runtime_limit())
        );
        assert_eq!(
            context.eval(Source::from_bytes("caught")).unwrap(),
            JsValue::from(false)
        );
        assert_eq!(
            context.eval(Source::from_bytes("2 + 3")).unwrap(),
            JsValue::from(5)
        );

        // Ordinary JavaScript throws still reject, preserving the original value.
        let ordinary = expression.replace("while (true) {}", "throw globalThis.sentinel;");
        let source = format!(
            "globalThis.sentinel = {{ marker: 1 }}; globalThis.reason = undefined; {ordinary}.catch(value => {{ reason = value; }});"
        );
        context.eval(Source::from_bytes(&source)).unwrap();
        run_jobs(&mut context, asynchronous).unwrap();
        assert_eq!(
            context.eval(Source::from_bytes("reason === sentinel")).unwrap(),
            JsValue::from(true),
            "{expression}, asynchronous={asynchronous}"
        );
    }
}

#[test]
fn boa_direct_callback_preserves_throw_source_location() {
    use boa_engine::{Context, JsValue, Source};
    use std::path::Path;
    for filename in [None, Some(Path::new("/owned/timer.js"))] {
        let mut context = Context::default();
        let callback = context
            .eval(Source::from_reader(
                b"globalThis.callback = () => {\n throw new Error('timer');\n}; callback;"
                    .as_slice(),
                filename,
            ))
            .unwrap()
            .as_callable()
            .unwrap();
        let error = callback
            .call(&JsValue::undefined(), &[], &mut context)
            .unwrap_err();
        let (actual_filename, line, column) = error
            .source_location()
            .unwrap_or_else(|| panic!("source location missing: {error:#?}"));
        assert_eq!(actual_filename.as_deref(), filename.and_then(Path::to_str));
        assert_eq!(line, 2, "{error}");
        assert!(column > 0, "{error}");
    }
}

#[test]
fn worker_owner_preserves_runtime_error_message_and_source_location() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        globalThis.workerReports = [];
        globalThis.workerCoordinates = [];
        const source = `reportError('explicit');
queueMicrotask(() => { throw new Error('microtask'); });
setTimeout(() => { throw new Error('timer'); }, 1);
addEventListener('message', () => { throw new Error('listener'); });`;
        const url = 'data:text/javascript,' + encodeURIComponent(source);
        const worker = new Worker(url);
        worker.onerror = event => {
            workerCoordinates.push([event.message, event.lineno, event.colno]);
            const coordinates = event.message === 'explicit'
                ? event.lineno >= 0 && event.colno >= 0 : event.lineno > 0 && event.colno > 0;
            workerReports.push([event.message, event.filename === url, coordinates,
                event.error === null, event.isTrusted && event.cancelable].join('|'));
            event.preventDefault();
        };
        worker.postMessage('trigger');
    "#,
        )
        .unwrap();
    runtime.run_timers(30, 1, 100);
    let observed = runtime
        .eval("workerReports.sort().join(',')")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    let coordinates = runtime
        .eval("JSON.stringify(workerCoordinates)")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        observed,
        "explicit|true|true|true|true,listener|true|true|true|true,microtask|true|true|true|true,timer|true|true|true|true",
        "{coordinates}"
    );
}

#[test]
fn worker_owner_receives_original_utf16_message_without_repeating_getter() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval(r#"
        globalThis.localReads = [];
        globalThis.ownerReport = null;
        const source = `let reads = 0;
const error = { get message() { reads++; return String.fromCharCode(0xd800); } };
reportError(error);
postMessage(reads);`;
        const url = 'data:text/javascript,' + encodeURIComponent(source);
        const worker = new Worker(url);
        worker.onmessage = event => localReads.push(event.data);
        worker.onerror = event => {
            ownerReport = [event.message.length, event.message.charCodeAt(0), event.filename === url];
            event.preventDefault();
        };
    "#).unwrap();
    runtime.run_timers(20, 1, 60);
    assert_eq!(
        runtime
            .eval("JSON.stringify([localReads, ownerReport])")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[[1],[1,55296,true]]"
    );
}

#[test]
fn shared_worker_timer_uses_owner_clock_once_for_multiple_connections() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        globalThis.sharedTimerReplies = [];
        const source = `let connections = 0;
            onconnect = event => {
                connections++;
                const port = event.ports[0];
                if (connections === 1) setTimeout(() => port.postMessage('deadline'), 10);
            };`;
        const url = 'data:text/javascript,' + encodeURIComponent(source);
        for (let i = 0; i < 2; i++) {
            const worker = new SharedWorker(url, 'shared-clock');
            worker.port.onmessage = event => sharedTimerReplies.push(event.data);
        }
    "#,
        )
        .unwrap();
    runtime.tick(6).unwrap();
    assert_eq!(
        runtime
            .eval("sharedTimerReplies.length")
            .unwrap()
            .as_number(),
        Some(0.0)
    );
    runtime.tick(4).unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(sharedTimerReplies)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[\"deadline\"]"
    );
    runtime.tick(10).unwrap();
    assert_eq!(
        runtime
            .eval("sharedTimerReplies.length")
            .unwrap()
            .as_number(),
        Some(1.0)
    );
}

#[test]
fn iframe_worker_load_error_uses_the_owner_event_realm() {
    let document = omoikane::html::TreeBuilder::parse(
        "<html><body><iframe id='frame'></iframe></body></html>",
    )
    .document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://example.com/").unwrap();
    runtime
        .eval(
            r#"
        globalThis.child = document.getElementById('frame').contentWindow;
        child.eval(`globalThis.loadEvents = [];
            for (const Constructor of [Worker, SharedWorker]) {
                const worker = new Constructor('data:text/plain,not-a-script');
                worker.onerror = event => loadEvents.push([event.constructor === Event,
                    event instanceof Event, event.target === worker, event.isTrusted]);
            }
        `);
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(child.loadEvents)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[[true,true,true,true],[true,true,true,true]]"
    );
}

#[test]
fn worker_parse_failure_dispatches_load_error_without_executing_the_script() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        globalThis.parseErrors = [];
        globalThis.parseMessages = [];
        const source = "postMessage('must not execute'); const = ;";
        for (const Constructor of [Worker, SharedWorker]) {
            const worker = new Constructor('data:text/javascript,' + encodeURIComponent(source));
            worker.onerror = event => parseErrors.push([event.constructor === Event,
                !(event instanceof ErrorEvent), !event.cancelable, event.isTrusted]);
            if (Constructor === Worker) worker.onmessage = event => parseMessages.push(event.data);
            else worker.port.onmessage = event => parseMessages.push(event.data);
        }
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify([parseErrors, parseMessages])")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[[[true,true,true,true],[true,true,true,true]],[]]"
    );
}

#[test]
fn worker_runtime_thrown_syntax_error_keeps_the_message_loop() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval(r#"
        globalThis.syntaxReports = [];
        globalThis.syntaxReplies = [];
        const source = "onmessage = event => postMessage(event.data); throw new SyntaxError('runtime syntax');";
        const worker = new Worker('data:text/javascript,' + encodeURIComponent(source));
        worker.onerror = event => {
            syntaxReports.push([event instanceof ErrorEvent, event.message, event.cancelable]);
            event.preventDefault();
        };
        worker.onmessage = event => syntaxReplies.push(event.data);
        worker.postMessage('alive');
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify([syntaxReports, syntaxReplies])")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[[[true,\"runtime syntax\",true]],[\"alive\"]]"
    );
}

#[test]
fn worker_fetch_failure_dispatches_plain_trusted_non_cancelable_event() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        globalThis.loadErrors = [];
        for (const Constructor of [Worker, SharedWorker]) {
            const worker = new Constructor('data:text/plain,not-a-script');
            worker.onerror = event => {
                event.preventDefault();
                loadErrors.push([event.constructor === Event, !(event instanceof ErrorEvent),
                    event.type === 'error', event.isTrusted, !event.cancelable,
                    !event.defaultPrevented, event.target === worker, event.message === undefined]);
            };
        }
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(loadErrors)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[[true,true,true,true,true,true,true,true],[true,true,true,true,true,true,true,true]]"
    );
}

#[test]
fn shared_worker_startup_exception_uses_global_handler_and_keeps_connect_listener() {
    for canceled in [false, true] {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.eval(&format!(r#"
            globalThis.sharedReplies = [];
            globalThis.sharedOwnerErrors = 0;
            const source = `const reports = [];
onerror = (message, filename, line, column, error) => {{ reports.push([message, filename === location.href, line === 4, column > 0, error.message === 'shared startup']); return {canceled}; }};
onconnect = event => {{ const port = event.ports[0]; port.postMessage(reports); port.onmessage = event => port.postMessage('alive:' + event.data); }};
throw new Error('shared startup');`;
            const worker = new SharedWorker('data:text/javascript,' + encodeURIComponent(source), 'startup-' + {canceled});
            worker.onerror = () => sharedOwnerErrors++;
            worker.port.onmessage = event => sharedReplies.push(event.data);
            worker.port.postMessage('ping');
        "#)).unwrap();
        runtime.run_until_idle().unwrap();
        let observed = runtime
            .eval("JSON.stringify([sharedOwnerErrors, sharedReplies])")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(
            observed, r#"[0,[[["shared startup",true,true,true,true]],"alive:ping"]]"#,
            "canceled={canceled}"
        );
    }
}

#[test]
fn worker_startup_exception_keeps_message_listener_and_reports_original_details() {
    for canceled in [false, true] {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.eval(&format!(r#"
            globalThis.startupReports = [];
            globalThis.startupReplies = [];
            const source = `addEventListener('message', event => postMessage('alive:' + event.data));
onerror = () => {{ postMessage('local'); return {canceled}; }};
throw new Error('startup');`;
            const url = 'data:text/javascript,' + encodeURIComponent(source);
            const worker = new Worker(url);
            worker.onmessage = event => startupReplies.push(event.data);
            worker.onerror = event => {{
                startupReports.push([event.message, event.filename === url,
                    event.lineno === 3, event.colno > 0, event.error === null].join('|'));
                event.preventDefault();
            }};
            worker.postMessage('ping');
        "#)).unwrap();
        runtime.run_timers(30, 1, 100);
        let observed = runtime
            .eval("JSON.stringify([startupReports, startupReplies])")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        let expected = if canceled {
            r#"[[],["local","alive:ping"]]"#
        } else {
            r#"[["startup|true|true|true|true"],["local","alive:ping"]]"#
        };
        assert_eq!(observed, expected, "canceled={canceled}");
    }
}

#[test]
fn worker_error_queued_before_startup_close_reaches_its_owner() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval(r#"
        globalThis.ownerReports = [];
        const worker = new Worker('data:text/javascript,' + encodeURIComponent("reportError(new Error('before close')); close();"));
        worker.onerror = event => { ownerReports.push(event.message); event.preventDefault(); };
    "#).unwrap();
    runtime.run_timers(20, 1, 60);
    assert_eq!(
        runtime
            .eval("JSON.stringify(ownerReports)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[\"before close\"]"
    );
}

#[test]
fn worker_owner_receives_cancelable_error_event_without_worker_error_object() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval(r#"
        globalThis.ownerEvent = null;
        const worker = new Worker('data:text/javascript,' + encodeURIComponent("reportError(new Error('worker failure'));"));
        worker.onerror = event => {
            ownerEvent = [event instanceof ErrorEvent, event.cancelable, event.error === null, event.isTrusted];
            event.preventDefault();
            ownerEvent.push(event.defaultPrevented);
        };
    "#).unwrap();
    runtime.run_timers(20, 1, 40);
    assert_eq!(
        runtime
            .eval("JSON.stringify(ownerEvent)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[true,true,true,true,true]"
    );
}

#[test]
fn worker_owner_error_dispatch_uses_captured_constructor_and_dispatcher() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval(r#"
        globalThis.ownerEvent = null;
        const PlatformErrorEvent = ErrorEvent;
        const worker = new Worker('data:text/javascript,' + encodeURIComponent("reportError(new Error('worker failure'));"));
        worker.onerror = event => {
            ownerEvent = [event instanceof PlatformErrorEvent, event.target === worker, event.cancelable, event.error === null, event.isTrusted];
            event.preventDefault();
        };
        globalThis.ErrorEvent = function() { throw new Error('author constructor'); };
        EventTarget.prototype.dispatchEvent = function() { throw new Error('author prototype dispatch'); };
        worker.dispatchEvent = function() { throw new Error('author instance dispatch'); };
    "#).unwrap();
    runtime.run_timers(20, 1, 40);
    assert_eq!(
        runtime
            .eval("JSON.stringify(ownerEvent)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[true,true,true,true,true]"
    );
    assert!(runtime.take_task_errors().is_empty());
}

#[test]
fn retained_nested_windows_keep_documents_after_ancestor_removal() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        globalThis.outer = document.createElement('iframe');
        document.body.appendChild(outer);
        globalThis.outerWindow = outer.contentWindow;
        globalThis.outerDocument = outer.contentDocument;
        globalThis.inner = outerDocument.createElement('iframe');
        outerDocument.body.appendChild(inner);
        globalThis.innerWindow = inner.contentWindow;
        globalThis.innerDocument = inner.contentDocument;
        document.body.removeChild(outer);
    "#,
        )
        .unwrap();
    assert_eq!(runtime.eval("[outerWindow.closed,innerWindow.closed,outerWindow.document===outerDocument,innerWindow.document===innerDocument,outerDocument.defaultView===null,innerDocument.defaultView===null].join('|')").unwrap().as_string().unwrap().to_std_string_escaped(), "true|true|true|true|true|true");
}

#[test]
fn iframe_proxy_preserves_window_owned_storage_history_and_registry_identity() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval(r#"
        globalThis.frame = document.createElement('iframe');
        document.body.appendChild(frame);
        globalThis.windowView = frame.contentWindow;
        globalThis.oldDocument = frame.contentDocument;
        globalThis.owned = windowView.Function('return [localStorage, sessionStorage, history, customElements]')();
        globalThis.before = [windowView.localStorage, windowView.sessionStorage, windowView.history, windowView.customElements];
    "#).unwrap();
    assert_eq!(
        runtime
            .eval("owned.every((value, index) => before[index] === value)")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    runtime.eval("document.body.removeChild(frame)").unwrap();
    assert_eq!(runtime.eval("[windowView.localStorage,windowView.sessionStorage,windowView.history,windowView.customElements].every((value,index)=>owned[index]===value) && windowView.document===oldDocument && windowView.closed").unwrap().as_boolean(), Some(true));
    assert_eq!(runtime.eval("(()=>{try{owned[2].length;return false;}catch(error){return error.name==='SecurityError';}})()").unwrap().as_boolean(),Some(true));
}

#[test]
fn uncanceled_worker_owner_error_propagates_original_fields_to_its_global() {
    for cancel in [false, true] {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.eval(&format!(r#"
            globalThis.ownerFields = null;
            globalThis.globalFields = [];
            const fields = event => [event.message, event.filename, event.lineno,
                event.colno, event.error === null, event.isTrusted, event.cancelable];
            onerror = (message, filename, line, column, error) => {{
                if (error !== null) throw new Error('wrong propagated value');
                return true;
            }};
            addEventListener('error', event => globalFields.push(fields(event)));
            const source = "setTimeout(() => {{ throw new Error('original'); }}, 0);\n//# sourceURL=worker-propagation.js";
            const worker = new Worker('data:text/javascript,' + encodeURIComponent(source));
            worker.onerror = event => {{
                ownerFields = fields(event);
                Object.defineProperty(event, 'message', {{ value: 'changed' }});
                if ({cancel}) event.preventDefault();
            }};
        "#)).unwrap();
        runtime.tick(0).unwrap();
        assert_eq!(
            runtime.eval("globalFields.length").unwrap().as_number(),
            Some(if cancel { 0.0 } else { 1.0 })
        );
        assert_eq!(runtime.eval("ownerFields[0] === 'original' && ownerFields[2] > 0 && ownerFields[3] > 0 && ownerFields.slice(4).every(Boolean)").unwrap().as_boolean(), Some(true));
        if !cancel {
            assert_eq!(
                runtime
                    .eval("JSON.stringify(globalFields[0]) === JSON.stringify(ownerFields)")
                    .unwrap()
                    .as_boolean(),
                Some(true)
            );
        }
        assert!(runtime.take_task_errors().is_empty());
    }
}

#[test]
fn worker_owner_global_propagation_stays_in_iframe_realm() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval(r#"
        globalThis.topReports = 0;
        onerror = () => { topReports++; return true; };
        const frame = document.createElement('iframe');
        document.body.appendChild(frame);
        globalThis.child = frame.contentWindow;
        child.Function(`
            globalThis.reports = [];
            addEventListener('error', event => {
                reports.push([event instanceof ErrorEvent, event.error === null,
                    event.target === globalThis, event.message]);
                event.preventDefault();
            });
            globalThis.worker = new Worker('data:text/javascript,' + encodeURIComponent("reportError('iframe original')"));
            worker.onerror = event => { if (!(event instanceof ErrorEvent)) throw new Error('wrong owner realm'); };
        `)();
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.eval("topReports").unwrap().as_number(), Some(0.0));
    assert_eq!(
        runtime
            .eval("child.JSON.stringify(child.reports)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[[true,true,true,\"iframe original\"]]"
    );
    assert!(runtime.take_task_errors().is_empty());
}

#[test]
fn worker_error_propagates_through_dedicated_worker_owner_chain() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval(r#"
        globalThis.globalReports = [];
        addEventListener('error', event => {
            globalReports.push([event.message, event.error === null]);
            event.preventDefault();
        });
        const inner = "reportError('nested original')";
        const outer = "globalThis.inner = new Worker('data:text/javascript,' + encodeURIComponent(" + JSON.stringify(inner) + "));";
        globalThis.worker = new Worker('data:text/javascript,' + encodeURIComponent(outer));
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(globalReports)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[[\"nested original\",true]]"
    );
    assert!(runtime.take_task_errors().is_empty());
}

#[test]
fn source_script_error_cycle_is_reclaimed_after_external_owners_drop() {
    use boa_engine::{Context, HostDefined, JsError, Script, Source};
    use boa_gc::{Gc, GcRefCell};
    use std::{cell::Cell, rc::Rc};

    #[derive(boa_gc::Trace, boa_engine::JsData)]
    struct Marker {
        error: Gc<GcRefCell<Option<JsError>>>,
        #[unsafe_ignore_trace]
        finalized: Rc<Cell<bool>>,
    }
    impl boa_gc::Finalize for Marker {
        fn finalize(&self) {
            self.finalized.set(true);
        }
    }
    let finalized = Rc::new(Cell::new(false));
    let error_slot = Gc::new(GcRefCell::new(None));
    let mut metadata = HostDefined::default();
    metadata.insert(boa_engine::error::ScriptErrorMetadata::new(true));
    metadata.insert(Marker {
        error: error_slot.clone(),
        finalized: Rc::clone(&finalized),
    });
    let mut context = Context::default();
    let script = Script::parse_with_host_defined(
        Source::from_bytes("throw 42;"),
        None,
        metadata,
        &mut context,
    )
    .unwrap();
    let error = script.evaluate(&mut context).unwrap_err();
    assert_eq!(
        error.source_script_metadata().unwrap().get::<bool>(),
        Some(&true)
    );
    *error_slot.borrow_mut() = Some(error);
    drop(script);
    drop(context);
    drop(error_slot);
    boa_gc::force_collect();
    assert!(
        finalized.get(),
        "the error's source Script must not permanently root its own cycle"
    );
}

#[test]
fn script_cross_origin_property_reflects_cors_settings_attribute() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const script = document.createElement('script');
        const results = [script.crossOrigin === null];
        script.crossOrigin = 'anonymous';
        results.push(script.getAttribute('crossorigin') === 'anonymous', script.crossOrigin === 'anonymous');
        script.setAttribute('crossorigin', 'USE-CREDENTIALS');
        results.push(script.crossOrigin === 'use-credentials');
        script.crossOrigin = 'unknown';
        results.push(script.getAttribute('crossorigin') === 'unknown', script.crossOrigin === 'anonymous');
        script.crossOrigin = null;
        results.push(!script.hasAttribute('crossorigin'), script.crossOrigin === null);
        return results.every(Boolean);
    })()"#).unwrap().as_boolean(), Some(true));
}

#[test]
fn stream_internal_handling_does_not_read_promise_constructor() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        globalThis.constructorReads = 0;
        globalThis.abortThrew = false;
        globalThis.closedReason = null;
        globalThis.reason = new Error('stream abort');
        globalThis.writer = new WritableStream().getWriter();
        Object.defineProperty(writer.closed, 'constructor', {
            configurable: true,
            get() { constructorReads++; throw new Error('author constructor'); }
        });
        try { writer.abort(reason); } catch (_) { abortThrew = true; }
        delete writer.closed.constructor;
        writer.closed.catch(error => closedReason = error);
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("constructorReads === 0 && !abortThrew && closedReason === reason")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}
