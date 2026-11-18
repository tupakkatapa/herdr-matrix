//! The owner's `!` commands.

use std::fs;
use std::path::{Path, PathBuf};

use super::contacts::Why;
use super::{
    AgentSettings, Bot, FOLLOW_UP, Post, RETRY_MAX, SCREEN_LINES, SCREEN_MAX, Starting, To, due,
    herdr_failed, not_running, same_dir,
};
use crate::Result;
use crate::commands::{Cmd, HELP, Place, command, contact, helps, suggest};
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
            let said = match suggest(&word[1..], place) {
                Some(near) => format!("Unknown command {word}. Did you mean !{near}?"),
                None => format!("Unknown command {word}. !help lists the commands."),
            };
            self.say(&said, to)?;
            return Ok(());
        };
        if entry.help(place).is_none() {
            self.log(word);
            self.say(&format!("{word} works in the main chat only."), to)?;
            return Ok(());
        }
        let a = if entry.cmd.acts() {
            if let Some(contact) = contact.filter(|_| mine.is_none()) {
                let name = self.chat_agent_name(contact);
                self.say(&not_running(name.as_deref()), to)?;
                return Ok(());
            }
            let Some(a) = mine.map_or_else(|| self.target(ev, agents, None), |a| Ok(Some(a)))?
            else {
                return Ok(());
            };
            self.log(&format!("{} {word}", a.pane_id));
            Some(a)
        } else {
            self.log(word);
            None
        };
        let agent = || a.expect("an acting command has its agent");
        let replied = ev
            .reply_to
            .as_ref()
            .and_then(|q| self.state.sent.get(q))
            .cloned();
        let replied = replied.as_deref();
        let kill =
            (word == "!kill").then_some("!kill closes the chat; to quit the agent, use !exit.");
        match entry.cmd {
            Cmd::Agents => {
                let head = if self.away() {
                    "Agents (away is on):"
                } else {
                    "Agents (away is off):"
                };
                self.ask_which(head, agents)?;
            }
            Cmd::Attach => self.attach(rest, agents, replied)?,
            Cmd::New => self.new_agent(rest, agents)?,
            Cmd::Dm => self.open_chat(rest, agents, replied)?,
            Cmd::Leave => self.leave(rest, agents, chat, contact, replied, kill)?,
            Cmd::Last => self.last(agent(), to)?,
            Cmd::Screen => self.screen(agent(), rest, to)?,
            Cmd::Stop => self.stop(agent(), to)?,
            Cmd::Exit => self.exit(agent(), rest, chat, contact)?,
            Cmd::Away => {
                self.set_away(
                    Away::On,
                    "Away is on: every agent that stops or asks posts here. !here turns it off.",
                    to,
                )?;
            }
            Cmd::Here => {
                self.set_away(
                    Away::Off,
                    "Away is off: only answers to what you send from here come here.",
                    to,
                )?;
            }
            Cmd::Help => self.help(place, mine, to)?,
        }
        Ok(())
    }

    /// Turns away on or off; the main chat's name shows it.
    fn set_away(&mut self, away: Away, said: &str, to: To) -> Result<()> {
        self.state.away = away;
        self.save()?;
        let name = self.attached_name().map(str::to_owned);
        self.nicked = self.retry("nick", RETRY_MAX, |bot| bot.nick(name.as_deref()));
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
        let lines = if rest.is_empty() {
            SCREEN_LINES
        } else if rest.chars().all(|c| c.is_ascii_digit()) {
            rest.parse::<u64>()
                .map_or(SCREEN_MAX, |n| n.clamp(1, SCREEN_MAX))
        } else {
            let said = format!("!screen takes a number of lines, up to {SCREEN_MAX}.");
            self.say(&said, to)?;
            return Ok(());
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
            let said = if a.status == Status::Blocked {
                format!(
                    "**{}** is not working; it waits on a choice. Answer esc to cancel it.",
                    a.name
                )
            } else {
                format!("**{}** is not working; nothing to stop.", a.name)
            };
            self.say(&said, to)?;
            return Ok(());
        }
        let sent = herdr::send_keys(self.deps.herdr.as_mut(), &a.pane_id, "esc");
        self.done(&a.pane_id)?;
        let said = match sent {
            Ok(()) => format!("Interrupted **{}**.", a.name),
            Err(crate::Error::Herdr(out)) => herdr_failed("stop", &a.name, &out),
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
        self.say(&format!("{}\n\n{text}", helps(place)), to)?;
        Ok(())
    }

    fn attach(&mut self, rest: &str, agents: &[Agent], replied: Option<&str>) -> Result<()> {
        let Some(a) = self.pick(rest, agents, "Attach to which", replied)? else {
            return Ok(());
        };
        self.attach_to(a)?;
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
        self.send_kept(a, agents)
    }

    /// Quits an agent and closes its pane; asks first, and acts only on
    /// `now` for the agent it asked about lately.
    fn exit(
        &mut self,
        a: &Agent,
        rest: &str,
        chat: Option<&Chat>,
        contact: Option<&str>,
    ) -> Result<()> {
        let pane = a.pane_id.as_str();
        let to = To::chat(chat).pane(pane);
        let now = self.now();
        let confirmed = rest.to_lowercase() == "now"
            && self
                .exit_asked
                .as_ref()
                .is_some_and(|(asked, at)| asked == pane && !due(now, *at, FOLLOW_UP));
        if !confirmed {
            self.exit_asked = Some((pane.to_owned(), now));
            let said = format!("Send !exit now to quit **{}** and close its pane.", a.name);
            self.say(&said, to)?;
            return Ok(());
        }
        match herdr::close_pane(self.deps.herdr.as_mut(), pane) {
            Err(crate::Error::Herdr(out)) => {
                self.say(&herdr_failed("close", &a.name, &out), to)?;
                return Ok(());
            }
            other => other?,
        }
        self.exit_asked = None;
        self.done(pane)?;
        if self.state.attached.as_deref() == Some(pane) {
            self.state.attached = None;
            self.nicked = self.retry("nick", RETRY_MAX, |bot| bot.nick(None));
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
                Some("Start an agent in which folder?".to_owned()),
                dirs.clone(),
            ),
            (Some(q), 0) => (
                Some(format!(
                    "No folder under {} matches \"{q}\". Start an agent in which one?",
                    self.deps.workspace.display()
                )),
                files::folders(&self.deps.workspace, None),
            ),
            (Some(q), n) => (
                Some(format!(
                    "\"{q}\" matches {n} folders. Start an agent in which one?"
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
            .find(|a| a.cwd.as_deref().is_some_and(|cwd| same_dir(cwd, path)))
        {
            self.attach_to(running)?;
            self.post(
                running,
                "runs there already; attached.",
                Post::new(&[], None).no_screen(),
            )?;
            return self.send_kept(running, agents);
        }
        let settings = match settings_at(&self.deps.agents, path) {
            Ok(settings) => settings.cloned(),
            Err((id, cwd)) => {
                let its = cwd
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or_default();
                let said = format!(
                    "**{path}** is inside the folder of the confined agent **{id}**; start it with !new {its}."
                );
                self.say(&said, To::default())?;
                return Ok(());
            }
        };
        let (pane, label) = self.start(path, settings.as_ref())?;
        let kind = settings.map(|s| s.kind).unwrap_or_default();
        self.state.attached = Some(pane.clone());
        self.attached_name = Some((pane.clone(), label.clone()));
        self.save()?;
        self.nicked = self.retry("nick", RETRY_MAX, |bot| bot.nick(Some(&label)));
        self.expect(
            &pane,
            &Turn::default(),
            Origin {
                started: true,
                ..Origin::default()
            },
        );
        let home = std::env::var("HOME").unwrap_or_default();
        let said = format!(
            "Started {kind} as **{label}** in {}; this chat is attached to it.",
            tilde(path, &home)
        );
        self.say(&said, To::default())?;
        Ok(())
    }

    /// Starts an agent in `path`, as its settings say: a new workspace,
    /// then the command of a confined agent, else the program of its kind.
    /// The pane is followed until the agent is ready, and holds the own
    /// chat of a configured agent. Its pane and label.
    fn start(&mut self, path: &str, settings: Option<&AgentSettings>) -> Result<(String, String)> {
        let label = path.rsplit('/').next().unwrap_or_default().to_owned();
        let kind = settings.map(|s| s.kind.clone()).unwrap_or_default();
        // The shell left after a confined agent would not be confined
        let command = settings.and_then(|s| s.sandbox.as_ref()).map_or_else(
            || kind.program().to_owned(),
            |s| format!("exec herdr-matrix-agent-{}", s.id),
        );
        let herdr = self.deps.herdr.as_mut();
        let pane = herdr::create_workspace(herdr, path, &label)?;
        herdr::run_in_pane(herdr, &pane, &command)?;
        let starting = Starting {
            folder: label.clone(),
            at: self.now(),
            seen: false,
            missing: None,
        };
        self.starting.insert(pane.clone(), starting);
        if let Some(s) = settings {
            self.started_for.insert(pane.clone(), (s.cwd.clone(), None));
        }
        Ok((pane, label))
    }

    /// Starts each agent set to start that does not run in its folder, once
    /// in a run of the bot; the main chat stays as it is.
    pub(super) fn autostart(&mut self, agents: &[Agent]) {
        let due: Vec<String> = self
            .deps
            .agents
            .iter()
            .filter(|s| s.autostart && !self.autostarted.contains(&s.cwd))
            .filter(|s| {
                !agents
                    .iter()
                    .any(|a| a.cwd.as_deref().is_some_and(|cwd| same_dir(cwd, &s.cwd)))
            })
            .map(|s| s.cwd.clone())
            .collect();
        for cwd in due {
            let path = cwd.strip_suffix('/').unwrap_or(&cwd);
            if !Path::new(path).is_dir() {
                self.log(&format!("autostart {cwd}: no such folder"));
                continue;
            }
            self.autostarted.insert(cwd.clone());
            let settings = match settings_at(&self.deps.agents, path) {
                Ok(settings) => settings.cloned(),
                Err((id, _)) => {
                    let said =
                        format!("autostart {cwd}: inside the folder of the confined agent {id}");
                    self.log(&said);
                    continue;
                }
            };
            match self.start(path, settings.as_ref()) {
                Ok((pane, _)) => self.log(&format!("{pane} autostarted in {cwd}")),
                Err(err) => self.log(&format!("error: autostart {cwd}: {err}")),
            }
        }
    }
}

/// A path with the home directory in it as `~`.
fn tilde(path: &str, home: &str) -> String {
    match path.strip_prefix(home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => {
            format!("~{rest}")
        }
        _ => path.to_owned(),
    }
}

/// The settings of the agent working in `path`, as written or with
/// symlinks resolved. Err with the id and `cwd` of a confined agent whose
/// folder holds `path`: that agent writes there, and could have made it a
/// link anywhere.
fn settings_at<'a>(
    agents: &'a [AgentSettings],
    path: &str,
) -> std::result::Result<Option<&'a AgentSettings>, (&'a str, &'a str)> {
    let resolved = |p: &str| fs::canonicalize(p).unwrap_or_else(|_| p.into());
    let paths = [PathBuf::from(path), resolved(path)];
    for s in agents {
        let Some(sandbox) = &s.sandbox else { continue };
        let cwds = [PathBuf::from(&s.cwd), resolved(&s.cwd)];
        let inside = paths
            .iter()
            .any(|p| cwds.iter().any(|cwd| p.starts_with(cwd) && p != cwd));
        if inside && !cwds.contains(&paths[1]) {
            return Err((&sandbox.id, &s.cwd));
        }
    }
    Ok(agents
        .iter()
        .find(|s| same_dir(&s.cwd, path) || resolved(&s.cwd) == paths[1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_is_shown_as_a_tilde() {
        assert_eq!(
            tilde("/home/kari/Workspace/a", "/home/kari"),
            "~/Workspace/a"
        );
        assert_eq!(tilde("/home/kari", "/home/kari"), "~");
        assert_eq!(tilde("/home/karin/a", "/home/kari"), "/home/karin/a");
        assert_eq!(tilde("/w/a", ""), "/w/a");
    }
}
