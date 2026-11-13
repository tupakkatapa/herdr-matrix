//! The owner's `!` commands.

use super::contacts::Why;
use super::{Bot, Post, SCREEN_LINES, SCREEN_MAX, To};
use crate::Result;
use crate::commands::{Cmd, HELP, Place, command, contact, helps};
use crate::files;
use crate::herdr::{self, Agent, Status, Turn};
use crate::matrix::Inbound;
use crate::render::render;
use crate::state::{Asked, Away, Chat, Origin, fingerprint};

/// Folders `!new` lists at most
const FOLDERS_SHOWN: usize = 20;

impl Bot {
    /// Runs a command where the command table allows it.
    pub(super) fn command(
        &mut self,
        word: &str,
        rest: &str,
        agents: &[Agent],
        ev: &Inbound,
        contact: Option<&str>,
        mine: Option<&Agent>,
    ) -> Result<()> {
        let chat = contact.and_then(|c| self.state.spawned.get(c)).cloned();
        let chat = chat.as_ref();
        let to = To::chat(chat);
        let place = if contact.is_some() {
            Place::Own
        } else {
            Place::Main
        };
        let Some(entry) = command(&word[1..]) else {
            let help = helps(place);
            self.say(
                &format!("Unknown command {word}. Commands:"),
                to.screen(&help),
            )?;
            return Ok(());
        };
        if entry.help(place).is_none() {
            self.log(word);
            self.say(&format!("{word} works in the main chat only."), to)?;
            return Ok(());
        }
        let a = if entry.cmd.acts() {
            let Some(a) = mine.or_else(|| self.target(ev, agents)) else {
                return self.ask_which("Not attached. !attach one of:", agents);
            };
            self.log(&format!("{} {word}", a.pane_id));
            Some(a)
        } else {
            self.log(word);
            None
        };
        let agent = || a.expect("an acting command has its agent");
        match entry.cmd {
            Cmd::Agents => self.ask_which("Agents:", agents)?,
            Cmd::Attach => self.attach(rest, agents)?,
            Cmd::New => self.new_agent(rest, agents)?,
            Cmd::Dm => self.open_chat(rest, agents)?,
            Cmd::Leave => self.leave(rest, agents, chat, contact)?,
            Cmd::Last => self.last(agent(), to)?,
            Cmd::Screen => self.screen(agent(), rest, to)?,
            Cmd::Stop => self.stop(agent(), to)?,
            Cmd::Exit => self.exit(agent(), rest, chat, contact)?,
            Cmd::Away => {
                self.set_away(
                    Away::On,
                    "Away: every agent that stops or waits posts here. !here ends it.",
                    to,
                )?;
            }
            Cmd::Here => {
                self.set_away(
                    Away::Off,
                    "Here: only replies to what you send from the chat post here.",
                    to,
                )?;
            }
            Cmd::Help => self.help(place, mine, to)?,
        }
        Ok(())
    }

    fn set_away(&mut self, away: Away, said: &str, to: To) -> Result<()> {
        self.state.away = away;
        self.save()?;
        self.say(said, to)?;
        Ok(())
    }

    fn last(&mut self, a: &Agent, to: To) -> Result<()> {
        let turn = self.turn(a);
        let said = turn
            .messages
            .last()
            .map_or("Nothing said yet.", |(_, t)| t.as_str());
        self.say(said, to.pane(&a.pane_id))?;
        Ok(())
    }

    fn screen(&mut self, a: &Agent, rest: &str, to: To) -> Result<()> {
        let lines = if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            rest.parse::<u64>()
                .map_or(SCREEN_MAX, |n| n.min(SCREEN_MAX))
        } else {
            SCREEN_LINES
        };
        let screen = self.read(&a.pane_id, lines)?;
        self.say(
            &format!("**{}** screen", a.name),
            to.screen(&screen).pane(&a.pane_id),
        )?;
        Ok(())
    }

    fn stop(&mut self, a: &Agent, to: To) -> Result<()> {
        let to = to.pane(&a.pane_id);
        if a.status != Status::Working {
            let said = format!("**{}** is not working; nothing to stop.", a.name);
            self.say(&said, to)?;
            return Ok(());
        }
        let sent = herdr::send_keys(self.deps.herdr.as_mut(), &a.pane_id, "esc");
        self.done(&a.pane_id)?;
        let said = match sent {
            Ok(()) => format!("Interrupted **{}**.", a.name),
            Err(crate::Error::Herdr(out)) => format!("Could not stop **{}**: {out}", a.name),
            Err(err) => return Err(err),
        };
        self.say(&said, to)?;
        Ok(())
    }

    fn help(&mut self, place: Place, a: Option<&Agent>, to: To) -> Result<()> {
        let text = match place {
            Place::Own => contact(a.map_or("", |a| a.name.as_str())),
            Place::Main => HELP.to_owned(),
        };
        self.say(&text, to.screen(&helps(place)))?;
        Ok(())
    }

    fn attach(&mut self, rest: &str, agents: &[Agent]) -> Result<()> {
        let Some(a) = self.pick(rest, agents, "Attach to which")? else {
            return Ok(());
        };
        self.state.attached = Some(a.pane_id.clone());
        self.save()?;
        self.nick(Some(&a.name))?;
        let turn = self.turn(a);
        let mut said = turn.messages.last().map(|(_, t)| t.clone());
        let asks = turn
            .question
            .as_ref()
            .filter(|q| a.status == Status::Blocked && !q.list.is_empty());
        if let Some(q) = asks {
            said = Some(render(&q.list));
        }
        let note = match &a.status {
            Status::Blocked => "attached; it waits on you.".to_owned(),
            Status::Other(_) => "attached.".to_owned(),
            known => format!("attached; it is {known}."),
        };
        let event = self.post(
            a,
            &note,
            Post::new(&[], None).text(said.as_deref()).no_screen(),
        )?;
        if let Some(q) = asks {
            self.state.asked.insert(
                a.pane_id.clone(),
                Asked {
                    event,
                    shown: Some(fingerprint(&q.raw)),
                },
            );
            self.save()?;
        }
        Ok(())
    }

    /// Ends an agent and closes its pane; asks first, and acts only on
    /// `now`.
    fn exit(
        &mut self,
        a: &Agent,
        rest: &str,
        chat: Option<&Chat>,
        contact: Option<&str>,
    ) -> Result<()> {
        let pane = a.pane_id.as_str();
        let to = To::chat(chat).pane(pane);
        if rest.to_lowercase() != "now" {
            let said = format!("Send !exit now to end **{}** and close its pane.", a.name);
            self.say(&said, to)?;
            return Ok(());
        }
        match herdr::close_pane(self.deps.herdr.as_mut(), pane) {
            Err(crate::Error::Herdr(out)) => {
                self.say(&format!("Could not close **{}**: {out}", a.name), to)?;
                return Ok(());
            }
            other => other?,
        }
        self.done(pane)?;
        if contact.is_none() || self.state.attached.as_deref() == Some(pane) {
            self.state.attached = None;
            self.nick(None)?;
        }
        self.save()?;
        self.say(&format!("Closed **{}**.", a.name), To::chat(chat))?;
        if let Some(contact) = contact {
            self.end_chat(contact, Why::Asked)?;
        }
        Ok(())
    }

    fn new_agent(&mut self, query: &str, agents: &[Agent]) -> Result<()> {
        let query = Some(query).filter(|q| !q.is_empty());
        let dirs = files::folders(&self.deps.workspace, query);
        let (head, shown) = match (query, dirs.len()) {
            (Some(_), 1) => (None, Vec::new()),
            (None, _) => (
                Some("Start Claude in which folder?".to_owned()),
                dirs.clone(),
            ),
            (Some(q), 0) => (
                Some(format!(
                    "No folder under {} matches \"{q}\". Start Claude in which one?",
                    self.deps.workspace.display()
                )),
                files::folders(&self.deps.workspace, None),
            ),
            (Some(q), n) => (
                Some(format!(
                    "\"{q}\" matches {n} folders. Start Claude in which one?"
                )),
                dirs.clone(),
            ),
        };
        if let Some(head) = head {
            let mut listed: Vec<String> = shown.iter().take(FOLDERS_SHOWN).cloned().collect();
            if shown.len() > FOLDERS_SHOWN {
                let more = shown.len() - FOLDERS_SHOWN;
                listed.push(format!("…and {more} more; write more of the name."));
            }
            let listed = if listed.is_empty() {
                "(none)".to_owned()
            } else {
                listed.join("\n")
            };
            self.say(&head, To::default().screen(&listed))?;
            return Ok(());
        }
        let path = &dirs[0];
        if let Some(running) = agents
            .iter()
            .find(|a| a.cwd.as_deref() == Some(path.as_str()))
        {
            self.state.attached = Some(running.pane_id.clone());
            self.save()?;
            self.nick(Some(&running.name))?;
            self.post(
                running,
                "runs there already; attached.",
                Post::new(&[], None).no_screen(),
            )?;
            return Ok(());
        }
        let label = path.rsplit('/').next().unwrap_or_default().to_owned();
        let herdr = self.deps.herdr.as_mut();
        let pane = herdr::create_workspace(herdr, path, &label)?;
        herdr::run_in_pane(herdr, &pane, "claude")?;
        self.state.attached = Some(pane.clone());
        self.save()?;
        self.nick(Some(&label))?;
        self.expect(
            &pane,
            &Turn::default(),
            Origin {
                started: true,
                ..Origin::default()
            },
        )?;
        self.say(
            &format!("Started Claude in {path}. This chat is attached to it."),
            To::default(),
        )?;
        Ok(())
    }
}
