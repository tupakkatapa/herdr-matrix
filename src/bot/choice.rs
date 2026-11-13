//! The choice an agent waits on, as the chat shows it, and what the owner
//! was shown of it.

use serde_json::{Value, json};

use super::{Bot, CHOICE_LINES, Post, To};
use crate::Result;
use crate::commands::{KEY_HINT, THUMBS_UP};
use crate::herdr::{Agent, Choice, Questions, Turn, choice};
use crate::render::render;
use crate::state::{Asked, Chat, fingerprint};

/// The choice an agent waits on, as the chat shows it.
enum Shown {
    Question(Questions),
    Choice(Choice),
    Nothing,
}

impl Shown {
    /// What the fingerprint is taken of.
    fn value(&self) -> Value {
        match self {
            Self::Question(q) => q.raw.clone(),
            Self::Choice(c) => json!([c.question, c.options, c.above]),
            Self::Nothing => Value::Null,
        }
    }
}

impl Bot {
    fn shown_now(&mut self, a: &Agent, turn: &Turn) -> Result<Shown> {
        if let Some(q) = turn.question.as_ref().filter(|q| !q.list.is_empty()) {
            return Ok(Shown::Question(q.clone()));
        }
        Ok(match choice(&self.read(&a.pane_id, CHOICE_LINES)?) {
            Some(c) => Shown::Choice(c),
            None => Shown::Nothing,
        })
    }

    /// The fingerprint of the choice an agent waits on now.
    pub(super) fn shown(&mut self, a: &Agent, turn: &Turn) -> Result<String> {
        Ok(fingerprint(&self.shown_now(a, turn)?.value()))
    }

    /// The question an agent waits on, or its screen for any other choice;
    /// a reaction to this message answers it. Remembers what it showed, so
    /// an answer goes only to that choice.
    pub(super) fn ask(&mut self, a: &Agent, turn: &Turn, how: Post) -> Result<()> {
        let mine = self.own_voice(a, how.agents, how.chat);
        let shown = self.shown_now(a, turn)?;
        let note = if mine { "" } else { "asks:" };
        let event = match &shown {
            Shown::Question(q) => {
                let text = render(&q.list);
                self.post(a, note, how.text(Some(&text)))?
            }
            Shown::Choice(c) => {
                let listed: Vec<String> = c
                    .options
                    .iter()
                    .enumerate()
                    .map(|(i, o)| format!("{}. {o}", i + 1))
                    .collect();
                let context = if c.above.is_empty() {
                    String::new()
                } else {
                    format!("\n\n```\n{}\n```", c.above)
                };
                let text = format!(
                    "**{}**\n{}\n\nAnswer with a number, or react {THUMBS_UP} for 1.{context}",
                    c.question,
                    listed.join("\n")
                );
                self.post(a, note, how.text(Some(&text)))?
            }
            Shown::Nothing => self.post(
                a,
                &format!("waits on a choice. Answer with one key: {KEY_HINT}."),
                how,
            )?,
        };
        self.state.asked.insert(
            a.pane_id.clone(),
            Asked {
                event,
                shown: Some(fingerprint(&shown.value())),
            },
        );
        self.save()
    }

    /// True when the choice an agent waits on is not the one the owner was
    /// shown; it is shown anew.
    pub(super) fn changed(
        &mut self,
        a: &Agent,
        agents: &[Agent],
        turn: &Turn,
        chat: Option<&Chat>,
    ) -> Result<bool> {
        let seen = self.state.asked.get(&a.pane_id).cloned();
        if let Some(seen) = &seen
            && seen.shown.as_deref() == Some(self.shown(a, turn)?.as_str())
        {
            return Ok(false);
        }
        if seen.is_some() {
            self.say(
                "The choice changed before your answer; nothing was sent.",
                To::chat(chat).pane(&a.pane_id),
            )?;
        }
        self.ask(a, turn, Post::new(agents, chat))?;
        Ok(true)
    }
}
