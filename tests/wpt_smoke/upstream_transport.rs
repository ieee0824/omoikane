//! Fixture-only origin routing over real HTTP/TLS with the pinned WPT CA.
use base64::Engine;
use omoikane::http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport};
use rustls::pki_types::{CertificateDer, ServerName};
use serde::Deserialize;
use std::{
    fs,
    io::{self, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Description {
    pub(super) base_url: String,
    pub(super) http_ports: Vec<u16>,
    pub(super) https_ports: Vec<u16>,
    pub(super) hosts: Vec<String>,
    pub(super) ca_certificate: PathBuf,
}

struct UpstreamTransport {
    description: Arc<Description>,
    tls: Arc<rustls::ClientConfig>,
}

fn invalid(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

pub(super) fn transport(description: Arc<Description>) -> io::Result<Arc<dyn HttpTransport>> {
    if description.hosts.is_empty()
        || description
            .http_ports
            .iter()
            .chain(&description.https_ports)
            .any(|port| *port == 0)
    {
        return Err(invalid("invalid WPT fixture origins"));
    }
    let pem = fs::read_to_string(&description.ca_certificate)?;
    let encoded = pem
        .split_once("-----BEGIN CERTIFICATE-----")
        .and_then(|(_, text)| text.split_once("-----END CERTIFICATE-----"))
        .map(|(text, _)| text.split_ascii_whitespace().collect::<String>())
        .ok_or_else(|| invalid("missing WPT CA certificate"))?;
    let der = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(invalid)?;
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(der)).map_err(invalid)?;
    Ok(configured(description, roots))
}

fn configured(
    description: Arc<Description>,
    roots: rustls::RootCertStore,
) -> Arc<dyn HttpTransport> {
    let tls = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Arc::new(UpstreamTransport {
        description,
        tls: Arc::new(tls),
    })
}

impl UpstreamTransport {
    fn send_checked(
        &self,
        request: &HttpRequest,
        timeout: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        let url = request.url();
        let ports = match url.scheme() {
            "http" => &self.description.http_ports,
            "https" => &self.description.https_ports,
            _ => {
                return Err(HttpParseError::Io(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "unsupported WPT protocol",
                )));
            }
        };
        if !self.description.hosts.iter().any(|host| host == url.host())
            || !ports.contains(&url.port())
        {
            return Err(HttpParseError::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "destination outside configured WPT origins",
            )));
        }
        let timeout = timeout.unwrap_or(super::http_fixture::READ_TIMEOUT);
        let endpoint = SocketAddr::from((Ipv4Addr::LOCALHOST, url.port()));
        let stream = TcpStream::connect_timeout(&endpoint, timeout).map_err(HttpParseError::Io)?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(HttpParseError::Io)?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(HttpParseError::Io)?;
        if url.scheme() == "https" {
            let name = ServerName::try_from(url.host().to_owned())
                .map_err(|error| HttpParseError::Io(invalid(error)))?;
            let session = rustls::ClientConnection::new(Arc::clone(&self.tls), name)
                .map_err(|error| HttpParseError::Io(invalid(error)))?;
            let mut tls = rustls::StreamOwned::new(session, stream);
            tls.write_all(&request.serialize())
                .map_err(HttpParseError::Io)?;
            HttpResponse::parse_for_browser(&mut tls)
        } else {
            let mut stream = stream;
            stream
                .write_all(&request.serialize())
                .map_err(HttpParseError::Io)?;
            HttpResponse::parse_for_browser(&mut stream)
        }
    }
}

impl HttpTransport for UpstreamTransport {
    fn send(
        &self,
        request: &HttpRequest,
        timeout: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        let result = self.send_checked(request, timeout);
        if std::env::var_os("WPT_TRACE_HTTP").is_some() {
            match &result {
                Ok(response) => eprintln!(
                    "WPT HTTP {}://{}{} -> {} bytes={}",
                    request.url().scheme(),
                    request.url().host(),
                    request.url().path(),
                    response.status_code(),
                    response.body().len()
                ),
                Err(error) => eprintln!(
                    "WPT HTTP {}://{}{} -> {error}",
                    request.url().scheme(),
                    request.url().host(),
                    request.url().path()
                ),
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::super::upstream::UpstreamServer;
    use super::*;

    fn pinned_root() -> Option<PathBuf> {
        let root =
            std::env::var_os("WPT_ROOT").map_or_else(|| PathBuf::from("target/wpt"), PathBuf::from);
        if root.join("tools/wptserve/wptserve/server.py").is_file() {
            return Some(root);
        }
        assert!(
            std::env::var_os("WPT_REQUIRED").is_none(),
            "fetch the pinned WPT server first"
        );
        eprintln!("WPT TLS fixture test skipped: pinned server checkout unavailable");
        None
    }

    fn certificate_error(error: &HttpParseError) -> Option<&rustls::CertificateError> {
        let HttpParseError::Io(error) = error else {
            return None;
        };
        let mut source: &(dyn std::error::Error + 'static) = error.get_ref()?;
        loop {
            if let Some(rustls::Error::InvalidCertificate(error)) =
                source.downcast_ref::<rustls::Error>()
            {
                return Some(error);
            }
            source = source.source()?;
        }
    }

    #[test]
    fn upstream_transport_rejects_undeclared_hosts_and_protocol_ports_before_io() {
        let description = Arc::new(Description {
            base_url: "https://web-platform.test:8443".into(),
            http_ports: vec![8000],
            https_ports: vec![8443],
            hosts: vec!["web-platform.test".into()],
            ca_certificate: "unused".into(),
        });
        let transport = configured(description, rustls::RootCertStore::empty());
        for url in [
            "https://outside.invalid:8443/",
            "http://web-platform.test:8443/",
            "https://web-platform.test:8000/",
        ] {
            let error = transport
                .send(&HttpRequest::get(url).unwrap(), None)
                .unwrap_err();
            assert!(
                matches!(error, HttpParseError::Io(error) if error.kind()==io::ErrorKind::PermissionDenied),
                "{url}"
            );
        }
    }

    #[test]
    fn upstream_http_primary_preserves_origin_templates_and_response_headers() {
        let Some(root) = pinned_root() else {
            return;
        };
        let server = UpstreamServer::start(&root).unwrap();
        let base = server.case_base_url(super::super::model::FixtureServer::WptserveHttp);
        let origin: omoikane::http::Url = base.parse().unwrap();
        assert_eq!(origin.scheme(), "http");
        assert_eq!(origin.host(), "web-platform.test");
        assert_eq!(origin.port(), server.description.http_ports[0]);
        assert_eq!(
            server.case_base_url(super::super::model::FixtureServer::Wptserve),
            server.base_url
        );
        assert_eq!(
            server.case_base_url(super::super::model::FixtureServer::Static),
            server.base_url
        );
        let response = server.transport.send(&HttpRequest::get(&format!(
            "{base}/storage-access-api/hasStorageAccess-insecure.sub.window.js?pipe=header(X-Omoikane-HTTP,true)"
        )).unwrap(), None).unwrap();
        assert_eq!(response.status_code(), 200);
        assert_eq!(response.header("X-Omoikane-HTTP"), Some("true"));
        let source = std::str::from_utf8(response.body()).unwrap();
        assert!(source.contains(&format!("http://www.web-platform.test:{}", origin.port())));
        assert!(!source.contains("{{"));
        let document = omoikane::html::TreeBuilder::parse("<!doctype html>").document();
        let mut runtime = omoikane::js::JsRuntime::with_document_sandbox_and_url(
            document,
            omoikane::js::SandboxConfig::default(),
            base,
        )
        .unwrap();
        assert_eq!(
            runtime.eval("isSecureContext").unwrap().as_boolean(),
            Some(false)
        );
        assert_eq!(
            runtime
                .eval("location.protocol + '//' + location.host")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            base
        );
    }

    #[test]
    fn upstream_tls_validates_templates_headers_and_shared_python_state() {
        let Some(root) = pinned_root() else {
            return;
        };
        let server = UpstreamServer::start(&root).unwrap();
        let get = |url: &str| {
            server
                .transport
                .send(&HttpRequest::get(url).unwrap(), None)
                .unwrap()
        };
        let source = get(&format!(
            "{}/common/security-features/resources/common.sub.js",
            server.base_url
        ));
        assert_eq!(source.status_code(), 200);
        let body = std::str::from_utf8(source.body()).unwrap();
        assert!(body.contains("www1.web-platform.test"));
        assert!(!body.contains("{{host}}") && !body.contains("{{ports[https][0]}}"));
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair(
                "pipe",
                "header(Content-Security-Policy,require-trusted-types-for 'script',True)",
            )
            .finish();
        let response = get(&format!(
            "{}/trusted-types/navigate-to-javascript-url-008.html?{query}",
            server.base_url
        ));
        let policies: Vec<_> = response
            .headers()
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("Content-Security-Policy"))
            .map(|(_, value)| value.as_str())
            .collect();
        assert!(policies.contains(&"connect-src 'none';"));
        assert!(policies.contains(&"require-trusted-types-for 'script'"));
        assert_eq!(
            response.header("Content-Security-Policy-Report-Only"),
            Some("require-trusted-types-for 'script'")
        );
        let key = "&key=00000000-0000-4000-8000-000000000001&path=/omoikane-native-fixture";
        let http = format!(
            "http://web-platform.test:{}/common/security-features/subresource/worker.py",
            server.description.http_ports[0]
        );
        let other = format!(
            "https://www1.web-platform.test:{}/common/security-features/subresource/worker.py",
            server.description.https_ports[0]
        );
        let status = |url: String| {
            serde_json::from_slice::<serde_json::Value>(get(&url).body()).unwrap()["status"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert_eq!(
            status(format!("{http}?action=put&value=present{key}")),
            "success"
        );
        assert_eq!(status(format!("{other}?action=take{key}")), "blocked");
        assert_eq!(status(format!("{http}?action=take{key}")), "allowed");
    }

    #[test]
    fn upstream_tls_rejects_an_untrusted_ca() {
        let Some(root) = pinned_root() else {
            return;
        };
        let server = UpstreamServer::start(&root).unwrap();
        let transport = configured(
            Arc::clone(&server.description),
            rustls::RootCertStore::empty(),
        );
        let error = transport
            .send(&HttpRequest::get(&server.base_url).unwrap(), None)
            .unwrap_err();
        assert!(
            matches!(
                certificate_error(&error),
                Some(rustls::CertificateError::UnknownIssuer)
            ),
            "{error:?}"
        );
    }

    #[test]
    fn upstream_tls_rejects_a_certificate_hostname_mismatch() {
        let Some(root) = pinned_root() else {
            return;
        };
        let server = UpstreamServer::start(&root).unwrap();
        let mut description = (*server.description).clone();
        description.hosts = vec!["outside.invalid".into()];
        let port = description.https_ports[0];
        let transport = transport(Arc::new(description)).unwrap();
        let error = transport
            .send(
                &HttpRequest::get(&format!("https://outside.invalid:{port}/")).unwrap(),
                None,
            )
            .unwrap_err();
        assert!(
            matches!(
                certificate_error(&error),
                Some(rustls::CertificateError::NotValidForName)
                    | Some(rustls::CertificateError::NotValidForNameContext { .. })
            ),
            "{error:?}"
        );
    }
}
