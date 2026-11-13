//! The chat: onboarding, away notices, prompts, answers and commands.

use crate::fakes::{BOT, OWNER, T, c, invite, ms, msg, reply_to};
use herdr_matrix::bot::To;
use herdr_matrix::matrix::quote;
use herdr_matrix::state::Away;
use serde_json::json;

#[test]
fn onboarding_creates_the_chat_once() {
    let mut t = T::new();
    t.bot.onboard().expect("onboard");
    let rooms: Vec<_> = t
        .matrix
        .calls()
        .into_iter()
        .filter(|c| c.1 == "/createRoom")
        .collect();
    assert_eq!(rooms.len(), 1);
    let body = rooms[0].2.clone().expect("body");
    assert!(body.get("name").is_none());
    assert_eq!(body["is_direct"], true);
    assert_eq!(body["invite"], json!([OWNER]));
    assert!(t.matrix.sent().join("\n").contains("!attach"));
}

#[test]
fn owner_change_onboards_again() {
    let mut t = T::new();
    t.bot.deps_mut().owner = "@new:example.org".into();
    t.bot.onboard().expect("onboard");
    assert_ne!(t.bot.state().control.as_deref(), Some(t.room.as_str()));
    let rooms: Vec<_> = t
        .matrix
        .calls()
        .into_iter()
        .filter(|c| c.1 == "/createRoom")
        .collect();
    assert_eq!(
        rooms.last().expect("room").2.as_ref().expect("body")["invite"],
        json!(["@new:example.org"])
    );
}

#[test]
fn owner_invite_becomes_the_chat() {
    let mut t = T::new();
    t.queue(invite("!dm:example.org", OWNER));
    t.sync();
    assert_eq!(t.bot.state().control.as_deref(), Some("!dm:example.org"));
    assert!(t.new_msgs().join("\n").contains("!help"));
}

#[test]
fn forged_invite_is_left() {
    let mut t = T::new();
    t.matrix.0.borrow_mut().members.insert(
        "!pub:evil".into(),
        vec![OWNER.into(), BOT.into(), "@x:evil".into()],
    );
    t.queue(invite("!pub:evil", OWNER));
    t.sync();
    assert!(
        t.matrix
            .has("POST", "/rooms/%21pub%3Aevil/leave", Some(json!({})), &[])
    );
    assert_eq!(t.bot.state().control.as_deref(), Some(t.room.as_str()));
    assert!(t.new_msgs().is_empty());
}

#[test]
fn invite_whose_members_cannot_be_read_is_left() {
    let mut t = T::new();
    t.matrix
        .0
        .borrow_mut()
        .refuse
        .push("/rooms/%21dm%3Aexample.org/state".into());
    t.queue(invite("!dm:example.org", OWNER));
    t.sync();
    assert!(t.matrix.has(
        "POST",
        "/rooms/%21dm%3Aexample.org/leave",
        Some(json!({})),
        &[]
    ));
    assert_eq!(t.bot.state().control.as_deref(), Some(t.room.as_str()));
}

#[test]
fn failed_join_keeps_the_messages_of_that_sync() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.matrix.0.borrow_mut().refuse.push("/join/".into());
    let mut batch = msg(&t.room, "hi", OWNER, 1_000_000, &json!({}));
    batch["rooms"]["invite"] = invite("!dm:example.org", OWNER)["rooms"]["invite"].clone();
    t.queue(batch);
    t.sync();
    assert_eq!(
        t.herdr.did("prompt"),
        vec![c(&["agent", "prompt", "wA:p1", "hi"])]
    );
    assert_eq!(t.bot.state().control.as_deref(), Some(t.room.as_str()));
}

#[test]
fn stranger_invite_is_ignored() {
    let mut t = T::new();
    t.queue(invite("!x:evil", "@x:evil"));
    t.sync();
    assert!(!t.matrix.calls().iter().any(|c| c.1.starts_with("/join/")));
}

#[test]
fn profile_is_set_once() {
    let mut t = T::new();
    let pic = t.dir.path().join("pic.png");
    std::fs::write(&pic, b"png").expect("write");
    t.bot.deps_mut().name = Some("Claude".into());
    t.bot.deps_mut().avatar = Some(pic);
    t.bot.profile().expect("profile");
    t.bot.profile().expect("profile");
    let puts: Vec<_> = t
        .matrix
        .calls()
        .into_iter()
        .filter(|c| c.1.starts_with("/profile/"))
        .map(|c| (c.1, c.2))
        .collect();
    let me = quote(BOT);
    assert_eq!(
        puts,
        vec![
            (
                format!("/profile/{me}/displayname"),
                Some(json!({"displayname": "Claude"}))
            ),
            (
                format!("/profile/{me}/avatar_url"),
                Some(json!({"avatar_url": "mxc://example.org/pic"}))
            ),
        ]
    );
    assert_eq!(t.matrix.0.borrow().uploads, vec![b"png".to_vec()]);
}

#[test]
fn first_poll_only_records() {
    let mut t = T::new();
    t.set("wA:p1", "blocked");
    t.go_afk();
    t.poll();
    assert!(t.new_msgs().is_empty());
}

#[test]
fn afk_notice_carries_the_answer() {
    let mut t = T::new();
    t.said(
        "wA:p1",
        &[("u0", "Looking."), ("u1", "All tests pass.")],
        None,
    );
    t.working_then("idle", "wA:p1");
    assert_eq!(t.new_msgs(), ["All tests pass."]);
    t.poll();
    assert_eq!(t.new_msgs().len(), 1);
}

#[test]
fn afk_notice_of_a_choice_shows_the_screen() {
    let mut t = T::new();
    t.working_then("blocked", "wA:p1");
    assert!(t.new_msgs()[0].starts_with("Waits on a choice"));
    assert!(t.new_msgs()[0].contains("screen of wA:p1"));
}

#[test]
fn quiet_while_here() {
    let mut t = T::new();
    t.set("wA:p1", "working");
    t.poll();
    t.set("wA:p1", "done");
    t.poll();
    assert!(t.new_msgs().is_empty());
}

#[test]
fn blocked_agent_is_posted_when_away_begins() {
    let mut t = T::new();
    t.set("wA:p1", "working");
    t.poll();
    t.set("wA:p1", "blocked");
    t.poll();
    assert!(t.new_msgs().is_empty());
    t.go_afk();
    t.poll();
    t.poll();
    assert_eq!(t.new_msgs().len(), 1);
}

#[test]
fn other_agent_says_how_to_answer() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.set("wB:p1", "idle");
    t.tell("!attach repo-wA");
    t.working_then("idle", "wB:p1");
    assert!(t.last_msg().contains("Reply to this message"));
    t.working_then("idle", "wA:p1");
    assert!(!t.last_msg().contains("Reply to this message"));
}

#[test]
fn text_prompts_the_only_agent() {
    let mut t = T::new();
    t.prompted("Hello");
    assert_eq!(t.herdr.raw_prompts(), ["[Element] Hello"]);
}

#[test]
fn slash_command_is_not_marked() {
    let mut t = T::new();
    t.prompted("/model opus");
    assert_eq!(t.herdr.raw_prompts(), ["/model opus"]);
}

#[test]
fn answer_comes_back_while_present() {
    let mut t = T::new();
    t.prompted("Hello");
    t.set("wA:p1", "working");
    t.poll();
    assert!(t.new_msgs().is_empty());
    t.said("wA:p1", &[("u2", "Looking.")], None);
    t.poll();
    assert!(t.new_msgs().is_empty());
    t.said(
        "wA:p1",
        &[("u2", "Looking."), ("u3", "Still looking.")],
        None,
    );
    t.poll();
    assert_eq!(t.new_msgs(), ["Looking."]);
    t.advance(21.0);
    t.poll();
    assert_eq!(t.new_msgs(), ["Looking.", "Still looking."]);
    t.said(
        "wA:p1",
        &[
            ("u2", "Looking."),
            ("u3", "Still looking."),
            ("u4", "Hello. What need doing?"),
        ],
        None,
    );
    t.set("wA:p1", "idle");
    t.poll();
    t.poll();
    assert_eq!(
        t.new_msgs(),
        ["Looking.", "Still looking.", "Hello. What need doing?"]
    );
    assert_eq!(
        t.matrix.sent_key("msgtype", None)[t.start..],
        ["m.notice", "m.notice", "m.text"]
    );
    assert_eq!(
        t.matrix
            .sent_key("formatted_body", None)
            .last()
            .expect("one"),
        "Hello. What need doing?"
    );
}

#[test]
fn answer_that_went_out_quietly_notifies_at_the_end() {
    let mut t = T::new();
    t.prompted("Hello");
    t.set("wA:p1", "working");
    t.said("wA:p1", &[("u2", "All done.")], None);
    t.poll();
    t.advance(21.0);
    t.poll();
    t.set("wA:p1", "idle");
    t.poll();
    assert_eq!(t.new_msgs(), ["All done.", "All done."]);
    assert_eq!(
        t.matrix.sent_key("msgtype", None)[t.start..],
        ["m.notice", "m.text"]
    );
}

#[test]
fn exit_now_ends_the_attached_agent() {
    let mut t = T::new();
    t.prompted("Hello");
    t.set("wA:p1", "working");
    t.poll();
    t.tell("!exit now");
    assert_eq!(t.herdr.last(), c(&["pane", "close", "wA:p1"]));
    assert_eq!(t.last_msg(), "Closed **repo-wA**.");
    assert_eq!(t.bot.state().attached, None);
    t.herdr.remove("wA:p1");
    t.poll();
    assert_eq!(t.matrix.typing(), [true, false]);
}

#[test]
fn fast_answer_without_seeing_working() {
    let mut t = T::new();
    t.prompted("Hello");
    t.said("wA:p1", &[("u2", "quick")], None);
    t.poll();
    assert!(t.last_msg().contains("quick"));
}

#[test]
fn screen_when_no_transcript() {
    let mut t = T::new();
    t.prompted("Hello");
    t.answers.borrow_mut().clear();
    t.set("wA:p1", "working");
    t.poll();
    t.set("wA:p1", "idle");
    t.poll();
    assert_eq!(t.last_msg(), "screen of wA:p1");
}

#[test]
fn last_repeats_the_answer() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.tell("!last");
    assert_eq!(t.last_msg(), "Nothing said yet.");
    t.said(
        "wA:p1",
        &[("u0", "Looking."), ("u1", "All tests pass.")],
        None,
    );
    t.tell("!last");
    assert_eq!(t.last_msg(), "All tests pass.");
}

#[test]
fn stop_interrupts() {
    let mut t = T::new();
    t.prompted("Hello");
    t.set("wA:p1", "working");
    t.tell("!stop");
    assert_eq!(
        t.herdr.did("send-keys"),
        vec![c(&["agent", "send-keys", "wA:p1", "esc"])]
    );
    assert_eq!(t.last_msg(), "Interrupted **repo-wA**.");
    t.set("wA:p1", "idle");
    t.poll();
    assert_eq!(t.last_msg(), "Interrupted **repo-wA**.");
    t.tell("!stop");
    assert_eq!(t.last_msg(), "**repo-wA** is not working; nothing to stop.");
    assert_eq!(t.herdr.did("send-keys").len(), 1);
}

#[test]
fn delivered_prompt_is_marked_read() {
    let mut t = T::new();
    t.prompted("Hello");
    let room = quote(&t.room);
    assert!(t.matrix.has(
        "POST",
        &format!("/rooms/{room}/receipt/m.read/%24mine"),
        Some(json!({})),
        &[]
    ));
    t.tell("!agents");
    assert_eq!(
        t.matrix
            .calls()
            .iter()
            .filter(|c| c.1.contains("/receipt/"))
            .count(),
        1
    );
}

#[test]
fn slash_command_shows_the_screen() {
    let mut t = T::new();
    t.prompted("/model opus");
    assert_eq!(
        t.herdr.did("prompt"),
        vec![c(&["agent", "prompt", "wA:p1", "/model opus"])]
    );
    t.poll();
    assert!(t.new_msgs().is_empty());
    t.poll();
    t.poll();
    assert_eq!(t.new_msgs(), ["screen of wA:p1"]);
}

#[test]
fn slash_command_that_answers_posts_the_answer() {
    let mut t = T::new();
    t.prompted("/tt:review");
    t.set("wA:p1", "working");
    t.poll();
    t.poll();
    t.said("wA:p1", &[("u2", "No findings.")], None);
    t.set("wA:p1", "idle");
    t.poll();
    assert_eq!(t.new_msgs(), ["No findings."]);
}

#[test]
fn attach_never_sends_the_screen() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.set("wB:p1", "blocked");
    t.tell("!attach repo-wA");
    assert_eq!(t.last_msg(), "**repo-wA** attached; it is idle.");
    t.said("wB:p1", &[], Some(crate::fakes::fruit()));
    t.tell("!attach repo-wB");
    assert!(
        t.last_msg()
            .starts_with("**repo-wB** attached; it waits on you.\n\n**Pick a fruit?**")
    );
    assert!(t.herdr.did("read").is_empty());
}

#[test]
fn new_starts_claude_in_a_folder() {
    let mut t = T::new();
    t.bot.deps_mut().name = Some("Claude".into());
    for folder in ["tupakkatapa/alpha", "jhvst/alpha", "tupakkatapa/beta"] {
        std::fs::create_dir_all(t.dir.path().join("ws").join(folder)).expect("mkdir");
    }
    t.tell("!new alpha");
    assert!(t.last_msg().contains("jhvst/alpha"));
    assert!(t.last_msg().contains("tupakkatapa/alpha"));
    assert!(t.herdr.did("prompt").is_empty());
    t.tell("!new beta");
    let beta = t
        .dir
        .path()
        .join("ws/tupakkatapa/beta")
        .to_string_lossy()
        .into_owned();
    let calls = t.herdr.calls();
    assert_eq!(
        calls[calls.len() - 2..],
        [
            c(&[
                "workspace",
                "create",
                "--cwd",
                &beta,
                "--label",
                "beta",
                "--no-focus"
            ]),
            c(&["pane", "run", "wN:p1", "claude"]),
        ]
    );
    assert_eq!(t.bot.state().attached.as_deref(), Some("wN:p1"));
    let names: Vec<String> = t
        .matrix
        .calls()
        .into_iter()
        .filter(|c| c.0 == "PUT" && c.1.contains("/state/m.room.member/"))
        .map(|c| {
            c.2.expect("body")["displayname"]
                .as_str()
                .expect("name")
                .to_owned()
        })
        .collect();
    assert_eq!(names, ["Claude · beta"]);
    assert!(t.last_msg().contains(&format!("Started Claude in {beta}")));
    t.set("wN:p1", "blocked");
    t.poll();
    t.poll();
    assert!(t.last_msg().contains("screen of wN:p1"));
    t.herdr.0.borrow_mut().cwd.insert("wN:p1".into(), beta);
    t.tell("!new tupakkatapa/beta");
    assert!(t.last_msg().contains("runs there already"));
    assert_eq!(
        t.herdr
            .calls()
            .iter()
            .filter(|c| c[0] == "workspace" && c[1] == "create")
            .count(),
        1
    );
}

#[test]
fn long_answer_is_split_into_messages() {
    let mut t = T::new();
    let text = ["a", "b", "c"].map(|p| p.repeat(5000)).join("\n\n");
    t.say(
        &text,
        To {
            pre: Some("screen"),
            ..To::default()
        },
    );
    assert_eq!(
        t.new_msgs(),
        [
            "a".repeat(5000),
            "b".repeat(5000),
            format!("{}\n\nscreen", "c".repeat(5000))
        ]
    );
}

#[test]
fn typing_while_the_agent_works() {
    let mut t = T::new();
    t.prompted("Hello");
    t.set("wA:p1", "working");
    t.poll();
    t.said("wA:p1", &[("u2", "done")], None);
    t.set("wA:p1", "idle");
    t.poll();
    t.poll();
    assert_eq!(t.matrix.typing(), [true, false]);
}

#[test]
fn attach_switches_the_chat() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.set("wB:p1", "idle");
    t.tell("go");
    assert!(t.last_msg().contains("Not attached"));
    t.tell("!attach repo");
    assert!(
        t.last_msg()
            .starts_with("\"repo\" matches 2 agents. Attach to which one?")
    );
    t.tell("!attach nope");
    assert!(
        t.last_msg()
            .starts_with("No agent matches \"nope\". Attach to which one?")
    );
    t.tell("!attach");
    assert!(t.last_msg().starts_with("Attach to which agent?"));
    t.said("wB:p1", &[("u1", "last words")], None);
    t.tell("!attach WB");
    assert_eq!(
        t.last_msg(),
        "**repo-wB** attached; it is idle.\n\nlast words"
    );
    t.tell("go");
    assert_eq!(
        t.herdr.did("prompt"),
        vec![c(&["agent", "prompt", "wB:p1", "go"])]
    );
    t.tell("!agents");
    assert!(t.last_msg().contains("> repo-wB  idle"));
    assert!(t.last_msg().contains("  repo-wA  idle"));
}

#[test]
fn attach_names_the_chat_after_the_agent() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.set("wB:p1", "idle");
    t.bot.deps_mut().name = Some("Claude".into());
    t.tell("!attach repo-wB");
    let path = format!(
        "/rooms/{}/state/m.room.member/{}",
        quote(&t.room),
        quote(BOT)
    );
    let puts: Vec<_> = t
        .matrix
        .calls()
        .into_iter()
        .filter(|c| c.0 == "PUT" && c.1 == path)
        .map(|c| c.2)
        .collect();
    assert_eq!(
        puts,
        vec![Some(
            json!({"membership": "join", "avatar_url": "mxc://a/b", "displayname": "Claude · repo-wB"})
        )]
    );
    t.tell("!exit now");
    let puts: Vec<_> = t
        .matrix
        .calls()
        .into_iter()
        .filter(|c| c.0 == "PUT" && c.1.contains("/state/m.room.member/"))
        .collect();
    assert_eq!(
        puts.last().expect("put").2.as_ref().expect("body")["displayname"],
        "Claude"
    );
}

#[test]
fn agents_of_one_workspace_are_numbered() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.set("wA:p2", "idle");
    t.tell("!attach repo-wA:2");
    assert_eq!(t.bot.state().attached.as_deref(), Some("wA:p2"));
}

#[test]
fn reply_goes_to_the_agent_of_that_message() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.set("wB:p1", "idle");
    t.tell("!attach repo-wA");
    t.working_then("idle", "wB:p1");
    let notice = t.matrix.last_id();
    t.tell_with(
        "> <@herdr:example.org> repo-wB is idle\n\nok",
        &reply_to(&notice),
    );
    assert_eq!(
        t.herdr.did("prompt"),
        vec![c(&["agent", "prompt", "wB:p1", "ok"])]
    );
    assert_eq!(t.bot.state().attached.as_deref(), Some("wA:p1"));
}

#[test]
fn screen_command() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.tell("!screen 5");
    assert!(t.herdr.calls().contains(&c(&[
        "agent", "read", "wA:p1", "--source", "recent", "--lines", "5", "--format", "text"
    ])));
}

#[test]
fn away_commands() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    for (word, away) in [
        ("away", Away::On),
        ("here", Away::Off),
        ("afk", Away::On),
        ("back", Away::Off),
        ("afk", Away::On),
        ("auto", Away::Off),
    ] {
        t.tell(&format!("!{word}"));
        assert_eq!(t.bot.state().away, away, "{word}");
    }
    assert!(t.last_msg().starts_with("Here:"));
}

#[test]
fn unknown_command() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    for word in ["!there", "!lst"] {
        t.tell(word);
        assert!(t.last_msg().contains("Unknown command"), "{word}");
    }
    assert!(t.herdr.did("prompt").is_empty());
}

#[test]
fn help_lists_the_commands_of_the_main_chat() {
    let mut t = T::new();
    t.tell("!help");
    let help = t.last_msg();
    for word in [
        "!agents",
        "!attach",
        "!dm <name>",
        "!leave <name>",
        "!exit",
        "!away",
    ] {
        assert!(help.contains(word), "{word}");
    }
    for word in [
        "!list", "!spawn", "!kill", "!close", "!voice", "!afk", "!auto",
    ] {
        assert!(!help.contains(word), "{word}");
    }
    let screen = help.split("\n\n").last().expect("a screen");
    for line in screen.lines() {
        assert!(line.chars().count() <= 40, "{line}");
    }
}

#[test]
fn ignores_strangers_edits_and_other_rooms() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.tell_full("rm -rf /", "@x:evil", &json!({}));
    t.tell_with("edit", &json!({"m.relates_to": {"rel_type": "m.replace"}}));
    t.queue(msg(
        "!other:example.org",
        "hi",
        OWNER,
        1_000_000,
        &json!({}),
    ));
    t.sync();
    assert!(t.herdr.did("prompt").is_empty());
}

#[test]
fn answer_to_a_reply_carries_no_hint() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.set("wB:p1", "idle");
    t.tell("!attach repo-wA");
    t.working_then("idle", "wB:p1");
    assert!(t.last_msg().contains("Reply to this message"));
    let notice = t.matrix.last_id();
    t.tell_with("ok", &reply_to(&notice));
    t.set("wB:p1", "working");
    t.poll();
    t.said("wB:p1", &[("u2", "Sure.")], None);
    t.set("wB:p1", "idle");
    t.poll();
    assert_eq!(t.last_msg(), "**repo-wB**\n\nSure.");
}

#[test]
fn flapping_agent_is_reported_once() {
    let mut t = T::new();
    t.said("wA:p1", &[("u1", "All tests pass.")], None);
    t.working_then("idle", "wA:p1");
    t.set("wA:p1", "working");
    t.poll();
    t.set("wA:p1", "idle");
    t.poll();
    assert_eq!(t.new_msgs(), ["All tests pass."]);
    t.said(
        "wA:p1",
        &[("u1", "All tests pass."), ("u2", "And lint.")],
        None,
    );
    t.set("wA:p1", "working");
    t.poll();
    t.set("wA:p1", "idle");
    t.poll();
    assert_eq!(t.new_msgs(), ["All tests pass.", "And lint."]);
}

#[test]
fn text_that_starts_with_dashes_is_delivered() {
    let mut t = T::new();
    t.prompted("--no-verify is wrong here");
    assert_eq!(
        t.herdr.did("prompt"),
        vec![c(&[
            "agent",
            "prompt",
            "wA:p1",
            "--no-verify is wrong here"
        ])]
    );
    t.said("wA:p1", &[], Some(crate::fakes::fruit()));
    t.set("wA:p1", "blocked");
    t.poll();
    t.tell("--help");
    let calls = t.herdr.calls();
    assert_eq!(
        calls[calls.len() - 2],
        c(&["pane", "send-text", "wA:p1", "--help"])
    );
}

#[test]
fn reply_keeps_the_owners_own_quote() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.say(
        "hi",
        To {
            pane: Some("wA:p1"),
            ..To::default()
        },
    );
    let reply = reply_to(&t.matrix.last_id());
    t.tell_with(
        "> <@herdr:example.org> hi\n> there\n\n> see this\nfix it",
        &reply,
    );
    t.tell_with("> see this\nfix it", &reply);
    let prompts: Vec<String> = t
        .herdr
        .did("prompt")
        .into_iter()
        .map(|c| c[3].clone())
        .collect();
    assert_eq!(prompts, ["> see this\nfix it"; 2]);
}

#[test]
fn exit_asks_first() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    for word in ["!exit", "!close"] {
        t.tell(word);
        assert!(!t.herdr.calls().iter().any(|c| c[0] == "pane"));
        assert_eq!(
            t.last_msg(),
            "Send !exit now to end **repo-wA** and close its pane."
        );
    }
    t.tell("!exit now");
    assert_eq!(t.herdr.last(), c(&["pane", "close", "wA:p1"]));
    assert_eq!(t.last_msg(), "Closed **repo-wA**.");
}

#[test]
fn default_answer_reads_the_agents_transcript() {
    let mut t = T::new();
    let home = t.dir.path().join("home");
    let project = home.join(".claude/projects/-x");
    std::fs::create_dir_all(&project).expect("mkdir");
    let lines = [
        json!({"type": "user", "message": {"content": "Hello"}}),
        json!({"type": "assistant", "uuid": "a1", "message": {"content": [{"type": "text", "text": "Hi there."}]}}),
    ];
    std::fs::write(
        project.join("s-wA:p1.jsonl"),
        lines
            .iter()
            .map(|l| l.to_string() + "\n")
            .collect::<String>(),
    )
    .expect("write");
    t.bot.deps_mut().answer = Box::new(move |a| herdr_matrix::herdr::claude_turn(a, Some(&home)));
    t.working_then("idle", "wA:p1");
    assert_eq!(t.new_msgs(), ["Hi there."]);
}

#[test]
fn failed_profile_is_set_on_a_later_tick() {
    let mut t = T::new();
    let pic = t.dir.path().join("pic.png");
    std::fs::write(&pic, b"png").expect("write");
    t.bot.deps_mut().avatar = Some(pic);
    t.matrix.0.borrow_mut().refuse.push("/profile/".into());
    t.bot.tick().expect("tick");
    assert!(
        t.log_lines()
            .iter()
            .any(|l| l.starts_with("error: profile"))
    );
    t.matrix.0.borrow_mut().refuse.clear();
    t.bot.tick().expect("tick");
    t.bot.tick().expect("tick");
    let puts = t
        .matrix
        .calls()
        .into_iter()
        .filter(|c| c.1.ends_with("/avatar_url"))
        .count();
    assert_eq!(puts, 1);
}

fn left(t: &T, room: &str) -> bool {
    t.matrix.has(
        "POST",
        &format!("/rooms/{}/leave", quote(room)),
        Some(json!({})),
        &[],
    )
}

#[test]
fn public_room_is_left() {
    let mut t = T::new();
    t.matrix
        .0
        .borrow_mut()
        .join_rules
        .insert("!dm:example.org".into(), "public".into());
    t.queue(invite("!dm:example.org", OWNER));
    t.sync();
    assert!(left(&t, "!dm:example.org"));
    assert_eq!(t.bot.state().control.as_deref(), Some(t.room.as_str()));
    assert!(t.new_msgs().is_empty());
}

#[test]
fn room_made_by_a_stranger_is_left() {
    let mut t = T::new();
    t.matrix
        .0
        .borrow_mut()
        .creators
        .insert("!dm:example.org".into(), "@x:evil".into());
    t.queue(invite("!dm:example.org", OWNER));
    t.sync();
    assert!(left(&t, "!dm:example.org"));
    assert_eq!(t.bot.state().control.as_deref(), Some(t.room.as_str()));
}

#[test]
fn room_with_a_stranger_invited_is_left() {
    let mut t = T::new();
    t.matrix.0.borrow_mut().members.insert(
        "!dm:example.org".into(),
        vec![OWNER.into(), BOT.into(), "@x:evil".into()],
    );
    t.queue(invite("!dm:example.org", OWNER));
    t.sync();
    assert!(left(&t, "!dm:example.org"));
    assert_eq!(t.bot.state().control.as_deref(), Some(t.room.as_str()));
}

#[test]
fn invite_of_someone_else_is_not_joined() {
    let mut t = T::new();
    let mut batch = invite("!dm:example.org", OWNER);
    batch["rooms"]["invite"]["!dm:example.org"]["invite_state"]["events"][0]["state_key"] =
        json!("@other:example.org");
    t.queue(batch);
    t.sync();
    assert!(!t.matrix.calls().iter().any(|c| c.1.starts_with("/join/")));
}

#[test]
fn stranger_in_the_chat_moves_it() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let old = t.room.clone();
    t.queue(
        json!({"next_batch": "n5", "rooms": {"join": {&old: {"timeline": {"events": [
        {"type": "m.room.member", "sender": "@x:evil", "state_key": "@x:evil",
         "content": {"membership": "join"}, "origin_server_ts": ms(t.now())}]}}}}}),
    );
    t.sync();
    assert!(left(&t, &old));
    let posted = t.matrix.sent_key("body", Some(&old)).len();
    t.bot.tick().expect("tick");
    let new = t.bot.state().control.clone().expect("a new chat");
    assert_ne!(new, old);
    assert_eq!(t.matrix.sent_key("body", Some(&old)).len(), posted);
    assert!(!t.matrix.sent_key("body", Some(&new)).is_empty());
}

#[test]
fn attach_to_an_agent_in_an_odd_state() {
    let mut t = T::new();
    t.set("wA:p1", "unknown");
    t.set("wB:p1", "idle");
    t.tell("!attach repo-wA");
    assert_eq!(t.last_msg(), "**repo-wA** attached.");
}
