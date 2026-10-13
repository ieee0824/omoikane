//! Classic worker source loading with owned response security metadata.
use super::*;

const MAX_WORKER_REDIRECTS: usize = 20;

/// The source and final response metadata needed before worker author code runs.
#[derive(Debug)]
pub(super) struct LoadedWorkerScript {
    /// Absolute initial request URL, retained separately from redirect targets.
    pub(super) initial_url: String,
    /// Final response URL used for the worker location and policy base.
    pub(super) effective_url: String,
    /// UTF-8 JavaScript source that passed the loader's response checks.
    pub(super) source: String,
    /// Final HTTP status, or the synthetic data-response status 200.
    pub(super) response_status: u16,
    /// Final response CSP, report-only CSP, and Reporting-Endpoints headers.
    pub(super) security_headers: Vec<(String, String)>,
}

/// Loads a classic worker source and preserves its final response metadata.
///
/// The caller checks the initial URL against the creator's origin. HTTP I/O
/// retains the existing public-IP and cookie-context checks and accepts only
/// status 200. Each redirect is checked against the initial origin before its
/// request is sent, with the Fetch limit of 20 followed redirects.
pub(super) fn load_worker_script(
    requested: &str,
    base: Option<&crate::http::Url>,
    client: &mut Client,
) -> Option<LoadedWorkerScript> {
    match resolve_resource_ref(requested, base)? {
        ResolvedResource::Data { mime_type, data } => {
            if !is_javascript_mime_type(&mime_type) {
                return None;
            }
            Some(LoadedWorkerScript {
                initial_url: requested.to_owned(),
                effective_url: requested.to_owned(),
                source: String::from_utf8(data).ok()?,
                response_status: 200,
                security_headers: Vec::new(),
            })
        }
        ResolvedResource::Url(_) => load_http_worker_script(requested, base, client),
    }
}

fn load_http_worker_script(
    requested: &str,
    base: Option<&crate::http::Url>,
    client: &mut Client,
) -> Option<LoadedWorkerScript> {
    // The transport URL omits fragments; retain a standard URL alongside it
    // for WorkerLocation and the environment's creation URL.
    let initial = match base {
        Some(base) => url::Url::parse(&base.to_string()).ok()?.join(requested),
        None => url::Url::parse(requested),
    }
    .ok()?;
    let mut current = initial.clone();
    for redirects in 0..=MAX_WORKER_REDIRECTS {
        let transport_url = current.as_str().parse::<crate::http::Url>().ok()?;
        let mut request = HttpRequest::new(Method::Get, transport_url.clone());
        if requires_public_fetch(&transport_url, base) {
            request.require_public_ip();
        }
        if let Some(site) = base {
            request.set_cookie_context(site.clone(), false);
        }
        let response = client.send_once_with_timeout(request, true, None).ok()?;
        if matches!(response.status_code(), 301 | 302 | 303 | 307 | 308) {
            let next = response
                .header("location")
                .and_then(|location| worker_redirect_url(&current, location));
            if redirects < MAX_WORKER_REDIRECTS
                && let Some(next) = next
                && next.origin() == initial.origin()
            {
                current = next;
                continue;
            }
            client.report_resource_failure();
            return None;
        }
        if response.status_code() != 200 {
            client.report_resource_failure();
            return None;
        }
        let source = match std::str::from_utf8(response.body()) {
            Ok(source) => source.to_owned(),
            Err(_) => {
                client.report_resource_failure();
                return None;
            }
        };
        return Some(LoadedWorkerScript {
            initial_url: initial.to_string(),
            effective_url: current.to_string(),
            source,
            response_status: response.status_code(),
            security_headers: child_document::response_security_headers(&response),
        });
    }
    None
}

fn worker_redirect_url(current: &url::Url, location: &str) -> Option<url::Url> {
    let mut next = current.join(location).ok()?;
    if !matches!(next.scheme(), "http" | "https") {
        return None;
    }
    if next.fragment().is_none() {
        next.set_fragment(current.fragment());
    }
    Some(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::http_fixture::{
        ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
        read_request_headers,
    };
    use std::io::Write;

    fn response(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
        let mut bytes = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        )
        .into_bytes();
        for (name, value) in headers {
            write!(bytes, "{name}: {value}\r\n").unwrap();
        }
        bytes.extend_from_slice(b"\r\n");
        bytes.extend_from_slice(body);
        bytes
    }

    fn serve(responses: Vec<Vec<u8>>) -> (String, FixtureWorker<Vec<String>>) {
        let listener = bind_loopback().unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let worker = FixtureWorker::spawn(move || {
            responses
                .into_iter()
                .map(|response| {
                    let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
                    let headers = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
                    stream.write_all(&response).unwrap();
                    headers
                })
                .collect()
        });
        (origin, worker)
    }

    #[test]
    fn data_worker_metadata_preserves_source_and_synthetic_response() {
        let requested = "DATA:text/javascript;charset=utf-8,postMessage(%22ready%22)";
        let loaded = load_worker_script(requested, None, &mut Client::new()).unwrap();
        assert_eq!(loaded.initial_url, requested);
        assert_eq!(loaded.effective_url, requested);
        assert_eq!(loaded.source, "postMessage(\"ready\")");
        assert_eq!(loaded.response_status, 200);
        assert!(loaded.security_headers.is_empty());
    }

    #[test]
    fn data_worker_rejects_non_javascript_invalid_utf8_and_malformed_data() {
        for requested in [
            "data:text/plain,postMessage(1)",
            "data:text/javascript,%FF",
            "data:text/javascript;base64,%%%",
            "data:text/javascript",
        ] {
            assert!(
                load_worker_script(requested, None, &mut Client::new()).is_none(),
                "{requested}"
            );
        }
    }

    #[test]
    fn http_worker_preserves_only_final_security_headers_after_same_origin_redirect() {
        let final_headers = [
            ("Content-Type", "text/javascript"),
            ("Content-Security-Policy", "connect-src 'none'"),
            ("content-security-policy", "script-src 'self'"),
            ("Content-Security-Policy-Report-Only", "font-src 'none'"),
            ("Reporting-Endpoints", "worker=\"/reports\""),
            ("X-Unrelated", "not a security header"),
        ];
        let (origin, worker) = serve(vec![
            response(
                "302 Found",
                &[
                    ("Location", "/scripts/final.js"),
                    ("Content-Security-Policy", "default-src 'none'"),
                    ("Reporting-Endpoints", "old=\"/old-reports\""),
                    ("Set-Cookie", "worker=redirect; Path=/; SameSite=Lax"),
                ],
                b"",
            ),
            response("200 OK", &final_headers, b"postMessage('loaded');"),
        ]);
        let base = format!("{origin}/page.html").parse().unwrap();
        let loaded =
            load_worker_script("/start.js#entry", Some(&base), &mut Client::new()).unwrap();
        assert_eq!(loaded.initial_url, format!("{origin}/start.js#entry"));
        assert_eq!(
            loaded.effective_url,
            format!("{origin}/scripts/final.js#entry")
        );
        assert_eq!(loaded.source, "postMessage('loaded');");
        assert_eq!(loaded.response_status, 200);
        assert_eq!(
            loaded.security_headers,
            final_headers[1..5]
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect::<Vec<_>>()
        );
        let requests = worker.join();
        assert!(requests[0].starts_with("GET /start.js HTTP/1.1\r\n"));
        assert!(requests[1].starts_with("GET /scripts/final.js HTTP/1.1\r\n"));
        assert!(requests[1].contains("Cookie: worker=redirect\r\n"));
    }

    #[test]
    fn http_worker_rejects_error_status_and_invalid_utf8_source() {
        let (origin, worker) = serve(vec![
            response("404 Not Found", &[], b"postMessage('error page');"),
            response("200 OK", &[], b"\xff"),
        ]);
        let base = format!("{origin}/page.html").parse().unwrap();
        let mut client = Client::new();
        assert!(load_worker_script("/error.js", Some(&base), &mut client).is_none());
        assert!(load_worker_script("/invalid.js", Some(&base), &mut client).is_none());
        let requests = worker.join();
        assert!(requests[0].starts_with("GET /error.js HTTP/1.1\r\n"));
        assert!(requests[1].starts_with("GET /invalid.js HTTP/1.1\r\n"));
    }

    #[test]
    fn http_worker_rejects_a_cross_origin_redirect_before_contacting_its_target() {
        let target_listener = bind_loopback().unwrap();
        let target = format!(
            "http://{}/private.js",
            target_listener.local_addr().unwrap()
        );
        let (origin, worker) = serve(vec![
            response("302 Found", &[("Location", "/same-origin.js")], b""),
            response("307 Temporary Redirect", &[("Location", &target)], b""),
        ]);
        let base = format!("{origin}/page.html").parse().unwrap();
        assert!(load_worker_script("/worker.js", Some(&base), &mut Client::new()).is_none());
        let requests = worker.join();
        assert!(requests[0].starts_with("GET /worker.js HTTP/1.1\r\n"));
        assert!(requests[1].starts_with("GET /same-origin.js HTTP/1.1\r\n"));
        assert_eq!(
            target_listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn http_worker_without_same_origin_base_keeps_public_ip_restriction() {
        let listener = bind_loopback().unwrap();
        let requested = format!("http://{}/worker.js", listener.local_addr().unwrap());
        assert!(load_worker_script(&requested, None, &mut Client::new()).is_none());
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn redirect_urls_inherit_replace_and_clear_fragments() {
        let current = url::Url::parse("https://worker.test/dir/start.js#initial").unwrap();
        for (location, expected) in [
            ("final.js", "https://worker.test/dir/final.js#initial"),
            ("/final.js#new", "https://worker.test/final.js#new"),
            ("/final.js#", "https://worker.test/final.js#"),
        ] {
            assert_eq!(
                worker_redirect_url(&current, location).unwrap().as_str(),
                expected
            );
        }
        assert!(worker_redirect_url(&current, "data:text/javascript,postMessage(1)").is_none());
    }

    #[test]
    fn http_worker_follows_each_get_redirect_status() {
        let responses = [
            "301 Moved Permanently",
            "302 Found",
            "303 See Other",
            "307 Temporary Redirect",
            "308 Permanent Redirect",
        ]
        .iter()
        .enumerate()
        .map(|(index, status)| {
            response(
                status,
                &[("Location", &format!("/{}.js#hop-{index}", index + 1))],
                b"",
            )
        })
        .chain(std::iter::once(response(
            "200 OK",
            &[],
            b"postMessage('final');",
        )))
        .collect();
        let (origin, worker) = serve(responses);
        let base = format!("{origin}/page.html").parse().unwrap();
        let loaded = load_worker_script("/0.js", Some(&base), &mut Client::new()).unwrap();
        assert_eq!(loaded.effective_url, format!("{origin}/5.js#hop-4"));
        let requests = worker.join();
        assert_eq!(requests.len(), 6);
        for (index, request) in requests.iter().enumerate() {
            assert!(request.starts_with(&format!("GET /{index}.js HTTP/1.1\r\n")));
        }
    }

    #[test]
    fn http_worker_allows_twenty_redirects_and_rejects_the_twenty_first() {
        for redirects in [20, 21] {
            let mut responses =
                vec![response("302 Found", &[("Location", "/loop.js")], b""); redirects];
            if redirects == 20 {
                responses.push(response("200 OK", &[], b"postMessage('loaded');"));
            }
            let (origin, worker) = serve(responses);
            let base = format!("{origin}/page.html").parse().unwrap();
            assert_eq!(
                load_worker_script("/loop.js", Some(&base), &mut Client::new()).is_some(),
                redirects == 20
            );
            assert_eq!(worker.join().len(), 21);
        }
    }

    #[test]
    fn http_worker_rejects_missing_invalid_and_non_http_redirect_targets() {
        let (origin, worker) = serve(vec![
            response("302 Found", &[], b""),
            response("302 Found", &[("Location", "http://[invalid")], b""),
            response(
                "302 Found",
                &[("Location", "data:text/javascript,postMessage(1)")],
                b"",
            ),
        ]);
        let base = format!("{origin}/page.html").parse().unwrap();
        let mut client = Client::new();
        for _ in 0..3 {
            assert!(load_worker_script("/worker.js", Some(&base), &mut client).is_none());
        }
        assert_eq!(worker.join().len(), 3);
    }
}
