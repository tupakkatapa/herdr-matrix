//! What the agents did since the last poll: answers to prompts from the
//! chat, and notices while the owner is away.

use std::collections::BTreeSet;

use indexmap::IndexMap;

use super::contacts::Why;
use super::{Bot, HOLD, PENDING_TTL, Post};
use crate::Result;
use crate::herdr::{Agent, Status, Turn};
use crate::matrix::Api as _;
use crate::state::{Chat, Origin, Pending};

/// Polls and time a pane is missing before its own chat ends
const GONE_POLLS: u32 = 3;
const GONE_AFTER: std::time::Duration = std::time::Duration::from_mins(1);

/// Where a poll stands: away is on, and it began with this poll.
#[derive(Clone, Copy)]
struct Sweep {
    afk: bool,
    began: bool,
}

impl Bot {
    /// What the agents did since the last poll.
    ///
    /// # Errors
    /// When Matrix or herdr fails.
    pub fn poll(&mut self) -> Result<()> {
        let agents = self.agents()?;
        let current: IndexMap<String, Status> = agents
            .iter()
            .map(|a| (a.pane_id.clone(), a.status.clone()))
            .collect();
        let now = self.now();
        let Some(statuses) = self.statuses.clone() else {
            self.statuses = Some(current);
            return Ok(());
        };
        self.end_gone_chats(&current, now)?;
        let afk = self.away();
        let began = afk && !self.was_afk;
        self.forget(&current, now)?;
        let mut seen = current.clone();
        for a in &agents {
            let pane = &a.pane_id;
            let prev = statuses.get(pane);
            let sweep = Sweep {
                afk,
                began: began || self.unswept.remove(pane),
            };
            // One agent's failure leaves the others alone; it is tried again
            if let Err(err) = self.step(a, &agents, prev, sweep) {
                self.log(&format!("error: {pane}: {err}"));
                match prev {
                    Some(prev) => seen.insert(pane.clone(), prev.clone()),
                    None => seen.shift_remove(pane),
                };
                if sweep.began {
                    self.unswept.insert(pane.clone());
                }
            }
        }
        self.statuses = Some(seen);
        self.was_afk = afk;
        if let Err(err) = self.update_typing(&current) {
            self.log(&format!("error: typing: {err}"));
        }
        Ok(())
    }

    /// What one agent did: its answer to a prompt from the chat, or a
    /// notice while the owner is away.
    fn step(
        &mut self,
        a: &Agent,
        agents: &[Agent],
        prev: Option<&Status>,
        sweep: Sweep,
    ) -> Result<()> {
        let Sweep { afk, began } = sweep;
        let (pane, status) = (&a.pane_id, &a.status);
        let changed = prev != Some(status)
            && status.notifies()
            && (prev == Some(&Status::Working) || *status == Status::Blocked);
        if *status != Status::Blocked && self.state.asked.shift_remove(pane).is_some() {
            self.save()?;
        }
        // An agent that waits when away begins still waits on the owner
        let missed = began && *status == Status::Blocked;
        if self.reply(a, agents)? {
            return Ok(());
        }
        if afk && (changed || missed) {
            self.report(a, agents)?;
        } else if *status == Status::Blocked
            && prev == Some(&Status::Blocked)
            && self.state.asked.contains_key(pane)
        {
            let turn = self.turn(a);
            let shown = self.shown(a, &turn)?;
            if self.state.asked[pane].shown.as_deref() != Some(shown.as_str()) {
                self.report(a, agents)?;
            }
        }
        Ok(())
    }

    /// Ends the own chats of agents missing from the list for a while; a
    /// pane is missing for a moment while its agent restarts or herdr
    /// restores its session.
    fn end_gone_chats(&mut self, current: &IndexMap<String, Status>, now: f64) -> Result<()> {
        let spawned = &self.state.spawned;
        self.missing
            .retain(|p, _| spawned.contains_key(p) && !current.contains_key(p));
        let mut ended = Vec::new();
        for pane in spawned.keys().filter(|p| !current.contains_key(*p)) {
            let (polls, since) = self.missing.entry(pane.clone()).or_insert((0, now));
            *polls += 1;
            if *polls >= GONE_POLLS && now - *since >= GONE_AFTER.as_secs_f64() {
                ended.push(pane.clone());
            }
        }
        for pane in ended {
            self.missing.shift_remove(&pane);
            self.end_chat(&pane, Why::Ended)?;
        }
        Ok(())
    }

    /// The chat a working agent's turn came from shows it typing.
    fn update_typing(&mut self, current: &IndexMap<String, Status>) -> Result<()> {
        let busy: BTreeSet<Option<String>> = self
            .state
            .pending
            .iter()
            .filter(|(pane, _)| current.get(*pane) == Some(&Status::Working))
            .map(|(pane, waiting)| waiting.origin.spawned.then(|| pane.clone()))
            .collect();
        let typing = self.typing.clone();
        for key in busy.union(&typing) {
            let chat = key.as_ref().and_then(|k| self.state.spawned.get(k));
            if key.is_some() && chat.is_none() {
                continue;
            }
            let chat = chat.cloned();
            let user = match &chat {
                Some(chat) => chat.user.clone(),
                None => self.me()?,
            };
            let (room, by) = self.room(chat.as_ref())?;
            self.matrix.typing(&room, by, &user, busy.contains(key))?;
        }
        self.typing = busy;
        Ok(())
    }

    /// Drops what the bot keeps for panes that are gone; a prompt's turn
    /// waits a while for its agent to appear.
    fn forget(&mut self, current: &IndexMap<String, Status>, now: f64) -> Result<()> {
        let mut gone: Vec<String> = self
            .state
            .asked
            .keys()
            .filter(|p| !current.contains_key(*p))
            .cloned()
            .collect();
        gone.extend(
            self.state
                .pending
                .iter()
                .filter(|(p, w)| {
                    !current.contains_key(*p) && now - w.at >= PENDING_TTL.as_secs_f64()
                })
                .map(|(p, _)| p.clone()),
        );
        for pane in &gone {
            self.state.asked.shift_remove(pane);
            self.state.pending.shift_remove(pane);
        }
        self.reported.retain(|p, _| current.contains_key(p));
        if gone.is_empty() { Ok(()) } else { self.save() }
    }

    fn waiting(&mut self, pane: &str) -> &mut Pending {
        self.state.pending.get_mut(pane).expect("a turn under way")
    }

    /// Posts what an agent says to a prompt from the chat until it stops.
    /// Progress goes out quietly; the newest message is held back a moment
    /// so that the last one of a turn can notify. A turn from an agent's own
    /// chat goes there. True when the turn is over.
    fn reply(&mut self, a: &Agent, agents: &[Agent]) -> Result<bool> {
        let pane = a.pane_id.as_str();
        let Some(spawned) = self.state.pending.get(pane).map(|w| w.origin.spawned) else {
            return Ok(false);
        };
        let chat = if spawned {
            self.state.spawned.get(pane).cloned()
        } else {
            None
        };
        if spawned && chat.is_none() {
            self.done(pane)?;
            return Ok(false);
        }
        let chat = chat.as_ref();
        let now = self.now();
        let turn = self.turn(a);
        for (uuid, text) in &turn.messages {
            if !self.waiting(pane).seen.contains(uuid) {
                self.flush(a, agents, chat)?;
                let waiting = self.waiting(pane);
                waiting.held = Some((text.clone(), now));
                waiting.progress.said = true;
                waiting.at = now;
                waiting.seen.push(uuid.clone());
                self.save()?;
            }
        }
        let held = self.waiting(pane).held.clone();
        match a.status {
            Status::Working => {
                let waiting = self.waiting(pane);
                waiting.progress.worked = true;
                waiting.at = now;
                if held.is_some_and(|(_, at)| now - at >= HOLD.as_secs_f64()) {
                    self.flush(a, agents, chat)?;
                }
                return Ok(false);
            }
            Status::Blocked => {
                self.flush(a, agents, chat)?;
                self.ask(a, &turn, Post::new(agents, chat).no_hint())?;
                self.done(pane)?;
                return Ok(true);
            }
            _ => {}
        }
        let waiting = self.waiting(pane).clone();
        if waiting.progress.said {
            // An answer that went out quietly goes out again, to notify
            let last = held.map(|(text, _)| text).or(waiting.last);
            let text = last
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "Done.".into());
            self.post(a, "", Post::new(agents, chat).text(Some(&text)).no_hint())?;
            self.done(pane)?;
            if let Some((uuid, _)) = turn.messages.last() {
                self.reported.insert(pane.to_owned(), uuid.clone());
            }
            return Ok(true);
        }
        // A new agent's turn ends once it is ready; one that never answers
        // must not claim a turn started later at the desk
        if (waiting.origin.started && a.status.notifies())
            || now - waiting.at >= PENDING_TTL.as_secs_f64()
        {
            self.done(pane)?;
            return Ok(false);
        }
        // A slash command answers on the screen, not in the transcript
        let polls = {
            let waiting = self.waiting(pane);
            waiting.polls += 1;
            waiting.polls
        };
        let settled = waiting.origin.slash && polls >= 2;
        if settled || (waiting.progress.worked && (self.deps.answer)(a).is_none()) {
            self.post(a, "", Post::new(agents, chat).no_hint())?;
            self.done(pane)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Posts the held message as progress.
    fn flush(&mut self, a: &Agent, agents: &[Agent], chat: Option<&Chat>) -> Result<()> {
        if let Some((text, _)) = self.waiting(&a.pane_id).held.clone() {
            self.post(
                a,
                "",
                Post::new(agents, chat).text(Some(&text)).quiet().no_hint(),
            )?;
            let waiting = self.waiting(&a.pane_id);
            waiting.last = Some(text);
            waiting.held = None;
            self.save()?;
        }
        Ok(())
    }

    /// An agent that stopped or waits while the owner is away, once for
    /// each thing it said.
    fn report(&mut self, a: &Agent, agents: &[Agent]) -> Result<()> {
        let turn = self.turn(a);
        let chat = self.state.spawned.get(&a.pane_id).cloned();
        if a.status == Status::Blocked {
            return self.ask(a, &turn, Post::new(agents, chat.as_ref()));
        }
        let (uuid, said) = turn
            .messages
            .last()
            .cloned()
            .map_or((None, None), |(u, t)| (u, Some(t)));
        if uuid.is_some() && self.reported.get(&a.pane_id) == Some(&uuid) {
            return Ok(());
        }
        let note = if self.own_voice(a, agents, chat.as_ref()) {
            String::new()
        } else {
            format!("is {}", a.status)
        };
        self.post(
            a,
            &note,
            Post::new(agents, chat.as_ref()).text(said.as_deref()),
        )?;
        self.reported.insert(a.pane_id.clone(), uuid);
        Ok(())
    }

    /// Waits for the answer to a prompt from `origin`.
    pub(super) fn expect(&mut self, pane: &str, turn: &Turn, origin: Origin) -> Result<()> {
        let pending = Pending {
            seen: turn.messages.iter().map(|(u, _)| u.clone()).collect(),
            origin,
            at: self.now(),
            ..Pending::default()
        };
        self.state.pending.insert(pane.to_owned(), pending);
        self.save()
    }

    pub(super) fn done(&mut self, pane: &str) -> Result<()> {
        if self.state.pending.shift_remove(pane).is_some() {
            self.save()?;
        }
        Ok(())
    }
}
