//! Agents of other kinds than Claude Code: pi's answers come from its
//! session file; a kind without a reader shows its screen.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use crate::fakes::{T, c};
use herdr_matrix::bot::{AgentSettings, Sandbox};
use herdr_matrix::kind::{self, Kind};
use serde_json::json;

/// Sessions pi 0.75.4 wrote: a greeting, and a turn with thinking, a tool
/// call and its result.
const HELLO: &str = include_str!("../fixtures/pi/hello.jsonl");
const TOOL: &str = include_str!("../fixtures/pi/tool.jsonl");

/// `wA:p1` is a pi agent in `/w/a`, read as the bot reads it; the path of
/// its session file, which holds an earlier turn.
fn pi(t: &mut T) -> PathBuf {
    let mut herdr = t.herdr.0.borrow_mut();
    herdr.kinds.insert("wA:p1".into(), "pi".into());
    herdr.cwd.insert("wA:p1".into(), "/w/a".into());
    drop(herdr);
    let home = t.dir.path().join("home");
    let session = home.join(".pi/agent/sessions/--w-a--/2026_s.jsonl");
    fs::create_dir_all(session.parent().expect("a directory")).expect("mkdir");
    fs::write(&session, HELLO).expect("write");
    t.bot.deps_mut().answer = Box::new(move |a| kind::turn(a, Some(&home)));
    session
}

fn append(path: &Path, text: &str) {
    let mut file = fs::File::options().append(true).open(path).expect("open");
    file.write_all(text.as_bytes()).expect("write");
}

/// Lines of the tool turn: its user message on.
fn tool_lines(range: std::ops::Range<usize>) -> String {
    TOOL.lines()
        .skip(3)
        .collect::<Vec<_>>()
        .get(range)
        .expect("lines")
        .iter()
        .map(|l| (*l).to_owned() + "\n")
        .collect()
}

/// A turn of pi in which it said `texts`.
fn pi_turn(texts: &[&str]) -> String {
    let user = json!({"type": "message", "id": "u", "parentId": null, "timestamp": "2026-11-18T18:06:58.776Z",
                      "message": {"role": "user", "content": [{"type": "text", "text": "Anything new?"}]}});
    let mut lines = vec![user.to_string()];
    for (i, text) in texts.iter().enumerate() {
        let said = json!({"type": "message", "id": format!("a{i}"), "parentId": "u",
                          "timestamp": "2026-11-18T18:06:59.000Z",
                          "message": {"role": "assistant", "content": [{"type": "text", "text": text}],
                                      "stopReason": "stop"}});
        lines.push(said.to_string());
    }
    lines.into_iter().map(|l| l + "\n").collect()
}

#[test]
fn pi_turn_posts_progress_and_its_last_message() {
    let mut t = T::new();
    let session = pi(&mut t);
    t.set("wA:p1", "idle");
    t.poll();
    t.tell("Use a tool please");
    t.set("wA:p1", "working");
    t.poll();
    append(&session, &tool_lines(0..2));
    t.poll();
    assert!(t.new_msgs().is_empty(), "{:?}", t.new_msgs());
    append(&session, &tool_lines(2..4));
    t.poll();
    t.set("wA:p1", "idle");
    t.poll();
    t.poll();
    assert_eq!(t.new_msgs(), ["Reading the file.", "It says hello."]);
    assert_eq!(
        t.matrix.sent_key("msgtype", None)[t.start..],
        ["m.notice", "m.text"]
    );
}

#[test]
fn pi_noop_posts_nothing_and_last_shows_it() {
    let mut t = T::new();
    let session = pi(&mut t);
    t.set("wA:p1", "idle");
    t.poll();
    t.tell("Anything new?");
    t.set("wA:p1", "working");
    t.poll();
    append(&session, &pi_turn(&["NOOP"]));
    t.set("wA:p1", "idle");
    t.poll();
    t.poll();
    assert!(t.new_msgs().is_empty(), "{:?}", t.new_msgs());
    assert!(t.bot.state().pending.is_empty());
    t.tell("!last");
    assert_eq!(t.new_msgs(), ["NOOP"]);
    append(&session, &pi_turn(&["Looking.", "Found one."]));
    t.set("wB:p1", "idle");
    t.tell("!attach repo-wA");
    assert_eq!(
        t.last_msg(),
        "**repo-wA** attached; it is idle.\n\nFound one."
    );
}

#[test]
fn new_starts_the_kind_of_the_folder() {
    let mut t = T::new();
    let beta = t.dir.path().join("ws/beta");
    fs::create_dir_all(&beta).expect("mkdir");
    let beta = beta.to_string_lossy().into_owned();
    t.bot.deps_mut().agents = vec![AgentSettings {
        cwd: beta.clone(),
        kind: Kind::Pi,
        ..AgentSettings::default()
    }];
    t.tell("!new beta");
    assert_eq!(t.herdr.last(), c(&["pane", "run", "wN:p1", "pi"]));
    assert!(t.last_msg().contains(&format!("Started pi in {beta}")));
}

/// `wA:p1` is the confined pi agent `nora` in `/w/a`; its state directory.
fn confine(t: &mut T) -> PathBuf {
    t.herdr
        .0
        .borrow_mut()
        .cwd
        .insert("wA:p1".into(), "/w/a".into());
    let state = t.dir.path().join("state/nora/pi");
    fs::create_dir_all(&state).expect("mkdir");
    t.bot.deps_mut().agents = vec![AgentSettings {
        cwd: "/w/a".into(),
        kind: Kind::Pi,
        sandbox: Some(Sandbox {
            id: "nora".into(),
            state: state.clone(),
            uploads: t.uploads().join("nora"),
            files: t.files().join("agents/nora"),
        }),
        ..AgentSettings::default()
    }];
    state
}

fn write_old(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("a directory")).expect("mkdir");
    fs::write(path, text).expect("write");
    fs::File::options()
        .write(true)
        .open(path)
        .expect("open")
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_mins(1))
        .expect("utime");
}

#[test]
fn confined_pi_turn_comes_from_its_newest_session() {
    let mut t = T::new();
    let state = confine(&mut t);
    t.said("wA:p1", &[("u1", "from elsewhere")], None);
    write_old(
        &state.join("sessions/--w-a--/old.jsonl"),
        &pi_turn(&["Old."]),
    );
    let newest = state.join("sessions/--w-b--/new.jsonl");
    fs::create_dir_all(newest.parent().expect("a directory")).expect("mkdir");
    fs::write(&newest, pi_turn(&["New."])).expect("write");
    t.working_then("idle", "wA:p1");
    assert_eq!(t.new_msgs(), ["New."]);
}

/// Mount points of `sandbox.files` stay in the state directory as empty
/// files, newer than the session.
#[test]
fn confined_pi_mount_points_are_not_sessions() {
    let mut t = T::new();
    let state = confine(&mut t);
    write_old(
        &state.join("sessions/--w-a--/mine.jsonl"),
        &pi_turn(&["Mine."]),
    );
    for file in [
        "models.json",
        "settings.json",
        "models.jsonl",
        "sessions/models.jsonl",
    ] {
        fs::write(state.join(file), "").expect("write");
    }
    t.working_then("idle", "wA:p1");
    assert_eq!(t.new_msgs(), ["Mine."]);
}

#[test]
fn confined_pi_session_behind_a_symlink_is_not_read() {
    let mut t = T::new();
    let state = confine(&mut t);
    let sessions = state.join("sessions");
    write_old(&sessions.join("--w-a--/mine.jsonl"), &pi_turn(&["Mine."]));
    let outside = t.dir.path().join("owner/sessions/--home--");
    fs::create_dir_all(&outside).expect("mkdir");
    fs::write(outside.join("secret.jsonl"), pi_turn(&["Secret."])).expect("write");
    symlink(
        outside.join("secret.jsonl"),
        sessions.join("--w-a--/link.jsonl"),
    )
    .expect("symlink");
    symlink(&outside, sessions.join("--link--")).expect("symlink");
    t.working_then("idle", "wA:p1");
    assert_eq!(t.new_msgs(), ["Mine."]);
}

#[test]
fn confined_pi_sessions_behind_a_symlink_read_as_nothing() {
    let mut t = T::new();
    let state = confine(&mut t);
    let outside = t.dir.path().join("owner/sessions");
    fs::create_dir_all(outside.join("--w-a--")).expect("mkdir");
    fs::write(outside.join("--w-a--/secret.jsonl"), pi_turn(&["Secret."])).expect("write");
    symlink(&outside, state.join("sessions")).expect("symlink");
    t.set("wA:p1", "idle");
    t.poll();
    t.tell("Hello");
    t.set("wA:p1", "working");
    t.poll();
    t.set("wA:p1", "idle");
    t.poll();
    assert_eq!(t.new_msgs(), ["screen of wA:p1"]);
    t.tell("!last");
    assert_eq!(t.last_msg(), "Nothing said yet.");
}

#[test]
fn kind_without_a_reader_shows_its_screen() {
    let mut t = T::new();
    let home = t.dir.path().join("home");
    t.herdr
        .0
        .borrow_mut()
        .kinds
        .insert("wA:p1".into(), "codex".into());
    t.bot.deps_mut().answer = Box::new(move |a| kind::turn(a, Some(&home)));
    t.prompted("Hello");
    t.set("wA:p1", "working");
    t.poll();
    t.set("wA:p1", "idle");
    t.poll();
    assert_eq!(t.new_msgs(), ["screen of wA:p1"]);
}
