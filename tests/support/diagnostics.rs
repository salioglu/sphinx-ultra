use std::io::Read;
use std::process::{Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_CAPTURE_BYTES: usize = 256 * 1024;
const TRUNCATED: &str = "[output truncated]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitStatusKind {
    Success,
    BuildError(i32),
    Timeout,
    SpawnError(String),
    IoError(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub status: ExitStatusKind,
    pub stdout: String,
    pub stderr: String,
}

pub fn run_bounded(command: Command) -> ProcessOutput {
    run_bounded_with_timeout(command, DEFAULT_TIMEOUT)
}

pub fn run_bounded_with_timeout(mut command: Command, timeout: Duration) -> ProcessOutput {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return ProcessOutput {
                status: ExitStatusKind::SpawnError(error.to_string()),
                stdout: String::new(),
                stderr: String::new(),
            }
        }
    };
    let stdout = match child.stdout.take() {
        Some(stdout) => spawn_reader(stdout),
        None => {
            return ProcessOutput {
                status: ExitStatusKind::IoError("child stdout pipe was unavailable".to_string()),
                stdout: String::new(),
                stderr: String::new(),
            }
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => spawn_reader(stderr),
        None => {
            return ProcessOutput {
                status: ExitStatusKind::IoError("child stderr pipe was unavailable".to_string()),
                stdout: String::new(),
                stderr: String::new(),
            }
        }
    };

    let deadline = Instant::now() + timeout;
    let mut status = None;
    let mut status_error = None;
    let timed_out = loop {
        match child.try_wait() {
            Ok(Some(exit)) => {
                status = Some(if exit.success() {
                    ExitStatusKind::Success
                } else {
                    ExitStatusKind::BuildError(exit.code().unwrap_or(-1))
                });
                break false;
            }
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                if let Err(error) = child.kill() {
                    status_error = Some(format!("kill child: {error}"));
                } else if let Err(error) = child.wait() {
                    status_error = Some(format!("wait after timeout: {error}"));
                }
                break true;
            }
            Err(error) => {
                status_error = Some(format!("wait for child: {error}"));
                if let Err(kill_error) = child.kill() {
                    status_error = Some(format!("{error}; kill child: {kill_error}"));
                }
                let _ = child.wait();
                break false;
            }
        }
    };

    let stdout = join_reader(stdout);
    let stderr = join_reader(stderr);
    let stdout_error = stdout.as_ref().err().cloned();
    let stderr_error = stderr.as_ref().err().cloned();
    let status = if let Some(error) = status_error {
        ExitStatusKind::IoError(error)
    } else if let Some(error) = stdout_error.or(stderr_error) {
        ExitStatusKind::IoError(error)
    } else if timed_out {
        ExitStatusKind::Timeout
    } else {
        status
            .unwrap_or_else(|| ExitStatusKind::IoError("child status was unavailable".to_string()))
    };
    let (stdout, stderr) = match (stdout, stderr) {
        (Ok(stdout), Ok(stderr)) => (stdout, stderr),
        (Err(error), Ok(stderr)) => (format!("{TRUNCATED}\nreader error: {error}"), stderr),
        (Ok(stdout), Err(error)) => (stdout, format!("{TRUNCATED}\nreader error: {error}")),
        (Err(stdout), Err(stderr)) => (
            format!("{TRUNCATED}\nreader error: {stdout}"),
            format!("{TRUNCATED}\nreader error: {stderr}"),
        ),
    };
    ProcessOutput {
        status,
        stdout,
        stderr,
    }
}

fn spawn_reader<R>(mut reader: R) -> JoinHandle<Result<String, String>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut retained = Vec::with_capacity(MAX_CAPTURE_BYTES);
        let mut buffer = [0u8; 8192];
        let mut truncated = false;
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|error| format!("read child output: {error}"))?;
            if read == 0 {
                break;
            }
            if retained.len() < MAX_CAPTURE_BYTES {
                let remaining = MAX_CAPTURE_BYTES - retained.len();
                let keep = remaining.min(read);
                retained.extend_from_slice(&buffer[..keep]);
                if keep < read {
                    truncated = true;
                }
            } else {
                truncated = true;
            }
        }
        let mut output = String::from_utf8_lossy(&retained).into_owned();
        if truncated {
            output.push_str(TRUNCATED);
        }
        Ok(output)
    })
}

fn join_reader(reader: JoinHandle<Result<String, String>>) -> Result<String, String> {
    reader
        .join()
        .map_err(|_| "output reader thread panicked".to_string())?
}
