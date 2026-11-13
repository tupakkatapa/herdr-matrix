//! Sizes, ages, failures and the journal.

use crate::fakes::{OWNER, T, c, fruit, ms, msg, react};
use herdr_matrix::bot::To;
use herdr_matrix::bot::{PENDING_TTL, SENT_MAX};
use herdr_matrix::render::EVENT_MAX;
use herdr_matrix::render::{chunks, size};
use serde_json::{Value, json};

fn assert_fits(t: &T) {
    for (body, html) in t
        .matrix
        .sent()
        .iter()
        .zip(t.matrix.sent_key("formatted_body", None))
    {
        assert!(body.len() + html.len() <= EVENT_MAX);
    }
}

#[test]
fn events_stay_under_the_size_limit() {
    let mut t = T::new();
    t.say(&"ä".repeat(24000), To::default());
    t.say(&"\"".repeat(20000), To::default());
    t.say(&"x".repeat(70000), To::default());
    assert_fits(&t);
    assert_eq!(
        t.new_msgs().concat(),
        "ä".repeat(24000) + &"\"".repeat(20000) + &"x".repeat(70000)
    );
}

#[test]
fn long_screen_is_split() {
    let mut t = T::new();
    let screen = (0..8000)
        .map(|i| format!("<line {i}>"))
        .collect::<Vec<_>>()
        .join("\n");
    t.say(
        "head",
        To {
            pre: Some(&screen),
            ..To::default()
        },
    );
    assert_fits(&t);
    assert!(t.new_msgs().len() > 1);
    assert!(t.new_msgs()[0].starts_with("head\n\n<line 0>"));
    assert_eq!(t.new_msgs().join("\n").matches("<line ").count(), 8000);
}

#[test]
fn long_code_block_fits_in_events() {
    let code = format!("```sh\n{}```", "echo hello world\n".repeat(4000));
    assert!(chunks(&code).iter().all(|p| size(p) <= EVENT_MAX));
}

#[test]
fn screen_is_capped() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.tell("!screen 5000");
    assert_eq!(t.herdr.did("read").last().expect("a read")[6], "200");
}

#[test]
fn sent_is_bounded() {
    let mut t = T::new();
    for i in 0..SENT_MAX + 5 {
        t.say(
            &format!("m{i}"),
            To {
                pane: Some("wA:p1"),
                ..To::default()
            },
        );
    }
    assert_eq!(t.bot.state().sent.len(), SENT_MAX);
    let saved: Value =
        serde_json::from_slice(&std::fs::read(t.state_path()).expect("read")).expect("JSON");
    assert_eq!(saved["sent"].as_object().expect("sent").len(), SENT_MAX);
}

#[test]
fn old_messages_are_skipped() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let mut batch = msg(
        &t.room,
        "delete that branch",
        OWNER,
        ms(t.now() - 601.0),
        &json!({}),
    );
    let room = t.room.clone();
    let events = batch["rooms"]["join"][&room]["timeline"]["events"]
        .as_array_mut()
        .expect("events");
    let mut zero = events[0].clone();
    zero["origin_server_ts"] = json!(0);
    let mut fresh = events[0].clone();
    fresh["content"] = json!({"msgtype": "m.text", "body": "fresh"});
    fresh["origin_server_ts"] = json!(ms(t.now() - 599.0));
    events.push(zero);
    events.push(fresh);
    t.queue(batch);
    t.sync();
    assert_eq!(
        t.herdr.did("prompt"),
        vec![c(&["agent", "prompt", "wA:p1", "fresh"])]
    );
    assert_eq!(t.new_msgs().len(), 1);
    assert!(t.new_msgs()[0].contains("Skipped 2 messages"));
}

#[test]
fn old_reaction_is_skipped() {
    let mut t = T::new();
    t.working_then("blocked", "wA:p1");
    let choice = t.matrix.last_id();
    t.advance(601.0);
    t.queue(react(
        &t.room,
        &choice,
        "\u{1F44D}",
        OWNER,
        ms(t.now() - 601.0),
    ));
    t.sync();
    assert!(t.herdr.did("send-keys").is_empty());
}

#[test]
fn failed_reply_does_not_drop_the_batch() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let mut batch = msg(&t.room, "first", OWNER, 1_000_000, &json!({}));
    let room = t.room.clone();
    let events = batch["rooms"]["join"][&room]["timeline"]["events"]
        .as_array_mut()
        .expect("events");
    let mut second = events[0].clone();
    second["content"] = json!({"msgtype": "m.text", "body": "second"});
    events.push(second);
    t.queue(batch);
    t.herdr.0.borrow_mut().fail = 1;
    t.sync();
    assert_eq!(
        t.herdr.did("prompt"),
        vec![c(&["agent", "prompt", "wA:p1", "second"])]
    );
    assert!(t.new_msgs().join("\n").contains("Failed"));
}

#[test]
fn instant_sync_does_not_spin_herdr() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.herdr.0.borrow_mut().calls.clear();
    for _ in 0..10 {
        t.bot.tick().expect("tick");
    }
    assert_eq!(t.herdr.did("list").len(), 1);
    t.advance(3.0);
    t.bot.tick().expect("tick");
    assert_eq!(t.herdr.did("list").len(), 2);
}

#[test]
fn unexpected_error_is_named_and_logged() {
    let mut t = T::new();
    t.herdr.0.borrow_mut().list_out = Some(json!({"result": {}}).to_string());
    t.clear_log();
    t.tell("!agents");
    assert_eq!(
        t.new_msgs(),
        [
            "Failed to deliver the message: unexpected data: no agents. \
          This is a bug; the journal has the details."
        ]
    );
    assert_eq!(t.log_lines(), ["error: unexpected data: no agents"]);
}

#[test]
fn journal_records_every_command() {
    let mut t = T::new();
    t.clear_log();
    t.set("wA:p1", "idle");
    for word in [
        "!help", "!list", "!away", "!here", "!leave", "!last", "!nope",
    ] {
        t.tell(word);
    }
    assert_eq!(
        t.log_lines(),
        ["!help", "!list", "!away", "!here", "!leave", "wA:p1 !last"]
    );
}

#[test]
fn journal_never_holds_what_the_owner_wrote() {
    let mut t = T::new();
    t.clear_log();
    t.prompted("secret plan");
    t.tell("!screen 3");
    t.said("wA:p1", &[], Some(fruit()));
    t.set("wA:p1", "blocked");
    t.poll();
    t.tell("secret answer");
    let id = t.matrix.last_id();
    t.react(&id, "\u{1F44D}");
    t.set("wA:p1", "idle");
    t.speak("secret voice");
    let lines = t.log_lines();
    assert_eq!(
        lines,
        [
            "wA:p1 prompt 11",
            "wA:p1 !screen",
            "wA:p1 answer 13",
            "wA:p1 voice 12"
        ]
    );
    assert!(!lines.iter().any(|l| l.contains("secret")));
}

#[test]
fn journal_records_a_reaction() {
    let mut t = T::new();
    t.clear_log();
    t.working_then("blocked", "wA:p1");
    let id = t.matrix.last_id();
    t.react(&id, "\u{1F44D}");
    assert_eq!(t.log_lines(), ["wA:p1 reaction 1"]);
}

#[test]
fn failed_post_keeps_the_message() {
    let mut t = T::new();
    t.prompted("Hello");
    t.set("wA:p1", "working");
    t.said("wA:p1", &[("u1", "old answer"), ("u2", "one")], None);
    t.poll();
    t.said(
        "wA:p1",
        &[("u1", "old answer"), ("u2", "one"), ("u3", "two")],
        None,
    );
    t.matrix.0.borrow_mut().fail_send = 1;
    t.poll();
    assert!(t.log_lines().iter().any(|l| l.contains("send failed")));
    t.poll();
    t.set("wA:p1", "idle");
    t.poll();
    assert_eq!(t.new_msgs(), ["one", "two"]);
}

#[test]
fn failed_last_post_keeps_the_answer() {
    let mut t = T::new();
    t.prompted("Hello");
    t.set("wA:p1", "working");
    t.poll();
    t.said("wA:p1", &[("u1", "old answer"), ("u2", "final")], None);
    t.set("wA:p1", "idle");
    t.matrix.0.borrow_mut().fail_send = 1;
    t.poll();
    assert!(t.log_lines().iter().any(|l| l.contains("send failed")));
    t.poll();
    assert_eq!(t.new_msgs(), ["final"]);
}

#[test]
fn failed_post_of_a_choice_keeps_the_turn() {
    let mut t = T::new();
    t.prompted("Hello");
    t.set("wA:p1", "working");
    t.poll();
    t.set("wA:p1", "blocked");
    t.matrix.0.borrow_mut().fail_send = 1;
    t.poll();
    assert!(t.log_lines().iter().any(|l| l.contains("send failed")));
    t.poll();
    assert_eq!(t.new_msgs().len(), 1);
    assert!(t.new_msgs()[0].contains("Waits on a choice"));
}

#[test]
fn failed_post_does_not_lose_the_away_sweep() {
    let mut t = T::new();
    t.set("wA:p1", "working");
    t.set("wB:p1", "working");
    t.poll();
    t.set("wA:p1", "blocked");
    t.set("wB:p1", "blocked");
    t.poll();
    t.go_afk();
    t.matrix.0.borrow_mut().fail_send = 1;
    t.poll();
    assert!(t.log_lines().iter().any(|l| l.contains("send failed")));
    t.poll();
    assert_eq!(t.new_msgs().len(), 2);
}

#[test]
fn turn_of_a_pane_that_is_gone_is_dropped() {
    let mut t = T::new();
    t.prompted("Hello");
    t.herdr.remove("wA:p1");
    t.poll();
    assert!(t.bot.state().pending.contains_key("wA:p1"));
    t.advance(PENDING_TTL.as_secs_f64());
    t.poll();
    assert!(!t.bot.state().pending.contains_key("wA:p1"));
    t.set("wA:p1", "working");
    t.poll();
    t.said(
        "wA:p1",
        &[("u1", "old answer"), ("u2", "someone else")],
        None,
    );
    t.set("wA:p1", "idle");
    t.poll();
    assert!(t.new_msgs().is_empty());
}

#[test]
fn failed_read_receipt_is_not_a_failure() {
    let mut t = T::new();
    t.matrix.0.borrow_mut().refuse.push("/receipt/".into());
    t.prompted("Hello");
    assert_eq!(t.herdr.did("prompt").len(), 1);
    assert!(t.new_msgs().is_empty());
}

#[test]
fn prompt_without_an_answer_expires() {
    let mut t = T::new();
    t.prompted("Hello");
    t.poll();
    t.advance(599.0);
    t.poll();
    assert!(t.bot.state().pending.contains_key("wA:p1"));
    t.advance(1.0);
    t.poll();
    assert!(!t.bot.state().pending.contains_key("wA:p1"));
    t.set("wA:p1", "working");
    t.poll();
    t.said(
        "wA:p1",
        &[("u1", "old answer"), ("u2", "at the desk")],
        None,
    );
    t.set("wA:p1", "idle");
    t.poll();
    assert!(t.new_msgs().is_empty());
    assert!(t.matrix.typing().is_empty());
}

#[test]
fn working_agent_does_not_expire() {
    let mut t = T::new();
    t.prompted("Hello");
    t.set("wA:p1", "working");
    for _ in 0..3 {
        t.advance(400.0);
        t.poll();
    }
    t.said("wA:p1", &[("u1", "old answer"), ("u2", "Finally.")], None);
    t.set("wA:p1", "idle");
    t.poll();
    assert_eq!(t.new_msgs(), ["Finally."]);
}

#[test]
fn new_agent_ends_its_turn_when_ready() {
    let mut t = T::new();
    std::fs::create_dir_all(t.dir.path().join("ws/a/beta")).expect("mkdir");
    t.tell("!new beta");
    t.poll();
    t.set("wN:p1", "unknown");
    t.poll();
    assert!(t.bot.state().pending.contains_key("wN:p1"));
    t.set("wN:p1", "idle");
    t.poll();
    assert!(!t.bot.state().pending.contains_key("wN:p1"));
    t.set("wN:p1", "working");
    t.poll();
    t.said("wN:p1", &[("u1", "at the desk")], None);
    t.set("wN:p1", "idle");
    t.poll();
    assert_eq!(t.new_msgs().len(), 1);
    assert!(t.matrix.typing().is_empty());
}

#[test]
fn failed_away_notice_is_posted_later() {
    let mut t = T::new();
    t.said("wA:p1", &[("u1", "Done.")], None);
    t.set("wA:p1", "working");
    t.poll();
    t.go_afk();
    t.set("wA:p1", "idle");
    t.matrix.0.borrow_mut().fail_send = 1;
    let _ = t.bot.poll();
    t.poll();
    assert_eq!(t.new_msgs(), ["Done."]);
}

#[test]
fn failed_save_fetches_the_messages_again() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let mut tmp = t.state_path().into_os_string();
    tmp.push(".tmp");
    std::fs::create_dir(&tmp).expect("mkdir");
    let batch = msg(&t.room, "hi", OWNER, ms(t.now()), &json!({}));
    t.queue(batch.clone());
    assert!(t.bot.sync().is_err());
    assert_eq!(t.bot.state().since.as_deref(), Some("n"));
    assert!(t.herdr.did("prompt").is_empty());
    std::fs::remove_dir(&tmp).expect("rmdir");
    t.queue(batch);
    t.sync();
    let syncs: Vec<_> = t
        .matrix
        .calls()
        .into_iter()
        .filter(|c| c.1 == "/sync")
        .collect();
    let since = |c: &(String, String, Option<Value>, Vec<(String, String)>)| {
        c.3.iter()
            .find(|(k, _)| k == "since")
            .map(|(_, v)| v.clone())
    };
    assert_eq!(since(&syncs[syncs.len() - 1]).as_deref(), Some("n"));
    assert_eq!(
        t.herdr.did("prompt"),
        vec![c(&["agent", "prompt", "wA:p1", "hi"])]
    );
}

fn two_prompts(t: &T) -> Value {
    let mut batch = msg(&t.room, "first", OWNER, ms(t.now()), &json!({}));
    let room = t.room.clone();
    let events = batch["rooms"]["join"][&room]["timeline"]["events"]
        .as_array_mut()
        .expect("events");
    let mut second = events[0].clone();
    second["content"] = json!({"msgtype": "m.text", "body": "second"});
    events.push(second);
    batch
}

#[test]
fn failed_notice_does_not_drop_the_batch() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.queue(two_prompts(&t));
    t.herdr.0.borrow_mut().fail = 1;
    t.matrix.0.borrow_mut().fail_send = 1;
    t.sync();
    assert_eq!(
        t.herdr.did("prompt"),
        vec![c(&["agent", "prompt", "wA:p1", "second"])]
    );
}

#[test]
fn failed_stale_notice_does_not_drop_the_batch() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let mut batch = two_prompts(&t);
    let room = t.room.clone();
    batch["rooms"]["join"][&room]["timeline"]["events"][0]["origin_server_ts"] =
        json!(ms(t.now() - 601.0));
    t.queue(batch);
    t.matrix.0.borrow_mut().fail_send = 1;
    t.sync();
    assert_eq!(
        t.herdr.did("prompt"),
        vec![c(&["agent", "prompt", "wA:p1", "second"])]
    );
}

#[test]
fn retried_message_keeps_its_transaction() {
    let mut t = T::new();
    t.matrix.0.borrow_mut().lose_reply = 1;
    assert!(t.bot.say("Hello.", To::default()).is_err());
    t.say("Hello.", To::default());
    t.say("Hello.", To::default());
    let puts: Vec<String> = t
        .matrix
        .calls()
        .into_iter()
        .filter(|c| c.1.contains("/send/"))
        .map(|c| c.1)
        .collect();
    let n = puts.len();
    assert_eq!(puts[n - 3], puts[n - 2]);
    assert_ne!(puts[n - 2], puts[n - 1]);
}

fn broken(t: &T, a: &str, b: &str) {
    t.herdr.0.borrow_mut().broken.insert((a.into(), b.into()));
}

#[test]
fn failed_key_of_a_reaction_is_said() {
    let mut t = T::new();
    t.working_then("blocked", "wA:p1");
    let id = t.matrix.last_id();
    broken(&t, "agent", "send-keys");
    t.react(&id, "\u{1F44D}");
    assert_eq!(
        t.last_msg(),
        "Could not answer **repo-wA**: send-keys failed"
    );
    assert!(t.bot.state().pending.is_empty());
    assert!(t.bot.state().asked.contains_key("wA:p1"));
}

#[test]
fn encrypted_file_is_refused() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.tell_with(
        "pic.png",
        &json!({"msgtype": "m.image", "file": {"url": "mxc://x/enc"}}),
    );
    assert_eq!(
        t.new_msgs(),
        ["Failed to take the file: the file is encrypted, and the bot cannot read encrypted chats"]
    );
    assert!(t.herdr.did("prompt").is_empty());
}

#[test]
fn unattached_command_lists_the_agents() {
    let mut t = T::new();
    t.set("wA:p1", "working");
    t.set("wB:p1", "working");
    t.tell("!stop");
    assert!(t.last_msg().starts_with("Not attached. !attach one of:"));
    assert!(t.herdr.did("send-keys").is_empty());
}

#[test]
fn failed_exit_is_said() {
    let mut t = T::new();
    t.prompted("Hello");
    broken(&t, "pane", "close");
    t.tell("!exit now");
    assert_eq!(t.last_msg(), "Could not close **repo-wA**: close failed");
    assert!(t.bot.state().pending.contains_key("wA:p1"));
}

#[test]
fn screen_defaults_to_twenty_lines() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.tell("!screen");
    assert_eq!(t.herdr.did("read").last().expect("a read")[6], "20");
}

#[test]
fn failed_new_agent_is_said() {
    let mut t = T::new();
    std::fs::create_dir_all(t.dir.path().join("ws/a/beta")).expect("mkdir");
    broken(&t, "workspace", "create");
    t.tell("!new beta");
    assert_eq!(
        t.new_msgs(),
        ["Failed to deliver the message: herdr: create failed. Is herdr running?"]
    );
    assert_eq!(t.bot.state().attached, None);
}
