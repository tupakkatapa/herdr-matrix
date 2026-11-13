//! What survives a restart, and state files of other versions.

use crate::fakes::{OWNER, PERMISSION, T, c, msg, reply_to};
use serde_json::{Value, json};
use std::fs;

#[test]
fn first_sync_skips_history() {
    let t = T::new();
    fs::remove_file(t.state_path()).expect("rm");
    let mut fresh = t.make();
    fresh.state_mut().control = Some(t.room.clone());
    fresh.state_mut().owner = Some(OWNER.into());
    t.set("wA:p1", "idle");
    t.queue(msg(&t.room, "old", OWNER, 1_000_000, &json!({})));
    fresh.sync().expect("sync");
    assert_eq!(fresh.state().since.as_deref(), Some("n2"));
    assert!(t.herdr.did("prompt").is_empty());
}

#[test]
fn idle_sync_does_not_write_state() {
    let mut t = T::new();
    t.queue(json!({"next_batch": "later"}));
    t.sync();
    assert_eq!(t.bot.state().since.as_deref(), Some("later"));
    let saved: Value =
        serde_json::from_slice(&fs::read(t.state_path()).expect("read")).expect("JSON");
    assert_eq!(saved["since"], "n");
}

#[test]
fn contact_survives_a_restart() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let chat = t.spawn("wA:p1", "repo-wA");
    t.bot = t.make();
    t.tell_in(&chat, "hi");
    assert_eq!(
        t.herdr.did("prompt"),
        vec![c(&["agent", "prompt", "wA:p1", "hi"])]
    );
}

#[test]
fn answer_in_flight_survives_a_restart() {
    let mut t = T::new();
    t.prompted("Hello");
    t.set("wA:p1", "working");
    t.poll();
    t.bot = t.make();
    t.said("wA:p1", &[("u1", "old answer"), ("u2", "Answer.")], None);
    t.set("wA:p1", "idle");
    t.poll();
    t.poll();
    assert_eq!(t.new_msgs(), ["Answer."]);
}

#[test]
fn reply_routing_survives_a_restart() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.set("wB:p1", "idle");
    t.tell("!attach repo-wA");
    t.working_then("idle", "wB:p1");
    let notice = t.matrix.last_id();
    t.bot = t.make();
    t.tell_with("ok", &reply_to(&notice));
    assert_eq!(
        t.herdr.did("prompt"),
        vec![c(&["agent", "prompt", "wB:p1", "ok"])]
    );
}

#[test]
fn reaction_survives_a_restart() {
    let mut t = T::new();
    t.working_then("blocked", "wA:p1");
    let choice = t.matrix.last_id();
    t.bot = t.make();
    t.react(&choice, "\u{1F44D}");
    assert_eq!(
        t.herdr.did("send-keys"),
        vec![c(&["agent", "send-keys", "wA:p1", "1"])]
    );
}

#[test]
fn state_file_of_an_older_version_loads() {
    let mut t = T::new();
    fs::write(
        t.state_path(),
        json!({"since": "s", "override": "auto", "control": t.room, "owner": OWNER,
               "attached": null, "profile": null})
        .to_string(),
    )
    .expect("write");
    t.bot = t.make();
    assert!(t.bot.state().pending.is_empty() && t.bot.state().sent.is_empty());
    assert!(t.bot.state().spawned.is_empty());
    t.set("wA:p1", "idle");
    t.tell("hi");
    assert_eq!(t.herdr.did("prompt").len(), 1);
}

#[test]
fn choice_announced_by_an_older_version_is_shown_again() {
    let mut t = T::new();
    t.herdr
        .0
        .borrow_mut()
        .screens
        .insert("wA:p1".into(), PERMISSION.into());
    t.working_then("blocked", "wA:p1");
    let mut state: Value =
        serde_json::from_slice(&fs::read(t.state_path()).expect("read")).expect("JSON");
    state["asked"] = json!({"wA:p1": "$old"});
    fs::write(t.state_path(), state.to_string()).expect("write");
    t.bot = t.make();
    t.react("$old", "\u{1F44D}");
    assert!(t.herdr.did("send-keys").is_empty());
    assert!(t.last_msg().contains("rm -r build"));
}

#[test]
fn broken_state_file_is_moved_aside() {
    let mut t = T::new();
    for broken in ["", r#"{"since": "s", "pend"#, "[]"] {
        fs::write(t.state_path(), broken).expect("write");
        t.bot = t.make();
        assert_eq!(t.bot.state().control, None);
        let mut aside = t.state_path().into_os_string();
        aside.push(".broken");
        assert_eq!(fs::read_to_string(&aside).expect("read"), broken);
        assert!(!t.state_path().exists());
    }
}

/// Every key a state file of the running bot holds, older ones too.
#[test]
fn state_file_with_every_key_round_trips() {
    let t = T::new();
    let file = json!({
        "since": "s1", "override": "auto", "control": "!c:example.org", "owner": OWNER,
        "attached": "wA:p1", "profile": ["Claude", "c81e"],
        "pending": {"wA:p1": {"seen": ["u1", null], "worked": true, "said": false, "slash": false,
                              "started": false, "polls": 2, "held": ["text", 1000.5], "at": 1000.25,
                              "voice": "en", "spawned": false, "last": "said"}},
        "sent": {"$e1": "wA:p1", "$e0": "wB:p1"},
        "asked": {"wA:p1": ["$e1", "ab12"], "wB:p1": [null, null]},
        "talk": true,
        "spawned": {"wG:p1": {"user": "@herdr_a:example.org", "room": "!u:example.org", "since": "46240"}},
        "avatars": {"c81e": "mxc://example.org/pic"},
        "rooms": {}, "target": {"!m:example.org": "wF:p1"}
    });
    fs::write(t.state_path(), file.to_string()).expect("write");
    let bot = t.make();
    assert_eq!(bot.state().control.as_deref(), Some("!c:example.org"));
    bot.save().expect("save");
    let after: Value =
        serde_json::from_slice(&fs::read(t.state_path()).expect("read")).expect("JSON");
    assert_eq!(after, file);
}

#[test]
fn unreadable_state_stops_the_start() {
    let t = T::new();
    fs::remove_file(t.state_path()).expect("rm");
    fs::create_dir(t.state_path()).expect("mkdir");
    let store = herdr_matrix::state::Store::new(t.state_path());
    assert!(store.load(&|_| {}).is_err());
    assert!(t.state_path().is_dir());
}

#[test]
fn state_file_is_private() {
    use std::os::unix::fs::PermissionsExt as _;
    let mut t = T::new();
    t.tell("!afk");
    let mode = fs::metadata(t.state_path())
        .expect("stat")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn mistyped_key_moves_the_state_aside() {
    let mut t = T::new();
    let file = json!({"since": "s", "control": "!c:example.org", "spawned": "yes"}).to_string();
    fs::write(t.state_path(), &file).expect("write");
    t.bot = t.make();
    assert_eq!(t.bot.state().control, None);
    let mut aside = t.state_path().into_os_string();
    aside.push(".broken");
    assert_eq!(fs::read_to_string(&aside).expect("read"), file);
}

#[test]
fn older_entries_load() {
    let t = T::new();
    fs::write(
        t.state_path(),
        json!({"asked": {"wA:p1": null, "wB:p1": "$e"}, "talk": true,
               "pending": {"wA:p1": {"at": 1.0, "last": null, "voice": "en"}}})
        .to_string(),
    )
    .expect("write");
    let bot = t.make();
    let asked = &bot.state().asked;
    assert_eq!(
        (
            asked["wA:p1"].event.as_deref(),
            asked["wA:p1"].shown.as_deref()
        ),
        (None, None)
    );
    assert_eq!(asked["wB:p1"].event.as_deref(), Some("$e"));
    bot.save().expect("save");
    let after: Value =
        serde_json::from_slice(&fs::read(t.state_path()).expect("read")).expect("JSON");
    assert_eq!(after["talk"], true);
    assert_eq!(after["pending"]["wA:p1"]["voice"], "en");
    assert!(after["pending"]["wA:p1"].get("last").is_none());
}
