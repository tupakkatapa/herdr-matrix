//! What the agents did since the last poll: answers to prompts from the
//! chat, and notices while the owner is away.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use indexmap::IndexMap;

use super::contacts::Why;
use super::{Bot, FOLLOW_UP, HOLD, PENDING_TTL, Post, RETRY_MAX, To, due, silent};
use crate::Result;
use crate::herdr::{Agent, Status, Turn};
use crate::matrix::{Api as _, TYPING_TIMEOUT};
use crate::state::{Chat, Origin, Pending};

/// Polls and time a pane is missing before its own chat ends
const GONE_POLLS: u32 = 3;
const GONE_AFTER: std::time::Duration = std::time::Duration::from_mins(1);
/// Characters of an answer that went out quietly that go out again, to
/// notify; a longer one is pointed to
const AGAIN_MAX: usize = 500;

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
        self.poll_listed(&agents)
    }

    /// What the agents herdr listed did since the last poll. The main chat
    /// is named after the attached agent when it is not yet; the first
    /// poll ends the chats of agents no longer configured. An own chat an
    /// older version opened learns the settings of its agent. The second
    /// poll in a row that herdr lists for starts the agents set to start.
    pub(super) fn poll_listed(&mut self, agents: &[Agent]) -> Result<()> {
        self.good_polls = self.good_polls.saturating_add(1);
        let current: IndexMap<String, Status> = agents
            .iter()
            .map(|a| (a.pane_id.clone(), a.status.clone()))
            .collect();
        let now = self.now();
        if let Some(a) = self
            .state
            .attached
            .as_ref()
            .and_then(|p| agents.iter().find(|a| &a.pane_id == p))
        {
            self.attached_name = Some((a.pane_id.clone(), a.name.clone()));
        }
        if !self.nicked {
            let name = self.attached_name().map(str::to_owned);
            self.nicked = self.retry("nick", RETRY_MAX, |bot| bot.nick(name.as_deref()));
        }
        let mut marked = false;
        for a in agents {
            let cwd = self.agent_settings(a).map(|s| s.cwd.clone());
            if let Some(chat) = self.state.spawned.get_mut(&a.pane_id)
                && chat.agent.is_none()
                && cwd.is_some()
            {
                chat.agent = cwd;
                marked = true;
            }
        }
        if marked {
            self.save()?;
        }
        let Some(statuses) = self.statuses.clone() else {
            self.end_unconfigured_chats()?;
            self.statuses = Some(current);
            return Ok(());
        };
        self.rename_contacts(agents);
        self.forget_started(agents, now);
        self.end_gone_chats(agents, now)?;
        self.open_configured_chats(agents);
        self.watch_starting(agents, now)?;
        if self.good_polls == 2 {
            self.autostart(agents);
        }
        self.let_go(agents, now)?;
        let afk = self.away();
        let began = afk && !self.was_afk;
        self.forget(&current, now)?;
        let mut seen = current.clone();
        for a in agents {
            let pane = &a.pane_id;
            let prev = statuses.get(pane);
            let sweep = Sweep {
                afk,
                began: began || self.unswept.remove(pane),
            };
            // One agent's failure leaves the others alone; it is tried again
            if let Err(err) = self.step(a, agents, prev, sweep) {
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
        // A new agent that waits on a choice, such as trusting its folder,
        // is not ready yet
        if self.reply(a, agents)? {
            if *status != Status::Blocked {
                self.starting.shift_remove(pane);
            }
            return Ok(());
        }
        if matches!(status, Status::Idle | Status::Done)
            && self.starting.shift_remove(pane).is_some()
        {
            return self.ready(a, agents);
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

    /// Forgets the panes the bot started that are missing from the list for
    /// a while, as `end_gone_chats` ends an own chat.
    fn forget_started(&mut self, agents: &[Agent], now: f64) {
        self.started_for.retain(|pane, (_, missing)| {
            if agents.iter().any(|a| &a.pane_id == pane) {
                *missing = None;
                return true;
            }
            let (polls, since) = missing.get_or_insert((0, now));
            *polls += 1;
            *polls < GONE_POLLS || !due(now, *since, GONE_AFTER)
        });
    }

    /// Ends the own chats of agents missing from the list for a while; a
    /// pane is missing for a moment while its agent restarts or herdr
    /// restores its session. The chat of a configured agent stays, and
    /// moves to the one pane the bot started for that agent: herdr takes
    /// the folder of a pane from what its terminal reports, and any agent
    /// can report any folder.
    fn end_gone_chats(&mut self, agents: &[Agent], now: f64) -> Result<()> {
        let here = |p: &str| agents.iter().any(|a| a.pane_id == p);
        let gone: Vec<(String, Option<String>)> = self
            .state
            .spawned
            .iter()
            .filter(|(p, _)| !here(p))
            .map(|(p, chat)| (p.clone(), chat.agent.clone().filter(|c| self.configured(c))))
            .collect();
        self.missing
            .retain(|p, _| gone.iter().any(|(g, cwd)| g == p && cwd.is_none()));
        let mut ended = Vec::new();
        for (pane, cwd) in gone {
            if let Some(cwd) = cwd {
                let started: Vec<&Agent> = agents
                    .iter()
                    .filter(|a| self.started_in(&a.pane_id, &cwd))
                    .filter(|a| !self.state.spawned.contains_key(&a.pane_id))
                    .collect();
                if let [a] = started.as_slice() {
                    self.move_chat(&pane, a)?;
                }
                continue;
            }
            let (polls, since) = self.missing.entry(pane.clone()).or_insert((0, now));
            *polls += 1;
            if *polls >= GONE_POLLS && due(now, *since, GONE_AFTER) {
                ended.push(pane);
            }
        }
        for pane in ended {
            self.missing.shift_remove(&pane);
            self.end_chat(&pane, Why::Ended)?;
        }
        Ok(())
    }

    /// Lets go of the attached agent once its pane is missing for a while,
    /// as `end_gone_chats` ends an own chat, and says so.
    fn let_go(&mut self, agents: &[Agent], now: f64) -> Result<()> {
        let Some(pane) =
            self.state.attached.clone().filter(|p| {
                !self.starting.contains_key(p) && !agents.iter().any(|a| &a.pane_id == p)
            })
        else {
            self.attached_missing = None;
            return Ok(());
        };
        let (polls, since) = self.attached_missing.get_or_insert((0, now));
        *polls += 1;
        if *polls < GONE_POLLS || !due(now, *since, GONE_AFTER) {
            return Ok(());
        }
        self.attached_missing = None;
        let name = self
            .attached_name()
            .map_or_else(|| "The attached agent".to_owned(), |n| format!("**{n}**"));
        let mut said = String::new();
        if self.state.pending.contains_key(&pane) {
            let _ = writeln!(said, "{name} ended before it answered.");
        }
        let _ = write!(
            said,
            "{name} ended. Nothing is attached now: !attach another agent, or !new to start one."
        );
        self.log(&format!("{pane} detached: gone"));
        self.state.attached = None;
        self.save()?;
        self.done(&pane)?;
        self.nicked = self.retry("nick", RETRY_MAX, |bot| bot.nick(None));
        self.say(&said, To::default())?;
        Ok(())
    }

    /// Follows the panes `!new` started until they are ready: one that is
    /// gone for a while, after herdr listed it or from the start, stopped;
    /// one that is not ready `FOLLOW_UP` after its start or its last choice
    /// is no longer followed. The message kept for an attached one that
    /// waits on a choice runs out as any other, and the chat says so.
    fn watch_starting(&mut self, agents: &[Agent], now: f64) -> Result<()> {
        let mut stopped = Vec::new();
        let mut waiting = Vec::new();
        self.starting.retain(|pane, s| {
            if let Some(a) = agents.iter().find(|a| &a.pane_id == pane) {
                s.seen = true;
                s.missing = None;
                if a.status == Status::Blocked {
                    s.at = now;
                    waiting.push(a.clone());
                    return true;
                }
                return !due(now, s.at, FOLLOW_UP);
            }
            let gone = if s.seen {
                let (polls, since) = s.missing.get_or_insert((0, now));
                *polls += 1;
                *polls >= GONE_POLLS && due(now, *since, GONE_AFTER)
            } else {
                due(now, s.at, GONE_AFTER)
            };
            if gone {
                stopped.push((pane.clone(), s.folder.clone()));
            }
            !gone
        });
        for a in waiting {
            if self.state.attached.as_deref() == Some(a.pane_id.as_str())
                && self
                    .kept
                    .as_ref()
                    .is_some_and(|k| due(now, k.at, FOLLOW_UP))
            {
                self.kept = None;
                let said = format!(
                    "**{}** still waits on a choice; your message was not sent.",
                    a.name
                );
                self.say(&said, To::default())?;
            }
        }
        for (pane, folder) in stopped {
            self.log(&format!("{pane} stopped while it started"));
            if self.state.attached.as_deref() == Some(pane.as_str()) {
                self.state.attached = None;
                self.save()?;
                self.nicked = self.retry("nick", RETRY_MAX, |bot| bot.nick(None));
            }
            self.done(&pane)?;
            self.say(
                &format!(
                    "**{folder}** stopped while it started. Run its command in a pane to see why."
                ),
                To::default(),
            )?;
        }
        Ok(())
    }

    /// An agent `!new` started is ready: said quietly, and the message that
    /// waits for it goes to it.
    fn ready(&mut self, a: &Agent, agents: &[Agent]) -> Result<()> {
        self.say(&format!("**{}** is ready.", a.name), To::default().quiet())?;
        if self.state.attached.as_deref() == Some(a.pane_id.as_str()) {
            self.send_kept(a, agents)?;
        }
        Ok(())
    }

    /// Ends the own chats of configured agents whose settings are gone.
    fn end_unconfigured_chats(&mut self) -> Result<()> {
        let ended: Vec<String> = self
            .state
            .spawned
            .iter()
            .filter(|(_, chat)| chat.agent.as_ref().is_some_and(|c| !self.configured(c)))
            .map(|(p, _)| p.clone())
            .collect();
        for pane in ended {
            self.end_chat(&pane, Why::Unconfigured)?;
        }
        Ok(())
    }

    /// The chat a working agent's turn came from shows it typing; the
    /// notice is sent again once half its time is out.
    fn update_typing(&mut self, current: &IndexMap<String, Status>) -> Result<()> {
        let busy: BTreeSet<Option<String>> = self
            .state
            .pending
            .iter()
            .filter(|(pane, waiting)| {
                current.get(*pane) == Some(&Status::Working) && !waiting.local
            })
            .map(|(pane, waiting)| waiting.origin.spawned.then(|| pane.clone()))
            .collect();
        let now = self.now();
        let keys: BTreeSet<Option<String>> = self.typing.keys().cloned().collect();
        for key in busy.union(&keys) {
            let on = busy.contains(key);
            let chat = key.as_ref().and_then(|k| self.state.spawned.get(k));
            if key.is_some() && chat.is_none() {
                self.typing.remove(key);
                continue;
            }
            if on
                && self
                    .typing
                    .get(key)
                    .is_some_and(|at| !due(now, *at, TYPING_TIMEOUT / 2))
            {
                continue;
            }
            let chat = chat.cloned();
            let user = match &chat {
                Some(chat) => chat.user.clone(),
                None => self.me()?,
            };
            let (room, by) = self.room(chat.as_ref())?;
            self.matrix.typing(&room, by, &user, on)?;
            if on {
                self.typing.insert(key.clone(), now);
            } else {
                self.typing.remove(key);
            }
        }
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

    pub(super) fn waiting(&mut self, pane: &str) -> &mut Pending {
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
        let read = self.read_turn(a);
        let reader = read.is_some();
        let turn = read.unwrap_or_default();
        let mut fresh = false;
        for (uuid, text) in &turn.messages {
            if !self.waiting(pane).seen.contains(uuid) {
                fresh = true;
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
                waiting.polls = 0;
                waiting.at = now;
                if held.is_some_and(|(_, at)| due(now, at, HOLD)) {
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
        let ended = a.status.notifies();
        if ended {
            self.waiting(pane).polls += 1;
        }
        let waiting = self.waiting(pane).clone();
        // herdr may tell the end before the last message is written: an end
        // without a new message holds for one more poll
        let sure = fresh || waiting.polls >= 2 || !reader;
        if ended && waiting.progress.said && sure {
            // An answer that went out quietly goes out again, to notify; a
            // long one is pointed to
            let quiet = held.is_none() && !waiting.local;
            let last = held.map(|(text, _)| text).or(waiting.last);
            let text = last
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "Done.".into());
            if quiet && text.chars().count() > AGAIN_MAX {
                let said = format!("**{}** is done; its answer is above.", a.name);
                self.say(&said, To::chat(chat).pane(pane))?;
            } else if !silent(&text) {
                self.post(a, "", Post::new(agents, chat).text(Some(&text)).no_hint())?;
            }
            self.done(pane)?;
            if let Some((uuid, _)) = turn.messages.last() {
                self.reported.insert(pane.to_owned(), uuid.clone());
            }
            return Ok(true);
        }
        // A new agent's turn ends once it is ready; one that never answers
        // must not claim a turn started later at the desk
        if (waiting.origin.started && ended) || now - waiting.at >= PENDING_TTL.as_secs_f64() {
            self.done(pane)?;
            return Ok(false);
        }
        if !ended {
            return Ok(false);
        }
        // A slash command answers on the screen, not in the transcript
        let settled = waiting.origin.slash && waiting.polls >= 2;
        let worked = waiting.progress.worked;
        if settled || (worked && !reader) {
            self.post(a, "", Post::new(agents, chat).no_hint())?;
        } else if worked && !waiting.origin.slash && sure {
            if !waiting.local {
                let done = Post::new(agents, chat).text(Some("Done.")).no_hint();
                self.post(a, "", done)?;
            }
        } else {
            return Ok(false);
        }
        self.done(pane)?;
        Ok(true)
    }

    /// Posts the held message as progress.
    fn flush(&mut self, a: &Agent, agents: &[Agent], chat: Option<&Chat>) -> Result<()> {
        if let Some((text, _)) = self.waiting(&a.pane_id).held.clone() {
            if !silent(&text) && !self.waiting(&a.pane_id).local {
                self.post(
                    a,
                    "",
                    Post::new(agents, chat).text(Some(&text)).quiet().no_hint(),
                )?;
            }
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
            ""
        } else {
            "finished:"
        };
        if !said.as_deref().is_some_and(silent) {
            self.post(
                a,
                note,
                Post::new(agents, chat.as_ref()).text(said.as_deref()),
            )?;
        }
        self.reported.insert(a.pane_id.clone(), uuid);
        Ok(())
    }

    /// Waits for the answer to a prompt from `origin`. The prompt reached
    /// its agent: a failed save is logged, not a failed delivery. True when
    /// it saved.
    pub(super) fn expect(&mut self, pane: &str, turn: &Turn, origin: Origin) -> bool {
        let pending = Pending {
            seen: turn.messages.iter().map(|(u, _)| u.clone()).collect(),
            origin,
            at: self.now(),
            ..Pending::default()
        };
        self.state.pending.insert(pane.to_owned(), pending);
        let saved = self.save();
        if let Err(err) = &saved {
            self.log(&format!("error: {err}"));
        }
        saved.is_ok()
    }

    pub(super) fn done(&mut self, pane: &str) -> Result<()> {
        if self.state.pending.shift_remove(pane).is_some() {
            self.save()?;
        }
        Ok(())
    }
}
