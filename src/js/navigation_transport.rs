//! Owned HTTP work for a navigation before its result reaches the DOM owner.

use super::child_document::NavigationRedirect;
use crate::http::corp::{self, EmbedderPolicyValue};
use crate::http::cors::{CorpViolation, Origin};
use crate::http::{Client, HttpParseError, HttpRequest, HttpResponse};

/// Parent policy captured before an iframe navigation leaves its owner thread.
#[derive(Debug, Clone)]
pub(super) struct NavigationParentPolicy {
    pub(super) origin: Origin,
    pub(super) enforced: EmbedderPolicyValue,
    pub(super) report_only: EmbedderPolicyValue,
}

/// Fully owned inputs for one redirect-following navigation HTTP operation.
pub(super) struct OwnedNavigationTransport {
    request: HttpRequest,
    parent: Option<NavigationParentPolicy>,
}

/// Owned transport result; DOM parsing and policy commit remain with the owner.
#[derive(Debug)]
pub(super) struct NavigationTransportResult {
    pub(super) response: Result<Option<HttpResponse>, HttpParseError>,
    pub(super) redirects: Vec<NavigationRedirect>,
    pub(super) violations: Vec<CorpViolation>,
}

impl OwnedNavigationTransport {
    pub(super) fn new(request: HttpRequest, parent: Option<NavigationParentPolicy>) -> Self {
        Self { request, parent }
    }

    /// Performs only HTTP, cookie, redirect, and redirect-CORP work.
    ///
    /// The synchronous owner passes its existing client so live connections
    /// remain reusable. A queued caller can instead pass an owned worker fork.
    pub(super) fn run(self, client: &mut Client) -> NavigationTransportResult {
        self.run_while(client, || true)
    }

    /// Performs transport while the owning resource scope remains live.
    ///
    /// A blocking request cannot be interrupted, but checking before every
    /// redirect prevents a replaced navigation from starting another hop.
    pub(super) fn run_while(
        self,
        client: &mut Client,
        mut should_continue: impl FnMut() -> bool,
    ) -> NavigationTransportResult {
        let Self { request, parent } = self;
        let initial_url = request.url().to_string();
        let mut redirects = Vec::new();
        let mut violations = Vec::new();
        if !should_continue() {
            return NavigationTransportResult {
                response: Ok(None),
                redirects,
                violations,
            };
        }
        let response = client.send_observing_navigation(request, &mut |request, response| {
            if !should_continue() {
                return false;
            }
            if !matches!(response.status_code(), 301 | 302 | 303 | 307 | 308) {
                return true;
            }
            if let Some(parent) = &parent {
                let check = corp::check_response(
                    &parent.origin,
                    request.url(),
                    response.headers(),
                    parent.enforced,
                    parent.report_only,
                    true,
                    true,
                );
                if check.enforced_violation || check.report_only_violation {
                    violations.push(CorpViolation {
                        blocked_url: initial_url.clone(),
                        report_only: check.report_only_violation,
                        enforced: check.enforced_violation,
                    });
                }
                if check.blocked {
                    return false;
                }
            }
            redirects.push(NavigationRedirect {
                url: request.url().to_string(),
                headers: response.headers().to_vec(),
                request_referrer: request.header("Referer").unwrap_or_default().to_owned(),
            });
            true
        });
        NavigationTransportResult {
            response,
            redirects,
            violations,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{CookieJar, HttpTransport};
    use std::io;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Debug, PartialEq, Eq)]
    struct SeenRequest {
        url: String,
        cookie: Option<String>,
    }

    struct RedirectTransport {
        seen: Arc<Mutex<Vec<SeenRequest>>>,
        fail_final: bool,
    }

    impl HttpTransport for RedirectTransport {
        fn send(
            &self,
            request: &HttpRequest,
            _: Option<Duration>,
        ) -> Result<HttpResponse, HttpParseError> {
            self.seen.lock().unwrap().push(SeenRequest {
                url: request.url().to_string(),
                cookie: request.header("Cookie").map(str::to_owned),
            });
            match request.url().path() {
                "/start" => Ok(HttpResponse::new(
                    302,
                    "Found",
                    vec![
                        ("Location".into(), "https://child.example.test/final".into()),
                        ("Set-Cookie".into(), "hop=value; Path=/; Secure".into()),
                        ("X-Redirect".into(), "first".into()),
                    ],
                    Vec::new(),
                )),
                "/final" if self.fail_final => Err(HttpParseError::Io(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "fixture final response failed",
                ))),
                "/final" => Ok(HttpResponse::new(
                    200,
                    "OK",
                    vec![("Content-Type".into(), "text/html".into())],
                    b"final".to_vec(),
                )),
                path => panic!("unexpected navigation path: {path}"),
            }
        }
    }

    fn operation(
        seen: Arc<Mutex<Vec<SeenRequest>>>,
        fail_final: bool,
        parent: Option<NavigationParentPolicy>,
    ) -> (OwnedNavigationTransport, Client, Arc<Mutex<CookieJar>>) {
        let cookies = Arc::new(Mutex::new(CookieJar::new()));
        let mut client = Client::new();
        client.set_shared_cookie_store(Arc::clone(&cookies));
        client.set_transport(Some(Arc::new(RedirectTransport { seen, fail_final })));
        let mut request = HttpRequest::get("https://child.example.test/start").unwrap();
        request.set_header("Referer", "https://owner.example.test/page");
        (
            OwnedNavigationTransport::new(request, parent),
            client,
            cookies,
        )
    }

    #[test]
    fn allowed_redirect_retains_metadata_referrer_and_cookie_order() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (operation, mut client, _) = operation(Arc::clone(&seen), false, None);
        let result = operation.run(&mut client);
        let response = result.response.unwrap().unwrap();

        assert_eq!(
            response.effective_url().unwrap().to_string(),
            "https://child.example.test/final"
        );
        assert_eq!(
            response.final_request_referrer(),
            Some("https://owner.example.test/page")
        );
        assert_eq!(result.redirects.len(), 1);
        assert_eq!(result.redirects[0].url, "https://child.example.test/start");
        assert_eq!(
            result.redirects[0].request_referrer,
            "https://owner.example.test/page"
        );
        assert!(
            result.redirects[0]
                .headers
                .iter()
                .any(|(name, value)| name == "X-Redirect" && value == "first")
        );
        assert!(result.violations.is_empty());
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                SeenRequest {
                    url: "https://child.example.test/start".into(),
                    cookie: None,
                },
                SeenRequest {
                    url: "https://child.example.test/final".into(),
                    cookie: Some("hop=value".into()),
                },
            ]
        );
    }

    #[test]
    fn enforced_corp_blocks_before_redirect_target_but_keeps_cookie_effect() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let owner: crate::http::Url = "https://owner.example.test/page".parse().unwrap();
        let parent = NavigationParentPolicy {
            origin: Origin::from_url(&owner),
            enforced: EmbedderPolicyValue::RequireCorp,
            report_only: EmbedderPolicyValue::UnsafeNone,
        };
        let (operation, mut client, cookies) = operation(Arc::clone(&seen), false, Some(parent));
        let result = operation.run(&mut client);

        assert!(matches!(&result.response, Ok(None)));
        assert!(result.redirects.is_empty());
        assert_eq!(result.violations.len(), 1);
        assert_eq!(
            result.violations[0].blocked_url,
            "https://child.example.test/start"
        );
        assert!(result.violations[0].enforced);
        assert!(!result.violations[0].report_only);
        assert_eq!(seen.lock().unwrap().len(), 1);
        let target: crate::http::Url = "https://child.example.test/final".parse().unwrap();
        assert_eq!(
            cookies.lock().unwrap().cookie_header(&target).as_deref(),
            Some("hop=value")
        );
    }

    #[test]
    fn transport_error_after_redirect_preserves_observed_metadata() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (operation, mut client, _) = operation(Arc::clone(&seen), true, None);
        let result = operation.run(&mut client);

        assert!(matches!(
            &result.response,
            Err(HttpParseError::Io(error)) if error.kind() == io::ErrorKind::ConnectionReset
        ));
        assert_eq!(result.redirects.len(), 1);
        assert_eq!(result.redirects[0].url, "https://child.example.test/start");
        assert!(result.violations.is_empty());
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[test]
    fn cancellation_before_start_performs_no_exchange() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (operation, mut client, _) = operation(Arc::clone(&seen), false, None);
        let result = operation.run_while(&mut client, || false);

        assert!(matches!(result.response, Ok(None)));
        assert!(seen.lock().unwrap().is_empty());
        assert!(result.redirects.is_empty());
        assert!(result.violations.is_empty());
    }

    #[test]
    fn cancellation_after_a_redirect_keeps_cookie_but_starts_no_next_hop() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (operation, mut client, cookies) = operation(Arc::clone(&seen), false, None);
        let mut checks = 0;
        let result = operation.run_while(&mut client, || {
            checks += 1;
            checks == 1
        });

        assert!(matches!(result.response, Ok(None)));
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert!(result.redirects.is_empty());
        let target: crate::http::Url = "https://child.example.test/final".parse().unwrap();
        assert_eq!(
            cookies.lock().unwrap().cookie_header(&target).as_deref(),
            Some("hop=value")
        );
    }
}
