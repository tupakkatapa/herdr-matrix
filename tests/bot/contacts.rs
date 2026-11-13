//! Agents with a chat of their own, as contacts.

use crate::fakes::{OWNER, PERMISSION, T, c, fruit, ms, reply_to};
use herdr_matrix::matrix::quote;
use serde_json::json;

#[test]
fn spawn_registers_a_contact() {
    let mut t = T::new();
    t.picture(b"blue");
    t.set("wA:p1", "idle");
    t.bot.deps_mut().name = Some("Claude".into());
    let chat = t.spawn("wA:p1", "repo-wA");
    let user = "@herdr_repo-wa:example.org";
    assert_eq!(chat.user, user);
    assert!(t.matrix.has(
        "POST",
        "/register",
        Some(json!({"type": "m.login.application_service", "username": "herdr_repo-wa", "inhibit_login": true})),
        &[]
    ));
    let me = quote(user);
    let mine: Vec<_> = t
        .matrix
        .as_user(user)
        .into_iter()
        .filter(|c| c.1 != "/sync")
        .map(|c| (c.0, c.1, c.2))
        .collect();
    assert_eq!(
        mine[..3],
        [
            (
                "PUT".into(),
                format!("/profile/{me}/displayname"),
                Some(json!({"displayname": "Claude · repo-wA"}))
            ),
            (
                "PUT".into(),
                format!("/profile/{me}/avatar_url"),
                Some(json!({"avatar_url": "mxc://example.org/pic"}))
            ),
            (
                "POST".into(),
                "/createRoom".into(),
                Some(
                    json!({"preset": "trusted_private_chat", "is_direct": true, "invite": [OWNER]})
                )
            ),
        ]
    );
    assert_eq!(t.matrix.0.borrow().uploads, vec![b"blue".to_vec()]);
    let room = quote(&chat.room);
    let theirs: Vec<_> = t
        .matrix
        .calls()
        .into_iter()
        .filter(|c| c.1.contains(&room) || c.1.contains(&me))
        .collect();
    assert!(!theirs.is_empty());
    assert!(
        theirs
            .iter()
            .all(|c| c.3 == vec![("user_id".to_owned(), user.to_owned())])
    );
    assert!(t.matrix.as_user(user).iter().any(|c| c.1 == "/sync"));
    assert_eq!(t.in_chat(&chat).len(), 1);
    assert!(t.in_chat(&chat)[0].contains("repo-wA"));
    assert!(t.main().last().expect("one").contains(user));
}

#[test]
fn spawn_is_idempotent() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.matrix.0.borrow_mut().users.insert("herdr_repo-wa".into());
    let chat = t.spawn("wA:p1", "repo-wA");
    t.tell("!spawn repo-wA");
    assert_eq!(t.bot.state().spawned.keys().collect::<Vec<_>>(), ["wA:p1"]);
    assert_eq!(t.bot.state().spawned["wA:p1"].room, chat.room);
    assert_eq!(
        t.matrix
            .calls()
            .iter()
            .filter(|c| c.1 == "/createRoom")
            .count(),
        2
    );
    assert!(t.main().last().expect("one").contains("has a chat already"));
    assert!(t.main().last().expect("one").contains(&chat.user));
}

#[test]
fn dm_needs_an_agent() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.tell("!dm nothing");
    assert!(
        t.main()
            .last()
            .expect("one")
            .starts_with("No agent matches \"nothing\". Open a chat with which one?")
    );
    assert!(t.bot.state().spawned.is_empty());
}

#[test]
fn contacts_of_one_name_get_numbered() {
    let mut t = T::new();
    t.set("w+:p1", "idle");
    t.set("w-:p1", "idle");
    let a = t.spawn("w+:p1", "repo-w+");
    let b = t.spawn("w-:p1", "repo-w-");
    assert_eq!(
        (a.user.as_str(), b.user.as_str()),
        ("@herdr_repo-w:example.org", "@herdr_repo-w_2:example.org")
    );
}

#[test]
fn contact_picture_is_uploaded_once() {
    let mut t = T::new();
    let pic = t.picture(b"blue");
    t.set("wA:p1", "idle");
    t.set("wB:p1", "idle");
    t.spawn("wA:p1", "repo-wA");
    t.spawn("wB:p1", "repo-wB");
    t.bot.deps_mut().avatar = Some(pic);
    t.bot.profile().expect("profile");
    assert_eq!(t.matrix.0.borrow().uploads, vec![b"blue".to_vec()]);
    t.bot = t.make();
    t.picture(b"blue");
    t.set("wC:p1", "idle");
    t.spawn("wC:p1", "repo-wC");
    assert_eq!(t.matrix.0.borrow().uploads, vec![b"blue".to_vec()]);
    assert_eq!(
        t.matrix
            .calls()
            .iter()
            .filter(|c| c.1.ends_with("/avatar_url"))
            .count(),
        4
    );
}

#[test]
fn message_in_a_contact_chat_reaches_only_its_agent() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.set("wB:p1", "blocked");
    t.said("wB:p1", &[], Some(fruit()));
    t.tell("!attach repo-wB");
    let chat = t.spawn("wA:p1", "repo-wA");
    let start = t.herdr.calls().len();
    for body in ["1", "Pear please", "y", "/compact"] {
        t.tell_in(&chat, body);
    }
    let calls = t.herdr.calls()[start..].to_vec();
    assert!(!calls.iter().any(|c| c.contains(&"wB:p1".to_owned())));
    assert_eq!(
        t.herdr.raw_prompts(),
        [
            "[Element] 1",
            "[Element] Pear please",
            "[Element] y",
            "/compact"
        ]
    );
    assert!(
        calls
            .iter()
            .filter(|c| c[0] == "agent" && c[1] == "prompt")
            .all(|c| c[2] == "wA:p1")
    );
    t.working_then("idle", "wB:p1");
    let notice = t.matrix.last_id();
    t.tell_in_full(&chat, "ok", None, None, OWNER, &reply_to(&notice));
    assert_eq!(
        t.herdr.did("prompt").last().expect("a prompt"),
        &c(&["agent", "prompt", "wA:p1", "ok"])
    );
}

#[test]
fn contact_chat_answers_its_agents_choice() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let chat = t.spawn("wA:p1", "repo-wA");
    t.said("wA:p1", &[], Some(fruit()));
    t.working_then("blocked", "wA:p1");
    assert!(
        t.in_chat(&chat)
            .last()
            .expect("one")
            .contains("Pick a fruit?")
    );
    t.tell_in(&chat, "2");
    assert_eq!(
        t.herdr.did("send-keys"),
        vec![c(&["agent", "send-keys", "wA:p1", "2"])]
    );
    t.tell_in(&chat, "Banana");
    let calls = t.herdr.calls();
    assert_eq!(
        calls[calls.len() - 3..],
        [
            c(&["agent", "send-keys", "wA:p1", "3"]),
            c(&["pane", "send-text", "wA:p1", "Banana"]),
            c(&["agent", "send-keys", "wA:p1", "enter"]),
        ]
    );
    assert!(t.herdr.did("prompt").is_empty());
}

#[test]
fn contact_turn_reports_only_in_its_chat() {
    let mut t = T::new();
    let (chat, before) = t.contact_turn();
    t.said("wA:p1", &[("u1", "Looking."), ("u2", "Still.")], None);
    t.poll();
    t.advance(21.0);
    t.poll();
    t.said(
        "wA:p1",
        &[("u1", "Looking."), ("u2", "Still."), ("u3", "Done it.")],
        None,
    );
    t.set("wA:p1", "idle");
    t.poll();
    t.poll();
    assert_eq!(t.in_chat(&chat)[1..], ["Looking.", "Still.", "Done it."]);
    assert_eq!(
        t.in_chat_key(&chat, "msgtype")[1..],
        ["m.notice", "m.notice", "m.text"]
    );
    assert_eq!(t.main().len(), before);
    let (room, me) = (quote(&chat.room), quote(&chat.user));
    let typing: Vec<_> = t
        .matrix
        .calls()
        .into_iter()
        .filter(|c| c.1.contains("/typing/"))
        .collect();
    assert!(!typing.is_empty());
    assert!(
        typing
            .iter()
            .all(|c| c.1 == format!("/rooms/{room}/typing/{me}")
                && c.3 == vec![("user_id".to_owned(), chat.user.clone())])
    );
    assert_eq!(t.matrix.typing().first(), Some(&true));
    assert_eq!(t.matrix.typing().last(), Some(&false));
    assert!(t.matrix.has(
        "POST",
        &format!("/rooms/{room}/receipt/m.read/%24mine"),
        Some(json!({})),
        &[("user_id", &chat.user)]
    ));
}

#[test]
fn contact_turn_that_ends_on_a_question() {
    let mut t = T::new();
    let (chat, before) = t.contact_turn();
    t.set("wA:p1", "blocked");
    t.said("wA:p1", &[("u1", "Looking.")], Some(fruit()));
    t.poll();
    let said = t.in_chat(&chat)[1..].to_vec();
    assert_eq!(said[0], "Looking.");
    assert!(said[1].starts_with("**Pick a fruit?**"));
    assert_eq!(said.len(), 2);
    assert_eq!(t.main().len(), before);
}

#[test]
fn reaction_in_a_contact_chat_answers_its_choice_only() {
    let mut t = T::new();
    t.herdr
        .0
        .borrow_mut()
        .screens
        .insert("wA:p1".into(), PERMISSION.into());
    t.set("wB:p1", "idle");
    let (chat, _) = t.contact_turn();
    t.set("wA:p1", "blocked");
    t.said("wA:p1", &[("u1", "Looking.")], None);
    t.poll();
    assert!(
        t.in_chat(&chat)
            .last()
            .expect("one")
            .contains("Do you want to proceed?")
    );
    assert!(
        t.in_chat(&chat)
            .last()
            .expect("one")
            .contains("react \u{1F44D}")
    );
    t.working_then("blocked", "wB:p1");
    let mine = t.bot.state().asked["wA:p1"]
        .event
        .clone()
        .expect("an event");
    let theirs = t.bot.state().asked["wB:p1"]
        .event
        .clone()
        .expect("an event");
    assert!(t.matrix.ids().contains(&theirs));
    t.react_in(&chat, &theirs, "\u{1F44D}");
    assert!(t.herdr.did("send-keys").is_empty());
    t.react_in(&chat, &mine, "\u{1F44D}");
    assert_eq!(
        t.herdr.did("send-keys"),
        vec![c(&["agent", "send-keys", "wA:p1", "1"])]
    );
}

#[test]
fn contact_chat_takes_pictures() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let chat = t.spawn("wA:p1", "repo-wA");
    t.tell_in_full(
        &chat,
        "shot.png",
        None,
        None,
        OWNER,
        &json!({"msgtype": "m.image", "url": "mxc://x/y"}),
    );
    let prompt = t.herdr.did("prompt").last().cloned().expect("a prompt");
    assert_eq!(prompt[2], "wA:p1");
    assert!(prompt[3].starts_with("See the file "));
}

#[test]
fn contact_chat_takes_voice() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let chat = t.spawn("wA:p1", "repo-wA");
    t.poll();
    *t.transcript.borrow_mut() = "what changed".into();
    t.tell_in_full(
        &chat,
        "Voice message.ogg",
        None,
        None,
        OWNER,
        &json!({"msgtype": "m.audio", "url": "mxc://x/v"}),
    );
    assert_eq!(
        t.in_chat(&chat).last().expect("one"),
        "\u{1F3A4} what changed"
    );
    assert_eq!(
        t.herdr.did("prompt"),
        vec![c(&["agent", "prompt", "wA:p1", "what changed"])]
    );
    t.set("wA:p1", "working");
    t.poll();
    t.said("wA:p1", &[("u1", "Nothing.")], None);
    t.set("wA:p1", "idle");
    t.poll();
    assert_eq!(t.in_chat(&chat).last().expect("one"), "Nothing.");
    assert_eq!(t.main().len(), 1);
}

#[test]
fn contact_chat_commands() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.set("wB:p1", "idle");
    t.tell("!attach repo-wB");
    let chat = t.spawn("wA:p1", "repo-wA");
    t.said("wA:p1", &[("u0", "All tests pass.")], None);
    t.tell_in(&chat, "!last");
    assert_eq!(t.in_chat(&chat).last().expect("one"), "All tests pass.");
    t.tell_in(&chat, "!screen 5");
    assert!(t.herdr.calls().contains(&c(&[
        "agent", "read", "wA:p1", "--source", "recent", "--lines", "5", "--format", "text"
    ])));
    assert!(
        t.in_chat(&chat)
            .last()
            .expect("one")
            .contains("screen of wA:p1")
    );
    t.set("wA:p1", "working");
    t.tell_in(&chat, "!stop");
    assert_eq!(
        t.herdr.did("send-keys"),
        vec![c(&["agent", "send-keys", "wA:p1", "esc"])]
    );
    for (word, said) in [("away", "Away:"), ("here", "Here:")] {
        t.tell_in(&chat, &format!("!{word}"));
        assert!(t.in_chat(&chat).last().expect("one").starts_with(said));
    }
    t.tell_in(&chat, "!help");
    let help = t.in_chat(&chat).last().cloned().expect("one");
    assert!(help.contains("This chat is repo-wA's own"), "{help}");
    assert!(help.contains("!leave"));
    assert!(help.contains("!screen"));
    for word in ["!attach", "!dm", "!agents", "!new"] {
        assert!(!help.contains(word), "{word}");
    }
    for word in [
        "!attach repo-wB",
        "!spawn repo-wB",
        "!agents",
        "!list",
        "!new x",
        "!kill repo-wB",
    ] {
        t.tell_in(&chat, word);
        assert!(
            t.in_chat(&chat).last().expect("one").contains("main chat"),
            "{word}"
        );
    }
    assert_eq!(t.bot.state().attached.as_deref(), Some("wB:p1"));
    assert_eq!(t.bot.state().spawned.keys().collect::<Vec<_>>(), ["wA:p1"]);
    assert!(t.herdr.did("prompt").is_empty());
    assert_eq!(t.main().len(), 2);
}

#[test]
fn exit_in_a_contact_chat_ends_it() {
    let mut t = T::new();
    t.set("wA:p1", "working");
    t.set("wB:p1", "idle");
    t.tell("!attach repo-wB");
    let chat = t.spawn("wA:p1", "repo-wA");
    t.tell_in(&chat, "!exit");
    assert!(t.in_chat(&chat).last().expect("one").contains("!exit now"));
    t.tell_in(&chat, "!exit now");
    assert_eq!(t.herdr.last(), c(&["pane", "close", "wA:p1"]));
    let said = t.in_chat(&chat);
    assert_eq!(said[said.len() - 2..], ["Closed **repo-wA**.", "Bye."]);
    assert_eq!(t.bot.state().attached.as_deref(), Some("wB:p1"));
    assert!(t.bot.state().spawned.is_empty());
}

#[test]
fn away_notices_of_a_contact_go_to_its_chat() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.set("wB:p1", "idle");
    let chat = t.spawn("wA:p1", "repo-wA");
    let before = t.main().len();
    t.said("wA:p1", &[("u1", "All tests pass.")], None);
    t.working_then("idle", "wA:p1");
    assert_eq!(t.in_chat(&chat)[1..], ["All tests pass."]);
    assert_eq!(t.main().len(), before);
    t.set("wA:p1", "working");
    t.poll();
    t.set("wA:p1", "blocked");
    t.poll();
    assert!(
        t.in_chat(&chat)
            .last()
            .expect("one")
            .starts_with("Waits on a choice")
    );
    assert_eq!(t.main().len(), before);
    t.working_then("idle", "wB:p1");
    assert_eq!(t.main().len(), before + 1);
}

#[test]
fn kill_in_its_chat_ends_the_contact() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let chat = t.spawn("wA:p1", "repo-wA");
    t.tell_in(&chat, "!kill");
    assert_eq!(t.in_chat(&chat).last().expect("one"), "Bye.");
    let room = quote(&chat.room);
    assert!(t.matrix.has(
        "POST",
        &format!("/rooms/{room}/leave"),
        Some(json!({})),
        &[("user_id", &chat.user)]
    ));
    assert!(t.bot.state().spawned.is_empty());
    assert!(!t.herdr.calls().iter().any(|c| c[0] == "pane"));
    t.tell_in(&chat, "hello");
    assert!(t.herdr.did("prompt").is_empty());
}

#[test]
fn kill_from_the_main_chat() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let chat = t.spawn("wA:p1", "repo-wA");
    t.tell("!kill repo-wA");
    assert_eq!(t.in_chat(&chat).last().expect("one"), "Bye.");
    assert!(t.bot.state().spawned.is_empty());
    assert!(t.main().last().expect("one").contains("repo-wA"));
    t.tell("!kill repo-wA");
    assert!(t.main().last().expect("one").contains("has no chat"));
    assert!(!t.herdr.calls().iter().any(|c| c[0] == "pane"));
}

#[test]
fn contact_of_an_ended_agent_says_so_and_leaves() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let chat = t.spawn("wA:p1", "repo-wA");
    t.poll();
    t.gone("wA:p1");
    let said = t.in_chat(&chat);
    assert_eq!(said[said.len() - 2..], ["The agent ended.", "Bye."]);
    let room = quote(&chat.room);
    assert!(t.matrix.has(
        "POST",
        &format!("/rooms/{room}/leave"),
        Some(json!({})),
        &[("user_id", &chat.user)]
    ));
    assert!(t.bot.state().spawned.is_empty());
}

#[test]
fn contact_chat_takes_only_the_owners_fresh_messages() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let chat = t.spawn("wA:p1", "repo-wA");
    t.tell_in_full(&chat, "rm -rf /", None, None, "@x:evil", &json!({}));
    let old = ms(t.now() - 601.0);
    t.tell_in_full(&chat, "old", Some(old), None, OWNER, &json!({}));
    t.tell_in_full(
        &chat,
        "hi",
        None,
        Some("!other:example.org"),
        OWNER,
        &json!({}),
    );
    assert!(t.herdr.did("prompt").is_empty());
    let said = t.in_chat(&chat)[1..].to_vec();
    assert!(said[0].contains("Skipped 1 message"));
    assert_eq!(said.len(), 1);
    t.tell_in(&chat, "!lst");
    assert!(
        t.in_chat(&chat)
            .last()
            .expect("one")
            .contains("Unknown command !lst")
    );
}

#[test]
fn broken_contact_does_not_stop_the_others() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.set("wB:p1", "idle");
    let a = t.spawn("wA:p1", "repo-wA");
    let b = t.spawn("wB:p1", "repo-wB");
    t.matrix.0.borrow_mut().broken.insert(a.user);
    t.tell_in(&b, "hi");
    assert_eq!(
        t.herdr.did("prompt"),
        vec![c(&["agent", "prompt", "wB:p1", "hi"])]
    );
}

#[test]
fn agents_marks_contacts() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.set("wB:p1", "idle");
    t.spawn("wA:p1", "repo-wA");
    t.tell("!agents");
    let last = t.main().last().cloned().expect("one");
    assert!(last.contains("repo-wA  idle  (own chat)"));
    assert!(!last.contains("repo-wB  idle  (own chat)"));
}

#[test]
fn failing_chat_does_not_stop_the_others() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.set("wB:p1", "idle");
    let chat = t.spawn("wA:p1", "repo-wA");
    t.poll();
    t.said("wA:p1", &[("a1", "A done.")], None);
    t.said("wB:p1", &[("b1", "B done.")], None);
    t.set("wA:p1", "working");
    t.set("wB:p1", "working");
    t.poll();
    t.go_afk();
    t.set("wA:p1", "idle");
    t.set("wB:p1", "idle");
    let room = herdr_matrix::matrix::quote(&chat.room);
    t.matrix.0.borrow_mut().refuse.push(room);
    t.bot.tick().expect("tick");
    assert!(t.main().last().expect("a notice").contains("B done."));
    t.matrix.0.borrow_mut().refuse.clear();
    t.poll();
    assert_eq!(t.in_chat(&chat).last().map(String::as_str), Some("A done."));
}

#[test]
fn contact_outlives_a_short_gap_in_the_agent_list() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let chat = t.spawn("wA:p1", "repo-wA");
    t.poll();
    let said = t.in_chat(&chat).len();
    t.herdr.remove("wA:p1");
    t.poll();
    t.advance(30.0);
    t.poll();
    t.set("wA:p1", "idle");
    t.poll();
    assert_eq!(t.in_chat(&chat).len(), said);
    assert!(t.bot.state().spawned.contains_key("wA:p1"));
}

#[test]
fn contact_survives_a_start_before_its_agent_is_back() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let chat = t.spawn("wA:p1", "repo-wA");
    t.herdr.remove("wA:p1");
    t.bot = t.make();
    for _ in 0..3 {
        t.advance(30.0);
        t.poll();
    }
    t.set("wA:p1", "idle");
    t.poll();
    assert!(t.bot.state().spawned.contains_key("wA:p1"));
    assert!(!t.in_chat(&chat).contains(&"Bye.".to_owned()));
}

#[test]
fn message_while_the_agent_is_missing_is_not_sent() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let chat = t.spawn("wA:p1", "repo-wA");
    t.herdr.remove("wA:p1");
    t.tell_in(&chat, "hi");
    assert!(t.herdr.did("prompt").is_empty());
    assert!(
        t.in_chat(&chat)
            .last()
            .expect("one")
            .contains("nothing was sent")
    );
    assert!(t.bot.state().spawned.contains_key("wA:p1"));
}

#[test]
fn batch_stops_when_its_chat_ends() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let chat = t.spawn("wA:p1", "repo-wA");
    let mut batch = crate::fakes::msg(&chat.room, "!leave", OWNER, ms(t.now()), &json!({}));
    let events = batch["rooms"]["join"][&chat.room]["timeline"]["events"]
        .as_array_mut()
        .expect("events");
    let mut second = events[0].clone();
    second["content"]["body"] = json!("hello");
    events.push(second);
    t.matrix
        .0
        .borrow_mut()
        .user_syncs
        .entry(chat.user.clone())
        .or_default()
        .push(batch);
    t.sync();
    assert!(t.bot.state().spawned.is_empty());
    assert!(t.herdr.did("prompt").is_empty());
}

#[test]
fn turn_whose_chat_ended_is_dropped() {
    let mut t = T::new();
    t.contact_turn();
    assert!(t.bot.state().pending.contains_key("wA:p1"));
    t.bot.state_mut().spawned.clear();
    t.poll();
    assert!(t.bot.state().pending.is_empty());
}
