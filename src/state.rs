//! What the bot keeps across restarts, in `matrix-state.json`. A file of
//! an older version loads; keys this version does not know are kept.

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

use crate::Result;

/// Away is on while the owner is not at the desk: every agent that stops
/// or waits posts in the chat. The file holds `afk` or `auto`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum Away {
    #[serde(rename = "afk")]
    On,
    #[default]
    #[serde(rename = "auto")]
    Off,
}

impl<'de> Deserialize<'de> for Away {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Value::deserialize(deserializer)?.as_str() {
            Some("afk") => Self::On,
            _ => Self::Off,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub since: Option<String>,
    #[serde(rename = "override")]
    pub away: Away,
    pub control: Option<String>,
    pub owner: Option<String>,
    pub attached: Option<String>,
    /// The display name and the sha256 of the picture last set
    pub profile: Option<(Option<String>, String)>,
    pub pending: IndexMap<String, Pending>,
    /// Event id → the pane it speaks for, oldest first
    pub sent: IndexMap<String, String>,
    pub asked: IndexMap<String, Asked>,
    pub spawned: IndexMap<String, Chat>,
    /// The sha256 of a picture → its mxc URI
    pub avatars: IndexMap<String, String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A prompt whose answer the bot waits for.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pending {
    pub seen: Vec<Option<String>>,
    #[serde(flatten)]
    pub progress: Progress,
    #[serde(flatten)]
    pub origin: Origin,
    pub polls: u64,
    /// The newest message, held back, and when it came
    pub held: Option<(String, f64)>,
    pub at: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// How far an agent got with a prompt: it worked on it, and said
/// something.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Progress {
    pub worked: bool,
    pub said: bool,
}

/// Where a prompt came from: a slash command, the start of a new agent, an
/// agent's own chat.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Origin {
    pub slash: bool,
    pub started: bool,
    pub spawned: bool,
}

/// An agent's own chat: its contact user and their direct room.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Chat {
    pub user: String,
    pub room: String,
    pub since: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The message that showed the choice an agent waits on, and the
/// fingerprint of that choice. An older version saved the event alone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Asked {
    pub event: Option<String>,
    pub shown: Option<String>,
}

impl Serialize for Asked {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.event, &self.shown).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Asked {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Saved {
            Pair(Option<String>, Option<String>),
            Event(Option<String>),
        }
        Ok(match Saved::deserialize(deserializer)? {
            Saved::Pair(event, shown) => Self { event, shown },
            Saved::Event(event) => Self { event, shown: None },
        })
    }
}

/// The sha256 of bytes, in hex.
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .fold(String::new(), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// The fingerprint of a choice as the chat shows it, saved in `asked`: the
/// sha256 of its JSON in ASCII, with `", "` and `": "` between items, as
/// Python's `json.dumps` writes it.
#[must_use]
pub fn fingerprint(shown: &Value) -> String {
    sha256_hex(ascii_json(shown).as_bytes())
}

fn ascii_json(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => {
            let mut out = String::from('"');
            for c in s.chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    '\x08' => out.push_str("\\b"),
                    '\x0c' => out.push_str("\\f"),
                    c if (c as u32) < 0x20 || (c as u32) > 0x7f => {
                        for unit in c.encode_utf16(&mut [0; 2]) {
                            let _ = write!(out, "\\u{unit:04x}");
                        }
                    }
                    c => out.push(c),
                }
            }
            out.push('"');
            out
        }
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(ascii_json).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!(
                    "{}: {}",
                    ascii_json(&Value::String(k.clone())),
                    ascii_json(v)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Where the state lives.
pub struct Store {
    path: PathBuf,
}

impl Store {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// The saved state; a broken file is moved aside, and the bot starts
    /// afresh with a new chat.
    ///
    /// # Errors
    /// When the file is there but cannot be read.
    pub fn load(&self, log: &dyn Fn(&str)) -> Result<State> {
        let path = self.path.display().to_string();
        let data = match fs::read(&self.path) {
            Ok(data) => data,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(State::default()),
            Err(err) => return Err(err.into()),
        };
        let parsed = serde_json::from_slice::<Value>(&data)
            .map_err(|err| err.to_string())
            .and_then(|v| {
                if v.is_object() {
                    Ok(v)
                } else {
                    Err("not an object".into())
                }
            })
            .and_then(|v| serde_json::from_value::<State>(v).map_err(|err| err.to_string()));
        match parsed {
            Ok(state) => Ok(state),
            Err(err) => {
                log(&format!("error: {path}: {err}; moved aside"));
                fs::rename(&self.path, format!("{path}.broken"))?;
                Ok(State::default())
            }
        }
    }

    /// Writes the state, whole or not at all, readable by its user alone.
    ///
    /// # Errors
    /// When the file cannot be written.
    pub fn save(&self, state: &State) -> Result<()> {
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let written = Self::write(&tmp, &serde_json::to_vec(state)?)
            .and_then(|()| Ok(fs::rename(&tmp, &self.path)?));
        if written.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        written?;
        // The rename is durable once the directory is
        let dir = self.path.parent().filter(|d| !d.as_os_str().is_empty());
        fs::File::open(dir.unwrap_or(Path::new(".")))?.sync_all()?;
        Ok(())
    }

    fn write(path: &Path, data: &[u8]) -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(data)?;
        file.sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_keeps_unknown_keys_apart() {
        let saved = serde_json::json!({"seen": ["u1"], "worked": true, "said": false, "slash": true,
            "started": false, "polls": 1, "held": null, "at": 2.5, "spawned": true, "later": 7});
        let pending: Pending = serde_json::from_value(saved.clone()).expect("a turn");
        assert!(pending.progress.worked && pending.origin.slash && pending.origin.spawned);
        assert_eq!(pending.extra.keys().collect::<Vec<_>>(), ["later"]);
        assert_eq!(serde_json::to_value(&pending).expect("JSON"), saved);
    }

    #[test]
    fn fingerprint_is_stable() {
        let shown = serde_json::json!([
            "Do you want to proceed?",
            ["Yes", "No \u{276F} \u{e9} \u{1F600}"],
            "\u{25CF} Bash\n\t\"x\""
        ]);
        assert_eq!(
            fingerprint(&shown),
            "de31238ba06c5db4f2ef7c6ad481ef3ee8f376c99dd20e7ad66c87325d4eed57"
        );
        let question: serde_json::Value = serde_json::from_str(
            r#"[{"question":"Pick?","multiSelect":false,"n":1,"options":[{"label":"A","description":null}]}]"#,
        )
        .expect("JSON");
        assert_eq!(
            fingerprint(&question),
            "4d166a091142347181965ba167e97e5d86ed17eb8c6307b90fd82914795a44c7"
        );
        assert_eq!(
            fingerprint(&Value::Null),
            "74234e98afe7498fb5daf1f36ac2d78acc339464f950703b8c019892f982b90b"
        );
    }
}
