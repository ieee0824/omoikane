//! Synchronous classic-worker script imports with owned fetched sources.
use super::*;

/// Owned script response retained by a ServiceWorker version.
#[derive(Clone)]
pub(super) struct ScriptResource {
    pub(super) filename: String,
    pub(super) source: String,
    pub(super) muted_errors: bool,
}

/// Refetches classic imports without executing author code.
pub(super) fn refetch_service_worker_imports(
    state: &mut HostState,
    previous: &BTreeMap<String, ScriptResource>,
) -> BTreeMap<String, Option<ScriptResource>> {
    let base = state.base_url.clone();
    let origin = script_errors::document_fetch_origin(state, state.document.identity());
    let mut resources = BTreeMap::new();
    for requested in previous.keys() {
        let resource = fetch_import_source(state, requested, base.as_ref(), &origin, None).map(
            |(filename, source, muted_errors)| ScriptResource {
                filename,
                source,
                muted_errors,
            },
        );
        // A bad response is ignored for byte comparison, but retained so a
        // newly installed worker cannot refetch a different response later.
        resources.insert(requested.clone(), resource);
    }
    resources
}

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
        let document_id = state.document.identity();
        let origin = script_errors::document_fetch_origin(&state, document_id);
        let mut sources = Vec::with_capacity(urls.len());
        for url in urls {
            let requested_url = resource_reference_timing_name(&url, base.as_ref());
            let cached = state
                .service_worker_imports
                .get(&requested_url)
                .cloned()
                .map(Some)
                .or_else(|| {
                    state
                        .service_worker_import_cache
                        .get(&requested_url)
                        .cloned()
                });
            let resource = if let Some(cached) = cached {
                let Some(resource) = cached else {
                    return Ok(None);
                };
                resource
            } else {
                let Some((filename, source, muted_errors)) = fetch_import_source(
                    &mut state,
                    &url,
                    base.as_ref(),
                    &origin,
                    location.as_ref(),
                ) else {
                    return Ok(None);
                };
                ScriptResource {
                    filename,
                    source,
                    muted_errors,
                }
            };
            if state.worker_service_worker_scope.is_some() {
                state
                    .service_worker_imports
                    .insert(requested_url.clone(), resource.clone());
            }
            let ScriptResource {
                filename,
                source,
                muted_errors,
            } = resource;
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
            if !state.csp_allows_imported_script_with_source(
                &document, requested, 0, requested, location,
            ) {
                return None;
            }
            let (url, source, _, _) = fetch_script_resource_with_client_in_context(
                requested,
                base,
                None,
                &mut state.http_client,
            )?;
            state
                .csp_allows_imported_script_with_source(
                    &document, requested, 0, requested, location,
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
    let embedder = state
        .isolation_policy_for_document(document.identity())
        .embedder;
    let mut crossed_origin = false;
    for redirects in 0..=10 {
        if !state.csp_allows_imported_script_with_source(
            document,
            &current.to_string(),
            redirects,
            initial,
            location,
        ) {
            return None;
        }
        crossed_origin |= !origin.is_same_origin(&current);
        muted |= crossed_origin;
        let mut request = HttpRequest::new(Method::Get, current.clone());
        if requires_public_fetch(&current, base) {
            request.require_public_ip();
        }
        cookie_context.apply(&mut request);
        let credentials = if crossed_origin
            && embedder.value == cross_origin_isolation::EmbedderPolicyValue::Credentialless
        {
            CredentialsMode::Omit
        } else {
            CredentialsMode::Include
        };
        let mut violations = Vec::new();
        let fetched = crate::http::cors::fetch_with_embedder_policy_reporting(
            &mut state.http_client,
            request,
            origin,
            RequestMode::NoCors,
            credentials,
            RedirectMode::Manual,
            &mut cache,
            embedder.value.into(),
            embedder.report_only_value.into(),
            &mut violations,
            None,
        );
        for violation in &mut violations {
            violation.blocked_url = initial.to_owned();
        }
        state.queue_coep_corp_violations(document.identity(), "script", violations);
        let fetched = fetched.ok()?;
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
        if !state.csp_allows_imported_script_with_source(
            document,
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
    use crate::http::{HttpParseError, HttpResponse, HttpTransport};
    use crate::test_support::http_fixture::{
        ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
        read_request_headers,
    };
    use std::io::Write;
    use std::time::Duration;

    #[test]
    fn credentialless_import_redirect_never_restores_same_origin_cookies() {
        struct RedirectTransport(Arc<Mutex<Vec<(String, Option<String>)>>>);

        impl HttpTransport for RedirectTransport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<HttpResponse, HttpParseError> {
                self.0.lock().unwrap().push((
                    request.url().to_string(),
                    request.header("cookie").map(str::to_owned),
                ));
                let response = match request.url().path() {
                    "/entry.js" => HttpResponse::new(
                        302,
                        "Found",
                        vec![(
                            "Location".into(),
                            "https://other.example.test/jump.js".into(),
                        )],
                        Vec::new(),
                    ),
                    "/jump.js" => HttpResponse::new(
                        302,
                        "Found",
                        vec![(
                            "Location".into(),
                            "https://isolated.example.test/final.js".into(),
                        )],
                        Vec::new(),
                    ),
                    "/final.js" => HttpResponse::new(
                        200,
                        "OK",
                        Vec::new(),
                        b"globalThis.imported = true;".to_vec(),
                    ),
                    path => panic!("unexpected import path: {path}"),
                };
                Ok(response)
            }
        }

        let mut runtime = JsRuntime::with_document_and_url(
            blank_html_document(),
            "https://isolated.example.test/worker.js",
        )
        .unwrap();
        runtime.install_isolation_policy(&[(
            "Cross-Origin-Embedder-Policy".into(),
            "credentialless".into(),
        )]);
        let initial: crate::http::Url = "https://isolated.example.test/entry.js".parse().unwrap();
        runtime
            .host_state
            .borrow()
            .cookie_store
            .lock()
            .unwrap()
            .add_from_header_for_url("session=secret; Secure; SameSite=None", &initial);
        let seen = Arc::new(Mutex::new(Vec::new()));
        runtime.set_http_resource_transport(Some(Arc::new(RedirectTransport(Arc::clone(&seen)))));
        let imported = runtime
            .with_active_host(|context| {
                import_scripts(
                    &JsValue::undefined(),
                    &[JsValue::from(JsString::from(initial.to_string()))],
                    context,
                )
            })
            .unwrap();
        assert_eq!(imported.as_boolean(), Some(true));
        assert!(runtime.eval("imported === true").unwrap().to_boolean());
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0].1.as_deref(), Some("session=secret"));
        assert_eq!(seen[1].1, None);
        assert_eq!(seen[2].1, None);
    }

    #[test]
    fn import_redirect_coep_report_keeps_the_initial_url() {
        struct RedirectTransport;

        impl HttpTransport for RedirectTransport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<HttpResponse, HttpParseError> {
                Ok(match request.url().path() {
                    "/entry.js" => HttpResponse::new(
                        302,
                        "Found",
                        vec![(
                            "Location".into(),
                            "https://other.example.test/blocked.js".into(),
                        )],
                        Vec::new(),
                    ),
                    "/blocked.js" => HttpResponse::new(
                        200,
                        "OK",
                        Vec::new(),
                        b"globalThis.unexpected = true;".to_vec(),
                    ),
                    path => panic!("unexpected import path: {path}"),
                })
            }
        }

        let mut runtime = JsRuntime::with_document_and_url(
            blank_html_document(),
            "https://isolated.example.test/worker.js",
        )
        .unwrap();
        runtime.install_isolation_policy(&[(
            "Cross-Origin-Embedder-Policy".into(),
            "require-corp; report-to=\"coep\"".into(),
        )]);
        runtime.set_http_resource_transport(Some(Arc::new(RedirectTransport)));
        let initial = "https://isolated.example.test/entry.js";
        let imported = runtime
            .with_active_host(|context| {
                import_scripts(
                    &JsValue::undefined(),
                    &[JsValue::from(JsString::from(initial))],
                    context,
                )
            })
            .unwrap();
        assert_eq!(imported.as_boolean(), Some(false));
        assert!(
            !runtime
                .eval("globalThis.unexpected === true")
                .unwrap()
                .to_boolean()
        );
        let state = runtime.host_state.borrow();
        assert_eq!(state.pending_isolation_reports.len(), 1);
        let body = &state.pending_isolation_reports[0].envelope["body"];
        assert_eq!(body["type"], "corp");
        assert_eq!(body["blockedURL"], initial);
        assert_eq!(body["destination"], "script");
    }

    #[test]
    fn strict_dynamic_allows_non_parser_import_scripts_request() {
        let listener = bind_loopback().unwrap();
        let address = listener.local_addr().unwrap();
        let server = FixtureWorker::spawn(move || {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let body = "globalThis.importedByStrictDynamic = true";
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        });
        let mut runtime =
            JsRuntime::with_document_and_url(blank_html_document(), "http://127.0.0.1:1/worker.js")
                .unwrap();
        runtime.set_base_url(format!("http://{address}/").parse().unwrap());
        runtime.install_csp_policy(&["script-src 'strict-dynamic' 'nonce-root'".into()]);
        let requested = JsValue::from(JsString::from(format!("http://{address}/import.js")));
        let imported = runtime
            .with_active_host(|context| {
                import_scripts(&JsValue::undefined(), &[requested], context)
            })
            .unwrap();
        assert_eq!(imported.as_boolean(), Some(true));
        assert_eq!(
            runtime
                .eval("importedByStrictDynamic && document.cspViolations.length === 0")
                .unwrap()
                .as_boolean(),
            Some(true)
        );
        server.join();
    }

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
