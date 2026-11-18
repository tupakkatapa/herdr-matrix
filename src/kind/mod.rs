//! Kinds of agents, and a reader for each kind whose own record of a turn
//! the bot knows: Claude Code's transcripts and pi's sessions.

pub mod claude;
pub mod pi;

use std::fmt;
use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::files::open_inside;
use crate::herdr::{Agent, Turn};

/// Bytes of a session file read from its end
pub const TAIL: u64 = 512 * 1024;

/// The agent program in a pane, as herdr names it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub enum Kind {
    #[default]
    Claude,
    Pi,
    Other(String),
}

impl Kind {
    #[must_use]
    pub fn parse(label: &str) -> Self {
        match label {
            "claude" => Self::Claude,
            "pi" => Self::Pi,
            other => Self::Other(other.to_owned()),
        }
    }

    /// The command that starts it.
    #[must_use]
    pub fn program(&self) -> &str {
        match self {
            Self::Claude => "claude",
            Self::Pi => "pi",
            Self::Other(label) => label,
        }
    }
}

/// A kind of the settings: one with a reader.
impl TryFrom<String> for Kind {
    type Error = String;

    fn try_from(label: String) -> Result<Self, String> {
        match Self::parse(&label) {
            Self::Other(_) => Err(format!("unknown agent kind \"{label}\"")),
            kind => Ok(kind),
        }
    }
}

/// The name the chat gives it.
impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Self::Claude => "Claude",
            other => other.program(),
        })
    }
}

/// What the agent did since the last prompt, from its own record; None for
/// a kind without a reader. `home` stands for `$HOME`, and for pi's
/// variable too.
#[must_use]
pub fn turn(agent: &Agent, home: Option<&Path>) -> Option<Turn> {
    match agent.kind {
        Kind::Claude => claude::turn(agent, home),
        Kind::Pi => pi::turn(agent, &home.map_or_else(pi::dir, pi::default_dir)),
        Kind::Other(_) => None,
    }
}

/// What a confined agent of `kind` did since the last prompt, from the
/// newest session file in `state`, its state directory. The agent writes
/// there: no symlink is followed and only regular files are read.
#[must_use]
pub fn confined_turn(kind: &Kind, state: &Path) -> Option<Turn> {
    let (sessions, parse): (&str, fn(&str) -> Turn) = match kind {
        Kind::Claude => ("projects", claude::parse),
        Kind::Pi => ("sessions", pi::parse),
        Kind::Other(_) => return None,
    };
    let newest = newest(
        entries(&state.join(sessions))
            .filter(|dir| dir.file_type().is_ok_and(|t| t.is_dir()))
            .flat_map(|dir| entries(&dir.path()).collect::<Vec<_>>()),
    )?;
    Some(parse(&tail(open_inside(state, &newest).ok()?, TAIL)?))
}

/// The entries of a directory, but hidden ones.
fn entries(dir: &Path) -> impl Iterator<Item = fs::DirEntry> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
}

/// The newest regular `.jsonl` file of `entries`.
fn newest(entries: impl Iterator<Item = fs::DirEntry>) -> Option<PathBuf> {
    let (_, path) = entries
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| e.file_name().to_string_lossy().ends_with(".jsonl"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max()?;
    Some(path)
}

/// The last `bytes` of a session file, from its first whole line.
fn tail(mut file: fs::File, bytes: u64) -> Option<String> {
    let end = file.seek(SeekFrom::End(0)).ok()?;
    let start = end.saturating_sub(bytes);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut data = Vec::new();
    file.read_to_end(&mut data).ok()?;
    let text = String::from_utf8_lossy(&data);
    Some(if start > 0 {
        text.split_once('\n')
            .map_or_else(String::new, |(_, rest)| rest.to_owned())
    } else {
        text.into_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_of_the_settings_has_a_reader() {
        let kind = |label: &str| serde_json::from_value::<Kind>(serde_json::json!(label));
        assert_eq!(kind("pi").expect("pi"), Kind::Pi);
        assert_eq!(kind("claude").expect("claude"), Kind::Claude);
        assert!(kind("codex").is_err());
        assert_eq!(Kind::parse("codex"), Kind::Other("codex".into()));
        assert_eq!(
            [Kind::Claude, Kind::Pi, Kind::parse("codex")].map(|k| k.to_string()),
            ["Claude", "pi", "codex"]
        );
    }
}
