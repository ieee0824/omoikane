//! Explicit fixture transport and original wptserve helper execution.
use super::http_fixture::READ_TIMEOUT;
use omoikane::http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport, Url};
use std::{
    io::{self, Write},
    net::{SocketAddr, TcpStream},
    path::Path,
    process::Command,
    sync::Arc,
};

struct FixtureTransport {
    host: String,
    endpoint: SocketAddr,
}
impl HttpTransport for FixtureTransport {
    fn send(
        &self,
        request: &HttpRequest,
        timeout: Option<std::time::Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        let url = request.url();
        if url.scheme() != "http"
            || url.port() != self.endpoint.port()
            || ![self.host.as_str(), &format!("www1.{}", self.host)].contains(&url.host())
        {
            return Err(HttpParseError::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "destination outside configured WPT origins",
            )));
        }
        let timeout = timeout.unwrap_or(READ_TIMEOUT);
        let mut stream =
            TcpStream::connect_timeout(&self.endpoint, timeout).map_err(HttpParseError::Io)?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(HttpParseError::Io)?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(HttpParseError::Io)?;
        stream
            .write_all(&request.serialize())
            .map_err(HttpParseError::Io)?;
        HttpResponse::parse(&mut stream)
    }
}

pub(super) fn transport(base_url: &str) -> Arc<dyn HttpTransport> {
    let url: Url = base_url.parse().expect("fixture URL");
    assert_eq!(url.host(), "localhost");
    let endpoint = SocketAddr::from(([127, 0, 0, 1], url.port()));
    assert!(endpoint.ip().is_loopback());
    Arc::new(FixtureTransport {
        host: url.host().to_owned(),
        endpoint,
    })
}

pub(super) fn serve(
    stream: &mut TcpStream,
    root: &Path,
    target: &str,
    secondary_port: u16,
) -> bool {
    let path = target.split('?').next().unwrap_or(target);
    if path == "/cors/support.js" {
        let Ok(source) = std::fs::read(root.join("cors/support.js")) else {
            return false;
        };
        let source = String::from_utf8(source)
            .expect("CORS support UTF-8")
            .replace("{{ports[http][1]}}", &secondary_port.to_string())
            .replace("{{ports[https][0]}}", "0");
        super::server::respond(stream, 200, "text/javascript", source.as_bytes());
        return true;
    }
    if path
        != "/html/webappapis/scripting/processing-model-2/unhandled-promise-rejections/support/promise-access-control.py"
    {
        return false;
    }
    // Execute the pinned helper itself, adapting only wptserve's GET interface.
    let output = Command::new("python3").args(["-c", r#"
import json, runpy, sys, urllib.parse
class Get:
    def first(self, key, default):
        values = urllib.parse.parse_qs(sys.argv[2], keep_blank_values=True)
        return values.get(key.decode(), [default.decode()])[0].encode()
class Request:
    GET = Get()
headers, body = runpy.run_path(sys.argv[1])['main'](Request(), None)
print(json.dumps({'headers': [[k.decode('latin1'), v.decode('latin1')] for k,v in headers], 'body': list(body)}))
"#]).arg(root.join(path.trim_start_matches('/'))).arg(target.split_once('?').map_or("", |(_, query)| query)).output().expect("run original WPT helper");
    assert!(
        output.status.success(),
        "WPT Python helper failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("helper response");
    let body: Vec<u8> = result["body"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| u8::try_from(value.as_u64().unwrap()).unwrap())
        .collect();
    let mut header = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for pair in result["headers"].as_array().unwrap() {
        header.push_str(&format!(
            "{}: {}\r\n",
            pair[0].as_str().unwrap(),
            pair[1].as_str().unwrap()
        ));
    }
    header.push_str("\r\n");
    stream.write_all(header.as_bytes()).unwrap();
    stream.write_all(&body).unwrap();
    true
}

#[test]
fn fixture_transport_rejects_unconfigured_hosts_ports_and_https() {
    let transport = transport("http://localhost:12345");
    for url in [
        "http://localhost:12346/",
        "http://www2.localhost:12345/",
        "https://www1.localhost:12345/",
    ] {
        let request = HttpRequest::new(omoikane::http::Method::Get, url.parse().unwrap());
        assert!(
            matches!(transport.send(&request, None), Err(HttpParseError::Io(error)) if error.kind()==io::ErrorKind::PermissionDenied)
        );
    }
}
