//! Owned Fetch request inputs and response application at the runtime boundary.
use super::*;
mod async_fetch;
pub(super) use async_fetch::FetchLoads;

pub(super) fn begin_native(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    async_fetch::begin(this, args, context)
}

pub(super) fn cancel_native(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    async_fetch::cancel(this, args, context)
}

pub(super) fn poll(runtime: &mut JsRuntime) {
    async_fetch::poll(runtime);
}

pub(super) struct Options {
    url: String,
    method: Method,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    mode: RequestMode,
    credentials: CredentialsMode,
    redirect_mode: RedirectMode,
    timeout: Option<Duration>,
    csp_source: Option<security_reporting::CspSourceLocation>,
}

pub(super) struct Prepared {
    pub context: ResponseContext,
    pub network: NetworkRequest,
}

/// Owner-only metadata; no borrowed document state crosses a network boundary.
#[derive(Clone)]
pub(super) struct ResponseContext {
    document_id: usize,
    normalized_url: String,
    method: Method,
    credentials: CredentialsMode,
    timeout: Option<Duration>,
    csp_source: Option<security_reporting::CspSourceLocation>,
}

/// Owned network inputs, independent of DOM and JavaScript values.
pub(super) struct NetworkRequest {
    request: HttpRequest,
    origin: CorsOrigin,
    mode: RequestMode,
    credentials: CredentialsMode,
    redirect_mode: RedirectMode,
    timeout: Option<Duration>,
    embedder: crate::http::corp::EmbedderPolicyValue,
    report_only: crate::http::corp::EmbedderPolicyValue,
}

pub(super) struct NetworkResponse {
    result: Result<crate::http::cors::FetchResponse, crate::http::cors::CorsError>,
    violations: Vec<crate::http::cors::CorpViolation>,
}

pub(super) fn parse_options(
    url: String,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<Options> {
    let method_name = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| js_string!("GET").into())
        .to_string(context)?
        .to_std_string_escaped();
    let method = match method_name.as_str() {
        "GET" => Method::Get,
        "POST" => Method::Post,
        "HEAD" => Method::Head,
        "PUT" => Method::Put,
        "DELETE" => Method::Delete,
        "OPTIONS" => Method::Options,
        "PATCH" => Method::Patch,
        _ => {
            return Err(JsNativeError::typ()
                .with_message(format!("unsupported HTTP method: {method_name}"))
                .into());
        }
    };
    let headers_json = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| js_string!("[]").into())
        .to_string(context)?
        .to_std_string_escaped();
    let headers: Vec<(String, String)> = serde_json::from_str(&headers_json).map_err(|error| {
        JsError::from(
            JsNativeError::typ().with_message(format!("invalid request headers: {error}")),
        )
    })?;
    let body = body_bytes_argument(args.get(3), context)?;
    let (mode, credentials, redirect_mode) = parse_modes(args, context)?;
    let timeout = if let Some(value) = args.get(7) {
        let milliseconds = value.to_number(context)?;
        if milliseconds.is_finite() && milliseconds > 0.0 {
            Some(Duration::from_millis(
                milliseconds.ceil().min(u64::MAX as f64) as u64,
            ))
        } else {
            None
        }
    } else {
        None
    };
    let csp_source = csp_source_location_argument(args, 8, context)?;

    Ok(Options {
        url,
        method,
        headers,
        body,
        mode,
        credentials,
        redirect_mode,
        timeout,
        csp_source,
    })
}

pub(super) fn prepare(
    state: &mut HostState,
    options: Options,
    context: &Context,
) -> JsResult<Prepared> {
    let Options {
        url,
        method,
        headers,
        body,
        mode,
        credentials,
        redirect_mode,
        timeout,
        csp_source,
    } = options;
    let parsed_url = url
        .parse::<crate::http::Url>()
        .map_err(|error| JsError::from(JsNativeError::typ().with_message(error.to_string())))?;
    let normalized_url = parsed_url.to_string();
    let document = state.csp_document_for_context(context)?;
    if !state.csp_allows_connect(&document, &normalized_url, csp_source.as_ref()) {
        state.record_csp_violation_with_source(
            &document,
            ResourceType::Connect,
            &normalized_url,
            csp_source.as_ref(),
        );
        return Err(JsNativeError::typ()
            .with_message("fetch blocked by CSP connect-src")
            .into());
    }
    let origin = script_errors::document_fetch_origin(&state, document.identity());
    let mut request = HttpRequest::new(method, parsed_url);
    state.set_document_cookie_request_context(document.identity(), &mut request);
    for (name, value) in headers {
        if !crate::http::is_valid_header(&name, &value) {
            return Err(JsNativeError::typ()
                .with_message("invalid request header")
                .into());
        }
        if crate::http::is_forbidden_request_header(&name, &value) {
            continue;
        }
        request.set_header(name, value);
    }
    if let Some(body) = body {
        request.set_body(body);
    }

    let embedder = state
        .isolation_policy_for_document(document.identity())
        .embedder;
    Ok(Prepared {
        context: ResponseContext {
            document_id: document.identity(),
            normalized_url,
            method,
            credentials,
            timeout,
            csp_source,
        },
        network: NetworkRequest {
            request,
            origin,
            mode,
            credentials,
            redirect_mode,
            timeout,
            embedder: embedder.value.into(),
            report_only: embedder.report_only_value.into(),
        },
    })
}

impl NetworkRequest {
    /// Performs transport I/O and updates the supplied client's cookies and preflight cache.
    pub(super) fn execute(
        self,
        client: &mut Client,
        cache: &mut PreflightCache,
    ) -> NetworkResponse {
        let mut violations = Vec::new();
        let result = crate::http::cors::fetch_with_embedder_policy_reporting(
            client,
            self.request,
            &self.origin,
            self.mode,
            self.credentials,
            self.redirect_mode,
            cache,
            self.embedder,
            self.report_only,
            &mut violations,
            self.timeout,
        );
        NetworkResponse { result, violations }
    }
}

pub(super) fn apply(
    state: &mut HostState,
    context: ResponseContext,
    response: NetworkResponse,
) -> JsResult<String> {
    let ResponseContext {
        document_id,
        normalized_url,
        method,
        credentials,
        timeout,
        csp_source,
    } = context;
    let document = state.get_node(document_id).ok_or_else(|| {
        JsError::from(JsNativeError::typ().with_message("fetch document no longer available"))
    })?;
    state.queue_coep_corp_violations(document_id, "", response.violations);
    let fetch_result = response.result;
    let fetched = match fetch_result {
        Ok(fetched) => fetched,
        Err(crate::http::cors::CorsError::Timeout) if timeout.is_some() => {
            return Ok(r#"{"__omoikane_timeout":true}"#.to_string());
        }
        Err(crate::http::cors::CorsError::CorpCheck) => {
            return Err(JsNativeError::typ()
                .with_message("CORP response check failed")
                .into());
        }
        Err(error) => {
            return Err(
                JsError::from(JsNativeError::error().with_message(error.to_string())).into(),
            );
        }
    };
    if let Some(effective_url) = fetched.response.effective_url()
        && !state.csp_allows_response_for_document_with_source(
            &document,
            ResourceType::Connect,
            &effective_url.to_string(),
            fetched.response.redirect_count(),
            &normalized_url,
            csp_source.as_ref(),
        )
    {
        return Err(JsNativeError::typ()
            .with_message("fetch redirect blocked by CSP connect-src")
            .into());
    }
    // Server-Timing is read from the full header list, before CORS filters
    // what script may see, and only after the timing allow check.
    let origin = script_errors::document_fetch_origin(state, document_id);
    let server_timing = super::server_timing::timing_allowed(
        fetched.response_type,
        fetched.response.headers(),
        &origin,
    )
    .then(|| super::server_timing::parse_server_timing(fetched.response.headers()))
    .unwrap_or_default();
    Ok(serialize(
        fetched,
        method,
        credentials,
        &normalized_url,
        super::server_timing::server_timing_json(&server_timing),
    ))
}

/// Encodes the existing Fetch/XHR payload without consulting runtime state.
fn serialize(
    fetched: crate::http::cors::FetchResponse,
    method: Method,
    credentials: CredentialsMode,
    normalized_url: &str,
    server_timing: serde_json::Value,
) -> String {
    let response = fetched.response;
    let opaque = matches!(
        fetched.response_type,
        ResponseType::Opaque | ResponseType::OpaqueRedirect
    );
    // Fetch creates a null body for HEAD responses and for status codes
    // whose semantics forbid a body. A successful response with an empty
    // payload (for example `200 Content-Length: 0`) still has an empty
    // ReadableStream, so presence cannot be inferred from byte length.
    let body_present = !opaque
        && !matches!(method, Method::Head)
        && !matches!(response.status_code(), 100..=199 | 204 | 205 | 304);
    // `bodyText` is the lossy UTF-8 decoding the Fetch and XHR text paths are
    // defined in terms of, so it stays the primary representation. It cannot
    // represent a payload that is not valid UTF-8 though (an image, a font),
    // and `Response.blob()`/`arrayBuffer()` must hand back the original
    // bytes. Carry those separately, and only when decoding actually lost
    // information, so text responses pay nothing for it.
    //
    // `from_utf8_lossy` borrows when the input is already valid UTF-8 and
    // only allocates to substitute replacement characters, so an owned `Cow`
    // is the signal that bytes were lost — no second validation pass needed.
    let decoded_body = body_present.then(|| String::from_utf8_lossy(response.body()));
    let body_base64 = matches!(decoded_body, Some(std::borrow::Cow::Owned(_)))
        .then(|| base64::engine::general_purpose::STANDARD.encode(response.body()));
    let body_text = decoded_body.map(std::borrow::Cow::into_owned);
    let effective_url = (!opaque).then(|| {
        response
            .effective_url()
            .map(ToString::to_string)
            .unwrap_or_else(|| normalized_url.to_owned())
    });
    let response_type = match fetched.response_type {
        ResponseType::Basic => "basic",
        ResponseType::Cors => "cors",
        ResponseType::Opaque => "opaque",
        ResponseType::OpaqueRedirect => "opaqueredirect",
    };
    let exposed_headers = exposed_response_headers(&response, fetched.response_type, credentials);
    let payload = serde_json::json!({
        "status": if opaque { 0 } else { response.status_code() },
        "statusText": if opaque { "" } else { response.reason() },
        "ok": !opaque && (200..300).contains(&response.status_code()),
        "url": effective_url.as_deref().unwrap_or(""),
        "redirected": !opaque && fetched.redirected,
        "type": response_type,
        "headers": exposed_headers,
        "bodyText": body_text.as_deref().unwrap_or(""),
        "bodyBase64": body_base64,
        "bodyPresent": body_present,
        "serverTiming": server_timing,
    })
    .to_string();
    payload
}

fn parse_modes(
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<(RequestMode, CredentialsMode, RedirectMode)> {
    let mode = match string_argument(args.get(4), "cors", context)?.as_str() {
        "same-origin" => RequestMode::SameOrigin,
        "cors" => RequestMode::Cors,
        "no-cors" => RequestMode::NoCors,
        value => {
            return Err(JsNativeError::typ()
                .with_message(format!("unsupported request mode: {value}"))
                .into());
        }
    };
    let credentials = match string_argument(args.get(5), "same-origin", context)?.as_str() {
        "omit" => CredentialsMode::Omit,
        "same-origin" => CredentialsMode::SameOrigin,
        "include" => CredentialsMode::Include,
        value => {
            return Err(JsNativeError::typ()
                .with_message(format!("unsupported credentials mode: {value}"))
                .into());
        }
    };
    let redirect_mode = match string_argument(args.get(6), "follow", context)?.as_str() {
        "follow" => RedirectMode::Follow,
        "error" => RedirectMode::Error,
        "manual" => RedirectMode::Manual,
        value => {
            return Err(JsNativeError::typ()
                .with_message(format!("unsupported redirect mode: {value}"))
                .into());
        }
    };
    Ok((mode, credentials, redirect_mode))
}
