//! Voice messages: heard, and typed as the owner would have.

use crate::fakes::{T, c, fruit, reply_to};
use herdr_matrix::Error;
use serde_json::json;

#[test]
fn voice_message_is_a_prompt() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.speak("run the tests");
    assert_eq!(*t.heard.borrow(), vec![b"img mxc://x/v".to_vec()]);
    assert_eq!(
        t.herdr.did("prompt"),
        vec![c(&["agent", "prompt", "wA:p1", "run the tests"])]
    );
    assert_eq!(t.new_msgs(), ["\u{1F3A4} run the tests"]);
    assert_eq!(
        t.matrix.sent_key("msgtype", None).last().expect("one"),
        "m.notice"
    );
    assert_eq!(std::fs::read_dir(t.files()).expect("list").count(), 0);
}

#[test]
fn voice_command() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.speak("!agents");
    assert_eq!(t.last_msg(), "Agents:\n\n> repo-wA  idle");
    assert!(t.herdr.did("prompt").is_empty());
}

#[test]
fn spoken_command() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.speak("Agents.");
    assert_eq!(t.last_msg(), "Agents:\n\n> repo-wA  idle");
    t.speak("Attach repo-wA.");
    assert_eq!(t.bot.state().attached.as_deref(), Some("wA:p1"));
    t.speak("Screen 5.");
    assert_eq!(t.herdr.did("read").last().expect("a read")[6], "5");
    assert!(t.herdr.did("prompt").is_empty());
}

#[test]
fn spoken_chat_and_exit_commands_are_prompts() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let heard = [
        "Dm repo-wA.",
        "Spawn repo-wA.",
        "Leave repo-wA.",
        "Exit now.",
        "Close now.",
    ];
    for said in heard {
        t.speak(said);
    }
    let prompts: Vec<String> = t
        .herdr
        .did("prompt")
        .into_iter()
        .map(|c| c[3].clone())
        .collect();
    assert_eq!(prompts, heard);
    assert!(t.bot.state().spawned.is_empty());
    assert!(!t.herdr.calls().iter().any(|c| c[0] == "pane"));
}

#[test]
fn echo_shows_what_the_voice_message_became() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.speak("Agents.");
    t.working_then("blocked", "wA:p1");
    t.speak("Yes.");
    let echoes: Vec<String> = t
        .new_msgs()
        .into_iter()
        .filter(|m| m.starts_with('\u{1F3A4}'))
        .collect();
    assert_eq!(
        echoes,
        [
            "\u{1F3A4} Agents. \u{2192} !agents",
            "\u{1F3A4} Yes. \u{2192} y"
        ]
    );
}

#[test]
fn spoken_key() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.working_then("blocked", "wA:p1");
    for (spoken, key) in [
        ("Yes.", "y"),
        ("Two.", "2"),
        ("three", "3"),
        ("Cancel.", "esc"),
    ] {
        t.speak(spoken);
        assert_eq!(
            t.herdr.last(),
            c(&["agent", "send-keys", "wA:p1", key]),
            "{spoken}"
        );
    }
}

#[test]
fn long_spoken_sentence_is_a_prompt() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.speak("Stop the tests and run only the parser ones.");
    assert_eq!(
        t.herdr.did("prompt").last().expect("a prompt")[3],
        "Stop the tests and run only the parser ones."
    );
    assert!(t.herdr.did("send-keys").is_empty());
}

#[test]
fn short_sentence_is_not_a_destructive_command() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    for heard in [
        "Close enough.",
        "Stop, wait.",
        "Help me fix",
        "Last one wins",
        "Agents are slow",
    ] {
        t.speak(heard);
        assert_eq!(t.herdr.did("prompt").last().expect("a prompt")[3], heard);
    }
    assert!(!t.herdr.calls().contains(&c(&["pane", "close", "wA:p1"])));
    assert!(t.herdr.did("send-keys").is_empty());
}

#[test]
fn voice_answers_a_question() {
    let mut t = T::new();
    t.said("wA:p1", &[], Some(fruit()));
    t.working_then("blocked", "wA:p1");
    t.speak("a banana");
    let calls = t.herdr.calls();
    assert_eq!(
        calls[calls.len() - 3..],
        [
            c(&["agent", "send-keys", "wA:p1", "3"]),
            c(&["pane", "send-text", "wA:p1", "a banana"]),
            c(&["agent", "send-keys", "wA:p1", "enter"]),
        ]
    );
}

#[test]
fn voice_reply_goes_to_the_agent_of_that_message() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.set("wB:p1", "idle");
    t.tell("!attach repo-wA");
    t.working_then("idle", "wB:p1");
    let id = t.matrix.last_id();
    t.speak_with("ok", &reply_to(&id));
    assert_eq!(
        t.herdr.did("prompt"),
        vec![c(&["agent", "prompt", "wB:p1", "ok"])]
    );
}

#[test]
fn empty_voice_message_is_refused() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.speak("");
    assert_eq!(t.new_msgs(), ["Could not understand the voice message."]);
    assert!(t.herdr.did("prompt").is_empty());
}

#[test]
fn failed_transcription_is_said() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.bot.deps_mut().transcribe = Some(Box::new(|_| {
        Err(Error::Tool("ffmpeg: invalid data".into()))
    }));
    t.speak("anything");
    assert_eq!(
        t.new_msgs(),
        ["Failed to hear the voice message: ffmpeg: invalid data"]
    );
    assert!(t.herdr.did("prompt").is_empty());
    assert_eq!(std::fs::read_dir(t.files()).expect("list").count(), 0);
}

#[test]
fn voice_without_a_model() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.bot.deps_mut().transcribe = None;
    t.speak("anything");
    assert_eq!(
        t.new_msgs(),
        ["Voice messages are not set up: set transcribe.model."]
    );
    assert!(t.heard.borrow().is_empty());
}

#[test]
fn voice_only_from_the_owner() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    *t.transcript.borrow_mut() = "rm -rf /".into();
    t.tell_full(
        "Voice message.ogg",
        "@x:evil",
        &json!({"msgtype": "m.audio", "url": "mxc://x/v"}),
    );
    assert!(t.heard.borrow().is_empty());
    assert!(t.herdr.did("prompt").is_empty());
}

#[test]
fn transcript_outlives_a_failed_delete() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.bot.deps_mut().transcribe = Some(Box::new(|path| {
        std::fs::remove_file(path)?;
        Ok("hello there".into())
    }));
    t.speak("unused");
    assert_eq!(
        t.herdr.did("prompt"),
        vec![c(&["agent", "prompt", "wA:p1", "hello there"])]
    );
}
