//! Results of loading a child browsing context's Document (iframe, object,
//! form submission targets and auxiliary windows).
use super::*;

/// A newly constructed child Document together with the response metadata
/// its commit needs. The value owns everything, so loading never borrows
/// `HostState` beyond the call that produced it.
#[derive(Debug)]
pub(super) struct LoadedChildDocument {
    pub(super) document: NodeHandle,
    /// `Content-Security-Policy` header values from the HTTP response.
    ///
    /// Empty when no response delivered the document (`about:blank`,
    /// `about:srcdoc`, `data:` URLs, fetch failures) and for GET responses
    /// whose content type is not rendered.
    pub(super) csp_headers: Vec<String>,
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
            csp_headers: Vec::new(),
            url: Some(url),
        }
    }

    /// The document parsed from an iframe's `srcdoc` markup.
    pub(super) fn srcdoc(markup: &str) -> Self {
        Self {
            document: crate::html::TreeBuilder::parse(markup).document(),
            csp_headers: Vec::new(),
            url: Some("about:srcdoc".to_owned()),
        }
    }

    /// The `about:blank` fallback for a GET that produced no resource.
    pub(super) fn fetch_failed() -> Self {
        Self {
            document: blank_html_document(),
            csp_headers: Vec::new(),
            url: None,
        }
    }
}

/// Bytes obtained for a child document before parsing.
#[derive(Debug)]
pub(super) struct FetchedChildResource {
    /// The response `Content-Type` (empty when absent) or the `data:` MIME type.
    pub(super) mime_type: String,
    /// The response body, adopted regardless of the HTTP status code.
    pub(super) body: Vec<u8>,
    /// See [`LoadedChildDocument::csp_headers`].
    pub(super) csp_headers: Vec<String>,
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
                csp_headers: self.csp_headers,
                url: Some(self.effective_url),
            },
            None => LoadedChildDocument {
                document: blank_html_document(),
                csp_headers: Vec::new(),
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

/// Returns every `Content-Security-Policy` header value of `response`.
pub(super) fn response_csp_headers(response: &crate::http::HttpResponse) -> Vec<String> {
    response
        .headers()
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("content-security-policy"))
        .map(|(_, value)| value.clone())
        .collect()
}
