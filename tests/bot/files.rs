//! Pictures and files from the chat.

use crate::fakes::{OWNER, T, reply_to};
use herdr_matrix::bot::To;
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;

fn image(url: &str) -> serde_json::Value {
    json!({"msgtype": "m.image", "url": url})
}

#[test]
fn picture_is_handed_to_the_agent() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let files = t.files().to_string_lossy().into_owned();
    t.tell_with("shot.png", &image("mxc://x/y"));
    t.files_wait();
    let prompt = t.herdr.did("prompt").last().expect("a prompt")[3].clone();
    assert!(
        prompt.starts_with(&format!("See the file {files}/1000-")),
        "{prompt}"
    );
    let path = prompt.strip_prefix("See the file ").expect("a path");
    assert!(path.ends_with("-shot.png"));
    assert_eq!(fs::read(path).expect("read"), b"img mxc://x/y");
    t.tell_with(
        "what is this",
        &json!({"msgtype": "m.image", "url": "mxc://x/z", "filename": "../a b.png"}),
    );
    t.files_wait();
    let prompt = t.herdr.did("prompt").last().expect("a prompt")[3].clone();
    let (caption, path) = prompt.split_once("\n\n").expect("caption and path");
    assert_eq!(caption, "what is this");
    assert_eq!(
        std::path::Path::new(path)
            .parent()
            .expect("dir")
            .to_string_lossy(),
        files
    );
    assert!(path.ends_with("-a_b.png"));
}

#[test]
fn files_of_one_name_do_not_overwrite() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.tell_with("shot.png", &image("mxc://x/1"));
    t.files_wait();
    t.tell_with("shot.png", &image("mxc://x/2"));
    t.files_wait();
    let paths: Vec<String> = t
        .herdr
        .did("prompt")
        .into_iter()
        .map(|c| {
            c[3].strip_prefix("See the file ")
                .expect("a path")
                .to_owned()
        })
        .collect();
    assert_ne!(paths[0], paths[1]);
    for (path, mxc) in paths.iter().zip(["mxc://x/1", "mxc://x/2"]) {
        assert_eq!(
            fs::read(path).expect("read"),
            format!("img {mxc}").into_bytes()
        );
    }
}

#[test]
fn files_go_only_to_a_private_directory() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let files = t.files();
    fs::create_dir(&files).expect("mkdir");
    fs::set_permissions(&files, fs::Permissions::from_mode(0o755)).expect("chmod");
    t.tell_with("shot.png", &image("mxc://x/y"));
    fs::remove_dir(&files).expect("rmdir");
    let elsewhere = t.dir.path().join("elsewhere");
    fs::create_dir(&elsewhere).expect("mkdir");
    fs::set_permissions(&elsewhere, fs::Permissions::from_mode(0o700)).expect("chmod");
    std::os::unix::fs::symlink(&elsewhere, &files).expect("symlink");
    t.tell_with("shot.png", &image("mxc://x/y"));
    assert!(t.herdr.did("prompt").is_empty());
    assert_eq!(fs::read_dir(&elsewhere).expect("list").count(), 0);
    assert!(t.new_msgs().iter().all(|m| m.starts_with("Failed")));
    assert_eq!(t.new_msgs().len(), 2);
}

#[test]
fn old_files_are_removed() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.clock.set(3_000_000.0);
    let files = t.files();
    fs::create_dir(&files).expect("mkdir");
    fs::set_permissions(&files, fs::Permissions::from_mode(0o700)).expect("chmod");
    for (name, age) in [("old", 2_592_001.0), ("young", 2_592_000.0 - 400.0)] {
        let path = files.join(name);
        fs::write(&path, name).expect("write");
        let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs_f64(t.now() - age);
        fs::File::options()
            .write(true)
            .open(&path)
            .expect("open")
            .set_modified(at)
            .expect("utime");
    }
    t.tell_with("shot.png", &image("mxc://x/y"));
    let mut left: Vec<String> = fs::read_dir(&files)
        .expect("list")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left.len(), 2);
    assert!(!left.contains(&"old".to_owned()));
    assert!(left.contains(&"young".to_owned()));
}

// Files from the agents

/// A PNG header of 640 by 480.
fn png() -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    b.extend(640_u32.to_be_bytes());
    b.extend(480_u32.to_be_bytes());
    b.extend([8, 6, 0, 0, 0]);
    b
}

#[test]
fn agent_file_posts_in_the_main_chat() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.send_file("wA:p1", "shot.png", &png(), None);
    t.bot.local().expect("local");
    let sent = t.files_sent(Some(&t.room));
    assert_eq!(
        sent,
        [
            json!({"msgtype": "m.image", "body": "shot.png", "filename": "shot.png",
                "url": "mxc://example.org/pic",
                "info": {"mimetype": "image/png", "size": 29, "w": 640, "h": 480}})
        ]
    );
    let s = t.matrix.0.borrow();
    assert_eq!(
        (s.uploads.clone(), s.mimes.clone()),
        (vec![png()], vec!["image/png".to_owned()])
    );
    drop(s);
    let event = t.matrix.last_id();
    assert_eq!(
        t.bot.state().sent.get(&event).map(String::as_str),
        Some("wA:p1")
    );
    assert_eq!(t.log_lines(), ["wA:p1 upload 29"]);
    assert_eq!(fs::read_dir(t.uploads()).expect("list").count(), 0);
    assert!(t.matrix.has("GET", "/media/config", None, &[]));
}

#[test]
fn agent_file_posts_in_its_own_chat_with_its_caption() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let chat = t.spawn("wA:p1", "repo-wA");
    t.send_file("wA:p1", "notes.txt", b"hello", Some("The notes"));
    t.bot.local().expect("local");
    assert!(t.files_sent(Some(&t.room)).is_empty());
    assert_eq!(
        t.files_sent(Some(&chat.room)),
        [
            json!({"msgtype": "m.file", "body": "The notes", "filename": "notes.txt",
                "url": "mxc://example.org/pic",
                "info": {"mimetype": "text/plain", "size": 5}})
        ]
    );
    let by_contact = t.matrix.as_user(&chat.user);
    assert!(
        by_contact
            .iter()
            .any(|c| c.2.as_ref().is_some_and(|b| b.get("url").is_some()))
    );
}

#[test]
fn file_over_the_homeservers_limit_is_said() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.matrix.0.borrow_mut().limit = Some(1 << 20);
    t.send_file("wA:p1", "big.bin", &vec![0; (1 << 20) + 1], None);
    t.bot.local().expect("local");
    let said = "it is 1.1 MB, and the homeserver takes 1.0 MB at most";
    assert_eq!(t.new_msgs(), [format!("Could not send big.bin: {said}")]);
    assert_eq!(
        t.matrix
            .sent_key("msgtype", None)
            .last()
            .map(String::as_str),
        Some("m.notice")
    );
    assert_eq!(t.log_lines(), [format!("error: wA:p1 upload: {said}")]);
    assert!(t.matrix.0.borrow().uploads.is_empty());
    t.matrix.0.borrow_mut().limit = Some((1 << 20) + 1);
    t.bot = t.make();
    t.clear_log();
    t.send_file("wA:p1", "big.bin", &vec![0; (1 << 20) + 1], None);
    t.bot.local().expect("local");
    assert_eq!(t.log_lines(), ["wA:p1 upload 1048577"]);
}

#[test]
fn upload_limit_is_asked_once() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let asked = |t: &T| {
        t.matrix
            .calls()
            .iter()
            .filter(|c| c.1 == "/media/config")
            .count()
    };
    t.matrix.0.borrow_mut().refuse.push("/media/config".into());
    t.send_file("wA:p1", "a.txt", b"a", None);
    t.bot.local().expect("local");
    t.matrix.0.borrow_mut().refuse.clear();
    t.advance(60.0);
    t.send_file("wA:p1", "b.txt", b"b", None);
    t.bot.local().expect("local");
    t.send_file("wA:p1", "c.txt", b"c", None);
    t.bot.local().expect("local");
    assert_eq!(t.files_sent(None).len(), 3);
    assert_eq!(asked(&t), 2);
}

#[test]
fn failed_upload_is_tried_again_until_it_is_old() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.matrix.0.borrow_mut().refuse.push("/upload".into());
    t.send_file("wA:p1", "shot.png", &png(), None);
    t.bot.local().expect("local");
    let said = "cannot reach the homeserver: /upload failed";
    assert!(t.new_msgs().is_empty(), "{:?}", t.new_msgs());
    assert_eq!(t.log_lines(), [format!("error: wA:p1 upload: {said}")]);
    assert!(t.files_sent(None).is_empty());
    assert_eq!(fs::read_dir(t.uploads()).expect("list").count(), 2);
    t.advance(601.0);
    t.bot.local().expect("local");
    assert!(t.files_sent(None).is_empty());
    assert_eq!(fs::read_dir(t.uploads()).expect("list").count(), 0);
}

#[test]
fn failed_upload_that_gets_old_is_said_once() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.matrix.0.borrow_mut().refuse.push("/upload".into());
    t.send_file("wA:p1", "shot.png", &png(), None);
    t.bot.local().expect("local");
    t.advance(300.0);
    t.bot.local().expect("local");
    assert!(t.new_msgs().is_empty(), "{:?}", t.new_msgs());
    t.advance(301.0);
    t.bot.local().expect("local");
    t.bot.local().expect("local");
    assert_eq!(
        t.new_msgs(),
        ["Could not send shot.png: cannot reach the homeserver: /upload failed"]
    );
}

#[test]
fn failed_upload_from_the_future_is_dropped_after_three_tries() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.matrix.0.borrow_mut().refuse.push("/upload".into());
    let source = t.dir.path().join("a.txt");
    fs::write(&source, "hi").expect("write");
    let upload = herdr_matrix::inbox::Upload {
        pane: "wA:p1".into(),
        name: "a.txt".into(),
        caption: None,
        at: t.now() + 1e9,
    };
    let mut file = fs::File::open(&source).expect("open");
    herdr_matrix::inbox::queue_upload(&t.uploads(), &upload, &mut file).expect("queued");
    for _ in 0..30 {
        t.bot.local().expect("local");
        t.advance(10.0);
    }
    assert_eq!(t.matrix.0.borrow().uploads.len(), 3);
    assert_eq!(
        t.new_msgs(),
        ["Could not send a.txt: cannot reach the homeserver: /upload failed"]
    );
    assert_eq!(fs::read_dir(t.uploads()).expect("list").count(), 0);
}

#[test]
fn upload_survives_a_transport_failure() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.send_file("wA:p1", "a.txt", b"hi", None);
    t.matrix.0.borrow_mut().refuse.push("/upload".into());
    t.bot.local().expect("local");
    t.matrix.0.borrow_mut().refuse.clear();
    t.bot.local().expect("local");
    assert_eq!(t.files_sent(None).len(), 0);
    t.advance(60.0);
    t.bot.local().expect("local");
    assert_eq!(t.files_sent(None).len(), 1);
    t.bot.local().expect("local");
    assert_eq!(t.files_sent(None).len(), 1);
}

#[test]
fn stale_or_unknown_agent_file_is_dropped() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.send_file("wB:p1", "a.txt", b"a", None);
    t.advance(1.0);
    t.send_file("wA:p1", "b.txt", b"b", None);
    t.advance(601.0);
    t.send_file("wB:p1", "c.txt", b"c", None);
    t.bot.local().expect("local");
    assert_eq!(
        t.log_lines(),
        [
            "upload for \"wB:p1\" dropped: older than 10 minutes",
            "upload for \"wA:p1\" dropped: older than 10 minutes",
            "upload for \"wB:p1\" dropped: no agent works there",
        ]
    );
    assert!(t.files_sent(None).is_empty() && t.new_msgs().is_empty());
    assert_eq!(fs::read_dir(t.uploads()).expect("list").count(), 0);
}

// Files of the owner, held for more

fn file(url: &str) -> serde_json::Value {
    json!({"msgtype": "m.file", "url": url})
}

#[test]
fn files_that_come_together_are_one_prompt() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let files = t.files().display().to_string();
    t.tell_with("a.png", &image("mxc://x/1"));
    t.advance(3.0);
    t.tell_with(
        "What do these show?",
        &json!({"msgtype": "m.image", "url": "mxc://x/2", "filename": "b.png"}),
    );
    t.advance(3.0);
    t.tell_with("c.pdf", &file("mxc://x/3"));
    t.advance(4.9);
    t.bot.hand_over_files();
    assert!(t.herdr.did("prompt").is_empty());
    assert!(!t.matrix.calls().iter().any(|c| c.1.contains("/receipt/")));
    t.advance(0.2);
    t.bot.hand_over_files();
    let prompts = t.herdr.did("prompt");
    assert_eq!(prompts.len(), 1);
    let (caption, paths) = prompts[0][3].split_once("\n\n").expect("caption and paths");
    assert_eq!(caption, "What do these show?");
    let paths: Vec<&str> = paths.lines().collect();
    assert_eq!(paths.len(), 3);
    for (path, (name, mxc)) in paths.iter().zip([
        ("a.png", "mxc://x/1"),
        ("b.png", "mxc://x/2"),
        ("c.pdf", "mxc://x/3"),
    ]) {
        assert!(
            path.starts_with(&format!("{files}/")) && path.ends_with(name),
            "{path}"
        );
        assert_eq!(
            fs::read(path).expect("read"),
            format!("img {mxc}").into_bytes()
        );
    }
    assert_eq!(
        t.log_lines().last().map(String::as_str),
        Some(format!("wA:p1 file {}", prompts[0][3].chars().count()).as_str())
    );
    t.bot.hand_over_files();
    assert_eq!(t.herdr.did("prompt").len(), 1);
}

#[test]
fn files_without_captions_are_listed() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.tell_with("a.png", &image("mxc://x/1"));
    t.tell_with("b.png", &image("mxc://x/2"));
    t.advance(5.0);
    t.bot.tick().expect("tick");
    let prompt = t.herdr.did("prompt").last().expect("a prompt")[3].clone();
    let (head, paths) = prompt.split_once("\n\n").expect("head and paths");
    assert_eq!(head, "See the files");
    assert_eq!(paths.lines().count(), 2);
}

#[test]
fn text_hands_the_held_files_over_first() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.tell_with("a.png", &image("mxc://x/1"));
    t.tell("And fix it");
    let prompts: Vec<String> = t
        .herdr
        .did("prompt")
        .into_iter()
        .map(|c| c[3].clone())
        .collect();
    assert_eq!(prompts.len(), 2);
    assert!(prompts[0].starts_with("See the file "), "{}", prompts[0]);
    assert_eq!(prompts[1], "And fix it");
    *t.transcript.borrow_mut() = "Look at this".into();
    t.tell_with("b.png", &image("mxc://x/2"));
    t.speak("Look at this");
    let prompts = t.herdr.did("prompt");
    assert_eq!(prompts.len(), 4);
    assert!(prompts[2][3].starts_with("See the file "));
    assert_eq!(prompts[3][3], "Look at this");
}

#[test]
fn files_for_two_agents_are_held_apart() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.set("wB:p1", "idle");
    t.tell("!attach repo-wA");
    let b_said = t
        .bot
        .say("from B", To::default().pane("wB:p1"))
        .expect("say")
        .expect("an id");
    let chat = t.spawn("wB:p1", "repo-wB");
    t.tell_with("a.png", &image("mxc://x/1"));
    let mut reply = reply_to(&b_said);
    reply["msgtype"] = json!("m.image");
    reply["url"] = json!("mxc://x/2");
    t.tell_with("b.png", &reply);
    t.tell_in_full(&chat, "c.png", None, None, OWNER, &image("mxc://x/3"));
    t.tell_in(&chat, "Hi");
    let prompts = t.herdr.did("prompt");
    assert_eq!(prompts.len(), 2);
    assert_eq!(
        (prompts[0][2].as_str(), prompts[1][2].as_str()),
        ("wB:p1", "wB:p1")
    );
    assert!(prompts[0][3].starts_with("See the file ") && prompts[0][3].ends_with("c.png"));
    assert_eq!(prompts[1][3], "Hi");
    t.advance(5.0);
    t.bot.hand_over_files();
    let prompts = t.herdr.did("prompt");
    assert_eq!(prompts.len(), 4);
    let mut rest: Vec<(&str, &str)> = prompts[2..]
        .iter()
        .map(|c| (c[2].as_str(), c[3].rsplit('-').next().unwrap_or_default()))
        .collect();
    rest.sort_unstable();
    assert_eq!(rest, [("wA:p1", "a.png"), ("wB:p1", "b.png")]);
}

#[test]
fn agent_gone_before_its_files_go_is_said() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.tell_with("a.png", &image("mxc://x/1"));
    t.herdr.remove("wA:p1");
    t.set("wB:p1", "idle");
    t.advance(5.0);
    t.bot.hand_over_files();
    assert!(t.herdr.did("prompt").is_empty());
    assert_eq!(t.last_msg(), "repo-wA is not running; nothing was sent.");
}

#[test]
fn clock_step_back_does_not_hold_the_files() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.tell_with("shot.png", &image("mxc://x/y"));
    t.advance(-600.0);
    t.bot.hand_over_files();
    assert_eq!(t.herdr.did("prompt").len(), 1);
}

#[test]
fn old_files_are_removed_hourly() {
    let mut t = T::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("since")
        .as_secs_f64();
    let ttl = herdr_matrix::files::FILE_TTL.as_secs_f64();
    let files = t.files();
    let sandbox = files.join("agents/nora");
    fs::create_dir_all(&sandbox).expect("mkdir");
    for dir in [&files, &sandbox] {
        fs::write(dir.join("old"), "old").expect("write");
    }
    t.bot.deps_mut().agents = vec![herdr_matrix::bot::AgentSettings {
        cwd: "/w/a".into(),
        sandbox: Some(herdr_matrix::bot::Sandbox {
            id: "nora".into(),
            state: t.dir.path().join("state/nora/claude"),
            uploads: t.uploads().join("nora"),
            files: sandbox.clone(),
        }),
        ..herdr_matrix::bot::AgentSettings::default()
    }];
    t.clock.set(now + ttl - 60.0);
    t.bot.tick().expect("tick");
    assert!(files.join("old").exists() && sandbox.join("old").exists());
    t.advance(120.0);
    t.bot.tick().expect("tick");
    assert!(files.join("old").exists(), "purged before the hour");
    t.advance(3600.0);
    t.bot.tick().expect("tick");
    assert!(!files.join("old").exists() && !sandbox.join("old").exists());
}

#[test]
fn audio_sent_as_a_file_reaches_the_agent_as_a_file() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.tell_with(
        "song.mp3",
        &json!({"msgtype": "m.audio", "url": "mxc://x/s"}),
    );
    t.files_wait();
    let prompt = t.herdr.did("prompt").last().expect("a prompt")[3].clone();
    assert!(prompt.starts_with("See the file "), "{prompt}");
    assert!(prompt.ends_with("-song.mp3"), "{prompt}");
    assert!(t.heard.borrow().is_empty());
}

#[test]
fn file_with_no_agent_attached_is_handed_over_once_one_is() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.set("wB:p1", "idle");
    t.tell_with("shot.png", &image("mxc://x/y"));
    t.files_wait();
    assert!(t.herdr.did("prompt").is_empty());
    assert!(
        t.last_msg()
            .starts_with("Nothing was sent: no agent is attached.")
    );
    t.tell("!attach repo-wB");
    let prompt = t.herdr.did("prompt").last().expect("a prompt").clone();
    assert_eq!(prompt[2], "wB:p1");
    assert!(prompt[3].ends_with("-shot.png"), "{}", prompt[3]);
    assert_eq!(t.last_msg(), "Sent your message to repo-wB.");
}
