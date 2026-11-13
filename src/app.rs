//! The bot as the service runs it: settings from the environment, and a
//! loop that outlives failures.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::bot::{Bot, Deps};
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
    /// The whisper.cpp model and the language it hears
    pub whisper: Option<(String, String)>,
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
            .field("whisper", &self.whisper)
            .finish()
    }
}

/// True for an `https://` URL, or an `http://` one on this host: the
/// token goes in clear text over plain HTTP.
fn safe_url(url: &str) -> bool {
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
    Ok(Settings {
        homeserver,
        token: need("HERDR_MATRIX_TOKEN")?,
        owner: need("HERDR_MATRIX_OWNER")?,
        state_path: state.join("matrix-state.json"),
        files: env("XDG_RUNTIME_DIR").map_or_else(
            || state.join("files"),
            |run| Path::new(&run).join("herdr-matrix"),
        ),
        name: need("HERDR_MATRIX_NAME")?,
        avatar: env("HERDR_MATRIX_AVATAR").map(PathBuf::from),
        spawn_avatar: env("HERDR_MATRIX_SPAWN_AVATAR").map(PathBuf::from),
        whisper: env("HERDR_MATRIX_WHISPER_MODEL").map(|model| {
            (
                model,
                env("HERDR_MATRIX_WHISPER_LANGUAGE").unwrap_or_else(|| "auto".into()),
            )
        }),
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
        files: s.files,
        name: Some(s.name),
        avatar: s.avatar,
        spawn_avatar: s.spawn_avatar,
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
    fn files_go_to_the_runtime_directory() {
        let dir = TempDir::new();
        let env = env(dir.path(), &[]);
        let s = settings(&|k| env.get(k).cloned()).expect("settings");
        assert_eq!(s.files, dir.path().join("files"));
        let env = self::env(dir.path(), &[("XDG_RUNTIME_DIR", "/run/user/1000".into())]);
        let s = settings(&|k| env.get(k).cloned()).expect("settings");
        assert_eq!(s.files, Path::new("/run/user/1000/herdr-matrix"));
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
