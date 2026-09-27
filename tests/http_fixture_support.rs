#[path = "support/http_fixture.rs"]
mod http_fixture;

use http_fixture::{
    ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
    read_request_headers,
};
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

#[test]
fn accept_stops_when_no_client_connects() {
    let listener = bind_loopback().unwrap();
    let error = accept_with_timeout(&listener, Duration::from_millis(50)).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::TimedOut);
}

#[test]
fn request_header_read_stops_when_client_stalls() {
    let listener = bind_loopback().unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let mut server_stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
    client.write_all(b"GET / HTTP/1.1\r\n").unwrap();
    let error = read_request_headers(&mut server_stream, Duration::from_millis(50)).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::TimedOut);
}

#[test]
fn request_header_read_preserves_request_line_and_headers() {
    let listener = bind_loopback().unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let mut server_stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
    client
        .write_all(b"GET /check HTTP/1.1\r\nCookie: visible=1\r\n\r\nbody")
        .unwrap();
    assert_eq!(
        read_request_headers(&mut server_stream, READ_TIMEOUT).unwrap(),
        "GET /check HTTP/1.1\r\nCookie: visible=1\r\n\r\n"
    );
    let mut body = [0; 4];
    server_stream.read_exact(&mut body).unwrap();
    assert_eq!(&body, b"body");
}

#[test]
fn worker_is_joined_when_client_side_panics() {
    let listener = bind_loopback().unwrap();
    let completed = Arc::new(AtomicBool::new(false));
    let worker_completed = completed.clone();
    let failure = std::panic::catch_unwind(move || {
        let _worker = FixtureWorker::spawn(move || {
            let error = accept_with_timeout(&listener, Duration::from_millis(50)).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::TimedOut);
            worker_completed.store(true, Ordering::SeqCst);
        });
        panic!("client fixture failed");
    });
    assert!(failure.is_err());
    assert!(completed.load(Ordering::SeqCst));
}

#[test]
fn explicit_join_returns_worker_result() {
    assert_eq!(FixtureWorker::spawn(|| 42).join(), 42);
}
