//! An agent's own chat: a user of the appservice in a direct chat with the
//! owner.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;

use super::{AgentSettings, Bot, To, same_dir};
use crate::commands::contact;
use crate::herdr::Agent;
use crate::matrix::{Api as _, As};
use crate::state::{Chat, sha256_hex};
use crate::{Error, Result};

/// Why an agent's own chat ends: the owner asked, or the agent is gone.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Why {
    Asked,
    Ended,
}

impl Bot {
    /// Gives an agent its own chat: a Matrix user of the appservice, named
    /// after the agent, in a direct chat with the owner.
    pub(super) fn open_chat(&mut self, query: &str, agents: &[Agent]) -> Result<()> {
        let Some(a) = self.pick(query, agents, "Open a chat with which")? else {
            return Ok(());
        };
        if let Some(chat) = self.state.spawned.get(&a.pane_id) {
            let text = format!("**{}** has a chat already: {}.", a.name, chat.user);
            self.say(&text, To::default())?;
            return Ok(());
        }
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
            ..Chat::default()
        };
        self.state.spawned.insert(a.pane_id.clone(), chat.clone());
        self.save()?;
        self.say(&contact(&a.name), To::chat(Some(&chat)))?;
        self.say(
            &format!(
                "**{}** has its own chat now: {user}. Accept its invite.",
                a.name
            ),
            To::default(),
        )?;
        Ok(())
    }

    /// The settings of the agent working in the directory of `a`.
    pub(super) fn agent_settings(&self, a: &Agent) -> Option<&AgentSettings> {
        let cwd = a.cwd.as_deref()?;
        self.deps.agents.iter().find(|s| same_dir(&s.cwd, cwd))
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
    /// fails is tried again at the next poll.
    pub(super) fn rename_contacts(&mut self, agents: &[Agent]) {
        for a in agents {
            let Some(chat) = self.state.spawned.get(&a.pane_id) else {
                continue;
            };
            let user = chat.user.clone();
            let name = self.contact_name(a);
            if chat.name.as_deref() != Some(name.as_str())
                && let Err(err) = self.rename(&a.pane_id, &user, name)
            {
                self.log(&format!("error: rename {user}: {err}"));
            }
            if let Err(err) = self.repaint(a, &user) {
                self.log(&format!("error: picture {user}: {err}"));
            }
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
        self.save()?;
        if let Err(err) = self.leave_chat(&chat, why) {
            self.log(&format!("error: kill {pane}: {err}"));
        }
        Ok(())
    }

    fn leave_chat(&mut self, chat: &Chat, why: Why) -> Result<()> {
        let to = To::chat(Some(chat));
        if why == Why::Ended {
            self.say("The agent ended.", to)?;
        }
        self.say("Bye.", to)?;
        self.matrix.leave(&chat.room, As::User(&chat.user))
    }

    /// `!leave`: ends the chat it is written in, or the own chat of an agent
    /// named in the main chat.
    pub(super) fn leave(
        &mut self,
        rest: &str,
        agents: &[Agent],
        chat: Option<&Chat>,
        contact: Option<&str>,
    ) -> Result<()> {
        if let Some(contact) = contact {
            if rest.is_empty() {
                return self.end_chat(contact, Why::Asked);
            }
            self.say(
                "!leave <name> works in the main chat only; !leave alone ends this chat.",
                To::chat(chat),
            )?;
            return Ok(());
        }
        let Some(a) = self.pick(rest, agents, "End the chat of which")? else {
            return Ok(());
        };
        if self.state.spawned.contains_key(&a.pane_id) {
            self.end_chat(&a.pane_id, Why::Asked)?;
            self.say(
                &format!("Ended the chat of **{}**; it keeps running.", a.name),
                To::default(),
            )?;
        } else {
            self.say(
                &format!("**{}** has no chat of its own.", a.name),
                To::default(),
            )?;
        }
        Ok(())
    }
}
