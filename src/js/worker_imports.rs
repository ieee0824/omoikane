//! Synchronous classic-worker script imports with owned fetched sources.
use super::*;

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_import_scripts"),
        0,
        NativeFunction::from_copy_closure(import_scripts),
    )
}

fn import_scripts(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    // Complete argument conversion before starting network I/O or evaluation.
    let urls = args
        .iter()
        .map(|value| {
            value
                .to_string(context)
                .map(|value| value.to_std_string_escaped())
        })
        .collect::<JsResult<Vec<_>>>()?;
    let location = security_reporting::capture_csp_source(context);
    let sources = with_host_state(|host| {
        let mut state = host.borrow_mut();
        let base = state.base_url.clone();
        let origin = script_errors::document_fetch_origin(&state, state.document.identity());
        let mut sources = Vec::with_capacity(urls.len());
        for url in urls {
            let Some((filename, source, muted_errors)) =
                fetch_import_source(&mut state, &url, base.as_ref(), &origin, location.as_ref())
            else {
                return Ok(None);
            };
            let requested_url = resource_reference_timing_name(&url, base.as_ref());
            sources.push((filename, source, muted_errors, requested_url));
        }
        Ok(Some(sources))
    })?;
    let Some(sources) = sources else {
        return Ok(JsValue::from(false));
    };
    for (filename, source, muted_errors, requested_url) in sources {
        let input = Source::from_reader(source.as_bytes(), Some(Path::new(&filename)));
        let metadata = script_errors::classic_script_metadata(muted_errors, Some(requested_url));
        let result = Script::parse_with_host_defined(input, None, metadata, context)
            .and_then(|script| script.evaluate(context));
        if let Err(error) = result {
            // Muted imported scripts cannot expose their thrown value to the
            // caller; the worker bootstrap turns false into NetworkError.
            if muted_errors
                && error.as_native().is_none_or(|error| {
                    !matches!(error.kind, boa_engine::JsNativeErrorKind::RuntimeLimit)
                })
            {
                return Ok(JsValue::from(false));
            }
            return Err(error);
        }
    }
    Ok(JsValue::from(true))
}

/// Fetches one classic import under the Worker global's script-src policy.
/// Redirects are handled one hop at a time so a disallowed target is never
/// contacted, while the initial URL remains the one exposed in reports.
fn fetch_import_source(
    state: &mut HostState,
    requested: &str,
    base: Option<&crate::http::Url>,
    origin: &CorsOrigin,
    location: Option<&security_reporting::CspSourceLocation>,
) -> Option<(String, String, bool)> {
    let document = state.document.clone();
    match resolve_resource_ref(requested, base)? {
        ResolvedResource::Data { .. } => {
            if !state.csp_allows_response_for_document_with_source(
                &document,
                ResourceType::Script,
                requested,
                0,
                requested,
                location,
            ) {
                return None;
            }
            let (url, source, _) = fetch_script_resource_with_client_in_context(
                requested,
                base,
                None,
                &mut state.http_client,
            )?;
            state
                .csp_allows_response_for_document_with_source(
                    &document,
                    ResourceType::Script,
                    requested,
                    0,
                    requested,
                    location,
                )
                .then_some((url, source, false))
        }
        ResolvedResource::Url(initial) => {
            fetch_http_import_source(state, &document, &initial, base, origin, location)
        }
    }
}

fn fetch_http_import_source(
    state: &mut HostState,
    document: &NodeHandle,
    initial: &str,
    base: Option<&crate::http::Url>,
    origin: &CorsOrigin,
    location: Option<&security_reporting::CspSourceLocation>,
) -> Option<(String, String, bool)> {
    let mut current = initial.parse::<crate::http::Url>().ok()?;
    let mut muted = false;
    let mut cache = PreflightCache::default();
    let cookie_context = state.document_cookie_request_context(document.identity());
    for redirects in 0..=10 {
        if !state.csp_allows_response_for_document_with_source(
            document,
            ResourceType::Script,
            &current.to_string(),
            redirects,
            initial,
            location,
        ) {
            return None;
        }
        muted |= !origin.is_same_origin(&current);
        let mut request = HttpRequest::new(Method::Get, current.clone());
        if requires_public_fetch(&current, base) {
            request.require_public_ip();
        }
        cookie_context.apply(&mut request);
        let fetched = crate::http::cors::fetch(
            &mut state.http_client,
            request,
            origin,
            RequestMode::NoCors,
            CredentialsMode::Include,
            RedirectMode::Manual,
            &mut cache,
        )
        .ok()?;
        let response = fetched.response;
        if matches!(response.status_code(), 301 | 302 | 303 | 307 | 308) {
            if redirects == 10 {
                state.http_client.report_resource_failure();
                return None;
            }
            let Some(next) = response
                .header("location")
                .and_then(|location| import_redirect_url(&current, location))
            else {
                state.http_client.report_resource_failure();
                return None;
            };
            current = next;
            continue;
        }
        if !state.csp_allows_response_for_document_with_source(
            document,
            ResourceType::Script,
            &current.to_string(),
            redirects,
            initial,
            location,
        ) {
            return None;
        }
        if response.status_code() != 200 {
            state.http_client.report_resource_failure();
            return None;
        }
        let source = match std::str::from_utf8(response.body()) {
            Ok(source) => source.to_owned(),
            Err(_) => {
                state.http_client.report_resource_failure();
                return None;
            }
        };
        return Some((current.to_string(), source, muted));
    }
    None
}

fn import_redirect_url(current: &crate::http::Url, location: &str) -> Option<crate::http::Url> {
    let base = url::Url::parse(&current.to_string()).ok()?;
    let mut next = base.join(location).ok()?;
    if !matches!(next.scheme(), "http" | "https") {
        return None;
    }
    if next.fragment().is_none() {
        next.set_fragment(base.fragment());
    }
    next.as_str().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::http_fixture::{
        ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
        read_request_headers,
    };
    use std::io::Write;

    #[test]
    fn imported_scripts_keep_csp_source_location() {
        let listener = bind_loopback().unwrap();
        let address = listener.local_addr().unwrap();
        let server = FixtureWorker::spawn(move || {
            let body = "fetch('https://blocked.test/imported').catch(()=>{});";
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let mut runtime =
            JsRuntime::with_document_and_url(blank_html_document(), "http://127.0.0.1:1/worker.js")
                .unwrap();
        runtime.set_base_url(format!("http://{address}/").parse().unwrap());
        runtime.install_csp_policy(&["connect-src 'none'; report-to imports".into()]);
        runtime
            .eval(
                "globalThis.importReports=[];\
                 new ReportingObserver(batch=>importReports.push(...batch),{\
                   types:['csp-violation']\
                 }).observe();",
            )
            .unwrap();
        let requested_url = format!("http://{address}/import.js");
        let imported = runtime
            .with_active_host(|context| {
                import_scripts(
                    &JsValue::undefined(),
                    &[JsValue::from(JsString::from(requested_url.clone()))],
                    context,
                )
            })
            .unwrap();
        assert_eq!(imported.as_boolean(), Some(true));
        runtime.run_until_idle().unwrap();
        assert!(
            runtime
                .eval(&format!(
                    "importReports.length===1 && \
                     importReports[0].body.sourceFile==={} && \
                     importReports[0].body.lineNumber===1 && \
                     importReports[0].body.columnNumber===1",
                    serde_json::to_string(&requested_url).unwrap()
                ))
                .unwrap()
                .as_boolean()
                .unwrap()
        );
        server.join();
    }

    #[test]
    fn imported_non_cors_scripts_keep_muting_and_hide_thrown_values() {
        let listener = bind_loopback().unwrap();
        let address = listener.local_addr().unwrap();
        let server = FixtureWorker::spawn(move || {
            for body in [
                "globalThis.imported=()=>Promise.reject('private');",
                "throw { private: true };",
            ] {
                let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
                read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            }
        });
        let mut runtime =
            JsRuntime::with_document_and_url(blank_html_document(), "http://127.0.0.1:1/worker.js")
                .unwrap();
        runtime.set_base_url(format!("http://{address}/").parse().unwrap());
        runtime.eval("globalThis.reports=[];addEventListener('unhandledrejection',e=>{reports.push(e.reason);e.preventDefault()});").unwrap();
        let url = JsValue::from(JsString::from(format!("http://{address}/import.js")));
        let imported = runtime
            .with_active_host(|context| {
                import_scripts(&JsValue::undefined(), &[url.clone()], context)
            })
            .unwrap();
        assert_eq!(imported.as_boolean(), Some(true));
        runtime.eval("imported()").unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(
            runtime.eval("reports.length").unwrap().as_number(),
            Some(0.0)
        );
        let failed = runtime
            .with_active_host(|context| import_scripts(&JsValue::undefined(), &[url], context))
            .unwrap();
        assert_eq!(failed.as_boolean(), Some(false));
        server.join();
    }
}
