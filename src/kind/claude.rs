//! Claude Code's transcripts.

use std::fs;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde_json::Value;

use super::{TAIL, tail};
use crate::herdr::{Agent, Questions, Session, Turn};

/// What Claude did since the last prompt, from its transcript. None for
/// other agents.
#[must_use]
pub fn turn(agent: &Agent, home: Option<&Path>) -> Option<Turn> {
    read_turn(agent.session.as_ref()?, home, TAIL)
}

/// `turn` from the last `tail` bytes of the transcript.
#[must_use]
pub fn read_turn(session: &Session, home: Option<&Path>, bytes: u64) -> Option<Turn> {
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
    Some(parse(&tail(fs::File::open(path).ok()?, bytes)?))
}

fn flag(entry: &Value, name: &str) -> bool {
    entry.get(name).and_then(Value::as_bool).unwrap_or(false)
}

/// The turn a transcript ends with.
pub(super) fn parse(text: &str) -> Turn {
    let mut messages = Vec::new();
    let mut asked: IndexMap<Option<String>, Option<Value>> = IndexMap::new();
    for line in text.split('\n') {
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
    use crate::herdr::Status;
    use crate::kind::Kind;
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

    fn expect(messages: &[(&str, &str)], question: Option<Value>) -> Turn {
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
            kind: Kind::Claude,
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
            turn(&agent, Some(&home)),
            Some(expect(&[("a2", "Looking."), ("a4", "Done.")], None))
        );
        append(
            &path,
            &json!({"type": "assistant", "uuid": "a5", "message": {"content": [
            {"type": "tool_use", "id": "t1", "name": "AskUserQuestion", "input": {"questions": fruit()}}]}}),
        );
        assert_eq!(
            turn(&agent, Some(&home))
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
        assert_eq!(turn(&agent, Some(&home)).expect("turn").question, None);
        assert_eq!(turn(&claude("nope"), Some(&home)), None);
        let bare = Agent {
            session: None,
            ..claude("sid")
        };
        assert_eq!(turn(&bare, Some(&home)), None);
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
        let want = Some(expect(&[("a2", "new")], None));
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
            turn(&agent, Some(&home)),
            Some(expect(&[("a1", "Looking.")], Some(fruit())))
        );
    }
}
