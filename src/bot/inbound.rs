//! What the owner sends: messages, files, voice messages and reactions,
//! from the main chat or an agent's own chat.

use std::fmt;
use std::fs;
use std::time::Duration;

use super::{Bot, MARK, STALE, To};
use crate::commands::{KEY_HINT, KEYS, reaction, spoken};
use crate::files;
use crate::herdr::{self, Agent, Herdr, Status};
use crate::matrix::{Api as _, As, Inbound, Media, SYNC_WAIT, What};
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
    /// history.
    ///
    /// # Errors
    /// When Matrix or herdr fails.
    pub fn sync(&mut self) -> Result<()> {
        let first = self.state.since.is_none();
        let resp = self
            .matrix
            .sync(self.state.since.as_deref(), As::Bot, SYNC_WAIT)?;
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
            let resp =
                match self
                    .matrix
                    .sync(chat.since.as_deref(), As::User(&chat.user), Duration::ZERO)
                {
                    Ok(resp) => resp,
                    Err(err) => {
                        self.log(&format!("error: sync {}: {}", chat.user, err));
                        continue;
                    }
                };
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
            let done = match &ev.what {
                What::Reaction { key, target } => self.react(key, target.as_deref(), pane),
                _ => self.handle(ev, pane),
            };
            if let Err(err) = done {
                self.log(&format!("error: {err}"));
                let verb = match ev.what {
                    What::Text(_) => "deliver the message",
                    What::File(_) => "take the file",
                    What::Voice(_) => "hear the voice message",
                    What::Reaction { .. } => "answer with the reaction",
                };
                let mut said = format!("Failed to {verb}: {err}");
                if let Some(hint) = err.hint() {
                    said = format!("{said}. {hint}");
                }
                self.notice(&said, To::chat(chat));
            }
        }
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
            let name = agents
                .iter()
                .find(|a| a.pane_id == pane)
                .map_or(pane.as_str(), |a| a.name.as_str());
            let said = format!("**{name}** no longer waits on that choice.");
            self.say(&said, to)?;
            return Ok(());
        };
        let turn = self.turn(a);
        if self.changed(a, &agents, &turn, chat.as_ref())? {
            return Ok(());
        }
        match herdr::send_keys(self.deps.herdr.as_mut(), &pane, key) {
            Err(Error::Herdr(out)) => {
                self.say(&format!("Could not answer **{}**: {out}", a.name), to)?;
                return Ok(());
            }
            other => other?,
        }
        self.log(&format!("{pane} reaction {key}"));
        self.state.asked.shift_remove(&pane);
        self.expect(&pane, &turn, Origin::default())
    }

    /// Saves a picture or file from the chat; the prompt that hands it to
    /// an agent.
    fn fetch(&mut self, media: &Media) -> Result<String> {
        let path = self.keep(media)?;
        let path = path.display();
        Ok(if media.body == media.name {
            format!("See the file {path}")
        } else {
            format!("{}\n\n{path}", media.body)
        })
    }

    /// Saves a file from the chat in the private directory; its path.
    fn keep(&mut self, media: &Media) -> Result<std::path::PathBuf> {
        let Some(url) = media.url.as_deref().filter(|u| !u.is_empty()) else {
            return Err(Error::Refused(
                "the file is encrypted, and the bot cannot read encrypted chats".into(),
            ));
        };
        let data = self.matrix.download(url)?;
        files::store(&self.deps.files, self.now(), &media.name, &data)
    }

    /// What the owner says in a voice message, as typed; None when nothing.
    fn listen(&mut self, media: &Media, chat: Option<&Chat>) -> Result<Option<String>> {
        if self.deps.transcribe.is_none() {
            self.say(
                "Voice messages are not set up: set transcribe.model.",
                To::chat(chat),
            )?;
            return Ok(None);
        }
        let path = self.keep(media)?;
        let heard = self.deps.transcribe.as_ref().map(|hear| hear(&path));
        if let Err(err) = fs::remove_file(&path)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            self.log(&format!("error: {}: {err}", path.display()));
        }
        let Some(heard) = heard.transpose()? else {
            return Ok(None);
        };
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
    }

    /// A message from the owner in the chat, or in the own chat of the
    /// agent in pane `contact`, where it goes to that agent only.
    fn handle(&mut self, ev: &Inbound, contact: Option<&str>) -> Result<()> {
        let chat = match contact {
            Some(c) => Some(self.state.spawned.get(c).cloned().ok_or(Error::NoChat)?),
            None => None,
        };
        let (text, media) = match &ev.what {
            What::Voice(media) => match self.listen(media, chat.as_ref())? {
                Some(text) => (text, None),
                None => return Ok(()),
            },
            What::File(media) => (String::new(), Some(media)),
            What::Text(text) => (text.clone(), None),
            What::Reaction { .. } => return Ok(()),
        };
        let agents = self.agents()?;
        let mine = contact.and_then(|c| agents.iter().find(|a| a.pane_id == c));
        if contact.is_some() && mine.is_none() {
            self.say(
                "The agent is not running right now; nothing was sent.",
                To::chat(chat.as_ref()),
            )?;
            return Ok(());
        }
        let (word, rest) = text.split_once(' ').unwrap_or((&text, ""));
        if word.starts_with('!') {
            return self.command(
                &word.to_lowercase(),
                rest.trim(),
                &agents,
                ev,
                contact,
                mine,
            );
        }
        let Some(a) = mine.or_else(|| self.target(ev, &agents)) else {
            return self.ask_which("Not attached. !attach one of:", &agents);
        };
        let voice = matches!(ev.what, What::Voice(_));
        let (text, kind) = match media {
            Some(media) => (self.fetch(media)?, Delivery::File),
            None if voice => (text, Delivery::Voice),
            None => (text, Delivery::Prompt),
        };
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
    }

    /// The agent of the message replied to, else the attached one.
    pub(super) fn target<'a>(&self, ev: &Inbound, agents: &'a [Agent]) -> Option<&'a Agent> {
        let pane = ev.reply_to.as_ref().and_then(|q| self.state.sent.get(q));
        agents
            .iter()
            .find(|a| Some(&a.pane_id) == pane)
            .or_else(|| self.attached(agents))
    }

    /// Hands a message to an agent: a prompt, or while it waits on a
    /// choice, a key or a written answer to that choice.
    fn deliver(&mut self, a: &Agent, agents: &[Agent], message: Message) -> Result<()> {
        let Message {
            id,
            text,
            mut kind,
            chat,
        } = message;
        let pane = a.pane_id.as_str();
        let turn = self.turn(a);
        let lower = text.to_lowercase();
        let mut written = None;
        if a.status == Status::Blocked {
            if self.changed(a, agents, &turn, chat)? {
                return Ok(());
            }
            let asked = turn
                .question
                .as_ref()
                .map(|q| q.list.as_slice())
                .unwrap_or_default();
            if KEYS.contains(&lower.as_str()) {
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
                return Ok(());
            }
        }
        let herdr = self.deps.herdr.as_mut();
        let sent = match (kind, written) {
            (Delivery::Key, _) => herdr::send_keys(herdr, pane, &lower),
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
                let said = format!("Could not send to **{}**: {out}", a.name);
                self.say(&said, To::chat(chat).pane(pane))?;
                return Ok(());
            }
            other => other?,
        }
        self.log(&format!("{pane} {kind} {}", text.chars().count()));
        // Keys may walk a choice that stays on screen
        if kind != Delivery::Key {
            self.state.asked.shift_remove(pane);
        }
        self.expect(
            pane,
            &turn,
            Origin {
                slash: text.starts_with('/'),
                spawned: chat.is_some(),
                ..Origin::default()
            },
        )?;
        self.mark_read(id, chat)
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
