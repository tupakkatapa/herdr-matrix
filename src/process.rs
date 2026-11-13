//! A program run to its end, or killed after a timeout.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::Result;

/// A finished process.
pub struct Ran {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> JoinHandle<String> {
    thread::spawn(move || {
        let mut out = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut out);
        }
        String::from_utf8_lossy(&out).into_owned()
    })
}

/// Runs a program to its end, or kills it after `timeout`: None.
///
/// # Errors
/// When the program cannot be started.
pub fn run_timed(program: &Path, args: &[&str], timeout: Duration) -> Result<Option<Ran>> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(5));
    };
    // A grandchild may hold the pipes open past a timeout; it is not waited for
    let Some(status) = status else {
        return Ok(None);
    };
    Ok(Some(Ran {
        code: status.code().unwrap_or(-1),
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    }))
}
