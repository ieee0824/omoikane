//! Lifecycle of the pinned upstream wptserve HTTP/HTTPS fixture process.
use super::model::FixtureServer;
use super::upstream_transport::{self, Description};
use omoikane::http::HttpTransport;
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub(super) struct UpstreamServer {
    pub(super) base_url: String,
    http_base_url: String,
    pub(super) transport: Arc<dyn HttpTransport>,
    pub(super) description: Arc<Description>,
    _process: ServerProcess,
}

struct ServerProcess {
    child: Child,
    directory: PathBuf,
}

fn artifact_directory() -> io::Result<PathBuf> {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join(".artifacts/wptserve");
    fs::create_dir_all(&parent)?;
    for _ in 0..1000 {
        let path = parent.join(format!(
            "server-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "cannot allocate WPT server evidence directory",
    ))
}

impl ServerProcess {
    fn start(root: &Path) -> io::Result<Self> {
        let directory = artifact_directory()?;
        let log = fs::File::create(directory.join("server.log"))?;
        let revision = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wpt/revision.txt"),
        )?;
        let child = Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wpt_smoke/wptserve.py"))
            .args([
                "--root",
                root.to_str()
                    .ok_or_else(|| io::Error::other("non-UTF8 WPT root"))?,
                "--revision",
                revision.trim(),
                "--ready-file",
            ])
            .arg(directory.join("ready.json"))
            .stdin(Stdio::piped())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .spawn()?;
        Ok(Self { child, directory })
    }

    fn description(&mut self) -> io::Result<Description> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match fs::read(self.directory.join("ready.json")) {
                Ok(source) => return serde_json::from_slice(&source).map_err(io::Error::other),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            if let Some(status) = self.child.try_wait()? {
                return Err(io::Error::other(format!(
                    "pinned wptserve exited {status}; see {} (run scripts/fetch-wpt.sh first)",
                    self.directory.join("server.log").display()
                )));
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "wptserve startup timed out; see {}",
                        self.directory.join("server.log").display()
                    ),
                ));
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl UpstreamServer {
    pub(super) fn case_base_url(&self, server: FixtureServer) -> &str {
        if server == FixtureServer::WptserveHttp {
            &self.http_base_url
        } else {
            &self.base_url
        }
    }

    pub(super) fn start(root: &Path) -> io::Result<Self> {
        let mut process = ServerProcess::start(root)?;
        let description = Arc::new(process.description()?);
        let base_url = description.base_url.clone();
        let origin: omoikane::http::Url = base_url.parse().map_err(io::Error::other)?;
        let port = description
            .http_ports
            .first()
            .filter(|port| **port != 0)
            .ok_or_else(|| io::Error::other("missing upstream HTTP port"))?;
        let http_base_url = format!("http://{}:{port}", origin.host());
        let transport = upstream_transport::transport(Arc::clone(&description))?;
        eprintln!(
            "WPT upstream HTTPS: {base_url}; HTTP: {http_base_url}; evidence {}",
            process.directory.display()
        );
        Ok(Self {
            base_url,
            http_base_url,
            transport,
            description,
            _process: process,
        })
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        if let Some(mut stdin) = self.child.stdin.take() {
            let _ = writeln!(stdin, "stop");
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => thread::sleep(Duration::from_millis(20)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
