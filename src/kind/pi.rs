//! pi's sessions: one JSONL file per session, a message on each line once
//! it ends.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{TAIL, entries, newest, tail};
use crate::herdr::{Agent, Turn};

/// What pi did since the last prompt, from the session file herdr reports,
/// else the newest one of the agent's directory under `dir`, pi's own
/// directory.
#[must_use]
pub fn turn(agent: &Agent, dir: &Path) -> Option<Turn> {
    let path = match reported(agent) {
        Some(path) => path,
        None => newest(entries(&sessions(dir, agent.cwd.as_deref()?)))?,
    };
    Some(parse(&tail(fs::File::open(path).ok()?, TAIL)?))
}

/// The session file herdr reports for a pi agent.
#[must_use]
pub fn reported(agent: &Agent) -> Option<PathBuf> {
    agent
        .session
        .as_ref()
        .filter(|s| s.agent == "pi" && Path::new(&s.id).is_absolute())
        .map(|s| PathBuf::from(&s.id))
}

/// pi's own directory: `$PI_CODING_AGENT_DIR`, else `~/.pi/agent`.
#[must_use]
pub fn dir() -> PathBuf {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    dir_of(std::env::var_os("PI_CODING_AGENT_DIR"), &home)
}

/// pi's own directory in `home` when its variable is unset.
#[must_use]
pub fn default_dir(home: &Path) -> PathBuf {
    home.join(".pi").join("agent")
}

/// pi's own directory from its variable, `~` expanded as pi does.
fn dir_of(var: Option<OsString>, home: &Path) -> PathBuf {
    let Some(var) = var.filter(|v| !v.is_empty()) else {
        return default_dir(home);
    };
    let path = PathBuf::from(var);
    match path.strip_prefix("~") {
        Ok(rest) => home.join(rest),
        Err(_) => path,
    }
}

/// The directory of the sessions in `cwd`, named as pi names it.
fn sessions(dir: &Path, cwd: &str) -> PathBuf {
    let cwd = cwd.strip_prefix(['/', '\\']).unwrap_or(cwd);
    dir.join("sessions")
        .join(format!("--{}--", cwd.replace(['/', '\\', ':'], "-")))
}

/// The turn a session ends with: a user message starts it, and each
/// assistant message with text is a message of it. pi asks no questions.
pub(super) fn parse(text: &str) -> Turn {
    let mut messages = Vec::new();
    for line in text.split('\n') {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if entry.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let message = entry.get("message").unwrap_or(&Value::Null);
        match message.get("role").and_then(Value::as_str) {
            Some("user") => messages.clear(),
            Some("assistant") => {
                let said: Vec<&str> = message
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|b| b.get("text").and_then(Value::as_str))
                    .filter(|t| !t.is_empty())
                    .collect();
                if !said.is_empty() {
                    let id = entry.get("id").and_then(Value::as_str).map(str::to_owned);
                    messages.push((id, said.join("\n\n")));
                }
            }
            _ => {}
        }
    }
    Turn {
        messages,
        question: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::{Session, Status};
    use crate::kind::{self, Kind};
    use crate::testutil::TempDir;
    use std::time::{Duration, SystemTime};

    /// Sessions pi 0.75.4 wrote: a greeting, and a turn with thinking, a
    /// tool call and its result.
    const HELLO: &str = include_str!("../../tests/fixtures/pi/hello.jsonl");
    const TOOL: &str = include_str!("../../tests/fixtures/pi/tool.jsonl");

    fn said(messages: &[(&str, &str)]) -> Turn {
        Turn {
            messages: messages
                .iter()
                .map(|(id, t)| (Some((*id).to_owned()), (*t).to_owned()))
                .collect(),
            question: None,
        }
    }

    fn pi(cwd: &str) -> Agent {
        Agent {
            pane_id: "w:p".into(),
            status: Status::Idle,
            name: "w".into(),
            kind: Kind::Pi,
            cwd: Some(cwd.into()),
            session: None,
        }
    }

    fn write(path: &Path, text: &str, age: u64) {
        fs::create_dir_all(path.parent().expect("a directory")).expect("mkdir");
        fs::write(path, text).expect("write");
        fs::File::options()
            .write(true)
            .open(path)
            .expect("open")
            .set_modified(SystemTime::now() - Duration::from_secs(age))
            .expect("utime");
    }

    #[test]
    fn turn_holds_the_texts_of_the_assistant() {
        assert_eq!(parse(HELLO), said(&[("76292d00", "Hi there.")]));
        assert_eq!(
            parse(TOOL),
            said(&[
                ("16acdbad", "Reading the file."),
                ("1ad227b6", "It says hello.")
            ])
        );
        assert_eq!(
            parse(&format!("{TOOL}{HELLO}")),
            said(&[("76292d00", "Hi there.")])
        );
        let asked = TOOL.lines().take(4).collect::<Vec<_>>().join("\n");
        assert_eq!(parse(&asked), Turn::default());
    }

    #[test]
    fn session_is_the_newest_of_the_directory() {
        let dir = TempDir::new();
        let agent = dir.path().join("agent");
        let here = agent.join("sessions/--home-u-proj--");
        write(&here.join("2026_a.jsonl"), TOOL, 0);
        write(&here.join("2026_b.jsonl"), HELLO, 60);
        write(&here.join("2026_c.txt"), HELLO, 0);
        write(
            &agent.join("sessions/--home-u-other--/2026_d.jsonl"),
            HELLO,
            0,
        );
        let want = Some(said(&[
            ("16acdbad", "Reading the file."),
            ("1ad227b6", "It says hello."),
        ]));
        assert_eq!(turn(&pi("/home/u/proj"), &agent), want);
        assert_eq!(turn(&pi("/home/u/none"), &agent), None);
        assert_eq!(
            turn(
                &Agent {
                    cwd: None,
                    ..pi("/")
                },
                &agent
            ),
            None
        );
        let home = dir.path();
        write(&default_dir(home).join("sessions/--w-a--/s.jsonl"), TOOL, 0);
        assert_eq!(kind::turn(&pi("/w/a"), Some(home)), want);
        assert_eq!(
            kind::turn(
                &Agent {
                    kind: Kind::Other("codex".into()),
                    ..pi("/w/a")
                },
                Some(home)
            ),
            None
        );
    }

    #[test]
    fn session_herdr_reports_comes_first() {
        let dir = TempDir::new();
        let agent = dir.path().join("agent");
        write(&agent.join("sessions/--w-a--/s.jsonl"), TOOL, 0);
        let reported = dir.path().join("elsewhere/r.jsonl");
        write(&reported, HELLO, 60);
        let with = |agent: &str, id: &Path| Agent {
            session: Some(Session {
                agent: agent.into(),
                id: id.to_string_lossy().into_owned(),
            }),
            ..pi("/w/a")
        };
        assert_eq!(
            turn(&with("pi", &reported), &agent),
            Some(said(&[("76292d00", "Hi there.")]))
        );
        assert_eq!(
            turn(&with("pi", Path::new("01a1efd7")), &agent),
            turn(&pi("/w/a"), &agent)
        );
        assert_eq!(
            turn(&with("claude", &reported), &agent),
            turn(&pi("/w/a"), &agent)
        );
    }

    #[test]
    fn directory_is_named_as_pi_names_it() {
        assert_eq!(
            sessions(Path::new("/d"), "/tmp/pi-run.x/C:\\a b"),
            Path::new("/d/sessions/--tmp-pi-run.x-C--a b--")
        );
        let home = Path::new("/home/u");
        assert_eq!(dir_of(None, home), Path::new("/home/u/.pi/agent"));
        assert_eq!(
            dir_of(Some("".into()), home),
            Path::new("/home/u/.pi/agent")
        );
        assert_eq!(dir_of(Some("/s/pi".into()), home), Path::new("/s/pi"));
        assert_eq!(dir_of(Some("~/p".into()), home), Path::new("/home/u/p"));
        assert_eq!(dir_of(Some("~".into()), home), Path::new("/home/u"));
    }
}
