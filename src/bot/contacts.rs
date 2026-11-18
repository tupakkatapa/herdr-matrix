//! An agent's own chat: a user of the appservice in a direct chat with the
//! owner.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;

use super::{AgentSettings, Bot, RETRY_MAX, To, same_dir};
use crate::commands::{Place, contact, helps};
use crate::herdr::Agent;
use crate::matrix::{Api as _, As};
use crate::state::{Chat, sha256_hex};
use crate::{Error, Result};

/// Why an agent's own chat ends: the owner asked, the agent is gone, its
/// settings are gone, the owner changed, or someone else is in it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Why {
    Asked,
    Ended,
    Unconfigured,
    Owner,
    Stranger,
}

impl Why {
    fn reason(self) -> &'static str {
        match self {
            Self::Asked => "asked",
            Self::Ended => "gone",
            Self::Unconfigured => "unconfigured",
            Self::Owner => "owner changed",
            Self::Stranger => "someone else is in it",
        }
    }
}

impl Bot {
    /// Gives an agent its own chat: a Matrix user of the appservice, named
    /// after the agent, in a direct chat with the owner. The chat of the
    /// configured agent of its folder whose pane is gone goes on with it.
    pub(super) fn open_chat(
        &mut self,
        query: &str,
        agents: &[Agent],
        replied: Option<&str>,
    ) -> Result<()> {
        let Some(a) = self.pick(query, agents, "Open a chat with which", replied)? else {
            return Ok(());
        };
        if let Some(chat) = self.state.spawned.get(&a.pane_id) {
            let text = format!("**{}** has a chat already: {}.", a.name, chat.user);
            self.say(&text, To::default())?;
            return Ok(());
        }
        let cwd = self.agent_settings(a).map(|s| s.cwd.clone());
        let dormant = self
            .state
            .spawned
            .iter()
            .filter(|(p, _)| !agents.iter().any(|b| &b.pane_id == *p))
            .find(|(_, c)| {
                c.agent
                    .as_deref()
                    .zip(cwd.as_deref())
                    .is_some_and(|(x, y)| same_dir(x, y))
            })
            .map(|(p, _)| p.clone());
        let Some(dormant) = dormant else {
            return self.open_chat_for(a);
        };
        self.move_chat(&dormant, a)?;
        let chat = self.state.spawned[&a.pane_id].clone();
        let again = format!(
            "This chat is {}'s own again: what you write here goes to it alone.",
            a.name
        );
        self.say(&again, To::chat(Some(&chat)))?;
        let said = format!("**{}** has its own chat again: {}.", a.name, chat.user);
        self.say(&said, To::default())?;
        Ok(())
    }

    /// Moves the own chat in pane `from`, which is gone, to the agent `a`,
    /// with the main chat when it is attached to `from`. The turn and the
    /// choice of `from` end; the files held for it and its typing notice
    /// go to `a`.
    pub(super) fn move_chat(&mut self, from: &str, a: &Agent) -> Result<()> {
        let to = a.pane_id.clone();
        let Some(chat) = self.state.spawned.shift_remove(from) else {
            return Ok(());
        };
        self.forget_retries(from, &chat.user);
        self.state.spawned.insert(to.clone(), chat);
        self.missing.shift_remove(from);
        self.state.pending.shift_remove(from);
        self.state.asked.shift_remove(from);
        if self.state.attached.as_deref() == Some(from) {
            self.state.attached = Some(to.clone());
            self.attached_name = Some((to.clone(), a.name.clone()));
            self.attached_missing = None;
            self.nicked = false;
        }
        if let Some(at) = self.typing.remove(&Some(from.to_owned())) {
            self.typing.insert(Some(to.clone()), at);
        }
        self.move_held(from, &to);
        self.save()?;
        self.log(&format!("{from} chat moved to {to}"));
        Ok(())
    }

    /// True when the bot started the agent in `pane` in this run, for the
    /// settings of `cwd`.
    pub(super) fn started_in(&self, pane: &str, cwd: &str) -> bool {
        self.started_for
            .get(pane)
            .is_some_and(|(started, _)| same_dir(started, cwd))
    }

    /// Forgets the failed steps of the own chat in `pane` of `user`.
    fn forget_retries(&mut self, pane: &str, user: &str) {
        for label in [
            format!("chat {pane}"),
            format!("rename {user}"),
            format!("picture {user}"),
            format!("sync {user}"),
        ] {
            self.retries.shift_remove(&label);
        }
    }

    /// The name of the configured agent whose own chat is in `pane`: its
    /// display name, else its folder's.
    pub(super) fn chat_agent_name(&self, pane: &str) -> Option<String> {
        let cwd = self.state.spawned.get(pane)?.agent.as_deref()?;
        let settings = self.deps.agents.iter().find(|s| same_dir(&s.cwd, cwd))?;
        let folder = cwd.trim_end_matches('/').rsplit('/').next();
        settings.name.clone().or_else(|| folder.map(str::to_owned))
    }

    /// Opens the own chat of `a`.
    fn open_chat_for(&mut self, a: &Agent) -> Result<()> {
        let user = self.contact_user(&a.name)?;
        let display = self.contact_name(a);
        let avatar = self.register(&user, &display, self.contact_avatar(a))?;
        let since = self
            .matrix
            .sync(None, As::User(&user), Duration::ZERO)?
            .next_batch;
        let room = self.matrix.create_dm(As::User(&user), &self.deps.owner)?;
        let chat = Chat {
            user: user.clone(),
            room,
            since: Some(since),
            name: Some(display),
            avatar,
            agent: self.started_for.get(&a.pane_id).map_or_else(
                || self.agent_settings(a).map(|s| s.cwd.clone()),
                |(cwd, _)| Some(cwd.clone()),
            ),
            ..Chat::default()
        };
        if let Some(cwd) = &chat.agent {
            self.state.left.remove(cwd);
        }
        self.state.spawned.insert(a.pane_id.clone(), chat.clone());
        self.save()?;
        let help = format!("{}\n\n{}", contact(&a.name), helps(Place::Own));
        self.say(&help, To::chat(Some(&chat)))?;
        self.say(
            &format!(
                "**{}** has its own chat now: {user}. Accept its invite.",
                a.name
            ),
            To::default(),
        )?;
        Ok(())
    }

    /// Opens the own chat of each configured agent with `chat` that has
    /// none, unless the owner closed it: in the one pane the bot started
    /// for it.
    pub(super) fn open_configured_chats(&mut self, agents: &[Agent]) {
        let due: Vec<&Agent> = self
            .deps
            .agents
            .iter()
            .filter(|s| s.chat && !self.state.left.contains(&s.cwd))
            .filter(|s| {
                !self
                    .state
                    .spawned
                    .values()
                    .any(|c| c.agent.as_deref().is_some_and(|cwd| same_dir(cwd, &s.cwd)))
            })
            .filter_map(|s| {
                let mut there = agents
                    .iter()
                    .filter(|a| self.started_in(&a.pane_id, &s.cwd));
                there.next().filter(|_| there.next().is_none())
            })
            .filter(|a| !self.state.spawned.contains_key(&a.pane_id))
            .collect();
        for a in due {
            self.retry(&format!("chat {}", a.pane_id), RETRY_MAX, |bot| {
                bot.open_chat_for(a)
            });
        }
    }

    /// The settings of the agent working in the directory of `a`.
    pub(super) fn agent_settings(&self, a: &Agent) -> Option<&AgentSettings> {
        let cwd = a.cwd.as_deref()?;
        self.deps.agents.iter().find(|s| same_dir(&s.cwd, cwd))
    }

    /// True when settings name the agent working in `cwd`.
    pub(super) fn configured(&self, cwd: &str) -> bool {
        self.deps.agents.iter().any(|s| same_dir(&s.cwd, cwd))
    }

    /// The display name of an agent's contact: the bot's name and the
    /// agent's, or the agent's alone.
    fn contact_name(&self, a: &Agent) -> String {
        let settings = self.agent_settings(a);
        let agent = settings.and_then(|s| s.name.as_deref()).unwrap_or(&a.name);
        let prefix = settings
            .and_then(|s| s.prefix)
            .unwrap_or(self.deps.spawn_prefix);
        match &self.deps.name {
            Some(name) if prefix => format!("{name} · {agent}"),
            _ => agent.to_owned(),
        }
    }

    /// The picture of an agent's contact, if any.
    fn contact_avatar(&self, a: &Agent) -> Option<PathBuf> {
        self.agent_settings(a)
            .and_then(|s| s.avatar.clone())
            .or_else(|| self.deps.spawn_avatar.clone())
            .filter(|p| !p.as_os_str().is_empty())
    }

    /// Gives each contact the name and picture its agent has now; one that
    /// fails is tried again later.
    pub(super) fn rename_contacts(&mut self, agents: &[Agent]) {
        for a in agents {
            let Some(chat) = self.state.spawned.get(&a.pane_id) else {
                continue;
            };
            let user = chat.user.clone();
            let name = self.contact_name(a);
            if chat.name.as_deref() != Some(name.as_str()) {
                self.retry(&format!("rename {user}"), RETRY_MAX, |bot| {
                    bot.rename(&a.pane_id, &user, name)
                });
            }
            self.retry(&format!("picture {user}"), RETRY_MAX, |bot| {
                bot.repaint(a, &user)
            });
        }
    }

    /// Sets the picture of an agent's contact when it differs from the one
    /// last set; each picture is read once per run.
    fn repaint(&mut self, a: &Agent, user: &str) -> Result<()> {
        let Some(path) = self.contact_avatar(a) else {
            return Ok(());
        };
        let want = if let Some(digest) = self.digests.get(&path) {
            digest.clone()
        } else {
            let digest = sha256_hex(&fs::read(&path)?);
            self.digests.insert(path.clone(), digest.clone());
            digest
        };
        let Some(chat) = self.state.spawned.get(&a.pane_id) else {
            return Ok(());
        };
        if chat.avatar.as_deref() == Some(want.as_str()) {
            return Ok(());
        }
        let digest = self.set_picture(user, &path)?;
        if let Some(chat) = self.state.spawned.get_mut(&a.pane_id) {
            chat.avatar = Some(digest);
        }
        self.save()
    }

    /// Sets a contact's picture; its sha256.
    fn set_picture(&mut self, user: &str, path: &Path) -> Result<String> {
        let data = fs::read(path)?;
        let digest = sha256_hex(&data);
        self.digests.insert(path.to_owned(), digest.clone());
        let url = self.picture(&data)?;
        self.matrix.set_avatar(user, As::User(user), &url)?;
        Ok(digest)
    }

    fn rename(&mut self, pane: &str, user: &str, name: String) -> Result<()> {
        self.matrix
            .set_displayname(user, As::User(user), Some(&name))?;
        if let Some(chat) = self.state.spawned.get_mut(pane) {
            chat.name = Some(name);
        }
        self.save()
    }

    /// A free user of the appservice for an agent of this name.
    fn contact_user(&mut self, name: &str) -> Result<String> {
        static SLUG: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"[^a-z0-9._=-]+").expect("a valid pattern"));
        let me = self.me()?;
        let (local, server) = me
            .get(1..)
            .and_then(|m| m.split_once(':'))
            .ok_or_else(|| Error::Malformed(format!("{me} is not a user id")))?;
        let slug = SLUG
            .replace_all(&name.to_lowercase(), "-")
            .trim_matches('-')
            .to_owned();
        let slug = if slug.is_empty() {
            "agent".to_owned()
        } else {
            slug
        };
        let taken: BTreeSet<&str> = self
            .state
            .spawned
            .values()
            .map(|c| c.user.as_str())
            .collect();
        let mut user = format!("@{local}_{slug}:{server}");
        let mut n = 1;
        while taken.contains(user.as_str()) {
            n += 1;
            user = format!("@{local}_{slug}_{n}:{server}");
        }
        Ok(user)
    }

    /// Registers a user of the appservice, once, with its name and picture;
    /// the sha256 of the picture.
    fn register(
        &mut self,
        user: &str,
        display: &str,
        avatar: Option<PathBuf>,
    ) -> Result<Option<String>> {
        let username = user[1..].split(':').next().unwrap_or_default();
        self.matrix.register(username)?;
        self.matrix
            .set_displayname(user, As::User(user), Some(display))?;
        avatar.map(|path| self.set_picture(user, &path)).transpose()
    }

    /// Ends an agent's own chat; the agent keeps running.
    pub(super) fn end_chat(&mut self, pane: &str, why: Why) -> Result<()> {
        let chat = self.state.spawned.shift_remove(pane).ok_or(Error::NoChat)?;
        self.forget_retries(pane, &chat.user);
        self.save()?;
        self.log(&format!("{pane} chat ended: {}", why.reason()));
        if let Err(err) = self.leave_chat(&chat, why) {
            self.log(&format!("error: kill {pane}: {err}"));
        }
        Ok(())
    }

    /// Says goodbye and leaves; a room someone else is in hears nothing.
    fn leave_chat(&mut self, chat: &Chat, why: Why) -> Result<()> {
        let to = To::chat(Some(chat));
        if why == Why::Ended {
            self.say("The agent ended.", to)?;
        }
        if why != Why::Stranger {
            self.say("Bye. You can leave this room.", to)?;
        }
        self.matrix.leave(&chat.room, As::User(&chat.user))
    }

    /// Notes that the owner closed the own chat in `pane`, of an agent that
    /// gets one as soon as the bot sees it; `end_chat` saves it.
    fn mark_left(&mut self, pane: &str) {
        let cwd = self.state.spawned.get(pane).and_then(|c| c.agent.clone());
        if let Some(cwd) = cwd.filter(|cwd| {
            self.deps
                .agents
                .iter()
                .any(|s| s.chat && same_dir(&s.cwd, cwd))
        }) {
            self.state.left.insert(cwd);
        }
    }

    /// `!leave`: ends the chat it is written in, or the own chat of an agent
    /// named in the main chat, or of the agent `replied` to. `note` follows
    /// the answer.
    pub(super) fn leave(
        &mut self,
        rest: &str,
        agents: &[Agent],
        chat: Option<&Chat>,
        contact: Option<&str>,
        replied: Option<&str>,
        note: Option<&str>,
    ) -> Result<()> {
        if let Some(contact) = contact {
            if rest.is_empty() {
                if let Some(note) = note {
                    self.say(note, To::chat(chat))?;
                }
                self.mark_left(contact);
                return self.end_chat(contact, Why::Asked);
            }
            self.say(
                "!leave <name> works in the main chat only; !leave alone closes this chat.",
                To::chat(chat),
            )?;
            return Ok(());
        }
        let Some(a) = self.pick(rest, agents, "Close the chat of which", replied)? else {
            return Ok(());
        };
        let mut said = if self.state.spawned.contains_key(&a.pane_id) {
            self.mark_left(&a.pane_id);
            self.end_chat(&a.pane_id, Why::Asked)?;
            format!("Closed the chat of **{}**; it keeps running.", a.name)
        } else {
            format!("**{}** has no chat of its own.", a.name)
        };
        if let Some(note) = note {
            said = format!("{said}\n{note}");
        }
        self.say(&said, To::default())?;
        Ok(())
    }
}
