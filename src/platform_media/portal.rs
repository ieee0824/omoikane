//! Process boundary for the Linux desktop portal helper.
use std::io::{self, Read};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Launches the portal helper with an owner-enforced process and output deadline.
/// The helper emits one small settings response; stderr is intentionally ignored.
pub(super) fn output(mut command: Command, timeout: Duration) -> Option<Output> {
    let started = Instant::now();
    let (mut reader, writer) = UnixStream::pair().ok()?;
    reader.set_nonblocking(true).ok()?;
    let mut child = command
        .stdout(Stdio::from(OwnedFd::from(writer)))
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Command retains its configured output fd after spawn. Close that copy
    // so EOF reflects the helper and its descendants rather than this owner.
    drop(command);
    let mut bytes = Vec::new();
    let mut finished_output = false;
    let mut status = None;
    loop {
        if started.elapsed() >= timeout {
            stop(&mut child);
            return None;
        }
        if !finished_output {
            match read_available(&mut reader, &mut bytes) {
                Ok(eof) => finished_output = eof,
                Err(_) => {
                    stop(&mut child);
                    return None;
                }
            }
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(next) => status = next,
                Err(_) => {
                    stop(&mut child);
                    return None;
                }
            }
        }
        if started.elapsed() < timeout && finished_output {
            if let Some(status) = status {
                return Some(Output {
                    status,
                    stdout: bytes,
                    stderr: Vec::new(),
                });
            }
        }
        std::thread::sleep(
            timeout
                .saturating_sub(started.elapsed())
                .min(Duration::from_millis(10)),
        );
    }
}

fn read_available(reader: &mut UnixStream, bytes: &mut Vec<u8>) -> io::Result<bool> {
    let mut buffer = [0; 256];
    for _ in 0..32 {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(length) => {
                if bytes.len() + length > 4096 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "portal response too large",
                    ));
                }
                bytes.extend_from_slice(&buffer[..length]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_success_is_rejected_after_process_deadline() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 0.2; printf '(<uint32 1>,)'"]);
        assert!(output(command, Duration::from_millis(20)).is_none());
    }

    #[test]
    fn exited_tool_with_inherited_stdout_still_respects_deadline() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 0.2 & printf '(<uint32 1>,)'"]);
        assert!(output(command, Duration::from_millis(20)).is_none());
    }

    #[test]
    fn completed_response_is_preserved() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf '(<uint32 2>,)'"]);
        let result = output(command, Duration::from_secs(5)).unwrap();
        assert!(result.status.success());
        assert_eq!(result.stdout, b"(<uint32 2>,)");
    }

    #[test]
    fn failed_tool_does_not_become_a_successful_response() {
        let mut command = Command::new("sh");
        command.args(["-c", "exit 7"]);
        let result = output(command, Duration::from_secs(5)).unwrap();
        assert_eq!(result.status.code(), Some(7));
    }
}
