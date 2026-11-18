//! Turns that end on `NOOP`, and prompts from the machine.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;

use crate::fakes::{T, c, fruit};
use herdr_matrix::inbox::{Prompt, queue};
use herdr_matrix::state::Away;

/// A prompt from the machine, queued as `herdr-matrix prompt` does.
fn local(t: &T, agent: &str, text: &str, at: f64) {
    let prompt = Prompt {
        agent: agent.into(),
        text: text.into(),
        at,
        cwd: None,
    };
    queue(&t.inbox(), &prompt).expect("queued");
}

/// A prompt from the machine, queued as `herdr-matrix prompt --cwd` does.
fn local_in(t: &T, dir: &str, text: &str, at: f64) {
    let prompt = Prompt {
        agent: String::new(),
        text: text.into(),
        at,
        cwd: Some(dir.into()),
    };
    queue(&t.inbox(), &prompt).expect("queued");
}

#[test]
fn silent_turn_posts_nothing() {
    let mut t = T::new();
    t.prompted("Anything new?");
    t.set("wA:p1", "working");
    t.poll();
    t.said("wA:p1", &[("u1", "old answer"), ("u2", "NOOP\n")], None);
    t.set("wA:p1", "idle");
    t.poll();
    t.poll();
    assert!(t.new_msgs().is_empty(), "{:?}", t.new_msgs());
    assert!(t.bot.state().pending.is_empty());
    t.tell("!last");
    assert_eq!(t.new_msgs(), ["NOOP\n"]);
}

#[test]
fn held_noop_is_not_progress() {
    let mut t = T::new();
    t.prompted("Anything new?");
    t.set("wA:p1", "working");
    t.said("wA:p1", &[("u1", "old answer"), ("u2", "NOOP")], None);
    t.poll();
    t.advance(21.0);
    t.poll();
    t.said(
        "wA:p1",
        &[("u1", "old answer"), ("u2", "NOOP"), ("u3", "Found one.")],
        None,
    );
    t.poll();
    t.said(
        "wA:p1",
        &[
            ("u1", "old answer"),
            ("u2", "NOOP"),
            ("u3", "Found one."),
            ("u4", " NOOP "),
        ],
        Some(fruit()),
    );
    t.set("wA:p1", "blocked");
    t.poll();
    let said = t.new_msgs();
    assert_eq!(said[0], "Found one.");
    assert!(said[1].contains("Pick a fruit?"), "{said:?}");
    assert_eq!(said.len(), 2);
    assert!(said.iter().all(|m| !m.contains("NOOP")), "{said:?}");
}

#[test]
fn away_report_skips_noop() {
    let mut t = T::new();
    t.said("wA:p1", &[("u1", "NOOP")], None);
    t.working_then("idle", "wA:p1");
    assert!(t.new_msgs().is_empty(), "{:?}", t.new_msgs());
    t.set("wA:p1", "working");
    t.poll();
    t.set("wA:p1", "idle");
    t.poll();
    assert!(t.new_msgs().is_empty(), "{:?}", t.new_msgs());
    t.said("wA:p1", &[("u1", "NOOP"), ("u2", "Mail came.")], None);
    t.set("wA:p1", "working");
    t.poll();
    t.set("wA:p1", "idle");
    t.poll();
    assert_eq!(t.new_msgs(), ["Mail came."]);
}

#[test]
fn local_prompt_answers_in_the_own_chat() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.set("wB:p1", "idle");
    let chat = t.spawn("wA:p1", "repo-wA");
    t.poll();
    let before = t.main().len();
    t.clear_log();
    local(&t, "repo-wA", "Check the mail", t.now());
    t.bot.tick().expect("tick");
    assert_eq!(t.herdr.raw_prompts(), ["Check the mail"]);
    assert!(t.log_lines().contains(&"wA:p1 local 14".to_owned()));
    assert_eq!(fs::read_dir(t.inbox()).expect("list").count(), 0);
    t.set("wA:p1", "working");
    t.poll();
    t.said("wA:p1", &[("u1", "Two new mails.")], None);
    t.set("wA:p1", "idle");
    t.poll();
    t.poll();
    assert_eq!(t.bot.state().away, Away::Off);
    assert_eq!(t.in_chat(&chat).last().expect("one"), "Two new mails.");
    assert_eq!(
        t.in_chat_key(&chat, "msgtype").last().expect("one"),
        "m.text"
    );
    assert_eq!(t.main().len(), before);
    assert!(t.bot.state().pending.is_empty());
}

#[test]
fn local_prompt_without_own_chat() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.poll();
    local(&t, "wA:p1", "/compact", t.now());
    t.bot.local().expect("local");
    assert_eq!(
        t.herdr.did("prompt"),
        vec![c(&["agent", "prompt", "wA:p1", "/compact"])]
    );
    assert!(t.bot.state().pending["wA:p1"].origin.slash);
    assert!(!t.bot.state().pending["wA:p1"].origin.spawned);
    t.said("wA:p1", &[("u1", "Compacted.")], None);
    t.set("wA:p1", "working");
    t.poll();
    t.set("wA:p1", "idle");
    t.poll();
    t.poll();
    assert_eq!(t.new_msgs(), ["Compacted."]);
}

#[test]
fn dropped_local_prompts() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.set("wB:p1", "blocked");
    t.poll();
    let calls = t.herdr.calls().len();
    t.bot.local().expect("local");
    assert_eq!(t.herdr.calls().len(), calls);
    t.clear_log();
    let now = t.now();
    local(&t, "repo-wA", "s3cret", now - 601.0);
    local(&t, "nothing", "s3cret", now - 3.0);
    local(&t, "repo", "s3cret", now - 2.0);
    local(&t, "repo-wB", "s3cret", now - 1.0);
    t.bot.local().expect("local");
    assert!(t.herdr.did("prompt").is_empty());
    assert_eq!(
        t.log_lines(),
        [
            "prompt for \"repo-wA\" dropped: older than 10 minutes",
            "prompt for \"nothing\" dropped: it matches 0 agents",
            "prompt for \"repo\" dropped: it matches 2 agents",
            "wB:p1 prompt dropped: it waits on a choice",
        ]
    );
    assert!(t.bot.state().pending.is_empty());
    assert!(t.new_msgs().is_empty());

    t.clear_log();
    t.herdr
        .0
        .borrow_mut()
        .broken
        .insert(("agent".into(), "prompt".into()));
    local(&t, "repo-wA", "s3cret", now - 1.0);
    local(&t, "wA:p1", "s3cret", now);
    t.bot.local().expect("local");
    assert_eq!(t.herdr.did("prompt").len(), 2);
    assert_eq!(
        t.log_lines(),
        [
            "error: wA:p1 prompt: herdr: prompt failed",
            "error: wA:p1 prompt: herdr: prompt failed",
        ]
    );
    assert!(t.bot.state().pending.is_empty());

    t.clear_log();
    fs::set_permissions(t.inbox(), fs::Permissions::from_mode(0o755)).expect("chmod");
    t.bot.tick().expect("tick");
    let lines = t.log_lines();
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("error: prompts: ") && l.ends_with("is not a private directory")),
        "{lines:?}"
    );
    assert!(t.log_lines().iter().all(|l| !l.contains("s3cret")));
}

#[test]
fn local_turn_holds_its_progress() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let chat = t.spawn("wA:p1", "repo-wA");
    t.poll();
    let before = t.in_chat(&chat).len();
    local(&t, "repo-wA", "wake", t.now());
    t.bot.local().expect("local");
    t.set("wA:p1", "working");
    t.said("wA:p1", &[("u1", "Checking.")], None);
    t.poll();
    t.said("wA:p1", &[("u1", "Checking."), ("u2", "Still.")], None);
    t.advance(21.0);
    t.poll();
    t.said(
        "wA:p1",
        &[("u1", "Checking."), ("u2", "Still."), ("u3", "NOOP")],
        None,
    );
    t.set("wA:p1", "idle");
    t.poll();
    t.poll();
    assert_eq!(t.in_chat(&chat).len(), before);
    assert!(t.matrix.typing().is_empty());
    assert!(t.bot.state().pending.is_empty());

    local(&t, "repo-wA", "wake", t.now());
    t.bot.local().expect("local");
    t.set("wA:p1", "working");
    t.said("wA:p1", &[("u3", "NOOP"), ("u4", "Looking.")], None);
    t.poll();
    t.said(
        "wA:p1",
        &[("u3", "NOOP"), ("u4", "Looking."), ("u5", "Mail came.")],
        None,
    );
    t.set("wA:p1", "idle");
    t.poll();
    t.poll();
    assert_eq!(t.in_chat(&chat)[before..], ["Mail came."]);
    assert_eq!(t.in_chat_key(&chat, "msgtype")[before..], ["m.text"]);
}

#[test]
fn local_prompt_for_a_working_agent_is_dropped() {
    let mut t = T::new();
    t.set("wA:p1", "working");
    t.poll();
    t.clear_log();
    local(&t, "repo-wA", "wake", t.now());
    t.bot.local().expect("local");
    assert!(t.herdr.did("prompt").is_empty());
    assert_eq!(t.log_lines(), ["wA:p1 prompt dropped: it works"]);
    assert!(t.bot.state().pending.is_empty());
}

#[test]
fn local_prompt_by_cwd() {
    let mut t = T::new();
    t.set("wA:p1", "done");
    t.set("wB:p1", "idle");
    {
        let cwd = &mut t.herdr.0.borrow_mut().cwd;
        cwd.insert("wA:p1".into(), "/w/a".into());
        cwd.insert("wB:p1".into(), "/w/b/".into());
    }
    t.poll();
    t.clear_log();
    let now = t.now();
    local_in(&t, "/w/a/", "wake", now - 2.0);
    local_in(&t, "/w/b", "wake", now - 1.0);
    local_in(&t, "/w", "wake", now);
    t.bot.local().expect("local");
    assert_eq!(
        t.herdr.did("prompt"),
        vec![
            c(&["agent", "prompt", "wA:p1", "wake"]),
            c(&["agent", "prompt", "wB:p1", "wake"]),
        ]
    );
    assert_eq!(
        t.log_lines(),
        [
            "wA:p1 local 4",
            "wB:p1 local 4",
            "prompt for \"/w\" dropped: it matches 0 agents",
        ]
    );
}

#[test]
fn local_prompt_by_shared_cwd_is_dropped() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.set("wB:p1", "idle");
    {
        let cwd = &mut t.herdr.0.borrow_mut().cwd;
        cwd.insert("wA:p1".into(), "/w".into());
        cwd.insert("wB:p1".into(), "/w/".into());
    }
    t.poll();
    t.clear_log();
    let now = t.now();
    local_in(&t, "/w", "wake", now);
    local_in(&t, "/w", "wake", now - 601.0);
    t.bot.local().expect("local");
    assert!(t.herdr.did("prompt").is_empty());
    assert_eq!(
        t.log_lines(),
        [
            "prompt for \"/w\" dropped: older than 10 minutes",
            "prompt for \"/w\" dropped: it matches 2 agents",
        ]
    );
    assert!(t.bot.state().pending.is_empty());
}

#[test]
fn local_prompt_waits_out_a_herdr_failure() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    local(&t, "repo-wA", "wake", t.now());
    t.herdr.0.borrow_mut().fail = 1;
    assert!(t.bot.local().is_err());
    t.bot.local().expect("local");
    assert_eq!(
        t.herdr.did("prompt"),
        vec![c(&["agent", "prompt", "wA:p1", "wake"])]
    );
}

#[test]
fn empty_queues_ask_herdr_nothing() {
    let mut t = T::new();
    t.herdr.0.borrow_mut().calls.clear();
    t.bot.local().expect("local");
    assert!(t.herdr.calls().is_empty(), "{:?}", t.herdr.calls());
}

#[test]
fn failed_save_after_a_local_prompt_is_logged_once() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.poll();
    let mut tmp = t.state_path().into_os_string();
    tmp.push(".tmp");
    let blocked = tmp.clone();
    t.herdr.0.borrow_mut().hook = Some(Box::new(move |args| {
        if args[1] == "prompt" {
            fs::create_dir(&blocked).expect("mkdir");
        }
    }));
    local(&t, "repo-wA", "wake", t.now());
    t.clear_log();
    t.bot.local().expect("local");
    fs::remove_dir(&tmp).expect("rmdir");
    assert_eq!(t.herdr.did("prompt").len(), 1);
    let errors = t
        .log_lines()
        .into_iter()
        .filter(|l| l.starts_with("error: "))
        .count();
    assert_eq!(errors, 1, "{:?}", t.log_lines());
}

#[test]
fn local_prompt_survives_a_failed_send() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    local(&t, "repo-wA", "wake", t.now());
    t.herdr
        .0
        .borrow_mut()
        .broken
        .insert(("agent".into(), "prompt".into()));
    t.bot.local().expect("local");
    t.herdr.0.borrow_mut().broken.clear();
    t.bot.local().expect("local");
    assert_eq!(
        t.herdr.did("prompt"),
        vec![c(&["agent", "prompt", "wA:p1", "wake"]); 2]
    );
    t.bot.local().expect("local");
    assert_eq!(t.herdr.did("prompt").len(), 2);
}

#[test]
fn local_turn_without_text_posts_nothing() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.said("wA:p1", &[("u1", "old")], None);
    t.poll();
    let before = t.new_msgs().len();
    local(&t, "repo-wA", "nightly", t.now());
    t.bot.local().expect("local");
    t.set("wA:p1", "working");
    t.poll();
    t.set("wA:p1", "idle");
    t.poll();
    t.poll();
    assert_eq!(t.new_msgs().len(), before, "{:?}", t.new_msgs());
    assert!(t.bot.state().pending.is_empty());
}
