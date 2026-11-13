//! herdr and a homeserver in memory, and a bot wired to them.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use crate::testutil::TempDir;
use herdr_matrix::bot::{Bot, Deps, MARK, To};
use herdr_matrix::herdr::{Herdr, Turn};
use herdr_matrix::matrix::{Matrix, quote};
use herdr_matrix::state::{Away, Store};
use herdr_matrix::{Error, Result};
use indexmap::IndexMap;
use serde_json::{Value, json};

pub const OWNER: &str = "@kari:example.org";
pub const BOT: &str = "@herdr:example.org";

pub fn fruit() -> Value {
    json!([{"question": "Pick a fruit?", "header": "Fruit", "multiSelect": false,
            "options": [{"label": "Apple", "description": "red"},
                        {"label": "Pear", "description": "green"}]}])
}

pub const PERMISSION: &str = "\
● Bash(rm -r build)
  ⎿  Running…

 Bash command

   rm -r build
   Remove the build directory

 Do you want to proceed?
 ❯ 1. Yes
   2. Yes, and don't ask again for rm commands
   3. No, and tell Claude what to do differently (esc)

 Esc to cancel
";

pub fn push() -> String {
    PERMISSION
        .replace("rm -r build", "git push --force")
        .replace("Remove the build directory", "Force-push main")
}

/// One call, as a list of its words.
pub fn c(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| (*w).to_owned()).collect()
}

#[derive(Default)]
pub struct HerdrState {
    pub agents: IndexMap<String, String>,
    pub cwd: HashMap<String, String>,
    pub screens: HashMap<String, String>,
    pub calls: Vec<Vec<String>>,
    pub fail: usize,
    pub broken: HashSet<(String, String)>,
    /// What `agent list` answers instead of the agents
    pub list_out: Option<String>,
}

#[derive(Clone, Default)]
pub struct FakeHerdr(pub Rc<RefCell<HerdrState>>);

impl FakeHerdr {
    pub fn set(&self, pane: &str, status: &str) {
        self.0
            .borrow_mut()
            .agents
            .insert(pane.into(), status.into());
    }

    pub fn remove(&self, pane: &str) {
        self.0.borrow_mut().agents.shift_remove(pane);
    }

    pub fn calls(&self) -> Vec<Vec<String>> {
        self.0.borrow().calls.clone()
    }

    pub fn last(&self) -> Vec<String> {
        self.calls().last().cloned().unwrap_or_default()
    }

    /// Calls of one verb, prompts without the bot's marker.
    pub fn did(&self, verb: &str) -> Vec<Vec<String>> {
        let mark = format!("{MARK} ");
        self.calls()
            .into_iter()
            .filter(|c| c.len() >= 2 && c[0] == "agent" && c[1] == verb)
            .map(|mut c| {
                if verb == "prompt"
                    && let Some(bare) = c[3].strip_prefix(&mark).map(str::to_owned)
                {
                    c[3] = bare;
                }
                c
            })
            .collect()
    }

    pub fn raw_prompts(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|c| c[0] == "agent" && c[1] == "prompt")
            .map(|c| c[3].clone())
            .collect()
    }
}

impl Herdr for FakeHerdr {
    fn run(&mut self, args: &[&str]) -> Result<(i32, String)> {
        let mut s = self.0.borrow_mut();
        s.calls.push(c(args));
        let verb = (args[0], args.get(1).copied().unwrap_or_default());
        if verb == ("agent", "list") && s.fail > 0 {
            s.fail -= 1;
            return Ok((1, "herdr is down".into()));
        }
        if s.broken.contains(&(verb.0.to_owned(), verb.1.to_owned())) {
            return Ok((1, format!("{} failed", verb.1)));
        }
        Ok(match verb {
            ("agent", "list") => {
                if let Some(out) = &s.list_out {
                    return Ok((0, out.clone()));
                }
                let agents: Vec<Value> = s
                    .agents
                    .iter()
                    .map(|(p, status)| {
                        json!({
                            "agent": "claude",
                            "agent_session": {"agent": "claude", "kind": "id",
                                              "source": "herdr:claude", "value": format!("s-{p}")},
                            "agent_status": status, "completion_seq": 0, "cwd": s.cwd.get(p),
                            "focused": false, "foreground_cwd": "/w/repo", "pane_id": p,
                            "revision": 1, "state_change_seq": 1, "tab_id": "w1:t1",
                            "terminal_id": "term_1", "terminal_title": "claude",
                            "terminal_title_stripped": "claude",
                            "workspace_id": p.split(':').next().unwrap_or_default()})
                    })
                    .collect();
                (
                    0,
                    json!({"id": "1", "result": {"type": "agent_list", "agents": agents}})
                        .to_string(),
                )
            }
            ("workspace", "create") => (
                0,
                json!({"result": {"root_pane": {"pane_id": "wN:p1"}}}).to_string(),
            ),
            ("workspace", "list") => {
                let ids: HashSet<&str> = s
                    .agents
                    .keys()
                    .filter_map(|p| p.split(':').next())
                    .collect();
                let spaces: Vec<Value> = ids
                    .into_iter()
                    .map(|w| json!({"workspace_id": w, "label": format!("repo-{w}")}))
                    .collect();
                (0, json!({"result": {"workspaces": spaces}}).to_string())
            }
            ("agent", "read") => (
                0,
                s.screens
                    .get(args[2])
                    .cloned()
                    .unwrap_or_else(|| format!("screen of {}\n", args[2])),
            ),
            _ => (0, "{}".into()),
        })
    }
}

pub type Call = (String, String, Option<Value>, Vec<(String, String)>);

#[derive(Default)]
pub struct MatrixState {
    pub calls: Vec<Call>,
    pub syncs: Vec<Value>,
    pub user_syncs: HashMap<String, Vec<Value>>,
    pub users: HashSet<String>,
    pub broken: HashSet<String>,
    pub rooms: usize,
    pub txn: usize,
    /// Members of a room, joined or invited; owner and bot by default
    pub members: HashMap<String, Vec<String>>,
    /// Who made a room; the owner by default
    pub creators: HashMap<String, String>,
    /// How a room is joined; by invite by default
    pub join_rules: HashMap<String, String>,
    pub ids: Vec<String>,
    pub uploads: Vec<Vec<u8>>,
    pub mimes: Vec<String>,
    pub fail_send: usize,
    /// Sends the homeserver stores but whose answer is lost
    pub lose_reply: usize,
    pub refuse: Vec<String>,
}

#[derive(Clone, Default)]
pub struct FakeMatrix(pub Rc<RefCell<MatrixState>>);

impl FakeMatrix {
    pub fn calls(&self) -> Vec<Call> {
        self.0.borrow().calls.clone()
    }

    pub fn ids(&self) -> Vec<String> {
        self.0.borrow().ids.clone()
    }

    pub fn last_id(&self) -> String {
        self.ids().last().cloned().expect("an event")
    }

    /// A field of every message sent, or of those in one room.
    pub fn sent_key(&self, key: &str, room: Option<&str>) -> Vec<String> {
        let inside = room.map_or_else(
            || "/send/".to_owned(),
            |r| format!("/rooms/{}/send/", quote(r)),
        );
        self.calls()
            .into_iter()
            .filter(|c| c.0 == "PUT" && c.1.contains(&inside))
            .map(|c| {
                c.2.and_then(|b| b.get(key).and_then(Value::as_str).map(str::to_owned))
                    .unwrap_or_default()
            })
            .collect()
    }

    pub fn sent(&self) -> Vec<String> {
        self.sent_key("body", None)
    }

    pub fn as_user(&self, user: &str) -> Vec<Call> {
        self.calls()
            .into_iter()
            .filter(|c| c.3.iter().any(|(k, v)| k == "user_id" && v == user))
            .collect()
    }

    pub fn typing(&self) -> Vec<bool> {
        self.calls()
            .into_iter()
            .filter(|c| c.1.contains("/typing/"))
            .map(|c| c.2.and_then(|b| b["typing"].as_bool()).expect("typing"))
            .collect()
    }

    pub fn has(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        query: &[(&str, &str)],
    ) -> bool {
        let want: Call = (
            method.into(),
            path.into(),
            body,
            query
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        );
        self.calls().contains(&want)
    }
}

impl Matrix for FakeMatrix {
    fn call(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        query: &[(&str, &str)],
        _: Duration,
    ) -> Result<Value> {
        let mut s = self.0.borrow_mut();
        let query: Vec<(String, String)> = query
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect();
        let user = query
            .iter()
            .find(|(k, _)| k == "user_id")
            .map(|(_, v)| v.clone());
        s.calls
            .push((method.into(), path.into(), body.cloned(), query));
        if s.refuse.iter().any(|part| path.contains(part.as_str())) {
            return Err(Error::Transport(format!("{path} failed")));
        }
        if path == "/createRoom" {
            s.rooms += 1;
            return Ok(json!({"room_id": format!("!r{}:example.org", s.rooms)}));
        }
        if path == "/sync" {
            if let Some(user) = user {
                if s.broken.contains(&user) {
                    return Err(Error::Transport("sync failed".into()));
                }
                let queued = s.user_syncs.entry(user).or_default();
                return Ok(if queued.is_empty() {
                    json!({"next_batch": "v"})
                } else {
                    queued.remove(0)
                });
            }
            return Ok(if s.syncs.is_empty() {
                json!({"next_batch": "n"})
            } else {
                s.syncs.remove(0)
            });
        }
        if path == "/register" {
            let name = body
                .and_then(|b| b["username"].as_str())
                .unwrap_or_default()
                .to_owned();
            if !s.users.insert(name.clone()) {
                return Err(Error::Http {
                    status: 400,
                    code: "M_USER_IN_USE".into(),
                    body: br#"{"errcode": "M_USER_IN_USE"}"#.to_vec(),
                });
            }
            return Ok(json!({"user_id": format!("@{name}:example.org")}));
        }
        if path.contains("/state/m.room.member/") && method == "GET" {
            return Ok(
                json!({"membership": "join", "avatar_url": "mxc://a/b", "displayname": "Claude"}),
            );
        }
        if path == "/account/whoami" {
            return Ok(json!({"user_id": BOT}));
        }
        if path.starts_with("/rooms/") && path.ends_with("/state") && method == "GET" {
            let room = unquote(path.split('/').nth(2).unwrap_or_default());
            let members = s
                .members
                .get(&room)
                .cloned()
                .unwrap_or_else(|| vec![OWNER.into(), BOT.into()]);
            let creator = s.creators.get(&room).map_or(OWNER, String::as_str);
            let rule = s.join_rules.get(&room).map_or("invite", String::as_str);
            let mut events = vec![
                json!({"type": "m.room.create", "sender": creator, "state_key": "", "content": {}}),
                json!({"type": "m.room.join_rules", "sender": creator, "state_key": "",
                       "content": {"join_rule": rule}}),
            ];
            events.extend(members.into_iter().map(|m| {
                json!({"type": "m.room.member", "sender": creator, "state_key": m,
                       "content": {"membership": "join"}})
            }));
            return Ok(Value::Array(events));
        }
        if path == "/upload" {
            return Ok(json!({"content_uri": "mxc://example.org/pic"}));
        }
        if path.contains("/send/") && s.fail_send > 0 {
            s.fail_send -= 1;
            s.calls.pop();
            return Err(Error::Transport("send failed".into()));
        }
        if path.contains("/send/") {
            let id = format!("$e{}", s.ids.len());
            s.ids.push(id.clone());
            if s.lose_reply > 0 {
                s.lose_reply -= 1;
                return Err(Error::Transport("reply lost".into()));
            }
            return Ok(json!({"event_id": id}));
        }
        Ok(json!({}))
    }

    fn upload(&mut self, data: &[u8], content_type: &str) -> Result<String> {
        {
            let mut s = self.0.borrow_mut();
            s.uploads.push(data.to_vec());
            s.mimes.push(content_type.into());
        }
        let resp = self.call("POST", "/upload", None, &[], Duration::from_secs(30))?;
        Ok(resp["content_uri"].as_str().unwrap_or_default().to_owned())
    }

    fn download(&mut self, mxc: &str) -> Result<Vec<u8>> {
        Ok(format!("img {mxc}").into_bytes())
    }

    fn next_txn(&mut self) -> String {
        let mut s = self.0.borrow_mut();
        s.txn += 1;
        s.txn.to_string()
    }
}

fn unquote(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A sync with one message of the owner in a room.
pub fn msg(room: &str, body: &str, sender: &str, ts: i64, extra: &Value) -> Value {
    let mut content = json!({"msgtype": "m.text", "body": body});
    if let Some(extra) = extra.as_object() {
        for (k, v) in extra {
            content[k] = v.clone();
        }
    }
    json!({"next_batch": "n2", "rooms": {"join": {room: {"timeline": {"events": [
        {"type": "m.room.message", "sender": sender, "event_id": "$mine",
         "origin_server_ts": ts, "content": content}]}}}}})
}

pub fn react(room: &str, target: &str, key: &str, sender: &str, ts: i64) -> Value {
    json!({"next_batch": "n4", "rooms": {"join": {room: {"timeline": {"events": [
        {"type": "m.reaction", "sender": sender, "origin_server_ts": ts,
         "content": {"m.relates_to": {"rel_type": "m.annotation", "event_id": target, "key": key}}}]}}}}})
}

pub fn invite(room: &str, sender: &str) -> Value {
    json!({"next_batch": "n3", "rooms": {"invite": {room: {"invite_state": {"events": [
        {"type": "m.room.member", "sender": sender, "state_key": BOT,
         "content": {"membership": "invite"}}]}}}}})
}

/// Seconds as Matrix's milliseconds.
pub fn ms(seconds: f64) -> i64 {
    format!("{:.0}", seconds * 1000.0)
        .parse()
        .expect("whole milliseconds")
}

/// A reply to an event.
pub fn reply_to(event: &str) -> Value {
    json!({"m.relates_to": {"m.in_reply_to": {"event_id": event}}})
}

/// The bot as the tests drive it: a clock, presence, answers, voice and
/// the journal all in hand.
pub struct T {
    pub dir: TempDir,
    pub herdr: FakeHerdr,
    pub matrix: FakeMatrix,
    pub clock: Rc<Cell<f64>>,
    pub answers: Rc<RefCell<HashMap<String, Turn>>>,
    pub heard: Rc<RefCell<Vec<Vec<u8>>>>,
    pub transcript: Rc<RefCell<String>>,
    pub lines: Rc<RefCell<Vec<String>>>,
    pub bot: Bot,
    pub room: String,
    pub start: usize,
}

impl T {
    pub fn new() -> Self {
        let dir = TempDir::new();
        let herdr = FakeHerdr::default();
        let matrix = FakeMatrix::default();
        let mut t = Self {
            bot: Bot::new(
                Box::new(matrix.clone()),
                Store::new(dir.path().join("unused.json")),
                herdr_matrix::state::State::default(),
                Deps::default(),
            ),
            dir,
            herdr,
            matrix,
            clock: Rc::new(Cell::new(1000.0)),
            answers: Rc::default(),
            heard: Rc::default(),
            transcript: Rc::default(),
            lines: Rc::default(),
            room: String::new(),
            start: 0,
        };
        t.bot = t.make();
        t.bot.onboard().expect("onboard");
        t.bot.sync().expect("sync");
        t.room = t.bot.state().control.clone().expect("a chat");
        t.start = t.matrix.sent().len();
        t
    }

    pub fn state_path(&self) -> std::path::PathBuf {
        self.dir.path().join("state.json")
    }

    pub fn make(&self) -> Bot {
        let (herdr, clock, answers) =
            (self.herdr.clone(), self.clock.clone(), self.answers.clone());
        let (heard, transcript, lines) = (
            self.heard.clone(),
            self.transcript.clone(),
            self.lines.clone(),
        );
        let store = Store::new(self.state_path());
        let log = lines.clone();
        let state = store
            .load(&move |line| log.borrow_mut().push(line.to_owned()))
            .expect("a state");
        Bot::new(
            Box::new(self.matrix.clone()),
            store,
            state,
            Deps {
                owner: OWNER.into(),
                herdr: Box::new(herdr),
                now: Box::new(move || clock.get()),
                answer: Box::new(move |a| answers.borrow().get(&a.pane_id).cloned()),
                transcribe: Some(Box::new(move |path| {
                    heard.borrow_mut().push(std::fs::read(path)?);
                    Ok(transcript.borrow().clone())
                })),
                log: Box::new(move |line| lines.borrow_mut().push(line.to_owned())),
                files: self.dir.path().join("files"),
                workspace: self.dir.path().join("ws"),
                name: None,
                avatar: None,
                spawn_avatar: None,
            },
        )
    }

    pub fn now(&self) -> f64 {
        self.clock.get()
    }

    pub fn advance(&self, seconds: f64) {
        self.clock.set(self.clock.get() + seconds);
    }

    fn ts(&self) -> i64 {
        ms(self.now())
    }

    pub fn set(&self, pane: &str, status: &str) {
        self.herdr.set(pane, status);
    }

    pub fn said(&self, pane: &str, messages: &[(&str, &str)], question: Option<Value>) {
        self.answers.borrow_mut().insert(
            pane.into(),
            Turn {
                messages: messages
                    .iter()
                    .map(|(u, t)| (Some((*u).into()), (*t).into()))
                    .collect(),
                question: question.map(Into::into),
            },
        );
    }

    /// The bodies posted since the chat was set up.
    pub fn new_msgs(&self) -> Vec<String> {
        self.matrix.sent()[self.start..].to_vec()
    }

    pub fn last_msg(&self) -> String {
        self.matrix.sent().last().cloned().unwrap_or_default()
    }

    pub fn sync(&mut self) {
        self.bot.sync().expect("sync");
    }

    pub fn poll(&mut self) {
        self.bot.poll().expect("poll");
    }

    pub fn queue(&self, batch: Value) {
        self.matrix.0.borrow_mut().syncs.push(batch);
    }

    pub fn tell_full(&mut self, body: &str, sender: &str, extra: &Value) {
        self.queue(msg(&self.room, body, sender, self.ts(), extra));
        self.sync();
    }

    pub fn tell(&mut self, body: &str) {
        self.tell_full(body, OWNER, &json!({}));
    }

    pub fn tell_with(&mut self, body: &str, extra: &Value) {
        self.tell_full(body, OWNER, extra);
    }

    pub fn react_from(&mut self, target: &str, key: &str, sender: &str) {
        self.queue(react(&self.room, target, key, sender, self.ts()));
        self.sync();
    }

    pub fn react(&mut self, target: &str, key: &str) {
        self.react_from(target, key, OWNER);
    }

    /// Away on, as `!away` turns it on.
    pub fn go_afk(&mut self) {
        self.bot.state_mut().away = Away::On;
    }

    pub fn working_then(&mut self, status: &str, pane: &str) {
        self.set(pane, "working");
        self.poll();
        self.go_afk();
        self.set(pane, status);
        self.poll();
    }

    pub fn speak_with(&mut self, transcript: &str, extra: &Value) {
        *self.transcript.borrow_mut() = transcript.into();
        let mut extra = extra.clone();
        extra["msgtype"] = json!("m.audio");
        extra["url"] = json!("mxc://x/v");
        self.tell_with("Voice message.ogg", &extra);
    }

    pub fn speak(&mut self, transcript: &str) {
        self.speak_with(transcript, &json!({}));
    }

    pub fn prompted(&mut self, text: &str) {
        self.set("wA:p1", "idle");
        self.said("wA:p1", &[("u1", "old answer")], None);
        self.poll();
        self.tell(text);
    }

    pub fn say(&mut self, text: &str, to: To) {
        self.bot.say(text, to).expect("say");
    }

    /// An agent gone from the list for good: its own chat ends.
    pub fn gone(&mut self, pane: &str) {
        self.herdr.remove(pane);
        for _ in 0..4 {
            self.advance(30.0);
            self.poll();
        }
    }

    pub fn files(&self) -> std::path::PathBuf {
        self.dir.path().join("files")
    }

    pub fn log_lines(&self) -> Vec<String> {
        self.lines.borrow().clone()
    }

    pub fn clear_log(&self) {
        self.lines.borrow_mut().clear();
    }

    // Agent contacts

    pub fn spawn(&mut self, pane: &str, name: &str) -> herdr_matrix::state::Chat {
        self.tell(&format!("!dm {name}"));
        self.bot.state().spawned[pane].clone()
    }

    pub fn tell_in_full(
        &mut self,
        chat: &herdr_matrix::state::Chat,
        body: &str,
        ts: Option<i64>,
        room: Option<&str>,
        sender: &str,
        extra: &Value,
    ) {
        let ts = ts.unwrap_or_else(|| self.ts());
        let batch = msg(room.unwrap_or(&chat.room), body, sender, ts, extra);
        self.matrix
            .0
            .borrow_mut()
            .user_syncs
            .entry(chat.user.clone())
            .or_default()
            .push(batch);
        self.sync();
    }

    pub fn tell_in(&mut self, chat: &herdr_matrix::state::Chat, body: &str) {
        self.tell_in_full(chat, body, None, None, OWNER, &json!({}));
    }

    pub fn in_chat(&self, chat: &herdr_matrix::state::Chat) -> Vec<String> {
        self.matrix.sent_key("body", Some(&chat.room))
    }

    pub fn in_chat_key(&self, chat: &herdr_matrix::state::Chat, key: &str) -> Vec<String> {
        self.matrix.sent_key(key, Some(&chat.room))
    }

    pub fn main(&self) -> Vec<String> {
        self.matrix.sent_key("body", Some(&self.room))[self.start..].to_vec()
    }

    pub fn picture(&mut self, data: &[u8]) -> std::path::PathBuf {
        let pic = self.dir.path().join("contact.png");
        std::fs::write(&pic, data).expect("write");
        self.bot.deps_mut().spawn_avatar = Some(pic.clone());
        pic
    }

    pub fn react_in(&mut self, chat: &herdr_matrix::state::Chat, target: &str, key: &str) {
        let batch = react(&chat.room, target, key, OWNER, self.ts());
        self.matrix
            .0
            .borrow_mut()
            .user_syncs
            .entry(chat.user.clone())
            .or_default()
            .push(batch);
        self.sync();
    }

    /// A turn from an agent's own chat, under way.
    pub fn contact_turn(&mut self) -> (herdr_matrix::state::Chat, usize) {
        self.set("wA:p1", "idle");
        let chat = self.spawn("wA:p1", "repo-wA");
        self.poll();
        let before = self.main().len();
        self.tell_in(&chat, "Hello");
        self.set("wA:p1", "working");
        self.poll();
        self.said("wA:p1", &[("u1", "Looking.")], None);
        self.poll();
        (chat, before)
    }
}
