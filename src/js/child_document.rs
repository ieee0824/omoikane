//! Results of loading a child browsing context's Document (iframe, object,
//! form submission targets and auxiliary windows).
use super::*;

/// A newly constructed child Document together with the response metadata
/// its commit needs. The value owns everything, so loading never borrows
/// `HostState` beyond the call that produced it.
#[derive(Debug)]
pub(super) struct LoadedChildDocument {
    pub(super) document: NodeHandle,
    /// The final response status, including HTTP error responses. Local
    /// `about:blank`, `about:srcdoc`, and `data:` navigation responses use 200;
    /// a fetch failure uses the network-error status 0.
    pub(super) response_status: u16,
    /// Referer on the final HTTP request; `None` for non-network Documents.
    pub(super) final_request_referrer: Option<String>,
    /// CSP, reporting endpoint, and referrer policy headers from the response.
    ///
    /// Empty when no response delivered the document (`about:blank`,
    /// `about:srcdoc`, `data:` URLs, fetch failures) and for GET responses
    /// whose content type is not rendered.
    pub(super) security_headers: Vec<(String, String)>,
    /// HTTP response fields retained for COOP, COEP and Permissions Policy.
    pub(super) response_headers: Vec<(String, String)>,
    /// Redirect responses in request order, checked before their next hop.
    pub(super) redirects: Vec<NavigationRedirect>,
    /// URL committed for the document: the response's effective URL (keeping
    /// the requested fragment), the `data:` URL itself, `about:blank` or
    /// `about:srcdoc`.
    ///
    /// `None` only when a GET reference could not be resolved or fetched;
    /// callers then commit `about:blank`.
    pub(super) url: Option<String>,
}

impl LoadedChildDocument {
    /// An empty HTML document committed at `url` (`about:blank` or a variant).
    pub(super) fn about_blank(url: String) -> Self {
        Self {
            document: blank_html_document(),
            response_status: 200,
            final_request_referrer: None,
            security_headers: Vec::new(),
            response_headers: Vec::new(),
            redirects: Vec::new(),
            url: Some(url),
        }
    }

    /// The document parsed from an iframe's `srcdoc` markup.
    pub(super) fn srcdoc(markup: &str) -> Self {
        Self {
            document: crate::html::TreeBuilder::parse(markup).document(),
            response_status: 200,
            final_request_referrer: None,
            security_headers: Vec::new(),
            response_headers: Vec::new(),
            redirects: Vec::new(),
            url: Some("about:srcdoc".to_owned()),
        }
    }

    /// The `about:blank` fallback for a GET that produced no resource.
    pub(super) fn fetch_failed() -> Self {
        Self {
            document: blank_html_document(),
            response_status: 0,
            final_request_referrer: None,
            security_headers: Vec::new(),
            response_headers: Vec::new(),
            redirects: Vec::new(),
            url: None,
        }
    }
}

/// One HTTP redirect's URL and policy fields, before its target is requested.
#[derive(Debug)]
pub(super) struct NavigationRedirect {
    pub(super) url: String,
    pub(super) headers: Vec<(String, String)>,
    /// Referer sent with the request that produced this response.
    pub(super) request_referrer: String,
}

/// Bytes obtained for a child document before parsing.
#[derive(Debug)]
pub(super) struct FetchedChildResource {
    /// The status of the final response before its body is consumed.
    pub(super) response_status: u16,
    /// Referer on the final HTTP request after redirects.
    pub(super) final_request_referrer: String,
    /// The response `Content-Type` (empty when absent) or the `data:` MIME type.
    pub(super) mime_type: String,
    /// The response body, adopted regardless of the HTTP status code.
    pub(super) body: Vec<u8>,
    /// See [`LoadedChildDocument::security_headers`].
    pub(super) security_headers: Vec<(String, String)>,
    /// All response fields, including COOP and COEP isolation policy.
    pub(super) response_headers: Vec<(String, String)>,
    /// Redirect responses preceding the final resource.
    pub(super) redirects: Vec<NavigationRedirect>,
    /// The response's effective URL (keeping the requested fragment) or the
    /// `data:` URL itself.
    pub(super) effective_url: String,
}

impl FetchedChildResource {
    /// Parses the resource for a GET navigation. A content type that is not
    /// rendered yields an empty document and drops the CSP headers, keeping
    /// only the committed URL.
    pub(super) fn into_get_document(self) -> LoadedChildDocument {
        match parse_child_response(&self.mime_type, &self.body, &self.response_headers) {
            Some(document) => LoadedChildDocument {
                document,
                response_status: self.response_status,
                final_request_referrer: Some(self.final_request_referrer),
                security_headers: self.security_headers,
                response_headers: self.response_headers,
                redirects: self.redirects,
                url: Some(self.effective_url),
            },
            None => LoadedChildDocument {
                document: blank_html_document(),
                response_status: self.response_status,
                final_request_referrer: Some(self.final_request_referrer),
                security_headers: Vec::new(),
                response_headers: self.response_headers,
                redirects: self.redirects,
                url: Some(self.effective_url),
            },
        }
    }
}

/// Identifies supported scriptable document signatures in an unknown response.
/// All inputs are explicit; response metadata and security policy stay with the
/// caller. Known MIME types are never promoted by inspecting their body.
pub(super) fn parse_child_response(
    mime_type: &str,
    body: &[u8],
    headers: &[(String, String)],
) -> Option<NodeHandle> {
    let sniffed = sniff_unknown_scriptable_type(mime_type, body, headers);
    if sniffed == Some("text/html") {
        // Type detection does not discard the transport's encoding metadata.
        let html = crate::html::encoding::decode_html_bytes(body, Some(mime_type));
        return Some(crate::html::TreeBuilder::parse_decoded(&html).document());
    }
    parse_child_document(sniffed.unwrap_or(mime_type), body)
}

fn sniff_unknown_scriptable_type(
    supplied: &str,
    body: &[u8],
    headers: &[(String, String)],
) -> Option<&'static str> {
    let essence = supplied.split(';').next().unwrap_or("").trim();
    let unknown = essence.is_empty()
        || ["unknown/unknown", "application/unknown", "*/*"]
            .iter()
            .any(|unknown| essence.eq_ignore_ascii_case(unknown))
        || !valid_mime_essence(essence);
    let no_sniff = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("x-content-type-options"))
        .flat_map(|(_, value)| value.split(','))
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("nosniff"));
    if !unknown || no_sniff {
        return None;
    }
    // MIME Sniffing §5.2 / §7.1: inspect the resource header, skipping only
    // the specified five whitespace bytes and requiring an exact tag boundary.
    // https://mimesniff.spec.whatwg.org/#identifying-a-resource-with-an-unknown-mime-type
    let header = &body[..body.len().min(1445)];
    let first = header
        .iter()
        .position(|byte| !matches!(byte, b'\t' | b'\n' | b'\x0c' | b'\r' | b' '))
        .unwrap_or(header.len());
    let header = &header[first..];
    for signature in [
        "<!DOCTYPE HTML",
        "<HTML",
        "<HEAD",
        "<SCRIPT",
        "<IFRAME",
        "<H1",
        "<DIV",
        "<FONT",
        "<TABLE",
        "<A",
        "<STYLE",
        "<TITLE",
        "<B",
        "<BODY",
        "<BR",
        "<P",
        "<!--",
    ] {
        if header
            .get(..signature.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(signature.as_bytes()))
            && matches!(header.get(signature.len()), Some(b' ' | b'>'))
        {
            return Some("text/html");
        }
    }
    header.starts_with(b"<?xml").then_some("text/xml")
}

fn valid_mime_essence(essence: &str) -> bool {
    let Some((kind, subtype)) = essence.split_once('/') else {
        return false;
    };
    [kind, subtype].iter().all(|part| {
        !part.is_empty()
            && part.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(
                        byte,
                        b'!' | b'#'
                            | b'$'
                            | b'%'
                            | b'&'
                            | b'\''
                            | b'*'
                            | b'+'
                            | b'-'
                            | b'.'
                            | b'^'
                            | b'_'
                            | b'`'
                            | b'|'
                            | b'~'
                    )
            })
    })
}

/// Builds a document from a rendered child resource: HTML (decoded with the
/// transport charset or in-document meta), XML including SVG, and
/// `text/plain` shown as text. Other content types such as images return
/// `None`; malformed XML becomes an empty HTML document.
pub(super) fn parse_child_document(mime_type: &str, body: &[u8]) -> Option<NodeHandle> {
    if is_html_mime_type(mime_type) {
        let html = crate::html::encoding::decode_html_bytes(body, Some(mime_type));
        return Some(crate::html::TreeBuilder::parse_decoded(&html).document());
    }
    if is_xml_mime_type(mime_type) {
        return Some(match crate::xml::parse(body) {
            Ok(document) => {
                let essence = mime_type.split(';').next().unwrap_or("").trim();
                document.set_document_content_type(essence.to_ascii_lowercase());
                document
            }
            Err(_) => blank_html_document(),
        });
    }
    let essence = mime_type.split(';').next().unwrap_or("").trim();
    essence
        .eq_ignore_ascii_case("text/plain")
        .then(|| form_submission::plain_text_document(body))
}

/// Returns security and referrer policy headers, preserving repeated values.
pub(super) fn response_security_headers(
    response: &crate::http::HttpResponse,
) -> Vec<(String, String)> {
    response
        .headers()
        .iter()
        .filter(|(name, _)| {
            matches!(
                name.to_ascii_lowercase().as_str(),
                "content-security-policy"
                    | "content-security-policy-report-only"
                    | "permissions-policy"
                    | "reporting-endpoints"
                    | "referrer-policy"
            )
        })
        .cloned()
        .collect()
}
