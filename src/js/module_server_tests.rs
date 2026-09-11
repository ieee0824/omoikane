use super::{join_module_workers, read_module_request};
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[test]
fn accepted_nonblocking_socket_waits_for_complete_request_headers() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut accepted, _) = listener.accept().unwrap();
    // Model BSD's inherited socket mode on every host. Before the fix the
    // handler's first read fails immediately while the client has sent no data.
    accepted.set_nonblocking(true).unwrap();
    assert_eq!(
        accepted.read(&mut [0; 1]).unwrap_err().kind(),
        ErrorKind::WouldBlock
    );
    let (ready_tx, ready_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        ready_tx.send(()).unwrap();
        result_tx.send(read_module_request(&mut accepted)).unwrap();
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        result_rx
            .recv_timeout(Duration::from_millis(50))
            .unwrap_err(),
        mpsc::RecvTimeoutError::Timeout
    );
    client
        .write_all(b"GET /body HTTP/1.1\r\nHost: test\r\n")
        .unwrap();
    assert_eq!(
        result_rx
            .recv_timeout(Duration::from_millis(50))
            .unwrap_err(),
        mpsc::RecvTimeoutError::Timeout
    );
    client.write_all(b"\r\n").unwrap();
    assert_eq!(
        result_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap(),
        "GET /body HTTP/1.1\r\nHost: test\r\n\r\n"
    );
    reader.join().unwrap();
}

#[test]
fn worker_failure_fails_the_test_but_preserves_an_existing_panic() {
    let failure = std::panic::catch_unwind(|| {
        join_module_workers([thread::spawn(|| panic!("server failed"))]);
    })
    .unwrap_err();
    assert_eq!(failure.downcast_ref::<&str>(), Some(&"server failed"));

    struct JoinOnDrop(Option<thread::JoinHandle<()>>);
    impl Drop for JoinOnDrop {
        fn drop(&mut self) {
            join_module_workers(self.0.take());
        }
    }
    let failure = std::panic::catch_unwind(|| {
        let _guard = JoinOnDrop(Some(thread::spawn(|| panic!("server also failed"))));
        panic!("original test assertion");
    })
    .unwrap_err();
    assert_eq!(
        failure.downcast_ref::<&str>(),
        Some(&"original test assertion")
    );
}
