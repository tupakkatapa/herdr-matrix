//! The bot: one chat with the owner, attached to one agent at a time, and
//! a chat of its own for each agent made a contact.

mod choice;
mod commands;
mod contacts;
mod inbound;
mod onboard;
mod poll;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use indexmap::IndexMap;
use serde_json::{Value, json};

use crate::herdr::{self, Agent, Herdr, RunHerdr, Status, Turn, claude_turn};
use crate::matrix::{Api as _, As, Matrix};
use crate::render::{EVENT_MAX, chunks, cut, escape, md, screen_size, size};
use crate::state::{Away, Chat, State, Store};
use crate::{Error, Result};

/// Marks a prompt from the chat, so the agent answers for a phone
pub const MARK: &str = "[Element]";
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
/// How long the newest message of a turn is held back
pub const HOLD: Duration = Duration::from_secs(20);
/// Messages older than this when they arrive are skipped
pub const STALE: Duration = Duration::from_mins(10);
/// How long a prompt waits for its answer while the agent does nothing
pub const PENDING_TTL: Duration = Duration::from_mins(10);

/// What a recording says
pub type Transcribe = Box<dyn Fn(&Path) -> Result<String>>;
pub type Answer = Box<dyn Fn(&Agent) -> Option<Turn>>;

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
            answer: Box::new(|a| claude_turn(a, None)),
            transcribe: None,
            log: Box::new(|line| eprintln!("{line}")),
            files: std::env::temp_dir().join("herdr-matrix"),
            workspace: Path::new(&home).join("Workspace"),
            name: None,
            avatar: None,
            spawn_avatar: None,
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
    typing: BTreeSet<Option<String>>,
    user_id: Option<String>,
    /// True once the name and picture are set
    profiled: bool,
    /// Transaction ids of messages not yet out whole, by what they send
    txns: IndexMap<String, String>,
}

impl Bot {
    #[must_use]
    pub fn new(matrix: Box<dyn Matrix>, store: Store, state: State, deps: Deps) -> Self {
        Self {
            matrix,
            deps,
            store,
            state,
            statuses: None,
            reported: IndexMap::new(),
            last_poll: None,
            was_afk: false,
            missing: IndexMap::new(),
            unswept: BTreeSet::new(),
            typing: BTreeSet::new(),
            user_id: None,
            profiled: false,
            txns: IndexMap::new(),
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
        herdr::list_agents(self.deps.herdr.as_mut())
    }

    fn read(&mut self, pane: &str, lines: u64) -> Result<String> {
        herdr::read(self.deps.herdr.as_mut(), pane, lines)
    }

    /// The agent this chat talks to; the only one when there is one.
    fn attached<'a>(&self, agents: &'a [Agent]) -> Option<&'a Agent> {
        agents
            .iter()
            .find(|a| Some(&a.pane_id) == self.state.attached.as_ref())
            .or(if agents.len() == 1 {
                agents.first()
            } else {
                None
            })
    }

    fn listing(&self, agents: &[Agent]) -> String {
        let here = self.attached(agents).map(|a| a.pane_id.clone());
        let lines: Vec<String> = agents
            .iter()
            .map(|a| {
                format!(
                    "{} {}  {}{}",
                    if Some(&a.pane_id) == here.as_ref() {
                        ">"
                    } else {
                        " "
                    },
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

    /// The agent `query` names; else asks in the main chat which one, as
    /// `which` says it ("Attach to which"), under the list of them.
    fn pick<'a>(
        &mut self,
        query: &str,
        agents: &'a [Agent],
        which: &str,
    ) -> Result<Option<&'a Agent>> {
        let head = match herdr::find(agents, query) {
            Ok(a) if !query.is_empty() => return Ok(Some(a)),
            _ if query.is_empty() => format!("{which} agent?"),
            Err(0) | Ok(_) => format!("No agent matches \"{query}\". {which} one?"),
            Err(n) => format!("\"{query}\" matches {n} agents. {which} one?"),
        };
        self.ask_which(&head, agents)?;
        Ok(None)
    }

    /// Asks in the main chat which agent, under the list of them.
    fn ask_which(&mut self, head: &str, agents: &[Agent]) -> Result<()> {
        let listing = self.listing(agents);
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

    /// The name and picture until they are set, one sync, and a herdr poll
    /// when one is due, so a homeserver that answers /sync at once does not
    /// spin herdr.
    ///
    /// # Errors
    /// When Matrix or herdr fails.
    pub fn tick(&mut self) -> Result<()> {
        if !self.profiled {
            match self.profile() {
                Ok(()) => self.profiled = true,
                Err(err) => self.log(&format!("error: profile: {err}")),
            }
        }
        self.onboard()?;
        self.sync()?;
        let now = self.now();
        if self
            .last_poll
            .is_none_or(|last| now - last >= POLL_EVERY.as_secs_f64())
        {
            self.last_poll = Some(now);
            self.poll()?;
        }
        Ok(())
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
        for (part, screen) in parts {
            let mut formatted = md(&part);
            if let Some(screen) = &screen {
                let _ = write!(formatted, "<pre>{}</pre>", escape(screen));
            }
            let body: Vec<&str> = [Some(part.as_str()), screen.as_deref()]
                .into_iter()
                .flatten()
                .filter(|x| !x.is_empty())
                .collect();
            let content = json!({"msgtype": if to.quiet { "m.notice" } else { "m.text" },
                   "body": body.join("\n\n"),
                   "format": "org.matrix.custom.html",
                   "formatted_body": formatted});
            let key = format!("{} {content}", to.chat.map_or("", |c| c.room.as_str()));
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

    fn turn(&self, a: &Agent) -> Turn {
        (self.deps.answer)(a).unwrap_or_default()
    }
}

/// What an agent does, as the chat says it.
fn doing(status: &Status) -> String {
    match status {
        Status::Blocked => "waits on you".into(),
        other => other.to_string(),
    }
}
