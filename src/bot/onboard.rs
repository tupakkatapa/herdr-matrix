//! The chat with the owner: made once, or adopted from the owner's invite,
//! and the bot's name and picture.

use std::fs;
use std::io::Cursor;

use super::contacts::Why;
use super::{Bot, To};
use crate::Result;
use crate::commands::{Place, helps};
use crate::matrix::{Api as _, As, Sync, private_room};
use crate::state::sha256_hex;

impl Bot {
    /// Creates the chat once per owner and explains the bot in it.
    ///
    /// # Errors
    /// When Matrix or herdr fails.
    pub fn onboard(&mut self) -> Result<()> {
        if self.state.owner.as_deref() != Some(self.deps.owner.as_str()) {
            let panes: Vec<String> = self.state.spawned.keys().cloned().collect();
            for pane in panes {
                self.end_chat(&pane, Why::Owner)?;
            }
            self.state.pending.clear();
            self.state.asked.clear();
            self.state.sent.clear();
            self.state.left.clear();
            self.state.owner = Some(self.deps.owner.clone());
            self.state.control = None;
            self.state.attached = None;
        }
        if self.state.control.is_some() {
            return Ok(());
        }
        let room = self.matrix.create_dm(As::Bot, &self.deps.owner)?;
        self.welcome(&room)
    }

    /// The commands, what the chat is, and the first thing to do.
    fn welcome(&mut self, room: &str) -> Result<()> {
        self.state.control = Some(room.to_owned());
        self.nicked = false;
        self.save()?;
        let agents = self.agents().unwrap_or_else(|err| {
            self.log(&format!("error: welcome: {err}"));
            Vec::new()
        });
        let action = match (self.attached(&agents), agents.len()) {
            (Some(a), _) => format!("Write here to talk to **{}**.", a.name),
            (None, 0) => {
                "No agent is running. Start one with !new <folder>; !new lists the folders.".into()
            }
            (None, _) => "!attach one of them to start.".into(),
        };
        let text = format!(
            "{}\n\nThis chat reaches the coding agents on this machine: what you write goes \
             to the attached one, and its answers come back here.\n!help explains the rest.\n{action}",
            helps(Place::Main)
        );
        if self.attached(&agents).is_none() && !agents.is_empty() {
            return self.ask_which(&text, &agents);
        }
        self.say(&text, To::default())?;
        Ok(())
    }

    /// A room the owner invites the bot to becomes the chat, like a
    /// bridge's management room.
    pub(super) fn accept_invites(&mut self, rooms: &[String]) {
        for room in rooms {
            if let Err(err) = self.join(room) {
                self.log(&format!("error: invite {room}: {err}"));
            }
        }
    }

    fn join(&mut self, room: &str) -> Result<()> {
        self.matrix.join(room)?;
        if self.private(room)? {
            self.welcome(room)
        } else {
            self.log(&format!("left {room}: not a room of the owner alone"));
            self.matrix.leave(room, As::Bot)
        }
    }

    /// The invite preview comes unverified from the inviting server; the
    /// state of a joined room does not. False when it cannot be read.
    fn private(&mut self, room: &str) -> Result<bool> {
        let state = match self.matrix.room_state(room) {
            Ok(state) => state,
            Err(err) => {
                self.log(&format!("error: state of {room}: {err}"));
                return Ok(false);
            }
        };
        let me = self.me()?;
        Ok(private_room(&state, &[&self.deps.owner, &me]))
    }

    /// Leaves the chat once anyone else is in it, invited or knocks;
    /// the next tick makes a new one.
    pub(super) fn guard(&mut self, sync: &Sync) -> Result<bool> {
        let Some(room) = self.state.control.clone() else {
            return Ok(false);
        };
        let me = self.me()?;
        if !sync.strangers(&room, &[&self.deps.owner, &me]) {
            return Ok(false);
        }
        self.log(&format!("left {room}: someone else is in it"));
        self.state.control = None;
        self.save()?;
        if let Err(err) = self.matrix.leave(&room, As::Bot) {
            self.log(&format!("error: leave {room}: {err}"));
        }
        Ok(true)
    }

    /// Display name and picture, set when they change.
    ///
    /// # Errors
    /// When the picture cannot be read or Matrix refuses.
    pub fn profile(&mut self) -> Result<()> {
        let Some(avatar) = self.deps.avatar.clone() else {
            return Ok(());
        };
        let data = fs::read(avatar)?;
        let want = (self.deps.name.clone(), sha256_hex(&data));
        if self.state.profile.as_ref() == Some(&want) {
            return Ok(());
        }
        let me = self.me()?;
        self.matrix
            .set_displayname(&me, As::Bot, self.deps.name.as_deref())?;
        self.nicked = false;
        let url = self.picture(&data)?;
        self.matrix.set_avatar(&me, As::Bot, &url)?;
        self.state.profile = Some(want);
        self.save()
    }

    /// The mxc URI of a PNG, uploaded once.
    pub(super) fn picture(&mut self, data: &[u8]) -> Result<String> {
        let digest = sha256_hex(data);
        if let Some(url) = self.state.avatars.get(&digest) {
            return Ok(url.clone());
        }
        let url = self
            .matrix
            .upload(&mut Cursor::new(data), data.len() as u64, "image/png")?;
        self.state.avatars.insert(digest, url.clone());
        self.save()?;
        Ok(url)
    }

    /// Names the chat after the attached agent, and away while it is on:
    /// the bot's name in this room only.
    pub(super) fn nick(&mut self, agent: Option<&str>) -> Result<()> {
        let Some(name) = self.deps.name.clone() else {
            return Ok(());
        };
        let (room, _) = self.room(None)?;
        let me = self.me()?;
        let mut shown = agent.map_or(name.clone(), |a| format!("{name} · {a}"));
        if self.away() {
            shown.push_str(" · away");
        }
        self.matrix.set_room_name(&room, &me, &shown)
    }
}
