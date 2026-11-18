//! What the owner sends: messages, files, voice messages and reactions,
//! from the main chat or an agent's own chat.

use std::fmt;
use std::fs;
use std::io::{Read as _, Seek as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use super::contacts::Why;
use super::{
    Bot, FILES_WAIT, FOLLOW_UP, MARK, RETRY_MAX, STALE, SYNC_TURN, Sandbox, To, due, herdr_failed,
    not_running, same_dir,
};
use crate::commands::{KEY_HINT, KEYS, reaction, spoken, typed_key};
use crate::files;
use crate::herdr::{self, Agent, Herdr, Status};
use crate::inbox::{self, Prompt, Upload};
use crate::matrix::{Api as _, As, Inbound, Media, SYNC_WAIT, What};
use crate::media;
use crate::state::{Chat, Origin};
use crate::{Error, Result};

/// What a message from the chat became for its agent.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Delivery {
    Prompt,
    Voice,
    File,
    Key,
    Answer,
}

impl fmt::Display for Delivery {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Self::Prompt => "prompt",
            Self::Voice => "voice",
            Self::File => "file",
            Self::Key => "key",
            Self::Answer => "answer",
        })
    }
}

/// Tries of a queued file the homeserver fails to take
const UPLOAD_TRIES: u32 = 3;
/// The wait before a file the homeserver failed to take is tried again
const UPLOAD_RETRY: Duration = Duration::from_mins(1);

/// Who files from the chat are for: the agent in a pane, from the main chat
/// or from its own chat.
#[derive(Clone, PartialEq, Eq)]
enum Holder {
    Main(String),
    Own(String),
}

impl Holder {
    /// The pane whose own chat they came from; none for the main chat.
    fn contact(&self) -> Option<&str> {
        match self {
            Self::Main(_) => None,
            Self::Own(pane) => Some(pane),
        }
    }
}

/// Files from the chat that wait for more for one agent, their captions,
/// the last event, and when the last came.
pub(super) struct Held {
    to: Holder,
    paths: Vec<PathBuf>,
    captions: Vec<String>,
    id: Option<String>,
    at: f64,
}

/// The messages of the main chat that found no agent, kept together: their
/// files, their text, the last event, and when the last came.
pub(super) struct Kept {
    files: Vec<Media>,
    text: Option<(String, Delivery)>,
    id: Option<String>,
    pub(super) at: f64,
}

/// A message of the owner on its way to an agent, and what kind it is.
#[derive(Clone, Copy)]
struct Message<'a> {
    id: Option<&'a str>,
    text: &'a str,
    kind: Delivery,
    chat: Option<&'a Chat>,
}

impl Bot {
    /// New messages in the chat, then in each agent's own chat; an agent's
    /// contact sees only its own room. The first sync of a chat skips
    /// history. While a turn is under way, the wait is short.
    ///
    /// # Errors
    /// When Matrix or herdr fails.
    pub fn sync(&mut self) -> Result<()> {
        let first = self.state.since.is_none();
        let wait = if self.state.pending.is_empty() {
            SYNC_WAIT
        } else {
            SYNC_TURN
        };
        let resp = self
            .matrix
            .sync(self.state.since.as_deref(), As::Bot, wait)?;
        let moved = !first && self.guard(&resp)?;
        let todo = match &self.state.control {
            Some(room) if !first && !moved => resp.inbound(room, &self.deps.owner),
            _ => Vec::new(),
        };
        let old = self.state.since.replace(resp.next_batch.clone());
        // Saved before acting, so a crash cannot prompt an agent twice;
        // unsaved, the batch comes again
        if (first || resp.has_invites() || !todo.is_empty())
            && let Err(err) = self.save()
        {
            self.state.since = old;
            return Err(err);
        }
        let me = self.me()?;
        self.accept_invites(&resp.invites_from(&self.deps.owner, &me));
        self.act(&todo, None);
        let panes: Vec<String> = self.state.spawned.keys().cloned().collect();
        for pane in panes {
            let Some(chat) = self.state.spawned.get(&pane).cloned() else {
                continue;
            };
            let fresh = chat.since.is_none();
            let mut synced = None;
            self.retry(&format!("sync {}", chat.user), RETRY_MAX, |bot| {
                let since = chat.since.as_deref();
                synced = Some(
                    bot.matrix
                        .sync(since, As::User(&chat.user), Duration::ZERO)?,
                );
                Ok(())
            });
            let Some(resp) = synced else {
                continue;
            };
            if resp.strangers(&chat.room, &[&self.deps.owner, &chat.user]) {
                self.end_chat(&pane, Why::Stranger)?;
                continue;
            }
            let todo = if fresh {
                Vec::new()
            } else {
                resp.inbound(&chat.room, &self.deps.owner)
            };
            if let Some(chat) = self.state.spawned.get_mut(&pane) {
                chat.since = Some(resp.next_batch.clone());
            }
            if (fresh || !todo.is_empty())
                && let Err(err) = self.save()
            {
                if let Some(saved) = self.state.spawned.get_mut(&pane) {
                    saved.since = chat.since;
                }
                return Err(err);
            }
            self.act(&todo, Some(&pane));
        }
        Ok(())
    }

    /// Handles the owner's messages from the chat, or from the own chat of
    /// the agent in `pane`.
    fn act(&mut self, todo: &[Inbound], pane: Option<&str>) {
        let chat = pane.and_then(|p| self.state.spawned.get(p)).cloned();
        let chat = chat.as_ref();
        // What the owner wrote before a long outage may no longer hold
        let now = self.now();
        let fresh: Vec<&Inbound> = todo
            .iter()
            .filter(|ev| now - ev.ts <= STALE.as_secs_f64())
            .collect();
        if fresh.len() < todo.len() {
            let stale = todo.len() - fresh.len();
            self.notice(
                &format!(
                    "Skipped {stale} message{} older than {:.0} minutes. Send again what still holds.",
                    if stale > 1 { "s" } else { "" },
                    STALE.as_secs_f64() / 60.0
                ),
                To::chat(chat),
            );
        }
        for ev in fresh {
            if let Some(pane) = pane
                && !self.state.spawned.contains_key(pane)
            {
                return;
            }
            if !matches!(ev.what, What::File(_)) {
                self.hand_over(|held| held.to.contact() == pane);
            }
            let done = match &ev.what {
                What::Reaction { key, target } => self.react(key, target.as_deref(), pane),
                _ => self.handle(ev, pane),
            };
            if let Err(err) = done {
                let verb = match ev.what {
                    What::Text(_) => "deliver the message",
                    What::File(_) => "take the file",
                    What::Voice(_) => "hear the voice message",
                    What::Reaction { .. } => "answer with the reaction",
                };
                self.failed(verb, &err, chat);
            }
        }
    }

    /// Logs a failure, and says it in the chat with what to do about it.
    fn failed(&mut self, verb: &str, err: &Error, chat: Option<&Chat>) {
        self.log(&format!("error: {err}"));
        let mut said = format!("Failed to {verb}: {err}");
        if let Some(hint) = err.hint() {
            said = format!("{said}. {hint}");
        }
        self.notice(&said, To::chat(chat));
    }

    /// Saves a file from the chat in `dir` and holds it for `to`, beside
    /// the files that came for it before; `id` is its event.
    fn hold(&mut self, to: Holder, media: &Media, id: Option<&str>, dir: &Path) -> Result<()> {
        let path = self.keep(media, dir)?;
        let at = self.now();
        let i = if let Some(i) = self.held_files.iter().position(|h| h.to == to) {
            i
        } else {
            self.held_files.push(Held {
                to,
                paths: Vec::new(),
                captions: Vec::new(),
                id: None,
                at,
            });
            self.held_files.len() - 1
        };
        let held = &mut self.held_files[i];
        held.paths.push(path);
        if media.body != media.name {
            held.captions.push(media.body.clone());
        }
        held.id = id.map(str::to_owned).or(held.id.take());
        held.at = at;
        Ok(())
    }

    /// Hands over the files from the chat for which no more came in
    /// `FILES_WAIT`.
    pub fn hand_over_files(&mut self) {
        let now = self.now();
        self.hand_over(|held| due(now, held.at, FILES_WAIT));
    }

    /// Hands over the held files `due` picks, each agent's as one prompt.
    fn hand_over(&mut self, due: impl Fn(&Held) -> bool) {
        let (now, later) = std::mem::take(&mut self.held_files)
            .into_iter()
            .partition(due);
        self.held_files = later;
        for held in now {
            let chat = held
                .to
                .contact()
                .map(|pane| self.state.spawned.get(pane).cloned().ok_or(()));
            // An own chat that ended takes nothing
            let Ok(chat) = chat.transpose() else {
                continue;
            };
            if let Err(err) = self.hand(&held, chat.as_ref()) {
                self.failed("take the file", &err, chat.as_ref());
            }
        }
    }

    /// Hands held files to their agent; true when they reached it.
    fn hand(&mut self, held: &Held, chat: Option<&Chat>) -> Result<bool> {
        let text = files_prompt(&held.captions, &held.paths);
        let (Holder::Main(pane) | Holder::Own(pane)) = &held.to;
        let name = self
            .listed
            .iter()
            .find(|a| &a.pane_id == pane)
            .map(|a| a.name.clone());
        let agents = self.agents()?;
        let Some(a) = agents.iter().find(|a| &a.pane_id == pane) else {
            self.say(&not_running(name.as_deref()), To::chat(chat))?;
            return Ok(false);
        };
        self.deliver(
            a,
            &agents,
            Message {
                id: held.id.as_deref(),
                text: &text,
                kind: Delivery::File,
                chat,
            },
        )
    }

    /// A message about the batch being handled; one that fails is logged,
    /// so the rest of the batch is still handled.
    fn notice(&mut self, text: &str, to: To) {
        if let Err(err) = self.say(text, to) {
            self.log(&format!("error: notice: {err}"));
        }
    }

    /// A thumbs-up on the message that announced the choice an agent waits
    /// on picks its first option; in an agent's own chat, only that
    /// agent's.
    fn react(&mut self, key: &str, target: Option<&str>, contact: Option<&str>) -> Result<()> {
        let (Some(key), Some(target)) = (reaction(key), target) else {
            return Ok(());
        };
        let Some(pane) = self
            .state
            .asked
            .iter()
            .find(|(_, asked)| asked.event.as_deref() == Some(target))
            .map(|(p, _)| p.clone())
        else {
            return Ok(());
        };
        if contact.is_some_and(|c| c != pane) {
            return Ok(());
        }
        let chat = contact.and_then(|c| self.state.spawned.get(c)).cloned();
        let to = To::chat(chat.as_ref()).pane(&pane);
        let agents = self.agents()?;
        let Some(a) = agents
            .iter()
            .find(|a| a.pane_id == pane && a.status == Status::Blocked)
        else {
            let said = agents.iter().find(|a| a.pane_id == pane).map_or_else(
                || "That agent no longer waits on that choice.".to_owned(),
                |a| format!("**{}** no longer waits on that choice.", a.name),
            );
            self.say(&said, to)?;
            return Ok(());
        };
        let turn = self.turn(a);
        if self.changed(a, &agents, &turn, chat.as_ref())? {
            return Ok(());
        }
        match herdr::send_keys(self.deps.herdr.as_mut(), &pane, key) {
            Err(Error::Herdr(out)) => {
                self.say(&herdr_failed("answer", &a.name, &out), to)?;
                return Ok(());
            }
            other => other?,
        }
        self.log(&format!("{pane} reaction {key}"));
        self.state.asked.shift_remove(&pane);
        self.expect(
            &pane,
            &turn,
            Origin {
                spawned: contact.is_some(),
                ..Origin::default()
            },
        );
        Ok(())
    }

    /// Saves a file from the chat in `dir`, a private directory; its path.
    fn keep(&mut self, media: &Media, dir: &Path) -> Result<std::path::PathBuf> {
        let Some(url) = media.url.as_deref().filter(|u| !u.is_empty()) else {
            return Err(Error::Refused(
                "the file is encrypted, and the bot cannot read encrypted chats".into(),
            ));
        };
        let now = self.now();
        files::store(dir, now, &media.name, &mut |file| {
            self.matrix.download(url, file)
        })
    }

    /// What the owner says in a voice message `id`, as typed; None when
    /// nothing. The message shows read, and the chat typing, while it is
    /// heard.
    fn listen(
        &mut self,
        media: &Media,
        id: Option<&str>,
        chat: Option<&Chat>,
    ) -> Result<Option<String>> {
        if self.deps.transcribe.is_none() {
            self.say(
                "Voice messages are not set up: set transcribe.model.",
                To::chat(chat),
            )?;
            return Ok(None);
        }
        self.mark_read(id, chat)?;
        self.show_typing(chat, true);
        let typed = self.hear(media).and_then(|heard| {
            if heard.is_empty() {
                self.say("Could not understand the voice message.", To::chat(chat))?;
                return Ok(None);
            }
            let typed = spoken(&heard);
            let echo = if typed == heard {
                format!("\u{1F3A4} {heard}")
            } else {
                format!("\u{1F3A4} {heard} \u{2192} {typed}")
            };
            self.say(&echo, To::chat(chat).quiet())?;
            Ok(Some(typed))
        });
        self.show_typing(chat, false);
        typed
    }

    /// What a voice message says.
    fn hear(&mut self, media: &Media) -> Result<String> {
        let dir = self.deps.files.clone();
        let path = self.keep(media, &dir)?;
        let heard = self
            .deps
            .transcribe
            .as_ref()
            .map_or_else(|| Ok(String::new()), |hear| hear(&path));
        if let Err(err) = fs::remove_file(&path)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            self.log(&format!("error: {}: {err}", path.display()));
        }
        heard
    }

    /// Shows the bot, or an agent's contact, typing in its chat, or stops
    /// it; a typing notice of a turn under way stays. A failure is logged.
    fn show_typing(&mut self, chat: Option<&Chat>, on: bool) {
        let key = chat.and_then(|c| {
            self.state
                .spawned
                .iter()
                .find(|(_, x)| x.room == c.room)
                .map(|(pane, _)| pane.clone())
        });
        if !on && self.typing.contains_key(&key) {
            return;
        }
        let shown = match chat {
            Some(c) => Ok(c.user.clone()),
            None => self.me(),
        }
        .and_then(|user| {
            let (room, by) = self.room(chat)?;
            self.matrix.typing(&room, by, &user, on)
        });
        if let Err(err) = shown {
            self.log(&format!("error: typing: {err}"));
        }
    }

    /// A message from the owner in the chat, or in the own chat of the
    /// agent in pane `contact`, where it goes to that agent only.
    fn handle(&mut self, ev: &Inbound, contact: Option<&str>) -> Result<()> {
        let chat = match contact {
            Some(c) => Some(self.state.spawned.get(c).cloned().ok_or(Error::NoChat)?),
            None => None,
        };
        let (text, media) = match &ev.what {
            What::Voice(media) => match self.listen(media, ev.id.as_deref(), chat.as_ref())? {
                Some(text) => (text, None),
                None => return Ok(()),
            },
            What::File(media) => (String::new(), Some(media)),
            What::Text(text) => (text.clone(), None),
            What::Reaction { .. } => return Ok(()),
        };
        let agents = self.agents()?;
        let mine = contact.and_then(|c| agents.iter().find(|a| a.pane_id == c));
        // A space after `!`, trailing punctuation and capitals are slips
        if let Some(command) = text.strip_prefix('!') {
            let command = command.trim_start();
            let (word, rest) = command
                .split_once(char::is_whitespace)
                .unwrap_or((command, ""));
            let word = word
                .trim_end_matches(|c: char| c.is_ascii_punctuation())
                .to_lowercase();
            return self.command(&format!("!{word}"), rest.trim(), &agents, ev, contact, mine);
        }
        if let Some(contact) = contact.filter(|_| mine.is_none()) {
            let name = self.chat_agent_name(contact);
            self.say(&not_running(name.as_deref()), To::chat(chat.as_ref()))?;
            return Ok(());
        }
        let kind = if matches!(ev.what, What::Voice(_)) {
            Delivery::Voice
        } else {
            Delivery::Prompt
        };
        let kept = Kept {
            files: media.into_iter().cloned().collect(),
            text: media.is_none().then(|| (text.clone(), kind)),
            id: ev.id.clone(),
            at: self.now(),
        };
        let Some(a) = mine.map_or_else(|| self.target(ev, &agents, Some(kept)), |a| Ok(Some(a)))?
        else {
            return Ok(());
        };
        if let Some(media) = media {
            let to = match contact {
                Some(_) => Holder::Own(a.pane_id.clone()),
                None => Holder::Main(a.pane_id.clone()),
            };
            let dir = self.files_dir(a);
            return self.hold(to, media, ev.id.as_deref(), &dir);
        }
        self.deliver(
            a,
            &agents,
            Message {
                id: ev.id.as_deref(),
                text: &text,
                kind,
                chat: chat.as_ref(),
            },
        )
        .map(|_| ())
    }

    /// Where files from the chat for `a` go; a confined agent reads only
    /// its own.
    fn files_dir(&self, a: &Agent) -> PathBuf {
        self.sandbox(a)
            .map_or_else(|| self.deps.files.clone(), |s| s.files.clone())
    }

    /// Hands the messages kept for want of an agent to `a`, the agent now
    /// attached, unless they are too old: their files with their text as
    /// one prompt, else their text.
    pub(super) fn send_kept(&mut self, a: &Agent, agents: &[Agent]) -> Result<()> {
        let Some(kept) = self.kept.take() else {
            return Ok(());
        };
        if due(self.now(), kept.at, FOLLOW_UP) {
            return Ok(());
        }
        let sent = if kept.files.is_empty() {
            match &kept.text {
                Some((text, kind)) => self.deliver(
                    a,
                    agents,
                    Message {
                        id: kept.id.as_deref(),
                        text,
                        kind: *kind,
                        chat: None,
                    },
                )?,
                None => false,
            }
        } else {
            let to = Holder::Main(a.pane_id.clone());
            let dir = self.files_dir(a);
            for media in &kept.files {
                self.hold(to.clone(), media, kept.id.as_deref(), &dir)?;
            }
            let i = self.held_files.iter().position(|h| h.to == to);
            match i.map(|i| self.held_files.remove(i)) {
                Some(mut held) => {
                    held.captions.extend(kept.text.map(|(text, _)| text));
                    self.hand(&held, None)?
                }
                None => false,
            }
        };
        if sent {
            let said = format!("Sent your message to **{}**.", a.name);
            self.say(&said, To::default().quiet())?;
        }
        Ok(())
    }

    /// Keeps a message that found no agent with those kept before, unless
    /// they are too old; true when some were.
    fn keep_unsent(&mut self, kept: Kept) -> bool {
        let now = self.now();
        let Some(old) = self.kept.as_mut().filter(|k| !due(now, k.at, FOLLOW_UP)) else {
            self.kept = Some(kept);
            return false;
        };
        old.files.extend(kept.files);
        old.text = match (old.text.take(), kept.text) {
            (Some((before, _)), Some((text, kind))) => Some((format!("{before}\n\n{text}"), kind)),
            (before, text) => text.or(before),
        };
        old.id = kept.id.or(old.id.take());
        old.at = kept.at;
        true
    }

    /// The files held for the own chat in pane `from` are for the one in
    /// pane `to`.
    pub(super) fn move_held(&mut self, from: &str, to: &str) {
        for held in &mut self.held_files {
            if held.to.contact() == Some(from) {
                held.to = Holder::Own(to.to_owned());
            }
        }
    }

    /// Hands the prompts `herdr-matrix prompt` queued to their agents, and
    /// posts the files `herdr-matrix send` queued. Nothing is taken while
    /// herdr cannot list the agents. A prompt herdr failed to send, and a
    /// file the homeserver failed to take, is queued again.
    ///
    /// # Errors
    /// When a queue directory cannot be read, or herdr cannot list the
    /// agents.
    pub fn local(&mut self) -> Result<()> {
        let sandboxes = self.deps.agents.iter().filter_map(|s| s.sandbox.as_ref());
        let queues: Vec<&PathBuf> = [&self.deps.inbox, &self.deps.uploads]
            .into_iter()
            .flatten()
            .chain(sandboxes.map(|s| &s.uploads))
            .collect();
        if !queues.into_iter().any(|dir| inbox::queued(dir)) {
            return Ok(());
        }
        let agents = self.agents()?;
        let prompts = match self.deps.inbox.clone() {
            Some(dir) => inbox::take(&dir, &*self.deps.log)?,
            None => Vec::new(),
        };
        let mut uploads: Vec<(Upload, PathBuf, Option<String>)> = match self.deps.uploads.clone() {
            Some(dir) => inbox::take_uploads(&dir, self.now(), &*self.deps.log)?,
            None => Vec::new(),
        }
        .into_iter()
        .map(|(upload, data)| (upload, data, None))
        .collect();
        uploads.extend(self.confined_uploads());
        for (upload, data, cwd) in &uploads {
            let again = match cwd {
                Some(cwd) => self.confined_upload(upload, data, cwd, &agents),
                None => self.local_upload(upload, data, &agents),
            };
            if again {
                match inbox::requeue(upload, data) {
                    Ok(()) => continue,
                    Err(err) => self.log(&format!("error: {}: {err}", data.display())),
                }
            }
            if let Err(err) = fs::remove_file(data)
                && err.kind() != std::io::ErrorKind::NotFound
            {
                self.log(&format!("error: {}: {err}", data.display()));
            }
        }
        for prompt in &prompts {
            if let Err(err) = self.local_prompt(prompt, &agents) {
                self.log(&format!("error: prompts: {err}"));
            }
        }
        Ok(())
    }

    /// The files the confined agents queued, each with the working
    /// directory of its agent. A queue that cannot be read holds nothing.
    fn confined_uploads(&mut self) -> Vec<(Upload, PathBuf, Option<String>)> {
        let mut found = Vec::new();
        let confined: Vec<(String, Sandbox)> = self
            .deps
            .agents
            .iter()
            .filter_map(|s| Some((s.cwd.clone(), s.sandbox.clone()?)))
            .collect();
        for (cwd, sandbox) in confined {
            self.retry(
                &format!("uploads of {}", sandbox.id),
                Duration::ZERO,
                |bot| {
                    let taken = inbox::take_uploads(&sandbox.uploads, bot.now(), &*bot.deps.log)?;
                    found.extend(
                        taken
                            .into_iter()
                            .map(|(upload, data)| (upload, data, Some(cwd.clone()))),
                    );
                    Ok(())
                },
            );
        }
        found
    }

    /// Posts a file a confined agent queued, as the one agent working in
    /// `cwd`: the queue names the agent, never the entry. True when it is
    /// to be tried again.
    fn confined_upload(
        &mut self,
        upload: &Upload,
        data: &Path,
        cwd: &str,
        agents: &[Agent],
    ) -> bool {
        let working: Vec<&Agent> = agents
            .iter()
            .filter(|a| a.cwd.as_deref().is_some_and(|dir| same_dir(dir, cwd)))
            .collect();
        let [a] = working.as_slice() else {
            self.log(&format!(
                "upload for \"{cwd}\" dropped: it matches {} agents",
                working.len()
            ));
            return false;
        };
        let upload = Upload {
            pane: a.pane_id.clone(),
            ..upload.clone()
        };
        self.local_upload(&upload, data, agents)
    }

    /// Posts a file an agent queued in its own chat, else in the main chat;
    /// one that fails is logged and said there. One the homeserver failed
    /// to take is tried `UPLOAD_TRIES` times, `UPLOAD_RETRY` apart, while it
    /// is not too old, and
    /// said when it is dropped. True when it is to be tried again.
    fn local_upload(&mut self, upload: &Upload, data: &Path, agents: &[Agent]) -> bool {
        let pane = upload.pane.as_str();
        let dropped = |why: &str| format!("upload for \"{pane}\" dropped: {why}");
        let failed = self.failed_uploads.shift_remove(data);
        let chat = self.state.spawned.get(pane).cloned();
        let said = |err: &dyn fmt::Display| format!("Could not send {}: {err}", upload.name);
        if self.now() - upload.at > STALE.as_secs_f64() {
            let minutes = STALE.as_secs_f64() / 60.0;
            self.log(&dropped(&format!("older than {minutes:.0} minutes")));
            if let Some((err, ..)) = failed {
                self.notice(&said(&err), To::chat(chat.as_ref()).quiet());
            }
            return false;
        }
        if let Some(waiting) = failed.clone()
            && !super::due(self.now(), waiting.2, UPLOAD_RETRY)
        {
            self.failed_uploads.insert(data.to_owned(), waiting);
            return true;
        }
        if !agents.iter().any(|a| a.pane_id == pane) {
            self.log(&dropped("no agent works there"));
            return false;
        }
        match self.post_file(upload, data, chat.as_ref()) {
            Ok(len) => self.log(&format!("{pane} upload {len}")),
            Err(err) => {
                self.log(&format!("error: {pane} upload: {err}"));
                let tries = failed.map_or(0, |(_, tries, _)| tries) + 1;
                if matches!(err, Error::Transport(_) | Error::Http { status: 500.., .. })
                    && tries < UPLOAD_TRIES
                {
                    let failed = (err.to_string(), tries, self.now());
                    self.failed_uploads.insert(data.to_owned(), failed);
                    return true;
                }
                self.notice(&said(&err), To::chat(chat.as_ref()).quiet());
            }
        }
        false
    }

    /// Uploads a queued file within the homeserver's limit, asked once per
    /// run, and posts it so that a reply to it reaches the agent; its size.
    fn post_file(&mut self, upload: &Upload, data: &Path, chat: Option<&Chat>) -> Result<u64> {
        let mut file = files::open_inside(data.parent().unwrap_or(data), data)?;
        let len = file.metadata()?.len();
        let limit = match self.upload_limit {
            Some(limit) => limit,
            None => *self
                .upload_limit
                .insert(self.matrix.upload_limit()?.unwrap_or(u64::MAX)),
        };
        if len > limit {
            return Err(Error::Refused(format!(
                "it is {}, and the homeserver takes {} at most",
                megabytes(len, true),
                megabytes(limit, false)
            )));
        }
        let mut head = Vec::new();
        (&mut file)
            .take(media::HEAD as u64)
            .read_to_end(&mut head)?;
        file.rewind()?;
        let kind = media::kind(&upload.name, &head);
        let url = self.matrix.upload(&mut file, len, kind.mimetype)?;
        let mut info = json!({"mimetype": kind.mimetype, "size": len});
        if let Some((w, h)) = kind.size {
            info["w"] = json!(w);
            info["h"] = json!(h);
        }
        let content = json!({"msgtype": kind.msgtype,
            "body": upload.caption.as_deref().unwrap_or(&upload.name),
            "filename": upload.name, "url": url, "info": info});
        let sent = self.send(&content, Some(&upload.pane), chat, &url);
        self.txns.shift_remove(&url);
        sent?;
        Ok(len)
    }

    fn local_prompt(&mut self, prompt: &Prompt, agents: &[Agent]) -> Result<()> {
        let to = prompt.cwd.as_deref().unwrap_or(&prompt.agent);
        let dropped = |why: String| format!("prompt for \"{to}\" dropped: {why}");
        if self.now() - prompt.at > STALE.as_secs_f64() {
            let minutes = STALE.as_secs_f64() / 60.0;
            self.log(&dropped(format!("older than {minutes:.0} minutes")));
            return Ok(());
        }
        let a = match addressee(prompt, agents) {
            Ok(a) => a,
            Err(n) => {
                self.log(&dropped(format!("it matches {n} agents")));
                return Ok(());
            }
        };
        let pane = a.pane_id.as_str();
        let busy = match a.status {
            Status::Blocked => Some("waits on a choice"),
            Status::Working => Some("works"),
            _ => None,
        };
        if let Some(busy) = busy {
            self.log(&format!("{pane} prompt dropped: it {busy}"));
            return Ok(());
        }
        let turn = self.turn(a);
        if let Err(err) = herdr::prompt(self.deps.herdr.as_mut(), pane, &prompt.text) {
            self.log(&format!("error: {pane} prompt: {err}"));
            if let Some(dir) = &self.deps.inbox {
                inbox::queue(dir, prompt)?;
            }
            return Ok(());
        }
        self.log(&format!("{pane} local {}", prompt.text.chars().count()));
        self.state.asked.shift_remove(pane);
        let spawned = self.state.spawned.contains_key(pane);
        let saved = self.expect(
            pane,
            &turn,
            Origin {
                slash: prompt.text.starts_with('/'),
                spawned,
                ..Origin::default()
            },
        );
        self.waiting(pane).local = true;
        if saved { self.save() } else { Ok(()) }
    }

    /// The agent of the message replied to, else the attached one; else
    /// None, once the chat says why. A message, `kept`, that finds no agent
    /// attached waits for the next `!attach`, with those kept before it.
    pub(super) fn target<'a>(
        &mut self,
        ev: &Inbound,
        agents: &'a [Agent],
        kept: Option<Kept>,
    ) -> Result<Option<&'a Agent>> {
        let replied = ev.reply_to.as_ref().and_then(|q| self.state.sent.get(q));
        let Some(pane) = replied.cloned() else {
            if let Some(a) = self.attached(agents) {
                return Ok(Some(a));
            }
            if self.state.attached.is_some() {
                let said = not_running(self.attached_name());
                self.say(&said, To::default())?;
                return Ok(None);
            }
            let keeping = kept.is_some();
            if let Some(kept) = kept
                && self.keep_unsent(kept)
            {
                let said = "Kept with the rest; !attach an agent and I send it.";
                self.say(said, To::default().quiet())?;
                return Ok(None);
            }
            let head = match (agents.is_empty(), keeping) {
                (true, true) => {
                    "No agent is running, so nothing was sent. Start one with !new <folder>."
                }
                (true, false) => "No agent is running. Start one with !new <folder>.",
                (false, true) => {
                    "Nothing was sent: no agent is attached. !attach one and I send it:"
                }
                (false, false) => "No agent is attached. !attach one first:",
            };
            if agents.is_empty() {
                self.say(head, To::default())?;
            } else {
                self.ask_which(head, agents)?;
            }
            return Ok(None);
        };
        if let Some(a) = agents.iter().find(|a| a.pane_id == pane) {
            return Ok(Some(a));
        }
        self.say(&not_running(None), To::default())?;
        Ok(None)
    }

    /// Hands a message to an agent: a prompt, or while it waits on a
    /// choice, a key or a written answer to that choice. A choice on screen
    /// takes a key, written as a key or a word for one; a question takes
    /// any other answer as written. True when it reached the agent.
    fn deliver(&mut self, a: &Agent, agents: &[Agent], message: Message) -> Result<bool> {
        let Message {
            id,
            text,
            mut kind,
            chat,
        } = message;
        let pane = a.pane_id.as_str();
        let turn = self.turn(a);
        let mut key = text.to_lowercase();
        let mut written = None;
        if a.status == Status::Blocked {
            if self.changed(a, agents, &turn, chat)? {
                return Ok(false);
            }
            let asked = turn
                .question
                .as_ref()
                .map(|q| q.list.as_slice())
                .unwrap_or_default();
            let pressed = if asked.is_empty() {
                typed_key(text)
            } else {
                KEYS.iter().find(|k| **k == key).copied()
            };
            if let Some(pressed) = pressed {
                key = pressed.to_owned();
                kind = Delivery::Key;
            } else if let [q] = asked
                && !q.multi_select
            {
                // The entry after the options takes a written answer
                written = Some((q.options.len() + 1).to_string());
                kind = Delivery::Answer;
            } else {
                self.say(
                    &format!(
                        "**{}** waits on a choice. Answer with one key: {KEY_HINT}.",
                        a.name
                    ),
                    To::chat(chat).pane(pane),
                )?;
                return Ok(false);
            }
        }
        let herdr = self.deps.herdr.as_mut();
        let sent = match (kind, written) {
            (Delivery::Key, _) => herdr::send_keys(herdr, pane, &key),
            (Delivery::Answer, Some(entry)) => write_answer(herdr, pane, &entry, text),
            _ => {
                // Tells the agent where the owner reads; a slash command stays bare
                let marked = if text.starts_with('/') {
                    text.to_owned()
                } else {
                    format!("{MARK} {text}")
                };
                herdr::prompt(herdr, pane, &marked)
            }
        };
        match sent {
            Err(Error::Herdr(out)) => {
                let said = herdr_failed("send to", &a.name, &out);
                self.say(&said, To::chat(chat).pane(pane))?;
                return Ok(false);
            }
            other => other?,
        }
        self.log(&format!("{pane} {kind} {}", text.chars().count()));
        // Keys may walk a choice that stays on screen
        if kind != Delivery::Key {
            self.state.asked.shift_remove(pane);
        }
        let origin = Origin {
            slash: text.starts_with('/'),
            spawned: chat.is_some(),
            ..Origin::default()
        };
        self.expect(pane, &turn, origin);
        self.mark_read(id, chat)?;
        Ok(true)
    }

    /// A read receipt tells the owner the agent has the message.
    fn mark_read(&mut self, id: Option<&str>, chat: Option<&Chat>) -> Result<()> {
        let Some(id) = id else {
            return Ok(());
        };
        let (room, by) = self.room(chat)?;
        match self.matrix.receipt(&room, by, id) {
            Ok(()) => Ok(()),
            Err(err @ (Error::Transport(_) | Error::Http { .. })) => {
                self.log(&format!("error: receipt: {err}"));
                Ok(())
            }
            Err(err) => Err(err),
        }
    }
}

/// The agent a prompt from the machine is for: the one its `cwd` names,
/// a trailing `/` aside, else the one its `agent` names; else how many
/// match.
fn addressee<'a>(prompt: &Prompt, agents: &'a [Agent]) -> std::result::Result<&'a Agent, usize> {
    let Some(dir) = prompt.cwd.as_deref() else {
        return if prompt.agent.is_empty() {
            Err(agents.len())
        } else {
            herdr::find(agents, &prompt.agent)
        };
    };
    let matches: Vec<&Agent> = agents
        .iter()
        .filter(|a| a.cwd.as_deref().is_some_and(|cwd| same_dir(cwd, dir)))
        .collect();
    if let [a] = matches[..] {
        Ok(a)
    } else {
        Err(matches.len())
    }
}

/// The prompt for files from the chat: one as its caption, else "See the
/// file", and its path; several as their captions, then one path a line.
fn files_prompt(captions: &[String], paths: &[PathBuf]) -> String {
    let paths: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    let head = match (captions.is_empty(), paths.as_slice()) {
        (true, [path]) => return format!("See the file {path}"),
        (true, _) => "See the files".to_owned(),
        (false, _) => captions.join("\n\n"),
    };
    format!("{head}\n\n{}", paths.join("\n"))
}

/// Bytes in megabytes with one decimal, rounded up or down.
fn megabytes(bytes: u64, up: bool) -> String {
    let mb = 1 << 20;
    let tenths = (u128::from(bytes) * 10 + if up { mb - 1 } else { 0 }) / mb;
    format!("{}.{} MB", tenths / 10, tenths % 10)
}

/// Picks the entry of a question that takes a written answer, and types
/// it. A written answer typed in part would wait in the field; it is
/// cancelled.
fn write_answer(herdr: &mut dyn Herdr, pane: &str, entry: &str, text: &str) -> Result<()> {
    herdr::send_keys(herdr, pane, entry)?;
    let typed =
        herdr::send_text(herdr, pane, text).and_then(|()| herdr::send_keys(herdr, pane, "enter"));
    if typed.is_err() {
        match herdr::send_keys(herdr, pane, "esc") {
            Ok(()) | Err(Error::Herdr(_)) => {}
            Err(err) => return Err(err),
        }
    }
    typed
}
