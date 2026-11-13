//! Choices an agent waits on: questions, permission prompts, keys and
//! reactions.

use crate::fakes::{PERMISSION, T, c, fruit, push, react};
use serde_json::json;

const THUMB: &str = "\u{1F44D}";

#[test]
fn permission_prompt_is_listed() {
    let mut t = T::new();
    t.herdr
        .0
        .borrow_mut()
        .screens
        .insert("wA:p1".into(), PERMISSION.into());
    t.working_then("blocked", "wA:p1");
    let note = t.last_msg();
    assert!(note.starts_with(
        "**Do you want to proceed?**\n1. Yes\n\
         2. Yes, and don't ask again for rm commands\n\
         3. No, and tell Claude what to do differently (esc)"
    ));
    assert!(note.contains("react \u{1F44D} for 1"));
    assert!(note.contains("rm -r build"));
    assert!(!note.contains("Esc to cancel"));
    let notice = t.matrix.last_id();
    t.queue(react(
        &t.room,
        &notice,
        "\u{1F44D}\u{fe0f}",
        crate::fakes::OWNER,
        1_000_000,
    ));
    t.sync();
    assert_eq!(
        t.herdr.did("send-keys").last().expect("a key"),
        &c(&["agent", "send-keys", "wA:p1", "1"])
    );
}

#[test]
fn reaction_of_a_stranger_or_elsewhere_is_ignored() {
    let mut t = T::new();
    t.working_then("blocked", "wA:p1");
    let id = t.matrix.last_id();
    t.react_from(&id, THUMB, "@x:evil");
    t.react("$unknown", THUMB);
    assert!(t.herdr.did("send-keys").is_empty());
}

#[test]
fn only_a_thumbs_up_answers() {
    let mut t = T::new();
    t.working_then("blocked", "wA:p1");
    let id = t.matrix.last_id();
    for key in ["\u{274c}", "\u{2705}", "\u{1F44E}", "2\u{fe0f}\u{20e3}"] {
        t.react(&id, key);
    }
    assert!(t.herdr.did("send-keys").is_empty());
}

#[test]
fn reaction_answers_a_choice_once() {
    let mut t = T::new();
    t.working_then("blocked", "wA:p1");
    let notice = t.matrix.last_id();
    t.react(&notice, THUMB);
    t.react(&notice, THUMB);
    assert_eq!(
        t.herdr.did("send-keys"),
        vec![c(&["agent", "send-keys", "wA:p1", "1"])]
    );
}

#[test]
fn reaction_only_answers_the_choice_it_was_posted_for() {
    let mut t = T::new();
    t.said("wA:p1", &[("u1", "All tests pass.")], None);
    t.working_then("idle", "wA:p1");
    let answer = t.matrix.last_id();
    t.set("wA:p1", "working");
    t.poll();
    t.set("wA:p1", "blocked");
    t.poll();
    let choice = t.matrix.last_id();
    t.react(&answer, THUMB);
    assert!(t.herdr.did("send-keys").is_empty());
    t.react(&choice, THUMB);
    assert_eq!(
        t.herdr.did("send-keys"),
        vec![c(&["agent", "send-keys", "wA:p1", "1"])]
    );
    t.react(&choice, THUMB);
    assert_eq!(t.herdr.did("send-keys").len(), 1);
}

#[test]
fn reaction_on_a_choice_that_ended_is_ignored() {
    let mut t = T::new();
    t.working_then("blocked", "wA:p1");
    let choice = t.matrix.last_id();
    t.set("wA:p1", "working");
    t.poll();
    t.set("wA:p1", "blocked");
    t.react(&choice, THUMB);
    assert!(t.herdr.did("send-keys").is_empty());
}

#[test]
fn choice_that_changes_while_blocked_is_posted_again() {
    let mut t = T::new();
    t.herdr
        .0
        .borrow_mut()
        .screens
        .insert("wA:p1".into(), PERMISSION.into());
    t.working_then("blocked", "wA:p1");
    let first = t.matrix.last_id();
    t.poll();
    assert_eq!(t.new_msgs().len(), 1);
    t.herdr
        .0
        .borrow_mut()
        .screens
        .insert("wA:p1".into(), push());
    t.poll();
    assert_eq!(t.new_msgs().len(), 2);
    assert!(t.last_msg().contains("git push --force"));
    t.react(&first, THUMB);
    assert!(t.herdr.did("send-keys").is_empty());
}

#[test]
fn reaction_to_a_choice_that_changed_is_refused() {
    let mut t = T::new();
    t.herdr
        .0
        .borrow_mut()
        .screens
        .insert("wA:p1".into(), PERMISSION.into());
    t.working_then("blocked", "wA:p1");
    let shown = t.matrix.last_id();
    t.herdr
        .0
        .borrow_mut()
        .screens
        .insert("wA:p1".into(), push());
    t.react(&shown, THUMB);
    assert!(t.herdr.did("send-keys").is_empty());
    let new = t.new_msgs();
    assert!(new[new.len() - 2].contains("changed"));
    assert!(new[new.len() - 1].contains("git push --force"));
    let id = t.matrix.last_id();
    t.react(&id, THUMB);
    assert_eq!(
        t.herdr.did("send-keys"),
        vec![c(&["agent", "send-keys", "wA:p1", "1"])]
    );
}

#[test]
fn typed_key_for_a_choice_that_changed_is_refused() {
    let mut t = T::new();
    t.prompted("Hello");
    t.herdr
        .0
        .borrow_mut()
        .screens
        .insert("wA:p1".into(), PERMISSION.into());
    t.set("wA:p1", "blocked");
    t.poll();
    t.herdr
        .0
        .borrow_mut()
        .screens
        .insert("wA:p1".into(), push());
    t.tell("1");
    assert!(t.herdr.did("send-keys").is_empty());
    assert!(t.last_msg().contains("git push --force"));
    t.tell("1");
    assert_eq!(
        t.herdr.did("send-keys"),
        vec![c(&["agent", "send-keys", "wA:p1", "1"])]
    );
}

#[test]
fn written_answer_to_a_question_that_changed_is_refused() {
    let mut t = T::new();
    t.prompted("Hello");
    t.said("wA:p1", &[("u1", "old answer")], Some(fruit()));
    t.set("wA:p1", "blocked");
    t.poll();
    let mut veg = fruit();
    veg[0]["question"] = json!("Pick a vegetable?");
    t.said("wA:p1", &[("u1", "old answer")], Some(veg));
    t.tell("Banana");
    assert!(t.herdr.did("send-keys").is_empty());
    assert!(t.last_msg().contains("Pick a vegetable?"));
}

#[test]
fn answer_to_a_choice_never_shown_shows_it_first() {
    let mut t = T::new();
    t.set("wA:p1", "blocked");
    t.said("wA:p1", &[], Some(fruit()));
    t.tell("2");
    assert!(t.herdr.did("send-keys").is_empty());
    assert!(t.last_msg().contains("Pick a fruit?"));
    t.tell("2");
    assert_eq!(
        t.herdr.did("send-keys"),
        vec![c(&["agent", "send-keys", "wA:p1", "2"])]
    );
}

#[test]
fn choice_after_a_prompt_shows_the_screen() {
    let mut t = T::new();
    t.prompted("Hello");
    t.set("wA:p1", "blocked");
    t.poll();
    assert!(t.last_msg().contains("screen of wA:p1"));
    t.tell("1");
    assert_eq!(
        t.herdr.did("send-keys"),
        vec![c(&["agent", "send-keys", "wA:p1", "1"])]
    );
    t.tell("Down");
    assert_eq!(
        t.herdr.did("send-keys").last().expect("a key"),
        &c(&["agent", "send-keys", "wA:p1", "down"])
    );
    t.tell("do something else");
    assert!(t.last_msg().contains("one key"));
    assert_eq!(t.herdr.did("prompt").len(), 1);
}

#[test]
fn question_is_listed_and_takes_a_number_or_text() {
    let mut t = T::new();
    t.prompted("Hello");
    t.said("wA:p1", &[("u1", "old answer")], Some(fruit()));
    t.set("wA:p1", "blocked");
    t.poll();
    assert_eq!(
        t.last_msg(),
        "**Pick a fruit?**\n1. Apple - red\n2. Pear - green\n\nAnswer with a number, or write your own answer."
    );
    t.tell("2");
    assert_eq!(t.herdr.last(), c(&["agent", "send-keys", "wA:p1", "2"]));
    t.tell("Banana");
    let calls = t.herdr.calls();
    assert_eq!(
        calls[calls.len() - 3..],
        [
            c(&["agent", "send-keys", "wA:p1", "3"]),
            c(&["pane", "send-text", "wA:p1", "Banana"]),
            c(&["agent", "send-keys", "wA:p1", "enter"]),
        ]
    );
}

#[test]
fn several_questions_point_to_the_screen() {
    let mut t = T::new();
    t.prompted("Hello");
    let mut two = fruit();
    two.as_array_mut().expect("list").push(fruit()[0].clone());
    t.said("wA:p1", &[], Some(two));
    t.set("wA:p1", "blocked");
    t.poll();
    assert!(t.last_msg().contains("use !screen and keys"));
    t.tell("Banana");
    assert!(t.last_msg().contains("one key"));
}

#[test]
fn answer_that_fails_halfway_is_cancelled() {
    let mut t = T::new();
    t.prompted("Hello");
    t.said("wA:p1", &[("u1", "old answer")], Some(fruit()));
    t.set("wA:p1", "blocked");
    t.poll();
    t.herdr
        .0
        .borrow_mut()
        .broken
        .insert(("pane".into(), "send-text".into()));
    t.tell("Banana");
    let calls = t.herdr.calls();
    assert_eq!(
        calls[calls.len() - 3..],
        [
            c(&["agent", "send-keys", "wA:p1", "3"]),
            c(&["pane", "send-text", "wA:p1", "Banana"]),
            c(&["agent", "send-keys", "wA:p1", "esc"]),
        ]
    );
    assert!(t.last_msg().contains("send-text failed"));
}

#[test]
fn choice_of_a_pane_that_is_gone_is_dropped() {
    let mut t = T::new();
    t.working_then("blocked", "wA:p1");
    t.herdr.remove("wA:p1");
    t.poll();
    assert!(t.bot.state().asked.is_empty());
}

#[test]
fn reaction_without_a_target_is_ignored() {
    let mut t = T::new();
    t.working_then("blocked", "wA:p1");
    t.bot
        .state_mut()
        .asked
        .get_mut("wA:p1")
        .expect("asked")
        .event = None;
    let mut batch = react(&t.room, "x", THUMB, crate::fakes::OWNER, 1_000_000);
    let room = t.room.clone();
    batch["rooms"]["join"][&room]["timeline"]["events"][0]["content"]["m.relates_to"]
        .as_object_mut()
        .expect("relation")
        .remove("event_id");
    t.queue(batch);
    t.sync();
    assert!(t.herdr.did("send-keys").is_empty());
}
