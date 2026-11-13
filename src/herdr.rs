//! herdr and its agents: the CLI, attached clients and Claude's transcripts.

use std::fmt;
use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use indexmap::IndexMap;
use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::json::{key, key_str};
use crate::process::run_timed;
use crate::{Error, Result};

/// Bytes of a transcript read from its end
pub const TRANSCRIPT_TAIL: u64 = 512 * 1024;
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
        agents.push(Agent {
            pane_id: key_str(raw, "pane_id")?.to_owned(),
            status: Status::parse(key_str(raw, "agent_status")?),
            name: label,
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

/// What Claude did since the last prompt, from its transcript. None for
/// other agents.
#[must_use]
pub fn claude_turn(agent: &Agent, home: Option<&Path>) -> Option<Turn> {
    read_turn(agent.session.as_ref()?, home, TRANSCRIPT_TAIL)
}

/// `claude_turn` from the last `tail` bytes of the transcript.
#[must_use]
pub fn read_turn(session: &Session, home: Option<&Path>, tail: u64) -> Option<Turn> {
    if session.agent != "claude" || session.id.is_empty() {
        return None;
    }
    let home = home.map_or_else(
        || PathBuf::from(std::env::var("HOME").unwrap_or_default()),
        Path::to_path_buf,
    );
    let projects = fs::read_dir(home.join(".claude").join("projects")).ok()?;
    let path = projects
        .filter_map(std::result::Result::ok)
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path().join(format!("{}.jsonl", session.id)))
        .find(|p| p.exists())?;
    let mut file = fs::File::open(path).ok()?;
    let end = file.seek(SeekFrom::End(0)).ok()?;
    let start = end.saturating_sub(tail);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut data = Vec::new();
    file.read_to_end(&mut data).ok()?;
    let text = String::from_utf8_lossy(&data);
    let mut lines = text.split('\n');
    if start > 0 {
        lines.next();
    }
    Some(parse_turn(lines))
}

fn flag(entry: &Value, name: &str) -> bool {
    entry.get(name).and_then(Value::as_bool).unwrap_or(false)
}

fn parse_turn<'a>(lines: impl Iterator<Item = &'a str>) -> Turn {
    let mut messages = Vec::new();
    let mut asked: IndexMap<Option<String>, Option<Value>> = IndexMap::new();
    for line in lines {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if flag(&entry, "isSidechain") {
            continue;
        }
        let content = entry.get("message").and_then(|m| m.get("content"));
        let blocks = content
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let kind = |b: &Value| b.get("type").and_then(Value::as_str).map(str::to_owned);
        match entry.get("type").and_then(Value::as_str) {
            Some("user") => {
                for b in blocks {
                    let id = b
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    asked.shift_remove(&id);
                }
                if !flag(&entry, "isMeta")
                    && (content.is_some_and(Value::is_string)
                        || blocks.iter().any(|b| kind(b).as_deref() == Some("text")))
                {
                    messages.clear();
                    asked.clear();
                }
            }
            Some("assistant") => {
                let said: Vec<&str> = blocks
                    .iter()
                    .filter(|b| kind(b).as_deref() == Some("text"))
                    .filter_map(|b| b.get("text").and_then(Value::as_str))
                    .filter(|t| !t.is_empty())
                    .collect();
                if !said.is_empty() {
                    let uuid = entry.get("uuid").and_then(Value::as_str).map(str::to_owned);
                    messages.push((uuid, said.join("\n\n")));
                }
                for b in blocks {
                    if kind(b).as_deref() == Some("tool_use")
                        && b.get("name").and_then(Value::as_str) == Some("AskUserQuestion")
                    {
                        let id = b.get("id").and_then(Value::as_str).map(str::to_owned);
                        let questions =
                            b.get("input")
                                .and_then(|i| i.get("questions"))
                                .filter(|q| match q {
                                    Value::Array(items) => !items.is_empty(),
                                    Value::Object(map) => !map.is_empty(),
                                    _ => false,
                                });
                        asked.insert(id, questions.cloned());
                    }
                }
            }
            _ => {}
        }
    }
    Turn {
        messages,
        question: asked
            .into_values()
            .next_back()
            .flatten()
            .map(Questions::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use serde_json::json;

    fn fruit() -> Value {
        json!([{"question": "Pick a fruit?", "header": "Fruit", "multiSelect": false,
                "options": [{"label": "Apple", "description": "red"},
                            {"label": "Pear", "description": "green"}]}])
    }

    fn write_lines(path: &Path, lines: &[Value]) -> String {
        let text: String = lines.iter().map(|x| x.to_string() + "\n").collect();
        fs::write(path, &text).expect("write");
        text
    }

    fn append(path: &Path, line: &Value) {
        let mut text = fs::read_to_string(path).expect("read");
        text.push_str(&line.to_string());
        text.push('\n');
        fs::write(path, text).expect("write");
    }

    fn turn(messages: &[(&str, &str)], question: Option<Value>) -> Turn {
        Turn {
            messages: messages
                .iter()
                .map(|(u, t)| (Some((*u).to_owned()), (*t).to_owned()))
                .collect(),
            question: question.map(Questions::from),
        }
    }

    fn claude(id: &str) -> Agent {
        Agent {
            pane_id: "w:p".into(),
            status: Status::Idle,
            name: "w".into(),
            cwd: None,
            session: Some(Session {
                agent: "claude".into(),
                id: id.into(),
            }),
        }
    }

    #[test]
    fn claude_turn_reads_the_transcript() {
        let dir = TempDir::new();
        let home = dir.path().join("home");
        let project = home.join(".claude/projects/-x");
        fs::create_dir_all(&project).expect("mkdir");
        let path = project.join("sid.jsonl");
        write_lines(
            &path,
            &[
                json!({"type": "user", "message": {"content": "first"}}),
                json!({"type": "assistant", "uuid": "a1", "message": {"content": [{"type": "text", "text": "old"}]}}),
                json!({"type": "user", "message": {"content": "Hello"}}),
                json!({"type": "assistant", "uuid": "a2", "message": {"content": [
                {"type": "text", "text": "Looking."}, {"type": "tool_use", "name": "Bash"}]}}),
                json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": "x"}]}}),
                json!({"type": "assistant", "uuid": "a3", "isSidechain": true,
                   "message": {"content": [{"type": "text", "text": "side"}]}}),
                json!({"type": "assistant", "uuid": "a4", "message": {"content": [{"type": "text", "text": "Done."}]}}),
                json!({"type": "mode"}),
            ],
        );
        let agent = claude("sid");
        assert_eq!(
            claude_turn(&agent, Some(&home)),
            Some(turn(&[("a2", "Looking."), ("a4", "Done.")], None))
        );
        append(
            &path,
            &json!({"type": "assistant", "uuid": "a5", "message": {"content": [
            {"type": "tool_use", "id": "t1", "name": "AskUserQuestion", "input": {"questions": fruit()}}]}}),
        );
        assert_eq!(
            claude_turn(&agent, Some(&home))
                .expect("turn")
                .question
                .map(|q| q.raw),
            Some(fruit())
        );
        append(
            &path,
            &json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "x"}]}}),
        );
        assert_eq!(
            claude_turn(&agent, Some(&home)).expect("turn").question,
            None
        );
        assert_eq!(claude_turn(&claude("nope"), Some(&home)), None);
        let bare = Agent {
            session: None,
            ..claude("sid")
        };
        assert_eq!(claude_turn(&bare, Some(&home)), None);
    }

    #[test]
    fn transcript_tail_starts_at_a_whole_line() {
        let dir = TempDir::new();
        let home = dir.path().join("home");
        let project = home.join(".claude/projects/-x");
        fs::create_dir_all(&project).expect("mkdir");
        let lines = [
            json!({"type": "user", "message": {"content": "x".repeat(300)}}),
            json!({"type": "assistant", "uuid": "a1", "message": {"content": [{"type": "text", "text": "old"}]}}),
            json!({"type": "user", "message": {"content": "Hello"}}),
            json!({"type": "assistant", "uuid": "a2", "message": {"content": [{"type": "text", "text": "new"}]}}),
        ];
        let text = write_lines(&project.join("sid.jsonl"), &lines);
        let agent = claude("sid");
        let want = Some(turn(&[("a2", "new")], None));
        for tail in [
            text.len() - 20,
            text.len() - 200,
            lines[3].to_string().len() + 30,
        ] {
            assert_eq!(
                read_turn(
                    agent.session.as_ref().expect("a session"),
                    Some(&home),
                    tail as u64
                ),
                want,
                "tail {tail}"
            );
        }
    }

    #[test]
    fn meta_entries_do_not_start_a_turn() {
        let dir = TempDir::new();
        let home = dir.path().join("home");
        let project = home.join(".claude/projects/-x");
        fs::create_dir_all(&project).expect("mkdir");
        write_lines(
            &project.join("sid.jsonl"),
            &[
                json!({"type": "user", "message": {"content": "Hello"}}),
                json!({"type": "assistant", "uuid": "a1", "message": {"content": [{"type": "text", "text": "Looking."}]}}),
                json!({"type": "assistant", "uuid": "a2", "message": {"content": [
                {"type": "tool_use", "id": "t1", "name": "AskUserQuestion", "input": {"questions": fruit()}}]}}),
                json!({"type": "user", "isMeta": true, "message": {"content": "<system-reminder>x</system-reminder>"}}),
                json!({"type": "user", "isMeta": true, "message": {"content": [{"type": "text", "text": "<local-command-caveat>"}]}}),
            ],
        );
        let agent = claude("sid");
        assert_eq!(
            claude_turn(&agent, Some(&home)),
            Some(turn(&[("a1", "Looking.")], Some(fruit())))
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
