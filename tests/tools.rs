//! The programs the bot runs: herdr, ffmpeg and whisper-cli, as scripts.
//! One test at a time, so no test forks while another writes a script it
//! runs.

use std::fs;
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
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

/// The binary, with `XDG_RUNTIME_DIR` alone set, so it can start no bot;
/// its exit code and what it wrote on stderr.
fn herdr_matrix(run: &Path, args: &[&str]) -> (Option<i32>, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_herdr-matrix"))
        .args(args)
        .env_clear()
        .env("XDG_RUNTIME_DIR", run)
        .output()
        .expect("ran");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn prompt_command_queues() {
    let _serial = serial();
    let dir = TempDir::new();
    let inbox = dir.path().join("herdr-matrix/prompts");
    let (code, _) = herdr_matrix(dir.path(), &["prompt", "mail", "Check", "the  mail"]);
    assert_eq!(code, Some(0));
    let files: Vec<PathBuf> = fs::read_dir(&inbox)
        .expect("a prompt directory")
        .map(|e| e.expect("entry").path())
        .collect();
    assert_eq!(files.len(), 1);
    let prompt: serde_json::Value =
        serde_json::from_slice(&fs::read(&files[0]).expect("read")).expect("JSON");
    assert_eq!(
        (&prompt["agent"], &prompt["text"]),
        (&"mail".into(), &"Check the  mail".into())
    );
    for args in [&["bogus"][..], &["prompt"], &["prompt", "mail"]] {
        let (code, err) = herdr_matrix(dir.path(), args);
        assert_eq!(code, Some(2), "{args:?}");
        assert!(err.starts_with("usage: herdr-matrix"), "{args:?}: {err}");
    }
    let (code, err) = herdr_matrix(dir.path(), &["prompt", "", "hi"]);
    assert_eq!(code, Some(1), "{err}");
    assert_eq!(fs::read_dir(&inbox).expect("list").count(), 1);
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

/// The binary with `env` alone and `input` on stdin; its exit code, stdout
/// and stderr.
fn run(args: &[&str], env: &[(&str, &Path)], input: &str) -> (Option<i32>, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_herdr-matrix"))
        .args(args)
        .env_clear()
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("ran");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("ran");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn chats_prints_the_answer_of_the_gate() {
    let _serial = serial();
    let dir = TempDir::new();
    let socket = dir.path().join("gate");
    let listener = UnixListener::bind(&socket).expect("bind");
    let gate = thread::spawn(move || {
        let mut asked = Vec::new();
        for reply in [
            r#"{"ok": [{"room": "!r:x", "name": "Friend"}]}"#,
            r#"{"error": "not a bridged chat"}"#,
            "",
        ] {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).expect("read");
            writeln!(stream, "{reply}").expect("write");
            asked.push(line);
        }
        asked
    });
    let env = [("HERDR_MATRIX_GATE_SOCKET", socket.as_path())];
    assert_eq!(
        run(&["chats", "rooms"], &env, ""),
        (
            Some(0),
            "[\n  {\n    \"room\": \"!r:x\",\n    \"name\": \"Friend\"\n  }\n]\n".to_owned(),
            String::new()
        )
    );
    assert_eq!(
        run(&["chats", "send", "!h:x", "hi", "--since"], &env, ""),
        (Some(1), String::new(), "not a bridged chat\n".to_owned())
    );
    assert_eq!(
        run(&["chats"], &env, ""),
        (
            Some(1),
            String::new(),
            "the gate gave no answer\n".to_owned()
        )
    );
    assert_eq!(
        gate.join().expect("the gate"),
        [
            "{\"argv\":[\"rooms\"]}\n",
            "{\"argv\":[\"send\",\"!h:x\",\"hi\",\"--since\"]}\n",
            "{\"argv\":[]}\n",
        ]
    );
    let (code, out, err) = run(&["chats", "rooms"], &env, "");
    assert_eq!((code, out.as_str()), (Some(1), ""));
    assert!(err.starts_with("cannot reach the gate: "), "{err}");
    assert_eq!(err.lines().count(), 1);
}

#[test]
fn gate_answers_a_bad_request() {
    let _serial = serial();
    for (input, want) in [
        (
            r#"{"argv": ["delete", "!r"]}"#,
            "unknown command; usage: herdr-matrix chats",
        ),
        ("delete !r", "the request is not {\"argv\": [...]}"),
        (
            r#"{"argv": ["rooms"]}"#,
            "HERDR_MATRIX_GATE_HOMESERVER is not set",
        ),
    ] {
        let (code, out, err) = run(&["gate"], &[], input);
        assert_eq!(code, Some(0), "{input}: {err}");
        let reply: serde_json::Value = serde_json::from_str(&out).expect("JSON");
        let error = reply["error"].as_str().expect("an error");
        assert!(error.starts_with(want), "{input}: {error}");
        assert_eq!(
            (out.lines().count(), err.lines().count()),
            (1, 1),
            "{input}"
        );
    }
}
