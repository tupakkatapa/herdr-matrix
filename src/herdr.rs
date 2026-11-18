//! herdr and its agents: the CLI and attached clients.

use std::fmt;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

use indexmap::IndexMap;
use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::json::{key, key_str};
use crate::kind::Kind;
use crate::process::run_timed;
use crate::{Error, Result};

pub const HERDR_TIMEOUT: Duration = Duration::from_secs(15);
/// Lines above a choice on the screen that show what it is about
const CHOICE_CONTEXT: usize = 8;

/// What an agent does, as herdr says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Working,
    Blocked,
    Idle,
    Done,
    Other(String),
}

impl Status {
    #[must_use]
    pub fn parse(word: &str) -> Self {
        match word {
            "working" => Self::Working,
            "blocked" => Self::Blocked,
            "idle" => Self::Idle,
            "done" => Self::Done,
            other => Self::Other(other.to_owned()),
        }
    }

    /// True when the agent stopped or waits: worth a notice.
    #[must_use]
    pub fn notifies(&self) -> bool {
        matches!(self, Self::Blocked | Self::Idle | Self::Done)
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Idle => "idle",
            Self::Done => "done",
            Self::Other(word) => word,
        })
    }
}

/// The agent program in a pane and its session id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub agent: String,
    pub id: String,
}

/// An agent of `herdr agent list`, with the name the chat knows it by.
#[derive(Clone, Debug, PartialEq)]
pub struct Agent {
    pub pane_id: String,
    pub status: Status,
    pub name: String,
    pub kind: Kind,
    pub cwd: Option<String>,
    pub session: Option<Session>,
}

/// An option of a question.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Opt {
    pub label: String,
    pub description: Option<String>,
}

/// A question of Claude's `AskUserQuestion`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Question {
    pub question: String,
    pub multi_select: bool,
    pub options: Vec<Opt>,
}

/// The questions an agent waits on, and their JSON as the transcript holds
/// it, which the choice's fingerprint is taken of.
#[derive(Clone, Debug, PartialEq)]
pub struct Questions {
    pub raw: Value,
    pub list: Vec<Question>,
}

impl From<Value> for Questions {
    fn from(raw: Value) -> Self {
        let one = |v: &Value| serde_json::from_value(v.clone()).unwrap_or_default();
        let list = match &raw {
            Value::Array(items) => items.iter().map(one).collect(),
            other => vec![one(other)],
        };
        Self { raw, list }
    }
}

/// What an agent did since the last prompt: its messages as (id, text),
/// and the questions it waits on, if any.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Turn {
    pub messages: Vec<(Option<String>, String)>,
    pub question: Option<Questions>,
}

/// The herdr CLI: its exit code and standard output.
pub trait Herdr {
    /// # Errors
    /// When herdr cannot be started.
    fn run(&mut self, args: &[&str]) -> Result<(i32, String)>;
}

pub struct RunHerdr {
    pub program: PathBuf,
    pub timeout: Duration,
}

impl Default for RunHerdr {
    fn default() -> Self {
        Self {
            program: "herdr".into(),
            timeout: HERDR_TIMEOUT,
        }
    }
}

impl Herdr for RunHerdr {
    fn run(&mut self, args: &[&str]) -> Result<(i32, String)> {
        Ok(match run_timed(&self.program, args, self.timeout)? {
            Some(ran) => (ran.code, ran.stdout),
            None => (
                1,
                format!("herdr {} timed out", args[..args.len().min(2)].join(" ")),
            ),
        })
    }
}

/// A herdr call that must succeed; its output.
fn checked(herdr: &mut dyn Herdr, args: &[&str]) -> Result<String> {
    let (code, out) = herdr.run(args)?;
    if code == 0 {
        Ok(out)
    } else {
        Err(Error::Herdr(out.trim().to_owned()))
    }
}

/// Agents with a name: the workspace label, numbered when a workspace
/// runs several.
///
/// # Errors
/// When herdr fails or answers what is not an agent list.
pub fn list_agents(herdr: &mut dyn Herdr) -> Result<Vec<Agent>> {
    let (code, out) = herdr.run(&["agent", "list"])?;
    if code != 0 {
        return Err(Error::Herdr(format!("agent list: {}", out.trim())));
    }
    let (code, ws) = herdr.run(&["workspace", "list"])?;
    let mut labels = Map::new();
    if code == 0 {
        let ws: Value = serde_json::from_str(&ws)?;
        for w in key(key(&ws, "result")?, "workspaces")?
            .as_array()
            .into_iter()
            .flatten()
        {
            labels.insert(
                key_str(w, "workspace_id")?.to_owned(),
                key(w, "label")?.clone(),
            );
        }
    }
    let out: Value = serde_json::from_str(&out)?;
    let mut agents = Vec::new();
    for raw in key(key(&out, "result")?, "agents")?
        .as_array()
        .into_iter()
        .flatten()
    {
        let workspace = key_str(raw, "workspace_id")?;
        let label = labels
            .get(workspace)
            .and_then(Value::as_str)
            .unwrap_or(workspace)
            .to_owned();
        let session = raw.get("agent_session").map(|s| Session {
            agent: s
                .get("agent")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            id: s
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        });
        let kind = raw
            .get("agent")
            .and_then(Value::as_str)
            .or(session.as_ref().map(|s| s.agent.as_str()))
            .map_or_else(|| Kind::Other(String::new()), Kind::parse);
        agents.push(Agent {
            pane_id: key_str(raw, "pane_id")?.to_owned(),
            status: Status::parse(key_str(raw, "agent_status")?),
            name: label,
            kind,
            cwd: raw.get("cwd").and_then(Value::as_str).map(str::to_owned),
            session,
        });
    }
    let mut total: IndexMap<String, usize> = IndexMap::new();
    for a in &agents {
        *total.entry(a.name.clone()).or_default() += 1;
    }
    let mut seen: IndexMap<String, usize> = IndexMap::new();
    for a in &mut agents {
        let n = seen.entry(a.name.clone()).or_default();
        *n += 1;
        if total[&a.name] > 1 {
            a.name = format!("{}:{n}", a.name);
        }
    }
    Ok(agents)
}

/// The last `lines` of an agent's screen; a placeholder when herdr cannot
/// read it.
///
/// # Errors
/// When herdr cannot be started.
pub fn read(herdr: &mut dyn Herdr, pane: &str, lines: u64) -> Result<String> {
    let lines = lines.to_string();
    let (code, out) = herdr.run(&[
        "agent", "read", pane, "--source", "recent", "--lines", &lines, "--format", "text",
    ])?;
    Ok(if code == 0 {
        out.trim_end().to_owned()
    } else {
        format!("(cannot read {pane})")
    })
}

/// # Errors
/// When herdr fails; `Error::Herdr` holds what it said.
pub fn send_keys(herdr: &mut dyn Herdr, pane: &str, key: &str) -> Result<()> {
    checked(herdr, &["agent", "send-keys", pane, key]).map(drop)
}

/// # Errors
/// When herdr fails; `Error::Herdr` holds what it said.
pub fn prompt(herdr: &mut dyn Herdr, pane: &str, text: &str) -> Result<()> {
    checked(herdr, &["agent", "prompt", pane, text]).map(drop)
}

/// # Errors
/// When herdr fails; `Error::Herdr` holds what it said.
pub fn send_text(herdr: &mut dyn Herdr, pane: &str, text: &str) -> Result<()> {
    checked(herdr, &["pane", "send-text", pane, text]).map(drop)
}

/// # Errors
/// When herdr fails; `Error::Herdr` holds what it said.
pub fn close_pane(herdr: &mut dyn Herdr, pane: &str) -> Result<()> {
    checked(herdr, &["pane", "close", pane]).map(drop)
}

/// A new workspace in `cwd`; its first pane.
///
/// # Errors
/// When herdr fails or its answer names no pane.
pub fn create_workspace(herdr: &mut dyn Herdr, cwd: &str, label: &str) -> Result<String> {
    let out = checked(
        herdr,
        &[
            "workspace",
            "create",
            "--cwd",
            cwd,
            "--label",
            label,
            "--no-focus",
        ],
    )?;
    let out: Value = serde_json::from_str(&out)?;
    Ok(key_str(key(key(&out, "result")?, "root_pane")?, "pane_id")?.to_owned())
}

/// # Errors
/// When herdr fails; `Error::Herdr` holds what it said.
pub fn run_in_pane(herdr: &mut dyn Herdr, pane: &str, command: &str) -> Result<()> {
    checked(herdr, &["pane", "run", pane, command]).map(drop)
}

/// The agent a query names: its whole name or pane id, else the only one
/// whose name holds it; else how many names hold it.
///
/// # Errors
/// The number of agents whose names hold the query, other than one.
pub fn find<'a>(agents: &'a [Agent], query: &str) -> Result<&'a Agent, usize> {
    let query = query.to_lowercase();
    if let Some(a) = agents
        .iter()
        .find(|a| query == a.name.to_lowercase() || query == a.pane_id.to_lowercase())
    {
        return Ok(a);
    }
    let matches: Vec<&Agent> = agents
        .iter()
        .filter(|a| a.name.to_lowercase().contains(&query))
        .collect();
    if let [a] = matches[..] {
        Ok(a)
    } else {
        Err(matches.len())
    }
}

/// A choice shown on an agent's screen, such as a permission prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub question: String,
    pub options: Vec<String>,
    /// The lines above it
    pub above: String,
}

/// Lines split at `\n`, `\r\n`, `\r` and the other Unicode line breaks.
fn splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            lines.push(&text[start..i]);
            start = i + c.len_utf8();
            if c == '\r' && chars.peek().is_some_and(|&(_, n)| n == '\n') {
                chars.next();
                start += 1;
            }
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// The choice on a screen, or None.
#[must_use]
pub fn choice(screen: &str) -> Option<Choice> {
    static OPTION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^\s*(?:\u{276F}\s*)?(\d)\.\s+(\S.*)$").expect("a valid pattern")
    });
    let lines: Vec<&str> = splitlines(screen).into_iter().map(str::trim_end).collect();
    let mut found: Vec<(usize, String, String)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, line)| {
            OPTION
                .captures(line)
                .map(|m| (i, m[1].to_owned(), m[2].to_owned()))
        })
        .collect();
    // The prompt's own list is the last one; numbered output may stand above
    if let Some(start) = found.iter().rposition(|(_, n, _)| n == "1") {
        found.drain(..start);
    }
    let first = found.first()?.0;
    let asked = (0..first).rev().find(|&i| !lines[i].trim().is_empty())?;
    let above: Vec<&str> = lines[asked.saturating_sub(CHOICE_CONTEXT)..asked]
        .iter()
        .copied()
        .filter(|line| !line.trim().is_empty())
        .collect();
    Some(Choice {
        question: lines[asked].trim().to_owned(),
        options: found.into_iter().map(|(_, _, o)| o).collect(),
        above: above.join("\n"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Lists(Value);

    impl Herdr for Lists {
        fn run(&mut self, args: &[&str]) -> Result<(i32, String)> {
            Ok(match args {
                ["agent", "list"] => (0, json!({"result": {"agents": self.0}}).to_string()),
                _ => (1, String::new()),
            })
        }
    }

    #[test]
    fn agent_list_names_each_kind() {
        let raw = |pane: &str, extra: Value| {
            let mut agent = json!({"pane_id": pane, "workspace_id": "w", "agent_status": "idle"});
            agent
                .as_object_mut()
                .expect("an object")
                .extend(extra.as_object().expect("an object").clone());
            agent
        };
        let session =
            json!({"agent": "claude", "kind": "id", "source": "herdr:claude", "value": "s"});
        let mut herdr = Lists(json!([
            raw("w:1", json!({"agent": "pi"})),
            raw("w:2", json!({"agent": "claude", "agent_session": session})),
            raw("w:3", json!({"agent": "codex"})),
            raw("w:4", json!({"agent_session": session})),
            raw("w:5", json!({})),
        ]));
        let kinds: Vec<Kind> = list_agents(&mut herdr)
            .expect("agents")
            .into_iter()
            .map(|a| a.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                Kind::Pi,
                Kind::Claude,
                Kind::Other("codex".into()),
                Kind::Claude,
                Kind::Other(String::new())
            ]
        );
    }

    #[test]
    fn choice_ignores_a_numbered_list_above_the_prompt() {
        let screen = "Plan:\n1. Build it\n2. Test it\n\n \
                      Do you want to proceed?\n ❯ 1. Yes\n   2. No (esc)\n";
        assert_eq!(
            choice(screen),
            Some(Choice {
                question: "Do you want to proceed?".into(),
                options: vec!["Yes".into(), "No (esc)".into()],
                above: "Plan:\n1. Build it\n2. Test it".into()
            })
        );
    }
}
