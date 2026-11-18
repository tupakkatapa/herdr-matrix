//! The bot as the service runs it: settings from the environment, and a
//! loop that outlives failures.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::bot::{AgentSettings, Bot, Deps};
use crate::inbox::{self, Prompt, Upload};
use crate::matrix::Http;
use crate::state::Store;
use crate::voice::{Bins, whisper};
use crate::{Error, Result};

/// The shortest time between two ticks
pub const MIN_TICK: Duration = Duration::from_millis(500);
/// The longest wait after failures in a row
const BACKOFF_MAX: Duration = Duration::from_mins(1);

pub struct Settings {
    pub homeserver: String,
    pub token: String,
    pub owner: String,
    pub state_path: PathBuf,
    /// Where files from the chat go
    pub files: PathBuf,
    pub name: String,
    pub avatar: Option<PathBuf>,
    pub spawn_avatar: Option<PathBuf>,
    /// The bot's name before an agent's in the name of its contact
    pub spawn_prefix: bool,
    /// The whisper.cpp model and the language it hears
    pub whisper: Option<(String, String)>,
    pub agents: Vec<AgentSettings>,
}

impl fmt::Debug for Settings {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Settings")
            .field("homeserver", &self.homeserver)
            .field("token", &"***")
            .field("owner", &self.owner)
            .field("state_path", &self.state_path)
            .field("files", &self.files)
            .field("name", &self.name)
            .field("avatar", &self.avatar)
            .field("spawn_avatar", &self.spawn_avatar)
            .field("spawn_prefix", &self.spawn_prefix)
            .field("whisper", &self.whisper)
            .field("agents", &self.agents)
            .finish()
    }
}

/// True for an `https://` URL, or an `http://` one on this host: the
/// token goes in clear text over plain HTTP.
pub(crate) fn safe_url(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or_default();
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => host.split(':').next().unwrap_or_default(),
    };
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Where the bot keeps files of the chat and what the machine queues for
/// it: `$XDG_CACHE_HOME/herdr-matrix`, else `$HOME/.cache/herdr-matrix`.
#[must_use]
pub fn files_dir(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    // A relative or empty XDG_CACHE_HOME is to be ignored
    env("XDG_CACHE_HOME")
        .filter(|cache| cache.starts_with('/'))
        .map(PathBuf::from)
        .or_else(|| {
            env("HOME")
                .filter(|home| !home.is_empty())
                .map(|home| Path::new(&home).join(".cache"))
        })
        .map(|cache| cache.join("herdr-matrix"))
}

/// The files directory, or why there is none.
fn need_files_dir(env: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf> {
    files_dir(env).ok_or_else(|| Error::Setting("neither XDG_CACHE_HOME nor HOME is set".into()))
}

/// The service's settings.
///
/// # Errors
/// When a required setting is missing or one is malformed.
pub fn settings(env: &dyn Fn(&str) -> Option<String>) -> Result<Settings> {
    let need = |name: &str| env(name).ok_or_else(|| Error::Setting(format!("{name} is not set")));
    let homeserver = need("HERDR_MATRIX_HOMESERVER")?;
    if !safe_url(&homeserver) {
        return Err(Error::Setting(format!(
            "HERDR_MATRIX_HOMESERVER {homeserver} is not https://, and not on this host"
        )));
    }
    let state = PathBuf::from(need("HERDR_MATRIX_STATE")?);
    let spawn_prefix = match env("HERDR_MATRIX_SPAWN_PREFIX").as_deref() {
        None | Some("true") => true,
        Some("false") => false,
        Some(other) => {
            return Err(Error::Setting(format!(
                "HERDR_MATRIX_SPAWN_PREFIX {other} is not true or false"
            )));
        }
    };
    let agents = match env("HERDR_MATRIX_AGENTS") {
        None => Vec::new(),
        Some(path) => fs::read(&path)
            .map_err(|err| err.to_string())
            .and_then(|data| serde_json::from_slice(&data).map_err(|err| err.to_string()))
            .map_err(|err| Error::Setting(format!("HERDR_MATRIX_AGENTS {path}: {err}")))?,
    };
    Ok(Settings {
        homeserver,
        token: need("HERDR_MATRIX_TOKEN")?,
        owner: need("HERDR_MATRIX_OWNER")?,
        state_path: state.join("matrix-state.json"),
        files: need_files_dir(env)?,
        name: need("HERDR_MATRIX_NAME")?,
        avatar: env("HERDR_MATRIX_AVATAR").map(PathBuf::from),
        spawn_avatar: env("HERDR_MATRIX_SPAWN_AVATAR").map(PathBuf::from),
        spawn_prefix,
        whisper: env("HERDR_MATRIX_WHISPER_MODEL").map(|model| {
            (
                model,
                env("HERDR_MATRIX_WHISPER_LANGUAGE").unwrap_or_else(|| "auto".into()),
            )
        }),
        agents,
    })
}

/// The bot as the service's environment sets it up. The
/// `herdr-matrix-models` service fetches the whisper model; until it is
/// there, voice messages are refused.
///
/// # Errors
/// When a setting is missing or malformed, or the state cannot be read.
pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Result<Bot> {
    let s = settings(env)?;
    let mut deps = Deps {
        owner: s.owner,
        inbox: Some(s.files.join("prompts")),
        uploads: Some(s.files.join("uploads")),
        files: s.files,
        name: Some(s.name),
        avatar: s.avatar,
        spawn_avatar: s.spawn_avatar,
        spawn_prefix: s.spawn_prefix,
        agents: s.agents,
        ..Deps::default()
    };
    if let Some((model, language)) = s.whisper {
        deps.transcribe = Some(Box::new(move |path| {
            if !Path::new(&model).exists() {
                return Err(Error::Refused(
                    "the model is still downloading; try again in a few minutes".into(),
                ));
            }
            whisper(&Bins::default(), path, &model, &language)
        }));
    }
    let store = Store::new(s.state_path);
    let state = store.load(&*deps.log)?;
    Ok(Bot::new(
        Box::new(Http::new(&s.homeserver, &s.token)),
        store,
        state,
        deps,
    ))
}

/// The agent a prompt is for: the one a name or pane id names, or the
/// one working in a directory.
#[derive(Clone, Copy, Debug)]
pub enum Target<'a> {
    Agent(&'a str),
    Cwd(&'a str),
}

/// Queues a prompt for the running bot, which hands it to the agent
/// `target` names; the path of its file.
///
/// # Errors
/// When the target or the text is empty, when neither `XDG_CACHE_HOME` nor
/// `HOME` is set, or when the file cannot be written.
pub fn queue_prompt(
    env: &dyn Fn(&str) -> Option<String>,
    target: Target,
    text: &str,
) -> Result<PathBuf> {
    let (Target::Agent(named) | Target::Cwd(named)) = target;
    if named.is_empty() || text.is_empty() {
        return Err(Error::Setting(
            "the agent and the text must not be empty".into(),
        ));
    }
    let files = need_files_dir(env)?;
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64());
    inbox::queue(
        &files.join("prompts"),
        &Prompt {
            agent: match target {
                Target::Agent(agent) => agent.to_owned(),
                Target::Cwd(_) => String::new(),
            },
            text: text.to_owned(),
            at,
            cwd: match target {
                Target::Agent(_) => None,
                Target::Cwd(dir) => Some(dir.to_owned()),
            },
        },
    )
}

/// Queues a file for the running bot, which posts it in the chat of the
/// agent in the pane `HERDR_PANE_ID` names; the path of its entry. In a
/// confined agent's sandbox, `HERDR_MATRIX_UPLOADS` is the agent's own
/// queue, which names the agent. The file is read here, in the caller's
/// process.
///
/// # Errors
/// When neither `HERDR_MATRIX_UPLOADS` nor `HERDR_PANE_ID` is set, when
/// `path` is not a regular file or is empty, when neither `XDG_CACHE_HOME`
/// nor `HOME` is set, or when the copy cannot be written.
pub fn queue_file(
    env: &dyn Fn(&str) -> Option<String>,
    path: &Path,
    caption: &str,
) -> Result<PathBuf> {
    let own = env("HERDR_MATRIX_UPLOADS")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from);
    let pane = match own {
        Some(_) => String::new(),
        None => env("HERDR_PANE_ID")
            .filter(|pane| !pane.is_empty())
            .ok_or_else(|| Error::Refused("not in a herdr pane".into()))?,
    };
    let refused = |why: &str| Error::Refused(format!("{}: {why}", path.display()));
    // Opening a FIFO would wait for a writer
    if !fs::metadata(path)
        .map_err(|err| refused(&err.to_string()))?
        .is_file()
    {
        return Err(refused("not a regular file"));
    }
    let mut file = fs::File::open(path).map_err(|err| refused(&err.to_string()))?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(refused("not a regular file"));
    }
    if meta.len() == 0 {
        return Err(refused("empty"));
    }
    let name = path
        .file_name()
        .ok_or_else(|| refused("not a regular file"))?
        .to_string_lossy()
        .into_owned();
    let dir = match own {
        Some(dir) => dir,
        None => need_files_dir(env)?.join("uploads"),
    };
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64());
    inbox::queue_upload(
        &dir,
        &Upload {
            pane,
            name,
            caption: Some(caption.to_owned()).filter(|c| !c.is_empty()),
            at,
        },
        &mut file,
    )
}

/// Ticks until `sleep` says stop, waiting longer after each failure in a
/// row.
pub fn serve(
    tick: &mut dyn FnMut() -> Result<()>,
    log: &dyn Fn(&str),
    sleep: &mut dyn FnMut(Duration) -> bool,
) {
    let mut failures: u32 = 0;
    loop {
        let started = Instant::now();
        match tick() {
            Ok(()) => failures = 0,
            Err(err) => {
                failures = failures.saturating_add(1);
                log(&format!("error: {err}"));
                let backoff = Duration::from_secs(1 << failures.min(6)).min(BACKOFF_MAX);
                if !sleep(backoff) {
                    return;
                }
            }
        }
        if !sleep(MIN_TICK.saturating_sub(started.elapsed())) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::fs;

    fn env(dir: &Path, extra: &[(&str, String)]) -> HashMap<String, String> {
        let mut env: HashMap<String, String> = [
            (
                "HERDR_MATRIX_HOMESERVER",
                "https://m.example.org".to_owned(),
            ),
            ("HERDR_MATRIX_TOKEN", "tok".to_owned()),
            ("HERDR_MATRIX_OWNER", "@kari:example.org".to_owned()),
            ("HERDR_MATRIX_STATE", dir.display().to_string()),
            ("HERDR_MATRIX_NAME", "Claude".to_owned()),
            ("HOME", dir.display().to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        env.extend(extra.iter().map(|(k, v)| ((*k).to_owned(), v.clone())));
        env
    }

    #[test]
    fn bare_settings() {
        let dir = TempDir::new();
        let env = env(dir.path(), &[]);
        let s = settings(&|k| env.get(k).cloned()).expect("settings");
        assert_eq!(s.state_path, dir.path().join("matrix-state.json"));
        assert_eq!(
            (s.owner.as_str(), s.name.as_str()),
            ("@kari:example.org", "Claude")
        );
        assert_eq!((s.whisper, s.spawn_avatar, s.avatar), (None, None, None));
        let bot = from_env(&|k| env.get(k).cloned()).expect("bot");
        assert!(bot.deps().transcribe.is_none() && bot.deps().spawn_avatar.is_none());
    }

    #[test]
    fn missing_model_is_said_to_be_downloading() {
        let dir = TempDir::new();
        let model = dir.path().join("ggml.bin");
        let env = env(
            dir.path(),
            &[
                ("HERDR_MATRIX_WHISPER_MODEL", model.display().to_string()),
                ("HERDR_MATRIX_WHISPER_LANGUAGE", "en".into()),
                ("HERDR_MATRIX_SPAWN_AVATAR", "/pic.png".into()),
            ],
        );
        let s = settings(&|k| env.get(k).cloned()).expect("settings");
        assert_eq!(
            s.whisper,
            Some((model.display().to_string(), "en".to_owned()))
        );
        assert_eq!(s.spawn_avatar.as_deref(), Some(Path::new("/pic.png")));
        let bot = from_env(&|k| env.get(k).cloned()).expect("bot");
        let hear = bot.deps().transcribe.as_ref().expect("a transcriber");
        let err = hear(&dir.path().join("a.ogg")).expect_err("no model yet");
        assert!(err.to_string().contains("still downloading"), "{err}");
    }

    #[test]
    fn files_go_to_the_cache_directory() {
        let dir = TempDir::new();
        let home = dir.path().join(".cache/herdr-matrix");
        for (cache, want) in [
            (None, home.as_path()),
            (Some(""), home.as_path()),
            (Some("cache"), home.as_path()),
            (Some("/c"), Path::new("/c/herdr-matrix")),
        ] {
            let extra: Vec<(&str, String)> = cache
                .map(|c| ("XDG_CACHE_HOME", c.to_owned()))
                .into_iter()
                .collect();
            let env = env(dir.path(), &extra);
            let s = settings(&|k| env.get(k).cloned()).expect("settings");
            assert_eq!(s.files, want, "{cache:?}");
        }
        let mut env = env(dir.path(), &[]);
        env.remove("HOME");
        let err = settings(&|k| env.get(k).cloned()).expect_err("no files directory");
        assert_eq!(err.to_string(), "neither XDG_CACHE_HOME nor HOME is set");
    }

    #[test]
    fn spawn_prefix_is_true_or_false() {
        let dir = TempDir::new();
        for (value, want) in [
            (None, Ok(true)),
            (Some("true"), Ok(true)),
            (Some("false"), Ok(false)),
            (
                Some("maybe"),
                Err("HERDR_MATRIX_SPAWN_PREFIX maybe is not true or false".to_owned()),
            ),
        ] {
            let extra: Vec<(&str, String)> = value
                .map(|v| ("HERDR_MATRIX_SPAWN_PREFIX", v.to_owned()))
                .into_iter()
                .collect();
            let env = env(dir.path(), &extra);
            let s = settings(&|k| env.get(k).cloned());
            assert_eq!(
                s.map(|s| s.spawn_prefix).map_err(|e| e.to_string()),
                want,
                "{value:?}"
            );
        }
        let env = env(dir.path(), &[("HERDR_MATRIX_SPAWN_PREFIX", "false".into())]);
        let bot = from_env(&|k| env.get(k).cloned()).expect("bot");
        assert!(!bot.deps().spawn_prefix);
        assert_eq!(
            bot.deps().inbox.as_deref(),
            Some(dir.path().join(".cache/herdr-matrix/prompts").as_path())
        );
    }

    #[test]
    fn agent_settings_come_from_a_file() {
        let dir = TempDir::new();
        let file = dir.path().join("agents.json");
        fs::write(
            &file,
            r#"[{"cwd": "/w/a", "name": "Kate", "prefix": false, "avatar": "/k.png"},
                {"cwd": "/w/b", "name": null, "prefix": null, "avatar": null}]"#,
        )
        .expect("write");
        let env = env(
            dir.path(),
            &[("HERDR_MATRIX_AGENTS", file.display().to_string())],
        );
        let bot = from_env(&|k| env.get(k).cloned()).expect("bot");
        assert_eq!(
            bot.deps().agents,
            [
                AgentSettings {
                    cwd: "/w/a".into(),
                    name: Some("Kate".into()),
                    prefix: Some(false),
                    avatar: Some("/k.png".into()),
                    ..AgentSettings::default()
                },
                AgentSettings {
                    cwd: "/w/b".into(),
                    ..AgentSettings::default()
                },
            ]
        );
        let env = self::env(dir.path(), &[]);
        assert!(
            settings(&|k| env.get(k).cloned())
                .expect("settings")
                .agents
                .is_empty()
        );
        fs::write(&file, "[{").expect("write");
        for path in [file.clone(), dir.path().join("missing.json")] {
            let env = self::env(
                dir.path(),
                &[("HERDR_MATRIX_AGENTS", path.display().to_string())],
            );
            let err = settings(&|k| env.get(k).cloned()).expect_err("refused");
            assert!(matches!(err, Error::Setting(_)), "{err}");
            assert!(
                err.to_string()
                    .starts_with(&format!("HERDR_MATRIX_AGENTS {}: ", path.display())),
                "{err}"
            );
        }
    }

    #[test]
    fn confined_agent_settings_come_from_a_file() {
        let dir = TempDir::new();
        let file = dir.path().join("agents.json");
        fs::write(
            &file,
            r#"[{"cwd": "/w/n", "name": null, "prefix": null, "avatar": null, "kind": "pi",
                 "sandbox": {"id": "nora", "state": "/s/pi", "uploads": "/c/uploads/nora",
                             "files": "/c/agents/nora"}}]"#,
        )
        .expect("write");
        let env = env(
            dir.path(),
            &[("HERDR_MATRIX_AGENTS", file.display().to_string())],
        );
        let s = settings(&|k| env.get(k).cloned()).expect("settings");
        assert_eq!(s.agents[0].kind, crate::kind::Kind::Pi);
        assert_eq!(
            s.agents[0].sandbox,
            Some(crate::bot::Sandbox {
                id: "nora".into(),
                state: "/s/pi".into(),
                uploads: "/c/uploads/nora".into(),
                files: "/c/agents/nora".into(),
            })
        );
        fs::write(&file, r#"[{"cwd": "/w/n", "sandbox": {"id": "nora"}}]"#).expect("write");
        assert!(settings(&|k| env.get(k).cloned()).is_err());
        fs::write(&file, r#"[{"cwd": "/w/n", "kind": "codex"}]"#).expect("write");
        assert!(settings(&|k| env.get(k).cloned()).is_err());
    }

    #[test]
    fn prompt_is_queued_where_the_bot_takes_it() {
        let dir = TempDir::new();
        let run = dir.path().join("run");
        for (env, inbox) in [
            (
                vec![("XDG_CACHE_HOME", run.display().to_string())],
                run.join("herdr-matrix/prompts"),
            ),
            (
                vec![("HOME", dir.path().display().to_string())],
                dir.path().join(".cache/herdr-matrix/prompts"),
            ),
        ] {
            let env: HashMap<&str, String> = env.into_iter().collect();
            let path = queue_prompt(&|k| env.get(k).cloned(), Target::Agent("mail"), "Check it")
                .expect("queued");
            assert_eq!(path.parent(), Some(inbox.as_path()));
            let taken = crate::inbox::take(&inbox, &|_| {}).expect("take");
            assert_eq!(
                taken
                    .iter()
                    .map(|p| (p.agent.as_str(), p.text.as_str()))
                    .collect::<Vec<_>>(),
                [("mail", "Check it")]
            );
            assert!(taken[0].at > 1_700_000_000.0);
        }
        let err = queue_prompt(&|_| None, Target::Agent("mail"), "Check it").expect_err("nowhere");
        assert!(matches!(err, Error::Setting(_)), "{err}");
        let env = HashMap::from([("XDG_CACHE_HOME", run.display().to_string())]);
        for (target, text) in [
            (Target::Agent(""), "Check it"),
            (Target::Agent("mail"), ""),
            (Target::Cwd(""), "Check it"),
            (Target::Cwd("/w"), ""),
        ] {
            let err = queue_prompt(&|k| env.get(k).cloned(), target, text).expect_err("empty");
            assert!(matches!(err, Error::Setting(_)), "{err}");
        }
    }

    #[test]
    fn missing_setting_is_named() {
        let err = settings(&|_| None).expect_err("no settings");
        assert_eq!(err.to_string(), "HERDR_MATRIX_HOMESERVER is not set");
    }

    #[test]
    fn plain_http_only_on_this_host() {
        for (url, ok) in [
            ("https://m.example.org", true),
            ("http://localhost:8008", true),
            ("http://127.0.0.1:8008/", true),
            ("http://[::1]:8008", true),
            ("http://m.example.org", false),
            ("http://127.0.0.1.example.org", false),
            ("ftp://m.example.org", false),
        ] {
            let dir = TempDir::new();
            let env = env(dir.path(), &[("HERDR_MATRIX_HOMESERVER", url.into())]);
            assert_eq!(settings(&|k| env.get(k).cloned()).is_ok(), ok, "{url}");
        }
    }

    #[test]
    fn token_stays_out_of_debug() {
        let dir = TempDir::new();
        let env = env(dir.path(), &[("HERDR_MATRIX_TOKEN", "s3cret".into())]);
        let s = settings(&|k| env.get(k).cloned()).expect("settings");
        assert!(!format!("{s:?}").contains("s3cret"));
    }

    #[test]
    fn unreadable_state_stops_the_start() {
        let dir = TempDir::new();
        fs::create_dir(dir.path().join("matrix-state.json")).expect("mkdir");
        let env = env(dir.path(), &[]);
        assert!(from_env(&|k| env.get(k).cloned()).is_err());
    }

    #[test]
    fn failures_back_off() {
        let outcomes = RefCell::new(
            [false, false, true, false]
                .into_iter()
                .chain([false; 6])
                .collect::<Vec<_>>(),
        );
        let mut tick = || {
            if outcomes.borrow_mut().remove(0) {
                Ok(())
            } else {
                Err(Error::Herdr("down".into()))
            }
        };
        let lines = RefCell::new(Vec::new());
        let mut waits = Vec::new();
        let mut sleep = |wait: Duration| {
            if wait >= Duration::from_secs(1) {
                waits.push(wait.as_secs());
            }
            !outcomes.borrow().is_empty()
        };
        serve(
            &mut tick,
            &|line| lines.borrow_mut().push(line.to_owned()),
            &mut sleep,
        );
        assert_eq!(waits, [2, 4, 2, 4, 8, 16, 32, 60, 60]);
        assert_eq!(lines.borrow()[0], "error: herdr: down");
    }

    #[test]
    fn good_tick_waits_out_the_shortest_tick() {
        let mut waits = Vec::new();
        let mut sleep = |wait: Duration| {
            waits.push(wait);
            waits.len() < 3
        };
        serve(&mut || Ok(()), &|_| {}, &mut sleep);
        assert_eq!(waits.len(), 3);
        assert!(waits.iter().all(|w| *w > Duration::ZERO && *w <= MIN_TICK));
    }
}
