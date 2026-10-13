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
    /// Enforced/monitoring CSP and reporting endpoint headers from the response.
    ///
    /// Empty when no response delivered the document (`about:blank`,
    /// `about:srcdoc`, `data:` URLs, fetch failures) and for GET responses
    /// whose content type is not rendered.
    pub(super) security_headers: Vec<(String, String)>,
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
            security_headers: Vec::new(),
            url: Some(url),
        }
    }

    /// The document parsed from an iframe's `srcdoc` markup.
    pub(super) fn srcdoc(markup: &str) -> Self {
        Self {
            document: crate::html::TreeBuilder::parse(markup).document(),
            response_status: 200,
            security_headers: Vec::new(),
            url: Some("about:srcdoc".to_owned()),
        }
    }

    /// The `about:blank` fallback for a GET that produced no resource.
    pub(super) fn fetch_failed() -> Self {
        Self {
            document: blank_html_document(),
            response_status: 0,
            security_headers: Vec::new(),
            url: None,
        }
    }
}

/// Bytes obtained for a child document before parsing.
#[derive(Debug)]
pub(super) struct FetchedChildResource {
    /// The status of the final response before its body is consumed.
    pub(super) response_status: u16,
    /// The response `Content-Type` (empty when absent) or the `data:` MIME type.
    pub(super) mime_type: String,
    /// The response body, adopted regardless of the HTTP status code.
    pub(super) body: Vec<u8>,
    /// See [`LoadedChildDocument::security_headers`].
    pub(super) security_headers: Vec<(String, String)>,
    /// The response's effective URL (keeping the requested fragment) or the
    /// `data:` URL itself.
    pub(super) effective_url: String,
}

impl FetchedChildResource {
    /// Parses the resource for a GET navigation. A content type that is not
    /// rendered yields an empty document and drops the CSP headers, keeping
    /// only the committed URL.
    pub(super) fn into_get_document(self) -> LoadedChildDocument {
        match parse_child_document(&self.mime_type, &self.body) {
            Some(document) => LoadedChildDocument {
                document,
                response_status: self.response_status,
                security_headers: self.security_headers,
                url: Some(self.effective_url),
            },
            None => LoadedChildDocument {
                document: blank_html_document(),
                response_status: self.response_status,
                security_headers: Vec::new(),
                url: Some(self.effective_url),
            },
        }
    }
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

/// Returns policy and reporting headers, preserving names and repeated values.
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
                    | "reporting-endpoints"
            )
        })
        .cloned()
        .collect()
}
