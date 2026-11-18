//! Confined agents: what they write is read as untrusted, and their files
//! go where only they read them.

use crate::fakes::{T, c};
use herdr_matrix::bot::{AgentSettings, Sandbox};
use herdr_matrix::inbox::{Upload, queue_upload};
use serde_json::json;
use std::fs;
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _, symlink};
use std::path::Path;
use std::time::{Duration, SystemTime};

/// The confined agent `nora` works in `/w/a`, as `wA:p1` does.
fn confine(t: &mut T) -> Sandbox {
    t.herdr
        .0
        .borrow_mut()
        .cwd
        .insert("wA:p1".into(), "/w/a".into());
    let sandbox = Sandbox {
        id: "nora".into(),
        state: t.dir.path().join("state/nora/claude"),
        uploads: t.uploads().join("nora"),
        files: t.files().join("agents/nora"),
    };
    for dir in [&sandbox.state, &sandbox.uploads] {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .expect("mkdir");
    }
    t.bot.deps_mut().agents = vec![AgentSettings {
        cwd: "/w/a/".into(),
        sandbox: Some(sandbox.clone()),
        ..AgentSettings::default()
    }];
    sandbox
}

/// A transcript in which the agent said `text` after a prompt.
fn transcript(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("a directory")).expect("mkdir");
    let lines = [
        json!({"type": "user", "message": {"content": "Hello"}}),
        json!({"type": "assistant", "uuid": text,
               "message": {"content": [{"type": "text", "text": text}]}}),
    ];
    fs::write(path, lines.map(|l| l.to_string() + "\n").concat()).expect("write");
}

fn age(path: &Path, seconds: u64) {
    fs::File::options()
        .write(true)
        .open(path)
        .expect("open")
        .set_modified(SystemTime::now() - Duration::from_secs(seconds))
        .expect("utime");
}

#[test]
fn turn_comes_from_the_newest_transcript_of_the_agent() {
    let mut t = T::new();
    let nora = confine(&mut t);
    t.said("wA:p1", &[("u1", "from the session")], None);
    let projects = nora.state.join("projects");
    transcript(&projects.join("-w-a/old.jsonl"), "Old.");
    age(&projects.join("-w-a/old.jsonl"), 60);
    transcript(&projects.join("-w-b/new.jsonl"), "New.");
    t.working_then("idle", "wA:p1");
    assert_eq!(t.new_msgs(), ["New."]);
}

/// Mount points of `sandbox.files` stay in the state directory as empty
/// files, newer than the transcript.
#[test]
fn mount_points_are_not_transcripts() {
    let mut t = T::new();
    let nora = confine(&mut t);
    let mine = nora.state.join("projects/-w-a/mine.jsonl");
    transcript(&mine, "Mine.");
    age(&mine, 60);
    for file in [
        "settings.json",
        ".claude.json",
        "models.jsonl",
        "projects/settings.jsonl",
    ] {
        fs::write(nora.state.join(file), "").expect("write");
    }
    t.working_then("idle", "wA:p1");
    assert_eq!(t.new_msgs(), ["Mine."]);
}

#[test]
fn transcript_behind_a_symlink_is_not_read() {
    let mut t = T::new();
    let nora = confine(&mut t);
    let projects = nora.state.join("projects");
    transcript(&projects.join("-w-a/mine.jsonl"), "Mine.");
    age(&projects.join("-w-a/mine.jsonl"), 60);
    let outside = t.dir.path().join("owner/projects/-home");
    transcript(&outside.join("secret.jsonl"), "Secret.");
    symlink(
        outside.join("secret.jsonl"),
        projects.join("-w-a/link.jsonl"),
    )
    .expect("symlink");
    symlink(&outside, projects.join("-link")).expect("symlink");
    t.working_then("idle", "wA:p1");
    assert_eq!(t.new_msgs(), ["Mine."]);
}

#[test]
fn file_of_a_confined_agent_posts_as_the_agent_of_its_queue() {
    let mut t = T::new();
    let nora = confine(&mut t);
    t.set("wA:p1", "idle");
    t.set("wB:p1", "idle");
    let source = t.dir.path().join("notes.txt");
    fs::write(&source, "hello").expect("write");
    let upload = |pane: &str| Upload {
        pane: pane.into(),
        name: "notes.txt".into(),
        caption: None,
        at: t.now(),
    };
    for pane in ["", "wB:p1"] {
        let mut file = fs::File::open(&source).expect("open");
        queue_upload(&nora.uploads, &upload(pane), &mut file).expect("queued");
    }
    t.bot.local().expect("local");
    assert_eq!(t.log_lines(), ["wA:p1 upload 5", "wA:p1 upload 5"]);
    let sent = t.files_sent(Some(&t.room));
    assert_eq!(sent.len(), 2);
    assert!(
        t.bot
            .state()
            .sent
            .values()
            .all(|pane| pane.as_str() == "wA:p1")
    );
    assert_eq!(fs::read_dir(&nora.uploads).expect("list").count(), 0);
}

#[test]
fn queue_of_a_confined_agent_is_taken_five_at_a_time() {
    let mut t = T::new();
    let nora = confine(&mut t);
    t.set("wA:p1", "idle");
    let source = t.dir.path().join("notes.txt");
    fs::write(&source, "hello").expect("write");
    for i in 0..12 {
        let upload = Upload {
            pane: String::new(),
            name: format!("{i:02}.txt"),
            caption: None,
            at: t.now() + f64::from(i) / 100.0,
        };
        let mut file = fs::File::open(&source).expect("open");
        queue_upload(&nora.uploads, &upload, &mut file).expect("queued");
    }
    let mut counts = Vec::new();
    for _ in 0..3 {
        t.bot.local().expect("local");
        counts.push(t.files_sent(None).len());
    }
    assert_eq!(counts, [5, 10, 12]);
    let names: Vec<String> = t
        .files_sent(None)
        .iter()
        .map(|e| e["filename"].as_str().expect("a name").to_owned())
        .collect();
    let want: Vec<String> = (0..12).map(|i| format!("{i:02}.txt")).collect();
    assert_eq!(names, want);
}

#[test]
fn long_name_of_a_confined_agents_file_is_cut() {
    let mut t = T::new();
    let nora = confine(&mut t);
    t.set("wA:p1", "idle");
    t.matrix.0.borrow_mut().limit = Some(1);
    let source = t.dir.path().join("notes.txt");
    fs::write(&source, "hello").expect("write");
    let upload = Upload {
        pane: String::new(),
        name: "é".repeat(500_000),
        caption: None,
        at: t.now(),
    };
    let mut file = fs::File::open(&source).expect("open");
    queue_upload(&nora.uploads, &upload, &mut file).expect("queued");
    t.bot.local().expect("local");
    let said = t.new_msgs();
    assert_eq!(said.len(), 1, "{} notices", said.len());
    assert!(
        said[0].starts_with("Could not send éé"),
        "{}",
        &said[0][..40]
    );
    assert!(said[0].len() < 512, "{} bytes", said[0].len());
}

#[test]
fn file_of_a_confined_agent_needs_one_agent_in_its_folder() {
    let mut t = T::new();
    let nora = confine(&mut t);
    t.set("wA:p1", "idle");
    t.set("wC:p1", "idle");
    t.herdr
        .0
        .borrow_mut()
        .cwd
        .insert("wC:p1".into(), "/w/a".into());
    let source = t.dir.path().join("notes.txt");
    fs::write(&source, "hello").expect("write");
    let mut file = fs::File::open(&source).expect("open");
    let upload = Upload {
        pane: "wA:p1".into(),
        name: "notes.txt".into(),
        caption: None,
        at: t.now(),
    };
    queue_upload(&nora.uploads, &upload, &mut file).expect("queued");
    t.bot.local().expect("local");
    assert_eq!(
        t.log_lines(),
        ["upload for \"/w/a/\" dropped: it matches 2 agents"]
    );
    assert!(t.files_sent(None).is_empty());
}

#[test]
fn symlinks_in_the_queue_of_a_confined_agent_are_not_followed() {
    let mut t = T::new();
    let nora = confine(&mut t);
    t.set("wA:p1", "idle");
    let secret = t.dir.path().join("secret");
    fs::write(&secret, "secret").expect("write");
    let entry = json!({"pane": "wA:p1", "name": "a.txt", "at": t.now()}).to_string();
    let outside = t.dir.path().join("entry.json");
    fs::write(&outside, &entry).expect("write");
    symlink(&outside, nora.uploads.join("0000000000001-a.json")).expect("symlink");
    fs::write(nora.uploads.join("0000000000001-a.data"), "data").expect("write");
    fs::write(nora.uploads.join("0000000000002-b.json"), &entry).expect("write");
    symlink(&secret, nora.uploads.join("0000000000002-b.data")).expect("symlink");
    t.bot.local().expect("local");
    assert!(t.matrix.0.borrow().uploads.is_empty());
    assert!(t.files_sent(None).is_empty());
    assert_eq!(fs::read(&outside).expect("kept"), entry.as_bytes());
    assert_eq!(fs::read(&secret).expect("kept"), b"secret");
    assert!(
        t.log_lines()
            .iter()
            .any(|l| l.starts_with("error: wA:p1 upload: ") && l.contains("not a regular file")),
        "{:?}",
        t.log_lines()
    );
}

#[test]
fn queue_of_a_confined_agent_that_is_not_private_holds_nothing() {
    let mut t = T::new();
    let nora = confine(&mut t);
    t.set("wA:p1", "idle");
    t.set("wB:p1", "idle");
    fs::set_permissions(&nora.uploads, fs::Permissions::from_mode(0o755)).expect("chmod");
    t.send_file("wB:p1", "notes.txt", b"hello", None);
    t.bot.local().expect("local");
    assert_eq!(t.files_sent(None).len(), 1);
    assert!(
        t.log_lines()
            .iter()
            .any(|l| l.starts_with("error: uploads of nora: ")),
        "{:?}",
        t.log_lines()
    );
}

#[test]
fn file_for_a_confined_agent_is_kept_in_its_own_directory() {
    let mut t = T::new();
    let nora = confine(&mut t);
    t.set("wA:p1", "idle");
    t.tell_with(
        "shot.png",
        &json!({"msgtype": "m.image", "url": "mxc://x/y"}),
    );
    t.files_wait();
    let prompt = t.herdr.did("prompt").last().expect("a prompt")[3].clone();
    let path = prompt.strip_prefix("See the file ").expect("a path");
    assert_eq!(Path::new(path).parent(), Some(nora.files.as_path()));
    assert_eq!(fs::read(path).expect("read"), b"img mxc://x/y");
    assert_eq!(
        fs::metadata(&nora.files)
            .expect("stat")
            .permissions()
            .mode()
            & 0o7777,
        0o700
    );
}

#[test]
fn new_starts_a_confined_agent_through_its_command() {
    let mut t = T::new();
    let beta = t.dir.path().join("ws/beta");
    fs::create_dir_all(&beta).expect("mkdir");
    let beta = beta.to_string_lossy().into_owned();
    t.bot.deps_mut().agents = vec![AgentSettings {
        cwd: beta.clone(),
        sandbox: Some(Sandbox {
            id: "nora".into(),
            state: "/c".into(),
            uploads: "/u".into(),
            files: "/f".into(),
        }),
        ..AgentSettings::default()
    }];
    t.tell("!new beta");
    assert_eq!(
        t.herdr.last(),
        c(&["pane", "run", "wN:p1", "exec herdr-matrix-agent-nora"])
    );
    assert!(
        t.last_msg()
            .contains(&format!("Started Claude Code as beta in {beta};"))
    );
}

#[test]
fn autostart_starts_a_confined_agent_through_its_command() {
    let mut t = T::new();
    let beta = t.dir.path().join("ws/beta");
    fs::create_dir_all(&beta).expect("mkdir");
    confine_beta(&mut t, &beta.to_string_lossy());
    t.bot.deps_mut().agents[0].autostart = true;
    for _ in 0..3 {
        t.poll();
    }
    assert_eq!(workspaces_made(&t), 1);
    assert!(t.herdr.calls().contains(&c(&[
        "pane",
        "run",
        "wN:p1",
        "exec herdr-matrix-agent-nora"
    ])));
}

/// The confined agent `nora` works in `ws/beta`, written as `cwd`.
fn confine_beta(t: &mut T, cwd: &str) {
    t.bot.deps_mut().agents = vec![AgentSettings {
        cwd: cwd.into(),
        sandbox: Some(Sandbox {
            id: "nora".into(),
            state: "/c".into(),
            uploads: "/u".into(),
            files: "/f".into(),
        }),
        ..AgentSettings::default()
    }];
}

fn workspaces_made(t: &T) -> usize {
    t.herdr
        .calls()
        .iter()
        .filter(|c| c[0] == "workspace" && c[1] == "create")
        .count()
}

#[test]
fn new_refuses_a_folder_inside_a_confined_agents_folder() {
    let mut t = T::new();
    let ws = t.dir.path().join("ws");
    fs::create_dir_all(ws.join("beta/api")).expect("mkdir");
    fs::create_dir_all(t.dir.path().join("home")).expect("mkdir");
    symlink(t.dir.path().join("home"), ws.join("beta/home")).expect("symlink");
    confine_beta(&mut t, &ws.join("beta").to_string_lossy());
    for folder in ["api", "home"] {
        t.tell(&format!("!new {folder}"));
        let inside = ws.join("beta").join(folder);
        assert_eq!(
            t.last_msg(),
            format!(
                "{} is inside the folder of the confined agent nora; start it with !new beta.",
                inside.display()
            )
        );
    }
    assert_eq!(workspaces_made(&t), 0);
}

#[test]
fn new_refuses_a_symlink_into_a_confined_agents_folder() {
    let mut t = T::new();
    let ws = t.dir.path().join("ws");
    fs::create_dir_all(ws.join("beta/api")).expect("mkdir");
    symlink(ws.join("beta/api"), ws.join("api-link")).expect("symlink");
    confine_beta(&mut t, &ws.join("beta").to_string_lossy());
    t.tell("!new api-link");
    assert!(
        t.last_msg()
            .contains("is inside the folder of the confined agent nora"),
        "{}",
        t.last_msg()
    );
    assert_eq!(workspaces_made(&t), 0);
}

#[test]
fn new_starts_a_confined_agent_through_a_symlink_onto_its_folder() {
    let mut t = T::new();
    let ws = t.dir.path().join("ws");
    fs::create_dir_all(ws.join("beta")).expect("mkdir");
    symlink(ws.join("beta"), ws.join("beta-link")).expect("symlink");
    confine_beta(&mut t, &format!("{}/", ws.join("beta").display()));
    t.tell("!new beta-link");
    assert_eq!(
        t.herdr.last(),
        c(&["pane", "run", "wN:p1", "exec herdr-matrix-agent-nora"])
    );
    t.tell("!new beta");
    assert_eq!(
        t.herdr.last(),
        c(&["pane", "run", "wN:p1", "exec herdr-matrix-agent-nora"])
    );
}

#[test]
fn new_starts_other_folders_unconfined() {
    let mut t = T::new();
    let ws = t.dir.path().join("ws");
    let project = t.dir.path().join("projects/gamma");
    fs::create_dir_all(ws.join("beta")).expect("mkdir");
    fs::create_dir_all(ws.join("alpha")).expect("mkdir");
    fs::create_dir_all(&project).expect("mkdir");
    symlink(&project, ws.join("gamma")).expect("symlink");
    confine_beta(&mut t, &ws.join("beta").to_string_lossy());
    for folder in ["alpha", "gamma"] {
        t.tell(&format!("!new {folder}"));
        assert_eq!(t.herdr.last(), c(&["pane", "run", "wN:p1", "claude"]));
    }
}

#[test]
fn second_pane_in_the_folder_has_its_own_turn() {
    let mut t = T::new();
    let nora = confine(&mut t);
    transcript(&nora.state.join("projects/-w-a/s.jsonl"), "confined says");
    t.herdr
        .0
        .borrow_mut()
        .cwd
        .insert("wA:p2".into(), "/w/a".into());
    t.set("wA:p1", "idle");
    t.set("wA:p2", "idle");
    t.herdr
        .0
        .borrow_mut()
        .kinds
        .insert("wA:p1".into(), "claude".into());
    t.said("wA:p2", &[("u1", "desk pane says")], None);
    t.tell("!attach wA:p2");
    t.tell("!last");
    assert_eq!(t.last_msg(), "desk pane says");
    t.tell("!attach wA:p1");
    t.tell("!last");
    assert_eq!(t.last_msg(), "confined says");
    t.herdr
        .0
        .borrow_mut()
        .kinds
        .insert("wA:p2".into(), "claude".into());
    t.tell("!last");
    assert_eq!(t.last_msg(), "Nothing said yet.");
}

#[test]
fn queue_that_is_not_private_is_logged_once() {
    let mut t = T::new();
    let nora = confine(&mut t);
    t.set("wA:p1", "idle");
    fs::set_permissions(&nora.uploads, fs::Permissions::from_mode(0o755)).expect("chmod");
    fs::write(nora.uploads.join("1-x.json"), "{}").expect("write");
    t.clear_log();
    for _ in 0..3 {
        t.advance(1.0);
        t.bot.local().expect("local");
    }
    let errors = t
        .log_lines()
        .iter()
        .filter(|l| l.starts_with("error: uploads of nora: "))
        .count();
    assert_eq!(errors, 1, "{:?}", t.log_lines());
}
