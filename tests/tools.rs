//! The programs the bot runs: herdr, ffmpeg and whisper-cli, as scripts.
//! One test at a time, so no test forks while another writes a script it
//! runs.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use herdr_matrix::herdr::{Herdr as _, RunHerdr};
use herdr_matrix::voice::{Bins, TOOL_TIMEOUT, whisper};

#[path = "../src/testutil.rs"]
mod testutil;
use testutil::TempDir;

static SERIAL: Mutex<()> = Mutex::new(());

/// One test at a time; a test that failed does not fail the others.
fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

fn bins(dir: &Path) -> Bins {
    Bins {
        ffmpeg: dir.join("ffmpeg"),
        whisper: dir.join("whisper-cli"),
        timeout: TOOL_TIMEOUT,
    }
}

/// An ffmpeg that writes its last argument.
const FFMPEG: &str = r#"for a; do out=$a; done; echo wav > "$out""#;

fn audio(dir: &Path) -> PathBuf {
    let audio = dir.join("a.ogg");
    fs::write(&audio, "").expect("write");
    audio
}

#[test]
fn whisper_cleans_up() {
    let _serial = serial();
    let dir = TempDir::new();
    script(dir.path(), "ffmpeg", FFMPEG);
    script(
        dir.path(),
        "whisper-cli",
        r#"echo " [BLANK_AUDIO]"; echo " Hello,"; echo " world.""#,
    );
    let audio = audio(dir.path());
    assert_eq!(
        whisper(&bins(dir.path()), &audio, "model", "auto").expect("whisper"),
        "Hello, world."
    );
    assert!(!dir.path().join("a.ogg.wav").exists());
}

#[test]
fn whisper_takes_a_language() {
    let _serial = serial();
    let dir = TempDir::new();
    script(dir.path(), "ffmpeg", FFMPEG);
    script(
        dir.path(),
        "whisper-cli",
        r#"while [ "$1" != -l ]; do shift; done; echo " Language $2.""#,
    );
    let audio = audio(dir.path());
    assert_eq!(
        whisper(&bins(dir.path()), &audio, "model", "en").expect("whisper"),
        "Language en."
    );
}

#[test]
fn failed_whisper_cleans_up() {
    let _serial = serial();
    let dir = TempDir::new();
    script(dir.path(), "ffmpeg", FFMPEG);
    script(dir.path(), "whisper-cli", "echo err >&2\nexit 1");
    let audio = audio(dir.path());
    let err = whisper(&bins(dir.path()), &audio, "model", "auto").expect_err("fails");
    assert_eq!(err.to_string(), "whisper-cli: err");
    assert!(!dir.path().join("a.ogg.wav").exists());
}

#[test]
fn failed_ffmpeg_cleans_up_and_hears_nothing() {
    let _serial = serial();
    let dir = TempDir::new();
    let log = dir.path().join("log");
    script(
        dir.path(),
        "ffmpeg",
        r#"for a; do out=$a; done; echo part > "$out"; echo "bad input" >&2; exit 1"#,
    );
    script(
        dir.path(),
        "whisper-cli",
        &format!("echo ran >> {}", log.display()),
    );
    let audio = audio(dir.path());
    let err = whisper(&bins(dir.path()), &audio, "model", "auto").expect_err("fails");
    assert_eq!(err.to_string(), "ffmpeg: bad input");
    assert!(!dir.path().join("a.ogg.wav").exists());
    assert!(!log.exists());
}

#[test]
fn hung_tool_is_named() {
    let _serial = serial();
    let dir = TempDir::new();
    script(dir.path(), "ffmpeg", "sleep 5");
    let audio = audio(dir.path());
    let bins = Bins {
        timeout: Duration::from_millis(200),
        ..bins(dir.path())
    };
    let started = Instant::now();
    let err = whisper(&bins, &audio, "model", "auto").expect_err("times out");
    assert_eq!(err.to_string(), "ffmpeg timed out");
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[test]
fn herdr_timeout_is_an_error() {
    let _serial = serial();
    let dir = TempDir::new();
    let program = script(dir.path(), "herdr", "sleep 5");
    let mut herdr = RunHerdr {
        program,
        timeout: Duration::from_millis(200),
    };
    let (code, out) = herdr.run(&["agent", "list"]).expect("ran");
    assert_ne!(code, 0);
    assert!(out.contains("timed out"), "{out}");
}
