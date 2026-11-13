//! The Matrix client-server API.

use std::fmt::Write as _;
use std::io::Read as _;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use ureq::Body;
use ureq::http::{Request, Response};

use crate::json::key_str;
use crate::{Error, Result};

/// How long a request may take, unless it says otherwise
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(1);
/// How long the homeserver holds a sync open for new events
pub const SYNC_WAIT: Duration = Duration::from_secs(3);
/// How long a typing notice lasts unless renewed
const TYPING_TIMEOUT: Duration = Duration::from_secs(10);
/// The wait after a rate limit that names none, and the longest one
/// waited out
const RATE_LIMIT_WAIT: Duration = Duration::from_secs(1);
const RATE_LIMIT_WAIT_MAX: Duration = Duration::from_secs(30);
/// Bytes of a download at most
pub const FILE_MAX: usize = 25 * 1024 * 1024;

pub trait Matrix {
    /// # Errors
    /// When the request fails.
    fn call(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        query: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<Value>;
    /// # Errors
    /// When the upload fails.
    fn upload(&mut self, data: &[u8], content_type: &str) -> Result<String>;
    /// # Errors
    /// When the download fails or is too large.
    fn download(&mut self, mxc: &str) -> Result<Vec<u8>>;
    fn next_txn(&mut self) -> String;
}

pub struct Http {
    root: String,
    token: String,
    txn: u128,
    agent: ureq::Agent,
    /// Bytes of a download at most
    pub file_max: usize,
}

impl Http {
    #[must_use]
    pub fn new(homeserver: &str, token: &str) -> Self {
        Self {
            root: format!("{}/_matrix", homeserver.trim_end_matches('/')),
            token: token.to_owned(),
            txn: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis()),
            agent: agent(),
            file_max: FILE_MAX,
        }
    }

    /// A request; one the homeserver rate-limits is sent again once, after
    /// the wait it asks for.
    fn open(
        &self,
        url: &str,
        data: Option<Vec<u8>>,
        method: &str,
        content_type: Option<&str>,
        timeout: Duration,
    ) -> Result<Response<Body>> {
        match self.open_once(url, data.clone(), method, content_type, timeout) {
            Err(err) => match retry_after(&err) {
                Some(wait) => {
                    std::thread::sleep(wait);
                    self.open_once(url, data, method, content_type, timeout)
                }
                None => Err(err),
            },
            ok => ok,
        }
    }

    fn open_once(
        &self,
        url: &str,
        data: Option<Vec<u8>>,
        method: &str,
        content_type: Option<&str>,
        timeout: Duration,
    ) -> Result<Response<Body>> {
        let mut request = Request::builder()
            .method(method)
            .uri(url)
            .header("Authorization", format!("Bearer {}", self.token));
        if let Some(kind) = content_type {
            request = request.header("Content-Type", kind);
        }
        let request = request
            .body(data.unwrap_or_default())
            .map_err(|err| Error::Setting(err.to_string()))?;
        let request = self
            .agent
            .configure_request(request)
            .timeout_global(Some(timeout))
            .build();
        checked(self.agent.run(request))
    }
}

/// The wait a rate-limited request asks for, when it is short enough to
/// wait out.
fn retry_after(err: &Error) -> Option<Duration> {
    let Error::Http {
        status: 429, body, ..
    } = err
    else {
        return None;
    };
    let asked = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("retry_after_ms")?.as_u64())
        .map_or(RATE_LIMIT_WAIT, Duration::from_millis);
    (asked <= RATE_LIMIT_WAIT_MAX).then_some(asked)
}

/// An HTTP client that leaves error statuses to `checked`.
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into()
}

/// A response; an error status is an `Error::Http` with its body.
fn checked(response: Result<Response<Body>, ureq::Error>) -> Result<Response<Body>> {
    let mut response = response.map_err(|err| Error::Transport(err.to_string()))?;
    let status = response.status();
    if status.is_client_error() || status.is_server_error() {
        let mut body = Vec::new();
        let _ = response.body_mut().as_reader().read_to_end(&mut body);
        let code = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|v| v.get("errcode")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| status.canonical_reason().unwrap_or_default().to_owned());
        return Err(Error::Http {
            status: status.as_u16(),
            code,
            body,
        });
    }
    Ok(response)
}

fn json(mut response: Response<Body>) -> Result<Value> {
    Ok(serde_json::from_reader(response.body_mut().as_reader())?)
}

impl Matrix for Http {
    fn call(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        query: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<Value> {
        let mut url = format!("{}/client/v3{path}", self.root);
        if !query.is_empty() {
            url.push('?');
            url.push_str(&urlencode(query));
        }
        let data = body.map(|b| b.to_string().into_bytes());
        json(self.open(&url, data, method, Some("application/json"), timeout)?)
    }

    fn upload(&mut self, data: &[u8], content_type: &str) -> Result<String> {
        let url = format!("{}/media/v3/upload", self.root);
        let resp = json(self.open(
            &url,
            Some(data.to_vec()),
            "POST",
            Some(content_type),
            DEFAULT_TIMEOUT,
        )?)?;
        Ok(key_str(&resp, "content_uri")?.to_owned())
    }

    fn download(&mut self, mxc: &str) -> Result<Vec<u8>> {
        let rest = mxc.strip_prefix("mxc://").unwrap_or(mxc);
        let (server, media) = rest.split_once('/').unwrap_or((rest, ""));
        let url = format!(
            "{}/client/v1/media/download/{}/{}",
            self.root,
            quote(server),
            quote(media)
        );
        let mut response = self.open(&url, None, "GET", None, DOWNLOAD_TIMEOUT)?;
        let mut data = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(self.file_max as u64 + 1)
            .read_to_end(&mut data)?;
        if data.len() > self.file_max {
            return Err(Error::Refused(format!(
                "the file is larger than {} MB",
                self.file_max >> 20
            )));
        }
        Ok(data)
    }

    fn next_txn(&mut self) -> String {
        self.txn += 1;
        self.txn.to_string()
    }
}

/// A path segment, every reserved character escaped.
#[must_use]
pub fn quote(value: &str) -> String {
    escape(value, false)
}

fn escape(value: &str, plus: bool) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                out.push(char::from(b));
            }
            b' ' if plus => out.push('+'),
            b => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// A query string, as a form encodes it.
#[must_use]
pub fn urlencode(query: &[(&str, &str)]) -> String {
    query
        .iter()
        .map(|(k, v)| format!("{}={}", escape(k, true), escape(v, true)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Who acts in a room: the bot, or a user of its appservice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum As<'a> {
    Bot,
    User(&'a str),
}

impl<'a> As<'a> {
    fn query(self) -> Vec<(&'static str, &'a str)> {
        match self {
            Self::Bot => Vec::new(),
            Self::User(user) => vec![("user_id", user)],
        }
    }
}

/// A file or recording the owner sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Media {
    pub url: Option<String>,
    pub name: String,
    pub body: String,
}

/// What an event of the owner says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum What {
    Text(String),
    File(Media),
    Voice(Media),
    Reaction { key: String, target: Option<String> },
}

/// A message or reaction of the owner.
#[derive(Clone, Debug, PartialEq)]
pub struct Inbound {
    pub id: Option<String>,
    /// Seconds since the epoch
    pub ts: f64,
    /// The event a message replies to
    pub reply_to: Option<String>,
    pub what: What,
}

impl Inbound {
    /// An event of `owner` the bot acts on: a message, not an edit, or a
    /// reaction.
    #[must_use]
    pub fn parse(ev: &Value, owner: &str) -> Option<Self> {
        if ev.get("sender").and_then(Value::as_str) != Some(owner) {
            return None;
        }
        let content = ev.get("content").unwrap_or(&Value::Null);
        let text = |name: &str| content.get(name).and_then(Value::as_str);
        let relates = content.get("m.relates_to");
        let what = match ev.get("type").and_then(Value::as_str)? {
            "m.reaction" => What::Reaction {
                key: relates
                    .and_then(|r| r.get("key")?.as_str())
                    .unwrap_or_default()
                    .to_owned(),
                target: relates
                    .and_then(|r| r.get("event_id")?.as_str())
                    .map(str::to_owned),
            },
            "m.room.message" => {
                let body = text("body")?;
                if relates.and_then(|r| r.get("rel_type")?.as_str()) == Some("m.replace") {
                    return None;
                }
                let media = || Media {
                    url: text("url").map(str::to_owned),
                    name: text("filename")
                        .filter(|f| !f.is_empty())
                        .unwrap_or(body)
                        .to_owned(),
                    body: body.to_owned(),
                };
                match text("msgtype")? {
                    "m.text" => What::Text(unquoted(body, relates)),
                    "m.image" | "m.file" => What::File(media()),
                    "m.audio" => What::Voice(media()),
                    _ => return None,
                }
            }
            _ => return None,
        };
        Some(Self {
            id: ev
                .get("event_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_owned),
            ts: ev
                .get("origin_server_ts")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                / 1000.0,
            reply_to: relates
                .and_then(|r| r.pointer("/m.in_reply_to/event_id")?.as_str())
                .map(str::to_owned),
            what,
        })
    }
}

/// The body without the quoted fallback of a reply.
fn unquoted(body: &str, relates: Option<&Value>) -> String {
    let lines: Vec<&str> = body.split('\n').collect();
    let start =
        if relates.and_then(|r| r.get("m.in_reply_to")).is_some() && lines[0].starts_with("> <") {
            lines
                .iter()
                .position(|l| l.is_empty())
                .map_or(lines.len(), |i| i + 1)
        } else {
            0
        };
    lines[start..].join("\n").trim().to_owned()
}

/// The answer to a sync.
pub struct Sync {
    pub next_batch: String,
    rooms: Value,
}

impl Sync {
    /// # Errors
    /// When the answer has no `next_batch`.
    pub fn parse(mut resp: Value) -> Result<Self> {
        Ok(Self {
            next_batch: key_str(&resp, "next_batch")?.to_owned(),
            rooms: resp.get_mut("rooms").map(Value::take).unwrap_or_default(),
        })
    }

    fn invites(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.rooms
            .get("invite")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
    }

    #[must_use]
    pub fn has_invites(&self) -> bool {
        self.invites().next().is_some()
    }

    /// Rooms `owner` invited `me` to, as the inviting server tells it.
    #[must_use]
    pub fn invites_from(&self, owner: &str, me: &str) -> Vec<String> {
        self.invites()
            .filter(|(_, data)| {
                data.pointer("/invite_state/events")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .any(|e| {
                        member(e)
                            .is_some_and(|(user, membership)| user == me && membership == "invite")
                            && e.get("sender").and_then(Value::as_str) == Some(owner)
                    })
            })
            .map(|(room, _)| room.clone())
            .collect()
    }

    /// True when anyone but `users` joined a room, was invited or knocks.
    #[must_use]
    pub fn strangers(&self, room: &str, users: &[&str]) -> bool {
        let joined = self.rooms.get("join").and_then(|j| j.get(room));
        ["timeline", "state"]
            .iter()
            .filter_map(|part| joined?.get(part)?.get("events")?.as_array())
            .flatten()
            .filter_map(member)
            .any(|(user, membership)| present(membership) && !users.contains(&user))
    }

    /// The owner's messages and reactions in a joined room.
    #[must_use]
    pub fn inbound(&self, room: &str, owner: &str) -> Vec<Inbound> {
        self.rooms
            .get("join")
            .and_then(|j| j.get(room)?.get("timeline")?.get("events")?.as_array())
            .into_iter()
            .flatten()
            .filter_map(|ev| Inbound::parse(ev, owner))
            .collect()
    }
}

/// The user and membership of an `m.room.member` event.
fn member(ev: &Value) -> Option<(&str, &str)> {
    if ev.get("type").and_then(Value::as_str) != Some("m.room.member") {
        return None;
    }
    Some((
        ev.get("state_key")?.as_str()?,
        ev.pointer("/content/membership")?.as_str()?,
    ))
}

/// True for a membership that lets a user read the room, or soon may.
fn present(membership: &str) -> bool {
    matches!(membership, "join" | "invite" | "knock")
}

/// True when a room's state shows a room of `users` alone: one of them
/// made it, it is joined by invite, and no one else is in it, invited or
/// knocking.
#[must_use]
pub fn private_room(state: &[Value], users: &[&str]) -> bool {
    let of = |kind: &str| {
        state
            .iter()
            .find(|e| e.get("type").and_then(Value::as_str) == Some(kind))
    };
    let made = of("m.room.create")
        .and_then(|e| e.get("sender")?.as_str())
        .is_some_and(|sender| users.contains(&sender));
    let invite_only = of("m.room.join_rules")
        .and_then(|e| e.pointer("/content/join_rule")?.as_str())
        == Some("invite");
    let mut members: Vec<&str> = state
        .iter()
        .filter_map(member)
        .filter(|(_, membership)| present(membership))
        .map(|(user, _)| user)
        .collect();
    members.sort_unstable();
    members.dedup();
    let mut want = users.to_vec();
    want.sort_unstable();
    made && invite_only && members == want
}

/// The requests of the client-server API the bot makes.
pub trait Api: Matrix {
    /// # Errors
    /// When the request fails.
    fn whoami(&mut self) -> Result<String> {
        let resp = self.call("GET", "/account/whoami", None, &[], DEFAULT_TIMEOUT)?;
        Ok(key_str(&resp, "user_id")?.to_owned())
    }

    /// Sends an event under the transaction id `txn`; its id.
    ///
    /// # Errors
    /// When the request fails.
    fn send(&mut self, room: &str, by: As, txn: &str, content: &Value) -> Result<Option<String>> {
        let resp = self.call(
            "PUT",
            &format!("/rooms/{}/send/m.room.message/{txn}", quote(room)),
            Some(content),
            &by.query(),
            DEFAULT_TIMEOUT,
        )?;
        Ok(resp
            .get("event_id")
            .and_then(Value::as_str)
            .map(str::to_owned))
    }

    /// Shows `user` typing in a room, or stops it.
    ///
    /// # Errors
    /// When the request fails.
    fn typing(&mut self, room: &str, by: As, user: &str, on: bool) -> Result<()> {
        let body = if on {
            json!({"typing": true, "timeout": TYPING_TIMEOUT.as_millis()})
        } else {
            json!({"typing": false})
        };
        self.call(
            "PUT",
            &format!("/rooms/{}/typing/{}", quote(room), quote(user)),
            Some(&body),
            &by.query(),
            DEFAULT_TIMEOUT,
        )?;
        Ok(())
    }

    /// # Errors
    /// When the request fails.
    fn receipt(&mut self, room: &str, by: As, event: &str) -> Result<()> {
        self.call(
            "POST",
            &format!("/rooms/{}/receipt/m.read/{}", quote(room), quote(event)),
            Some(&json!({})),
            &by.query(),
            DEFAULT_TIMEOUT,
        )?;
        Ok(())
    }

    /// A new direct chat with `invite`; its room id.
    ///
    /// # Errors
    /// When the request fails.
    fn create_dm(&mut self, by: As, invite: &str) -> Result<String> {
        let body = json!({"preset": "trusted_private_chat", "is_direct": true, "invite": [invite]});
        let resp = self.call(
            "POST",
            "/createRoom",
            Some(&body),
            &by.query(),
            DEFAULT_TIMEOUT,
        )?;
        Ok(key_str(&resp, "room_id")?.to_owned())
    }

    /// # Errors
    /// When the request fails.
    fn join(&mut self, room: &str) -> Result<()> {
        self.call(
            "POST",
            &format!("/join/{}", quote(room)),
            Some(&json!({})),
            &[],
            DEFAULT_TIMEOUT,
        )?;
        Ok(())
    }

    /// # Errors
    /// When the request fails.
    fn leave(&mut self, room: &str, by: As) -> Result<()> {
        self.call(
            "POST",
            &format!("/rooms/{}/leave", quote(room)),
            Some(&json!({})),
            &by.query(),
            DEFAULT_TIMEOUT,
        )?;
        Ok(())
    }

    /// The state events of a room, as the homeserver holds them.
    ///
    /// # Errors
    /// When the request fails.
    fn room_state(&mut self, room: &str) -> Result<Vec<Value>> {
        let resp = self.call(
            "GET",
            &format!("/rooms/{}/state", quote(room)),
            None,
            &[],
            DEFAULT_TIMEOUT,
        )?;
        match resp {
            Value::Array(events) => Ok(events),
            _ => Err(Error::Malformed("room state is not a list".into())),
        }
    }

    /// The bot's display name in one room.
    ///
    /// # Errors
    /// When the request fails.
    fn set_room_name(&mut self, room: &str, user: &str, name: &str) -> Result<()> {
        let path = format!("/rooms/{}/state/m.room.member/{}", quote(room), quote(user));
        let mut member = self.call("GET", &path, None, &[], DEFAULT_TIMEOUT)?;
        member["displayname"] = json!(name);
        self.call("PUT", &path, Some(&member), &[], DEFAULT_TIMEOUT)?;
        Ok(())
    }

    /// # Errors
    /// When the request fails.
    fn set_displayname(&mut self, user: &str, by: As, name: Option<&str>) -> Result<()> {
        self.call(
            "PUT",
            &format!("/profile/{}/displayname", quote(user)),
            Some(&json!({"displayname": name})),
            &by.query(),
            DEFAULT_TIMEOUT,
        )?;
        Ok(())
    }

    /// # Errors
    /// When the request fails.
    fn set_avatar(&mut self, user: &str, by: As, url: &str) -> Result<()> {
        self.call(
            "PUT",
            &format!("/profile/{}/avatar_url", quote(user)),
            Some(&json!({"avatar_url": url})),
            &by.query(),
            DEFAULT_TIMEOUT,
        )?;
        Ok(())
    }

    /// Registers a user of the appservice; one that exists is fine.
    ///
    /// # Errors
    /// When the request fails.
    fn register(&mut self, username: &str) -> Result<()> {
        let body = json!({"type": "m.login.application_service", "username": username, "inhibit_login": true});
        match self.call("POST", "/register", Some(&body), &[], DEFAULT_TIMEOUT) {
            Err(err) if err.errcode().as_deref() != Some("M_USER_IN_USE") => Err(err),
            _ => Ok(()),
        }
    }

    /// New events since `since`, waiting up to `wait` for some; the first
    /// sync skips history.
    ///
    /// # Errors
    /// When the request fails.
    fn sync(&mut self, since: Option<&str>, by: As, wait: Duration) -> Result<Sync> {
        let wait = wait.as_millis().to_string();
        let mut query: Vec<(&str, &str)> = match since {
            None => vec![("filter", r#"{"room": {"timeline": {"limit": 0}}}"#)],
            Some(since) => vec![("timeout", wait.as_str()), ("since", since)],
        };
        query.extend(by.query());
        Sync::parse(self.call("GET", "/sync", None, &query, SYNC_WAIT + DEFAULT_TIMEOUT)?)
    }
}

impl<T: Matrix + ?Sized> Api for T {}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'%' => {
                    out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).expect("hex"));
                    i += 3;
                }
                b'+' => {
                    out.push(b' ');
                    i += 1;
                }
                b => {
                    out.push(b);
                    i += 1;
                }
            }
        }
        String::from_utf8(out).expect("UTF-8")
    }

    #[test]
    fn quoting_round_trips() {
        let atoms = [
            "a",
            "Z9",
            " ",
            "&",
            "=",
            "+",
            "%",
            "/",
            "?",
            "#",
            ":",
            "!",
            "\"",
            "{",
            "\u{e4}",
            "\u{1F600}",
            "_.~-",
        ];
        let mut seed: u64 = 11;
        for _ in 0..200 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let text: String = (0..8)
                .map(|k| {
                    atoms[usize::try_from((seed >> (k * 5)) % atoms.len() as u64).expect("small")]
                })
                .collect();
            for quoted in [quote(&text), urlencode(&[("k", &text)])[2..].to_owned()] {
                assert!(
                    quoted
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "_.~%+-".contains(c)),
                    "{quoted}"
                );
                assert_eq!(decode(&quoted), text);
            }
        }
    }

    #[test]
    fn only_a_room_of_the_two_is_private() {
        let users = ["@o:e.org", "@b:e.org"];
        let room = |creator: &str, rule: &str, members: &[(&str, &str)]| {
            let mut state = vec![
                serde_json::json!({"type": "m.room.create", "sender": creator, "state_key": ""}),
                serde_json::json!({"type": "m.room.join_rules", "state_key": "",
                                   "content": {"join_rule": rule}}),
            ];
            state.extend(members.iter().map(|(user, membership)| {
                serde_json::json!({"type": "m.room.member", "state_key": user,
                                   "content": {"membership": membership}})
            }));
            state
        };
        let two = [("@o:e.org", "join"), ("@b:e.org", "join")];
        assert!(private_room(&room("@o:e.org", "invite", &two), &users));
        assert!(private_room(
            &room(
                "@b:e.org",
                "invite",
                &[two[0], two[1], ("@x:evil", "leave")]
            ),
            &users
        ));
        assert!(!private_room(&room("@x:evil", "invite", &two), &users));
        assert!(!private_room(&room("@o:e.org", "public", &two), &users));
        for membership in ["invite", "knock", "join"] {
            assert!(!private_room(
                &room(
                    "@o:e.org",
                    "invite",
                    &[two[0], two[1], ("@x:evil", membership)]
                ),
                &users
            ));
        }
        assert!(!private_room(&[], &users));
    }
}
