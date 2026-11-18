//! The bot: one chat with the owner, attached to one agent at a time, and
//! a chat of its own for each agent made a contact.

mod choice;
mod commands;
mod contacts;
mod inbound;
mod onboard;
mod poll;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::herdr::{self, Agent, Herdr, RunHerdr, Status, Turn};
use crate::kind::{self, Kind, pi};
use crate::matrix::{Api as _, As, Matrix};
use crate::render::{EVENT_MAX, chunks, cut, escape, md, plain, screen_size, size};
use crate::state::{Away, Chat, State, Store};
use crate::{Error, Result, files};

/// Marks a prompt from the chat, so the agent answers for a phone
pub const MARK: &str = "[Element]";
/// The whole last message of a turn that posts nothing
pub const SILENT: &str = "NOOP";
/// Lines of a screen at most
pub const SCREEN_MAX: u64 = 200;
/// Lines of a screen shown unless asked for more
pub const SCREEN_LINES: u64 = 20;
/// Lines of a screen read for a choice on it
const CHOICE_LINES: u64 = 40;
/// Messages whose replies find their agent
pub const SENT_MAX: usize = 200;
/// Transaction ids kept for messages not yet out whole
const TXNS_KEPT: usize = 100;
pub const POLL_EVERY: Duration = Duration::from_secs(3);
/// The long poll while a turn is under way, when herdr is polled each tick
const SYNC_TURN: Duration = Duration::from_millis(500);
/// How long the newest message of a turn is held back
pub const HOLD: Duration = Duration::from_secs(20);
/// Messages older than this when they arrive are skipped
pub const STALE: Duration = Duration::from_mins(10);
/// How long a prompt waits for its answer while the agent does nothing
pub const PENDING_TTL: Duration = Duration::from_mins(10);
/// How long files from the chat wait for more for the same agent
pub const FILES_WAIT: Duration = Duration::from_secs(5);
/// The wait after a step first fails; it doubles with each next failure
const RETRY_FIRST: Duration = Duration::from_secs(3);
const RETRY_MAX: Duration = Duration::from_hours(1);
/// The longest wait between two polls while herdr fails
const POLL_MAX: Duration = Duration::from_mins(1);
const PURGE_EVERY: Duration = Duration::from_hours(1);
/// How long the bot keeps what waits on the owner's next step or on an
/// agent that starts: an `!exit` to confirm, a message with no agent, a
/// pane started with `!new`
const FOLLOW_UP: Duration = Duration::from_mins(2);

/// What a recording says
pub type Transcribe = Box<dyn Fn(&Path) -> Result<String>>;
pub type Answer = Box<dyn Fn(&Agent) -> Option<Turn>>;

/// How the own chat of the agent working in `cwd` looks; each unset part
/// is as for any agent.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AgentSettings {
    pub cwd: String,
    /// What `!new` starts there, and what runs in its sandbox
    pub kind: Kind,
    pub name: Option<String>,
    /// The bot's name before the agent's, in place of `spawn_prefix`
    pub prefix: Option<bool>,
    pub avatar: Option<PathBuf>,
    pub sandbox: Option<Sandbox>,
    /// An own chat as soon as the bot sees the agent
    pub chat: bool,
    /// Started once when the bot comes up and it does not run
    pub autostart: bool,
}

/// The places of a confined agent, which `herdr-matrix-agent-<id>` binds
/// into its sandbox. Everything the agent writes there is untrusted.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Sandbox {
    pub id: String,
    /// Its state directory, with its sessions
    pub state: PathBuf,
    /// Its queue of `herdr-matrix send`
    pub uploads: PathBuf,
    /// Files from the chat for it, read-only inside
    pub files: PathBuf,
}

/// What the bot works with; the tests swap each part.
pub struct Deps {
    /// The only user the bot listens to
    pub owner: String,
    pub herdr: Box<dyn Herdr>,
    /// Seconds since the epoch
    pub now: Box<dyn Fn() -> f64>,
    pub answer: Answer,
    pub transcribe: Option<Transcribe>,
    /// A line of the journal
    pub log: Box<dyn Fn(&str)>,
    /// Where files from the chat go
    pub files: PathBuf,
    /// Where `!new` looks for folders
    pub workspace: PathBuf,
    pub name: Option<String>,
    /// The bot's picture
    pub avatar: Option<PathBuf>,
    /// The picture of an agent's own contact
    pub spawn_avatar: Option<PathBuf>,
    /// `name` before an agent's in the name of its contact
    pub spawn_prefix: bool,
    /// Where `herdr-matrix prompt` queues prompts from the machine
    pub inbox: Option<PathBuf>,
    /// Where `herdr-matrix send` queues files of the agents
    pub uploads: Option<PathBuf>,
    pub agents: Vec<AgentSettings>,
}

impl Default for Deps {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_default();
        Self {
            owner: String::new(),
            herdr: Box::new(RunHerdr::default()),
            now: Box::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0.0, |d| d.as_secs_f64())
            }),
            answer: Box::new(|a| kind::turn(a, None)),
            transcribe: None,
            log: Box::new(|line| eprintln!("{line}")),
            files: std::env::temp_dir().join("herdr-matrix"),
            workspace: Path::new(&home).join("Workspace"),
            name: None,
            avatar: None,
            spawn_avatar: None,
            spawn_prefix: true,
            inbox: None,
            uploads: None,
            agents: Vec::new(),
        }
    }
}

/// Where a message goes: a screen after it, the pane a reply to it
/// reaches, quiet as a notice, and an agent's own chat.
#[derive(Clone, Copy, Default)]
pub struct To<'a> {
    pub pre: Option<&'a str>,
    pub pane: Option<&'a str>,
    pub quiet: bool,
    pub chat: Option<&'a Chat>,
}

impl<'a> To<'a> {
    /// The main chat, or an agent's own chat.
    #[must_use]
    pub fn chat(chat: Option<&'a Chat>) -> Self {
        Self {
            chat,
            ..Self::default()
        }
    }

    /// A reply to it reaches the agent in `pane`.
    #[must_use]
    pub fn pane(self, pane: &'a str) -> Self {
        Self {
            pane: Some(pane),
            ..self
        }
    }

    /// A screen after the text.
    #[must_use]
    pub fn screen(self, pre: &'a str) -> Self {
        Self {
            pre: Some(pre),
            ..self
        }
    }

    /// A notice, which clients do not push.
    #[must_use]
    pub fn quiet(self) -> Self {
        Self {
            quiet: true,
            ..self
        }
    }
}

/// A step that failed: how often in a row, when last, and what it said.
#[derive(Default)]
struct Retry {
    failures: u32,
    at: f64,
    said: String,
}

/// A pane the bot started that is not ready yet.
struct Starting {
    folder: String,
    /// When it started, or last waited on a choice
    at: f64,
    /// True once herdr listed it
    seen: bool,
    /// Polls and since when herdr no longer lists it
    missing: Option<(u32, f64)>,
}

/// How an agent's message is posted: its text, else its screen; quiet as a
/// notice; with how to answer it when another agent is attached.
#[derive(Clone, Copy)]
struct Post<'a> {
    text: Option<&'a str>,
    agents: &'a [Agent],
    screen: bool,
    quiet: bool,
    hint: bool,
    chat: Option<&'a Chat>,
}

impl<'a> Post<'a> {
    fn new(agents: &'a [Agent], chat: Option<&'a Chat>) -> Self {
        Self {
            text: None,
            agents,
            screen: true,
            quiet: false,
            hint: true,
            chat,
        }
    }

    fn text(self, text: Option<&'a str>) -> Self {
        Self { text, ..self }
    }

    fn no_screen(self) -> Self {
        Self {
            screen: false,
            ..self
        }
    }

    fn quiet(self) -> Self {
        Self {
            quiet: true,
            ..self
        }
    }

    fn no_hint(self) -> Self {
        Self {
            hint: false,
            ..self
        }
    }
}

pub struct Bot {
    matrix: Box<dyn Matrix>,
    deps: Deps,
    store: Store,
    state: State,
    statuses: Option<IndexMap<String, Status>>,
    reported: IndexMap<String, Option<String>>,
    last_poll: Option<f64>,
    was_afk: bool,
    /// Panes with an own chat missing from the agent list: polls and since
    missing: IndexMap<String, (u32, f64)>,
    /// Agents a failure kept out of the sweep when away began
    unswept: BTreeSet<String>,
    /// The chats that show an agent typing: when the notice was sent
    typing: BTreeMap<Option<String>, f64>,
    user_id: Option<String>,
    /// True once the name and picture are set
    profiled: bool,
    /// Transaction ids of messages not yet out whole, by what they send
    txns: IndexMap<String, String>,
    /// The sha256 of each contact picture read in this run, by its path
    digests: HashMap<PathBuf, String>,
    /// Files from the chat that wait for more
    held_files: Vec<inbound::Held>,
    /// The agents herdr listed last
    listed: Vec<Agent>,
    /// Steps that failed, by what the journal calls them
    retries: IndexMap<String, Retry>,
    /// True once the main chat is named after the attached agent
    nicked: bool,
    /// When the old files were last removed
    purged: Option<f64>,
    /// Queued files the homeserver failed to take, by their copy: the last
    /// error, and how often it failed
    failed_uploads: IndexMap<PathBuf, (String, u32, f64)>,
    /// The largest upload the homeserver takes, once it said; `u64::MAX`
    /// when it names none
    upload_limit: Option<u64>,
    /// The pane `!exit` last asked to confirm, and when
    exit_asked: Option<(String, f64)>,
    /// The attached pane and its agent's name, as last listed
    attached_name: Option<(String, String)>,
    /// Polls and since when the attached pane is missing
    attached_missing: Option<(u32, f64)>,
    /// Panes `!new` started that are not ready yet
    starting: IndexMap<String, Starting>,
    /// Panes the bot started in this run for a configured agent: its `cwd`,
    /// and polls and since when herdr no longer lists the pane
    started_for: IndexMap<String, (String, Option<(u32, f64)>)>,
    /// The last message that found no agent
    kept: Option<inbound::Kept>,
    /// The panes of the agents as the chat last listed them, by number
    numbered: Vec<String>,
    /// Lists of herdr in a row that did not fail
    good_polls: u32,
    /// The `cwd` of the agents the bot tried to start in this run
    autostarted: BTreeSet<String>,
}

impl Bot {
    #[must_use]
    pub fn new(matrix: Box<dyn Matrix>, store: Store, state: State, deps: Deps) -> Self {
        Self {
            matrix,
            deps,
            store,
            statuses: None,
            reported: IndexMap::new(),
            last_poll: None,
            was_afk: state.away == Away::On,
            missing: IndexMap::new(),
            unswept: BTreeSet::new(),
            typing: BTreeMap::new(),
            user_id: None,
            profiled: false,
            txns: IndexMap::new(),
            digests: HashMap::new(),
            held_files: Vec::new(),
            listed: Vec::new(),
            retries: IndexMap::new(),
            nicked: false,
            purged: None,
            failed_uploads: IndexMap::new(),
            upload_limit: None,
            exit_asked: None,
            attached_name: None,
            attached_missing: None,
            starting: IndexMap::new(),
            started_for: IndexMap::new(),
            kept: None,
            numbered: Vec::new(),
            good_polls: 0,
            autostarted: BTreeSet::new(),
            state,
        }
    }

    #[must_use]
    pub fn state(&self) -> &State {
        &self.state
    }

    #[doc(hidden)]
    pub fn state_mut(&mut self) -> &mut State {
        &mut self.state
    }

    #[must_use]
    pub fn deps(&self) -> &Deps {
        &self.deps
    }

    #[doc(hidden)]
    pub fn deps_mut(&mut self) -> &mut Deps {
        &mut self.deps
    }

    /// Writes the state, whole or not at all.
    ///
    /// # Errors
    /// When the file cannot be written.
    pub fn save(&self) -> Result<()> {
        self.store.save(&self.state)
    }

    /// Logs what the bot starts with.
    pub fn log_start(&self) {
        let away = if self.away() { "on" } else { "off" };
        self.log(&format!(
            "started: contacts {}, attached {}, away {away}",
            self.state.spawned.len(),
            self.state.attached.as_deref().unwrap_or("none"),
        ));
    }

    fn log(&self, line: &str) {
        (self.deps.log)(line);
    }

    fn now(&self) -> f64 {
        (self.deps.now)()
    }

    /// True while away is on.
    fn away(&self) -> bool {
        self.state.away == Away::On
    }

    fn agents(&mut self) -> Result<Vec<Agent>> {
        let agents =
            herdr::list_agents(self.deps.herdr.as_mut()).inspect_err(|_| self.good_polls = 0)?;
        self.listed.clone_from(&agents);
        Ok(agents)
    }

    fn read(&mut self, pane: &str, lines: u64) -> Result<String> {
        herdr::read(self.deps.herdr.as_mut(), pane, lines)
    }

    /// The agent this chat talks to: the attached one while it runs; the
    /// only one when none is attached.
    fn attached<'a>(&self, agents: &'a [Agent]) -> Option<&'a Agent> {
        match &self.state.attached {
            Some(pane) => agents.iter().find(|a| &a.pane_id == pane),
            None if agents.len() == 1 => agents.first(),
            None => None,
        }
    }

    /// The name of the attached agent, as last listed.
    fn attached_name(&self) -> Option<&str> {
        self.attached_name
            .as_ref()
            .filter(|(pane, _)| Some(pane) == self.state.attached.as_ref())
            .map(|(_, name)| name.as_str())
    }

    /// Attaches the chat to an agent.
    fn attach_to(&mut self, a: &Agent) -> Result<()> {
        self.state.attached = Some(a.pane_id.clone());
        self.attached_name = Some((a.pane_id.clone(), a.name.clone()));
        self.save()?;
        self.nicked = self.retry("nick", RETRY_MAX, |bot| bot.nick(Some(&a.name)));
        Ok(())
    }

    fn listing(&self, agents: &[Agent]) -> String {
        let here = self.attached(agents).map(|a| a.pane_id.clone());
        let lines: Vec<String> = agents
            .iter()
            .enumerate()
            .map(|(i, a)| {
                format!(
                    "{} {}. {}  {}{}",
                    if Some(&a.pane_id) == here.as_ref() {
                        ">"
                    } else {
                        " "
                    },
                    i + 1,
                    a.name,
                    doing(&a.status),
                    if self.state.spawned.contains_key(&a.pane_id) {
                        "  (own chat)"
                    } else {
                        ""
                    }
                )
            })
            .collect();
        if lines.is_empty() {
            "(none)".into()
        } else {
            lines.join("\n")
        }
    }

    /// The agent `query` names, or its number in the list last shown; with
    /// no query, the agent in the pane `replied` to; else asks in the main
    /// chat which one, as `which` says it ("Attach to which"), under the
    /// list of them. A name that is a number wins over the number; a number
    /// matches no other name.
    fn pick<'a>(
        &mut self,
        query: &str,
        agents: &'a [Agent],
        which: &str,
        replied: Option<&str>,
    ) -> Result<Option<&'a Agent>> {
        let number = !query.is_empty() && query.bytes().all(|b| b.is_ascii_digit());
        let pane = if query.is_empty() {
            replied
        } else if let Some(a) = agents.iter().find(|a| number && a.name == query) {
            Some(a.pane_id.as_str())
        } else {
            query
                .parse::<usize>()
                .ok()
                .and_then(|n| self.numbered.get(n.wrapping_sub(1)))
                .map(String::as_str)
        };
        if let Some(a) = pane.and_then(|p| agents.iter().find(|a| a.pane_id == p)) {
            return Ok(Some(a));
        }
        if number {
            self.ask_which(&format!("No agent {query} in the last list."), agents)?;
            return Ok(None);
        }
        let head = match herdr::find(agents, query) {
            _ if query.is_empty() => format!("{which} agent?"),
            Ok(a) => return Ok(Some(a)),
            Err(0) => format!("No agent matches \"{query}\". {which} one?"),
            Err(n) => format!("\"{query}\" matches {n} agents. {which} one?"),
        };
        self.ask_which(&head, agents)?;
        Ok(None)
    }

    /// Asks in the main chat which agent, under the list of them.
    fn ask_which(&mut self, head: &str, agents: &[Agent]) -> Result<()> {
        let listing = self.listing(agents);
        self.numbered = agents.iter().map(|a| a.pane_id.clone()).collect();
        self.say(head, To::default().screen(&listing))?;
        Ok(())
    }

    fn me(&mut self) -> Result<String> {
        if let Some(me) = &self.user_id {
            return Ok(me.clone());
        }
        let me = self.matrix.whoami()?;
        self.user_id = Some(me.clone());
        Ok(me)
    }

    /// The room of the main chat, or of an agent's own chat, and who
    /// speaks there.
    fn room<'c>(&self, chat: Option<&'c Chat>) -> Result<(String, As<'c>)> {
        match chat {
            None => Ok((self.state.control.clone().ok_or(Error::NoChat)?, As::Bot)),
            Some(chat) => Ok((chat.room.clone(), As::User(&chat.user))),
        }
    }

    /// The name and picture until they are set, one sync, the files from
    /// the chat that no more joined, and a herdr poll when one is due, so a
    /// homeserver that answers /sync at once does not spin herdr. While a
    /// turn is under way, each tick polls. While herdr fails, its polls come
    /// further apart.
    ///
    /// # Errors
    /// When Matrix fails.
    pub fn tick(&mut self) -> Result<()> {
        if !self.profiled {
            self.profiled = self.retry("profile", RETRY_MAX, Self::profile);
        }
        self.onboard()?;
        let synced = self.sync();
        self.hand_over_files();
        synced?;
        if !self.retries.contains_key("poll")
            && let Err(err) = self.local()
        {
            self.log(&format!("error: prompts: {err}"));
        }
        let now = self.now();
        let turn = !self.state.pending.is_empty();
        if self
            .last_poll
            .is_none_or(|last| turn || due(now, last, POLL_EVERY))
        {
            let mut listed = None;
            if self.retry("poll", POLL_MAX, |bot| {
                listed = Some(bot.agents()?);
                Ok(())
            }) {
                self.last_poll = Some(now);
            }
            if let Some(agents) = listed {
                self.poll_listed(&agents)?;
            }
        }
        if self.purged.is_none_or(|at| due(now, at, PURGE_EVERY)) {
            self.purged = Some(now);
            self.purge_files(now);
        }
        Ok(())
    }

    /// Runs `step` unless it failed lately: after a failure it waits
    /// `RETRY_FIRST`, twice as long after each next one, `max` at most. A
    /// failure is logged when it differs from the one before. True when
    /// it ran and did not fail.
    fn retry(
        &mut self,
        label: &str,
        max: Duration,
        step: impl FnOnce(&mut Self) -> Result<()>,
    ) -> bool {
        let now = self.now();
        if let Some(r) = self.retries.get(label) {
            let wait = RETRY_FIRST
                .saturating_mul(1 << r.failures.saturating_sub(1).min(20))
                .min(max);
            if !due(now, r.at, wait) {
                return false;
            }
        }
        let err = match step(self) {
            Ok(()) => {
                if self.retries.shift_remove(label).is_some() {
                    self.log(&format!("{label}: ok again"));
                }
                return true;
            }
            Err(err) => err,
        };
        let said = err.to_string();
        let r = self.retries.entry(label.to_owned()).or_default();
        r.failures += 1;
        r.at = now;
        if r.said != said {
            r.said = said;
            self.log(&format!("error: {label}: {err}"));
        }
        false
    }

    /// Removes the files from the chat older than `FILE_TTL`, the confined
    /// agents' too.
    fn purge_files(&mut self, now: f64) {
        let old = now - files::FILE_TTL.as_secs_f64();
        let sandboxes = self.deps.agents.iter().filter_map(|s| s.sandbox.as_ref());
        let dirs: Vec<PathBuf> = std::iter::once(self.deps.files.clone())
            .chain(sandboxes.map(|s| s.files.clone()))
            .collect();
        for dir in dirs {
            match files::purge(&dir, old) {
                Err(Error::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => self.log(&format!("error: {}: {err}", dir.display())),
                Ok(()) => {}
            }
        }
    }

    /// Posts to the chat, or to an agent's own chat, a long text in several
    /// messages; `pane` lets a reply to it find its agent. A quiet message
    /// is a notice, which clients do not push. The id of the last message.
    ///
    /// # Errors
    /// When Matrix refuses a message.
    pub fn say(&mut self, text: &str, to: To) -> Result<Option<String>> {
        let mut parts: Vec<(String, Option<String>)> =
            chunks(text).into_iter().map(|p| (p, None)).collect();
        for screen in to.pre.map(|pre| cut(pre, screen_size)).unwrap_or_default() {
            match parts.last_mut() {
                Some(last)
                    if last.1.is_none() && size(&last.0) + screen_size(&screen) <= EVENT_MAX =>
                {
                    last.1 = Some(screen);
                }
                _ => parts.push((String::new(), Some(screen))),
            }
        }
        let mut event = None;
        let mut keys = Vec::new();
        for (i, (part, screen)) in parts.into_iter().enumerate() {
            let mut formatted = md(&part);
            if let Some(screen) = &screen {
                let _ = write!(formatted, "<pre>{}</pre>", escape(screen));
            }
            let text = plain(&part);
            let body: Vec<&str> = [Some(text.as_str()), screen.as_deref()]
                .into_iter()
                .flatten()
                .filter(|x| !x.is_empty())
                .collect();
            let content = json!({"msgtype": if to.quiet { "m.notice" } else { "m.text" },
                   "body": body.join("\n\n"),
                   "format": "org.matrix.custom.html",
                   "formatted_body": formatted});
            let key = format!("{} {i} {content}", to.chat.map_or("", |c| c.room.as_str()));
            let sent = self.send(&content, to.pane, to.chat, &key);
            keys.push(key);
            event = sent?;
        }
        for key in &keys {
            self.txns.shift_remove(key);
        }
        Ok(event)
    }

    /// Sends an event. A send tried again after a failure, until its whole
    /// message is out, keeps its transaction id under `key`, so the
    /// homeserver drops what it stored already.
    fn send(
        &mut self,
        content: &Value,
        pane: Option<&str>,
        chat: Option<&Chat>,
        key: &str,
    ) -> Result<Option<String>> {
        let (room, by) = self.room(chat)?;
        let txn = if let Some(txn) = self.txns.get(key) {
            txn.clone()
        } else {
            let txn = self.matrix.next_txn();
            self.txns.insert(key.to_owned(), txn.clone());
            while self.txns.len() > TXNS_KEPT {
                self.txns.shift_remove_index(0);
            }
            txn
        };
        let event = self.matrix.send(&room, by, &txn, content)?;
        if let (Some(pane), Some(event)) = (pane.filter(|p| !p.is_empty()), &event) {
            self.state.sent.insert(event.clone(), pane.to_owned());
            while self.state.sent.len() > SENT_MAX {
                self.state.sent.shift_remove_index(0);
            }
            self.save()?;
        }
        Ok(event)
    }

    /// True when an agent speaks without its name: the attached one, and
    /// any agent in its own chat.
    fn own_voice(&self, a: &Agent, agents: &[Agent], chat: Option<&Chat>) -> bool {
        chat.is_some()
            || (!agents.is_empty()
                && self
                    .attached(agents)
                    .is_some_and(|x| x.pane_id == a.pane_id))
    }

    /// What an agent said, or its screen when that is unknown. An agent
    /// that speaks in its own voice has no name; another one names itself,
    /// and with `hint` says how to answer it.
    fn post(&mut self, a: &Agent, note: &str, how: Post) -> Result<Option<String>> {
        let mut head = format!("**{}** {note}", a.name).trim_end().to_owned();
        if self.own_voice(a, how.agents, how.chat) {
            let mut chars = note.chars();
            head = chars
                .next()
                .map_or(String::new(), |c| c.to_uppercase().chain(chars).collect());
        } else if !how.agents.is_empty() && how.hint {
            let _ = write!(
                head,
                "\nReply to this message to answer it, or !attach {}.",
                a.name
            );
        }
        let to = To::chat(how.chat).pane(&a.pane_id);
        if how.text.is_none() && how.screen {
            let screen = self.read(&a.pane_id, SCREEN_LINES)?;
            return self.say(&head, to.screen(&screen));
        }
        let body: Vec<&str> = [Some(head.as_str()), how.text]
            .into_iter()
            .flatten()
            .filter(|x| !x.is_empty())
            .collect();
        self.say(&body.join("\n\n"), if how.quiet { to.quiet() } else { to })
    }

    /// What the agent did since the last prompt; a confined agent cannot
    /// tell herdr its session, so its newest session file is the one.
    fn turn(&self, a: &Agent) -> Turn {
        self.read_turn(a).unwrap_or_default()
    }

    /// `turn`; None for an agent without a reader. An agent herdr reports
    /// no session of is read from the newest session file of its folder;
    /// two such agents in one folder would read one file, and get none.
    fn read_turn(&self, a: &Agent) -> Option<Turn> {
        let others: Vec<&Agent> = self
            .listed
            .iter()
            .filter(|b| b.pane_id != a.pane_id)
            .filter(|b| {
                a.cwd
                    .as_deref()
                    .zip(b.cwd.as_deref())
                    .is_some_and(|(x, y)| same_dir(x, y))
            })
            .collect();
        let settings = self.agent_settings(a);
        if let Some((kind, sandbox)) =
            settings.and_then(|s| s.sandbox.as_ref().map(|sandbox| (&s.kind, sandbox)))
        {
            if others.is_empty() {
                return kind::confined_turn(kind, &sandbox.state);
            }
            if a.session.is_none() {
                return if others.iter().any(|b| b.session.is_none()) {
                    None
                } else {
                    kind::confined_turn(kind, &sandbox.state)
                };
            }
        } else if a.kind == Kind::Pi
            && pi::reported(a).is_none()
            && others
                .iter()
                .any(|b| b.kind == Kind::Pi && pi::reported(b).is_none())
        {
            return None;
        }
        (self.deps.answer)(a)
    }

    /// The sandbox of the agent working in the directory of `a`.
    fn sandbox(&self, a: &Agent) -> Option<&Sandbox> {
        self.agent_settings(a)?.sandbox.as_ref()
    }
}

/// True once `wait` passed since `since`, or the clock went back before it.
fn due(now: f64, since: f64, wait: Duration) -> bool {
    now < since || now - since >= wait.as_secs_f64()
}

/// True when two directories are one, a trailing `/` aside.
fn same_dir(a: &str, b: &str) -> bool {
    a.strip_suffix('/').unwrap_or(a) == b.strip_suffix('/').unwrap_or(b)
}

/// True for a message that says the turn has nothing to post.
fn silent(text: &str) -> bool {
    text.trim() == SILENT
}

/// What the chat says when herdr fails to `action` ("send to") the agent
/// `name`: the first line of what herdr said.
fn herdr_failed(action: &str, name: &str, said: &str) -> String {
    let cause = said.lines().next().unwrap_or_default().trim();
    format!(
        "Could not {action} **{name}**: {}. Is herdr running?",
        cause.trim_end_matches('.')
    )
}

/// What the chat says when the agent a message is for is not running.
fn not_running(name: Option<&str>) -> String {
    match name {
        Some(name) => format!("**{name}** is not running; nothing was sent."),
        None => "That agent is not running; nothing was sent.".into(),
    }
}

/// What an agent does, as the chat says it.
fn doing(status: &Status) -> String {
    match status {
        Status::Blocked => "waits on you".into(),
        other => other.to_string(),
    }
}
