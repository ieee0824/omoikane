//! Bounded loopback HTTP primitives shared by integration-test fixtures.

use std::io::{self, Read};
use std::net::{TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Maximum wait for the next request in the migrated fixtures.
pub const ACCEPT_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum wait for a complete request header in the migrated fixtures.
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Bind an ephemeral loopback port and prepare it for bounded accepts.
pub fn bind_loopback() -> io::Result<TcpListener> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// Accept one connection before `timeout`, returning a blocking stream.
pub fn accept_with_timeout(listener: &TcpListener, timeout: Duration) -> io::Result<TcpStream> {
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                // Accepted sockets can inherit nonblocking mode on BSD.
                stream.set_nonblocking(false)?;
                return Ok(stream);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "timed out waiting for HTTP request",
                    ));
                }
                thread::sleep(remaining.min(Duration::from_millis(10)));
            }
            Err(error) => return Err(error),
        }
    }
}

/// Read one request's headers without consuming a possible request body.
pub fn read_request_headers(stream: &mut TcpStream, timeout: Duration) -> io::Result<String> {
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out reading HTTP request headers",
            ));
        }
        stream.set_read_timeout(Some(remaining))?;
        let mut byte = [0];
        if let Err(error) = stream.read_exact(&mut byte) {
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "timed out reading HTTP request headers",
                ));
            }
            return Err(error);
        }
        bytes.push(byte[0]);
        if bytes.len() > 64 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP request headers exceed fixture limit",
            ));
        }
    }
    String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Join a fixture worker even if the client-side test unwinds first.
pub struct FixtureWorker<T: Send + 'static> {
    handle: Option<JoinHandle<T>>,
}

impl<T: Send + 'static> FixtureWorker<T> {
    /// Start a fixture worker that is joined on explicit completion or drop.
    pub fn spawn(worker: impl FnOnce() -> T + Send + 'static) -> Self {
        Self {
            handle: Some(thread::spawn(worker)),
        }
    }

    /// Return the worker's result and propagate any worker panic.
    pub fn join(mut self) -> T {
        self.handle.take().unwrap().join().unwrap()
    }
}

impl<T: Send + 'static> Drop for FixtureWorker<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            if let Err(panic) = handle.join() {
                if !thread::panicking() {
                    std::panic::resume_unwind(panic);
                }
            }
        }
    }
}
