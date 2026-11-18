//! The chat gate: reads, searches and sends in the owner's bridged chats
//! with the owner's token, one request of `herdr-matrix chats` per run.
//! Rooms of the bot and its users are refused, so the token cannot reach
//! the bot's chats.

use std::collections::BTreeSet;
use std::io::{BufRead as _, BufReader, Read, Write};
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde_json::{Value, json};

use crate::app::{credential, safe_url};
use crate::files::random_name;
use crate::matrix::{Api as _, As, DEFAULT_TIMEOUT, Http, Matrix, member, present, quote};
use crate::{Error, Result};

/// Most events one request walks through
pub const CAP: usize = 5000;
/// Most events one page asks for
const PAGE: usize = 100;
/// Messages `read` gives without a count or `--since`
const COUNT: usize = 20;
/// Messages around each hit of `search` without `--context`
const CONTEXT: usize = 2;
/// Leaves reactions, state and the like on the server
const FILTER: &str = r#"{"types":["m.room.message","m.room.encrypted"]}"#;
/// Bytes of a request at most
const REQUEST_MAX: u64 = 1 << 20;
const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;

pub const USAGE: &str = "usage: herdr-matrix chats rooms | read <room> [count] [--since T] [--until T] [--from END] | search <room> <words>... [--since T] [--until T] [--context N] | send <room> <text>...; T is <n>h, <n>d, <n>w, YYYY-MM-DD or YYYY-MM-DDTHH:MM[:SS][Z], in UTC";

/// Which rooms and commands the gate serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rules {
    /// Localpart prefixes of bridged contacts, as `signal` of `@signal_…`
    pub bridges: Vec<String>,
    /// Localpart of the bot, as `herdr` of `@herdr` and `@herdr_…`
    pub bot: String,
    /// The commands the caller may give; every one without a list
    pub commands: Option<Vec<String>>,
    /// The bridged chats the caller sees; every one without a list
    pub rooms: Option<Vec<String>>,
}

/// A time range, in milliseconds since the epoch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Range {
    pub since: Option<i64>,
    pub until: Option<i64>,
}

/// A request of `herdr-matrix chats`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Rooms,
    Read {
        room: String,
        count: Option<usize>,
        from: Option<String>,
        range: Range,
    },
    Search {
        room: String,
        words: Vec<String>,
        context: usize,
        range: Range,
    },
    Send {
        room: String,
        text: String,
    },
}

/// The owner's account, as the gate uses it.
pub struct Gate<M: Matrix> {
    pub matrix: M,
    /// The server part of the owner's user id
    server: String,
    rules: Rules,
}

/// Messages of a room, oldest first.
pub struct History {
    pub messages: Vec<Value>,
    /// Where `--from` continues, when the walk stopped at the count
    pub end: Option<String>,
    pub truncated: bool,
    /// Encrypted messages the gate cannot read
    pub encrypted: usize,
}

impl Rules {
    /// True for a room with a joined bridged contact on the owner's server
    /// `server`, and no user of the bot, on any server, joined, invited or
    /// knocking. `members` are user ids and their memberships.
    #[must_use]
    pub fn serves(&self, members: &[(&str, &str)], server: &str) -> bool {
        let bridged = members.iter().any(|&(user, membership)| {
            membership == "join"
                && parts(user).is_some_and(|(local, home)| {
                    home == server
                        && self
                            .bridges
                            .iter()
                            .any(|bridge| after(local, bridge).is_some_and(|rest| !rest.is_empty()))
                })
        });
        let bot = members.iter().any(|&(user, membership)| {
            present(membership)
                && parts(user).is_some_and(|(local, _)| {
                    local == self.bot || after(local, &self.bot).is_some()
                })
        });
        bridged && !bot
    }

    fn lets(&self, command: &str) -> bool {
        self.commands
            .as_ref()
            .is_none_or(|list| list.iter().any(|c| c == command))
    }

    fn shows(&self, room: &str) -> bool {
        self.rooms
            .as_ref()
            .is_none_or(|list| list.iter().any(|r| r == room))
    }
}

impl Request {
    /// The command, as `herdr-matrix chats` takes it.
    #[must_use]
    pub fn command(&self) -> &'static str {
        match self {
            Self::Rooms => "rooms",
            Self::Read { .. } => "read",
            Self::Search { .. } => "search",
            Self::Send { .. } => "send",
        }
    }
}

/// The localpart and the server of a user id.
fn parts(user: &str) -> Option<(&str, &str)> {
    user.strip_prefix('@')?.split_once(':')
}

/// What follows `<prefix>_` in a localpart.
fn after<'a>(local: &'a str, prefix: &str) -> Option<&'a str> {
    local.strip_prefix(prefix)?.strip_prefix('_')
}

/// A walk back through the messages of a room.
struct Walk {
    found: Vec<Value>,
    hidden: usize,
    walked: usize,
    token: Option<String>,
    more: bool,
}

impl Walk {
    /// Takes a page of events, newest first, as far as the range goes.
    fn take(&mut self, page: &Value, range: Range) {
        let chunk = page
            .get("chunk")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        self.token = page.get("end").and_then(Value::as_str).map(str::to_owned);
        self.more = self.token.is_some() && !chunk.is_empty();
        self.walked += chunk.len();
        for event in chunk {
            let Some(stamp) = event.get("origin_server_ts").and_then(Value::as_i64) else {
                continue;
            };
            if range.since.is_some_and(|since| stamp < since) {
                self.more = false;
                break;
            }
            if range.until.is_none_or(|until| stamp <= until) {
                self.keep(event, stamp);
            }
        }
    }

    fn keep(&mut self, event: &Value, stamp: i64) {
        match event.get("type").and_then(Value::as_str) {
            Some("m.room.encrypted") => self.hidden += 1,
            Some("m.room.message") => {
                if let Some(body) = event.pointer("/content/body").and_then(Value::as_str) {
                    self.found.push(json!({
                        "sender": event.get("sender").cloned().unwrap_or_default(),
                        "time": time(stamp),
                        "body": body,
                    }));
                }
            }
            _ => {}
        }
    }

    fn finish(mut self) -> Result<History> {
        if self.hidden > 0 && self.found.is_empty() {
            return Err(Error::Refused(format!(
                "the room is encrypted: {} messages the gate cannot read",
                self.hidden
            )));
        }
        self.found.reverse();
        Ok(History {
            messages: self.found,
            end: self.token.filter(|_| self.more),
            truncated: self.more,
            encrypted: self.hidden,
        })
    }
}

/// `answer` with the count of encrypted messages, when there are some.
fn with_encrypted(mut answer: Value, encrypted: usize) -> Value {
    if encrypted > 0 {
        answer["encrypted"] = json!(encrypted);
    }
    answer
}

impl<M: Matrix> Gate<M> {
    /// The gate of `me`, the user the token of `matrix` belongs to.
    #[must_use]
    pub fn new(matrix: M, me: &str, rules: Rules) -> Self {
        let server = me.split_once(':').map_or("", |(_, s)| s).to_owned();
        Self {
            matrix,
            server,
            rules,
        }
    }

    fn serves(&mut self, room: &str) -> Result<bool> {
        if !self.rules.shows(room) {
            return Ok(false);
        }
        let path = format!("/rooms/{}/members", quote(room));
        let resp = self.matrix.call("GET", &path, None, &[], DEFAULT_TIMEOUT)?;
        let members: Vec<(&str, &str)> = resp
            .get("chunk")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(member)
            .collect();
        Ok(self.rules.serves(&members, &self.server))
    }

    fn check(&mut self, room: &str) -> Result<()> {
        if self.serves(room)? {
            Ok(())
        } else {
            Err(Error::Refused("not a bridged chat".into()))
        }
    }

    /// The name of a room, else its id.
    fn name(&mut self, room: &str) -> Result<String> {
        let path = format!("/rooms/{}/state/m.room.name/", quote(room));
        match self.matrix.call("GET", &path, None, &[], DEFAULT_TIMEOUT) {
            Ok(state) => Ok(state
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(room)
                .to_owned()),
            Err(Error::Http { .. }) => Ok(room.to_owned()),
            Err(err) => Err(err),
        }
    }

    fn rooms(&mut self) -> Result<Value> {
        let joined = self
            .matrix
            .call("GET", "/joined_rooms", None, &[], DEFAULT_TIMEOUT)?;
        let mut served = Vec::new();
        for room in joined
            .get("joined_rooms")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if self.serves(room)? {
                served.push(json!({"room": room, "name": self.name(room)?}));
            }
        }
        Ok(Value::Array(served))
    }

    /// Up to `limit` messages, walking back from `from` or the newest. A
    /// page never asks for more than is still wanted, so `end` loses
    /// nothing.
    ///
    /// # Errors
    /// When the room is not a bridged chat, when the gate can read none of
    /// its messages, or when a request fails.
    pub fn history(
        &mut self,
        room: &str,
        limit: usize,
        from: Option<&str>,
        range: Range,
    ) -> Result<History> {
        self.check(room)?;
        let path = format!("/rooms/{}/messages", quote(room));
        let mut walk = Walk {
            found: Vec::new(),
            hidden: 0,
            walked: 0,
            token: from.map(str::to_owned),
            more: true,
        };
        while walk.more && walk.found.len() < limit && walk.walked < CAP {
            let want = PAGE
                .min(limit - walk.found.len())
                .min(CAP - walk.walked)
                .to_string();
            let token = walk.token.take();
            let mut query = vec![("dir", "b"), ("limit", want.as_str()), ("filter", FILTER)];
            if let Some(token) = &token {
                query.push(("from", token));
            }
            let page = self
                .matrix
                .call("GET", &path, None, &query, DEFAULT_TIMEOUT)?;
            walk.take(&page, range);
        }
        walk.finish()
    }

    /// The messages up to the cap with every word, in runs with `context`
    /// messages around each.
    ///
    /// # Errors
    /// As `history`.
    pub fn search(
        &mut self,
        room: &str,
        words: &[String],
        context: usize,
        range: Range,
    ) -> Result<Value> {
        let found = self.history(room, CAP, None, range)?;
        let messages = &found.messages;
        let hits: Vec<usize> = (0..messages.len())
            .filter(|&i| {
                messages[i]["body"]
                    .as_str()
                    .is_some_and(|b| matches(words, b))
            })
            .collect();
        let runs: Vec<Vec<Value>> = around(&hits, messages.len(), context)
            .iter()
            .map(|run| {
                run.iter()
                    .map(|&i| {
                        let mut message = messages[i].clone();
                        if hits.binary_search(&i).is_ok() {
                            message["hit"] = json!(true);
                        }
                        message
                    })
                    .collect()
            })
            .collect();
        Ok(with_encrypted(
            json!({"hits": runs, "scanned": messages.len(), "truncated": found.truncated}),
            found.encrypted,
        ))
    }

    fn send(&mut self, room: &str, text: &str) -> Result<Value> {
        self.check(room)?;
        // One process per request: a counter would repeat across them
        let txn = format!("{}-{}", now_ms(), random_name());
        let event = self.matrix.send(
            room,
            As::Bot,
            &txn,
            &json!({"msgtype": "m.text", "body": text}),
        )?;
        Ok(json!({ "event": event }))
    }

    /// The answer to a request.
    ///
    /// # Errors
    /// When the command is not allowed, when the room is not a bridged
    /// chat, when the gate can read none of its messages, or when a request
    /// fails.
    pub fn run(&mut self, request: &Request) -> Result<Value> {
        if !self.rules.lets(request.command()) {
            return Err(Error::Refused("not allowed for this agent".into()));
        }
        match request {
            Request::Rooms => self.rooms(),
            Request::Read {
                room,
                count,
                from,
                range,
            } => {
                let whole = if range.since.is_some() { CAP } else { COUNT };
                let found = self.history(
                    room,
                    count.unwrap_or(whole).min(CAP),
                    from.as_deref(),
                    *range,
                )?;
                Ok(with_encrypted(
                    json!({"messages": found.messages, "end": found.end, "truncated": found.truncated}),
                    found.encrypted,
                ))
            }
            Request::Search {
                room,
                words,
                context,
                range,
            } => self.search(room, words, *context, *range),
            Request::Send { room, text } => self.send(room, text),
        }
    }
}

/// Milliseconds since the epoch of `<n>h`, `<n>d` or `<n>w` before `now`,
/// or of a date and time in UTC.
fn moment(text: &str, now: i64) -> Result<i64, &'static str> {
    static AGO: LazyLock<Regex> =
        LazyLock::new(|| Regex::new("^([0-9]{1,6})([hdw])$").expect("a valid pattern"));
    static DATE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            "^([0-9]{4})-([0-9]{2})-([0-9]{2})(?:T([0-9]{2}):([0-9]{2})(?::([0-9]{2}))?Z?)?$",
        )
        .expect("a valid pattern")
    });
    const BAD: &str = "--since or --until is not a time";
    if let Some(ago) = AGO.captures(text) {
        let unit = match &ago[2] {
            "h" => HOUR_MS,
            "d" => DAY_MS,
            _ => 7 * DAY_MS,
        };
        return Ok(now - number(ago.get(1)) * unit);
    }
    let date = DATE.captures(text).ok_or(BAD)?;
    let [year, month, day, hour, minute, second] = [1, 2, 3, 4, 5, 6].map(|i| number(date.get(i)));
    let days = days(year, month, day);
    if civil(days) != (year, month, day) || hour > 23 || minute > 59 || second > 59 {
        return Err(BAD);
    }
    Ok(((days * 24 + hour) * 60 + minute) * 60_000 + second * 1000)
}

/// The number the pattern matched, else 0.
fn number(digits: Option<regex::Match>) -> i64 {
    digits
        .and_then(|d| d.as_str().parse().ok())
        .unwrap_or_default()
}

/// Days since 1970-01-01 of a date of the proleptic Gregorian calendar.
fn days(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + of_year - 719_468
}

/// The date of a count of days since 1970-01-01.
fn civil(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let of_era = shifted - era * 146_097;
    let year = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year + year / 4 - year / 100);
    let shifted_month = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    (year + era * 400 + i64::from(month <= 2), month, day)
}

/// A time in UTC, `YYYY-MM-DDTHH:MMZ`.
fn time(ms: i64) -> String {
    let minutes = ms.div_euclid(60_000);
    let (year, month, day) = civil(minutes.div_euclid(1440));
    let of_day = minutes.rem_euclid(1440);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}Z",
        of_day / 60,
        of_day % 60
    )
}

/// True when every word is in the body, whatever the case.
fn matches(words: &[String], body: &str) -> bool {
    let body = body.to_lowercase();
    words.iter().all(|word| body.contains(&word.to_lowercase()))
}

/// Runs of the indices below `count` that cover each hit, in order, and
/// `context` neighbours on each side.
fn around(hits: &[usize], count: usize, context: usize) -> Vec<Vec<usize>> {
    let keep: BTreeSet<usize> = hits
        .iter()
        .flat_map(|&hit| {
            hit.saturating_sub(context)..hit.saturating_add(context).saturating_add(1).min(count)
        })
        .collect();
    let mut runs: Vec<Vec<usize>> = Vec::new();
    for index in keep {
        match runs.last_mut() {
            Some(run) if run.last().is_some_and(|&last| last + 1 == index) => run.push(index),
            _ => runs.push(vec![index]),
        }
    }
    runs
}

/// The arguments after the room of `read` and `search`.
#[derive(Default)]
struct Args {
    words: Vec<String>,
    range: Range,
    from: Option<String>,
    context: Option<usize>,
}

impl Args {
    /// `rest`, with `--since`, `--until` and the flag `extra` anywhere in
    /// it.
    fn parse(rest: &[String], now: i64, extra: &str) -> Result<Self, &'static str> {
        let mut args = Self::default();
        let mut rest = rest.iter();
        while let Some(arg) = rest.next() {
            if !arg.starts_with("--") {
                args.words.push(arg.clone());
                continue;
            }
            if !["--since", "--until", extra].contains(&arg.as_str()) {
                return Err("unknown flag");
            }
            let value = rest.next().ok_or("a flag without its value")?;
            args.set(arg, value, now)?;
        }
        Ok(args)
    }

    fn set(&mut self, flag: &str, value: &str, now: i64) -> Result<(), &'static str> {
        match flag {
            "--since" => self.range.since = Some(moment(value, now)?),
            "--until" => self.range.until = Some(moment(value, now)?),
            "--from" => self.from = Some(value.to_owned()),
            _ => self.context = Some(value.parse().map_err(|_| "--context is not a number")?),
        }
        Ok(())
    }
}

fn read(room: String, args: Args) -> Result<Request, &'static str> {
    let count = match args.words.as_slice() {
        [] => None,
        [count] => Some(
            count
                .parse::<usize>()
                .ok()
                .filter(|&n| n > 0)
                .ok_or("the count is not a positive number")?,
        ),
        _ => return Err("too many arguments"),
    };
    Ok(Request::Read {
        room,
        count,
        from: args.from,
        range: args.range,
    })
}

fn search(room: String, args: Args) -> Result<Request, &'static str> {
    if args.words.is_empty() {
        return Err("no words");
    }
    Ok(Request::Search {
        room,
        words: args.words,
        context: args.context.unwrap_or(CONTEXT),
        range: args.range,
    })
}

fn request(argv: &[String], now: i64) -> Result<Request, &'static str> {
    let Some((op, rest)) = argv.split_first() else {
        return Err("no command");
    };
    let room = || {
        rest.first()
            .filter(|room| !room.is_empty() && !room.starts_with("--"))
            .cloned()
            .ok_or("no room")
    };
    match op.as_str() {
        "rooms" if rest.is_empty() => Ok(Request::Rooms),
        "rooms" => Err("rooms takes no arguments"),
        "read" => read(room()?, Args::parse(&rest[1..], now, "--from")?),
        "search" => search(room()?, Args::parse(&rest[1..], now, "--context")?),
        "send" => {
            let room = room()?;
            let text = rest[1..].join(" ");
            if text.is_empty() {
                return Err("no text");
            }
            Ok(Request::Send { room, text })
        }
        _ => Err("unknown command"),
    }
}

/// The request of the arguments of `herdr-matrix chats`; `now` is in
/// milliseconds since the epoch.
///
/// # Errors
/// When the arguments do not fit the usage: one line that holds it.
pub fn parse(argv: &[String], now: i64) -> Result<Request> {
    request(argv, now).map_err(|why| Error::Refused(format!("{why}; {USAGE}")))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// The homeserver and the rules, from the gate's unit; the unit of a
/// confined agent's socket sets the lists of commands and rooms.
fn settings(env: &dyn Fn(&str) -> Option<String>) -> Result<(String, Rules)> {
    let need = |name: &str| {
        env(name)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| Error::Setting(format!("{name} is not set")))
    };
    let homeserver = need("HERDR_MATRIX_GATE_HOMESERVER")?;
    if !safe_url(&homeserver) {
        return Err(Error::Setting(format!(
            "HERDR_MATRIX_GATE_HOMESERVER {homeserver} is not https://, and not on this host"
        )));
    }
    let bridges = need("HERDR_MATRIX_GATE_BRIDGES")?
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let bot = need("HERDR_MATRIX_GATE_BOT")?.trim().to_owned();
    let list =
        |name: &str| env(name).map(|value| value.split_whitespace().map(str::to_owned).collect());
    Ok((
        homeserver,
        Rules {
            bridges,
            bot,
            commands: list("HERDR_MATRIX_GATE_COMMANDS"),
            rooms: list("HERDR_MATRIX_GATE_ROOMS"),
        },
    ))
}

/// The arguments of a request, a JSON line `{"argv": [...]}`.
fn argv(input: &mut dyn Read) -> Result<Vec<String>> {
    let mut line = String::new();
    BufReader::new(input.take(REQUEST_MAX)).read_line(&mut line)?;
    let bad = || Error::Refused("the request is not {\"argv\": [...]}".into());
    let request: Value = serde_json::from_str(&line).map_err(|_| bad())?;
    request
        .get("argv")
        .and_then(Value::as_array)
        .and_then(|argv| argv.iter().map(|a| a.as_str().map(str::to_owned)).collect())
        .ok_or_else(bad)
}

fn answer(env: &dyn Fn(&str) -> Option<String>, argv: &[String]) -> Result<Value> {
    let request = parse(argv, now_ms())?;
    let (homeserver, rules) = settings(env)?;
    // The owner's token, which systemd passes
    let mut matrix = Http::new(&homeserver, &credential(env, "token")?);
    let me = matrix.whoami()?;
    Gate::new(matrix, &me, rules).run(&request)
}

/// Answers one request of `herdr-matrix chats`, as the filter systemd
/// runs per connection: a JSON line `{"argv": [...]}` from `input`, a JSON
/// line `{"ok": ...}` or `{"error": "..."}` to `output`. `log` gets one
/// line with the command and the outcome, never the token, a message or a
/// room name.
///
/// # Errors
/// When the answer cannot be written.
pub fn serve(
    env: &dyn Fn(&str) -> Option<String>,
    input: &mut dyn Read,
    output: &mut dyn Write,
    log: &dyn Fn(&str),
) -> Result<()> {
    let argv = argv(input);
    let op = argv
        .as_ref()
        .ok()
        .and_then(|argv| argv.first())
        .map(String::as_str)
        .filter(|op| ["rooms", "read", "search", "send"].contains(op))
        .unwrap_or("unknown")
        .to_owned();
    let reply = match argv.and_then(|argv| answer(env, &argv)) {
        Ok(ok) => {
            log(&format!("{op}: ok"));
            json!({ "ok": ok })
        }
        Err(err) => {
            log(&format!("{op}: {err}"));
            json!({ "error": err.to_string() })
        }
    };
    writeln!(output, "{reply}")?;
    output.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matrix::Source;

    const NOW: i64 = 1_800_000_000_000;
    const ME: &str = "@me:x";

    fn rules() -> Rules {
        Rules {
            bridges: ["whatsapp", "signal", "telegram"]
                .map(str::to_owned)
                .to_vec(),
            bot: "herdr".into(),
            commands: None,
            rooms: None,
        }
    }

    fn serves(members: &[(&str, &str)]) -> bool {
        rules().serves(members, "x")
    }

    /// Rooms of members, and a room of numbered events, newest first,
    /// served in pages.
    struct Fake {
        rooms: Vec<(&'static str, Vec<(&'static str, &'static str)>)>,
        names: Vec<(&'static str, &'static str)>,
        events: Vec<Value>,
        sent: Vec<(String, Value)>,
        paths: Vec<String>,
    }

    const BRIDGED: &[(&str, &str)] = &[("@me:x", "join"), ("@signal_1:x", "join")];
    const HERDR: &[(&str, &str)] = &[
        ("@me:x", "join"),
        ("@signal_1:x", "join"),
        ("@herdr:x", "join"),
    ];

    fn fake(events: Vec<Value>) -> Gate<Fake> {
        Gate::new(
            Fake {
                rooms: vec![("!r", BRIDGED.to_vec()), ("!h", HERDR.to_vec())],
                names: vec![("!r", "Friend")],
                events,
                sent: Vec::new(),
                paths: Vec::new(),
            },
            ME,
            rules(),
        )
    }

    impl Matrix for Fake {
        fn call(
            &mut self,
            method: &str,
            path: &str,
            body: Option<&Value>,
            query: &[(&str, &str)],
            _timeout: std::time::Duration,
        ) -> Result<Value> {
            self.paths.push(path.to_owned());
            let room = |suffix: &str| {
                self.rooms
                    .iter()
                    .find(|(id, _)| path == format!("/rooms/{}{suffix}", quote(id)))
                    .map(|(id, members)| (*id, members.clone()))
            };
            if path == "/joined_rooms" {
                let ids: Vec<&str> = self.rooms.iter().map(|(id, _)| *id).collect();
                return Ok(json!({ "joined_rooms": ids }));
            }
            if let Some((_, members)) = room("/members") {
                let chunk: Vec<Value> = members
                    .iter()
                    .map(|(user, membership)| {
                        json!({"type": "m.room.member", "state_key": user,
                               "content": {"membership": membership}})
                    })
                    .collect();
                return Ok(json!({ "chunk": chunk }));
            }
            if path.ends_with("/members") {
                return Err(Error::Http {
                    status: 403,
                    code: "M_FORBIDDEN".into(),
                    body: Vec::new(),
                });
            }
            if let Some((id, _)) = room("/state/m.room.name/") {
                return match self.names.iter().find(|(r, _)| *r == id) {
                    Some((_, name)) => Ok(json!({ "name": name })),
                    None => Err(Error::Http {
                        status: 404,
                        code: "M_NOT_FOUND".into(),
                        body: Vec::new(),
                    }),
                };
            }
            if method == "PUT" {
                self.sent
                    .push((path.to_owned(), body.cloned().unwrap_or_default()));
                return Ok(json!({"event_id": "$sent"}));
            }
            assert!(room("/messages").is_some(), "{path}");
            let get = |name: &str| query.iter().find(|(k, _)| *k == name).map(|(_, v)| *v);
            assert_eq!((get("dir"), get("filter")), (Some("b"), Some(FILTER)));
            let at: usize = get("from").map_or(0, |v| v.parse().expect("a token"));
            let limit: usize = get("limit").expect("a limit").parse().expect("a number");
            assert!(limit <= PAGE);
            let len = self.events.len();
            let chunk = self.events[at.min(len)..(at + limit).min(len)].to_vec();
            Ok(json!({"chunk": chunk, "end": (at + chunk.len()).to_string()}))
        }

        fn upload(&mut self, _: &mut dyn Source, _: u64, _: &str) -> Result<String> {
            unreachable!()
        }

        fn download(&mut self, _: &str, _: &mut dyn std::io::Write) -> Result<()> {
            unreachable!()
        }

        fn upload_limit(&mut self) -> Result<Option<u64>> {
            unreachable!()
        }

        fn next_txn(&mut self) -> String {
            unreachable!()
        }
    }

    fn event(stamp: i64, body: Option<&str>, kind: &str) -> Value {
        let content = body.map_or_else(|| json!({}), |b| json!({ "body": b }));
        json!({"type": kind, "sender": "@signal_1:x", "origin_server_ts": stamp, "content": content})
    }

    fn message(stamp: i64, body: Option<&str>) -> Value {
        event(stamp, body, "m.room.message")
    }

    /// 250 messages at stamps 250..1, an event without a body between each.
    fn room() -> Gate<Fake> {
        let mut events = Vec::new();
        for stamp in (1..=250).rev() {
            events.push(message(stamp, Some(&format!("m{stamp}"))));
            events.push(message(stamp, None));
        }
        fake(events)
    }

    fn answer(gate: &mut Gate<Fake>, argv: &[&str]) -> Result<Value> {
        let argv: Vec<String> = argv.iter().map(|a| (*a).to_owned()).collect();
        gate.run(&parse(&argv, NOW)?)
    }

    fn bodies(messages: &Value) -> Vec<&str> {
        messages
            .as_array()
            .expect("a list")
            .iter()
            .map(|m| m["body"].as_str().expect("a body"))
            .collect()
    }

    fn since(ms: i64) -> Range {
        Range {
            since: Some(ms),
            until: None,
        }
    }

    #[test]
    fn room_rule() {
        assert!(serves(&[("@me:x", "join"), ("@whatsapp_123:x", "join")]));
        assert!(!serves(&[
            ("@me:x", "join"),
            ("@whatsapp_1:x", "join"),
            ("@herdr:x", "join")
        ]));
        assert!(!serves(&[
            ("@me:x", "join"),
            ("@whatsapp_1:x", "join"),
            ("@herdr_molesk:x", "join")
        ]));
        assert!(!serves(&[("@me:x", "join"), ("@whatsappbot:x", "join")]));
        assert!(!serves(&[("@me:x", "join"), ("@friend:x", "join")]));
    }

    #[test]
    fn federated_bridge_user_is_no_bridged_chat() {
        assert!(!serves(&[
            ("@me:x", "join"),
            ("@signal_1:other.org", "join")
        ]));
        assert!(!serves(&[("@me:x", "join"), ("@signal_1:x", "invite")]));
        assert!(!serves(&[("@me:x", "join"), ("@signal_:x", "join")]));
    }

    #[test]
    fn bot_user_on_any_server_and_membership_refuses() {
        let with = |user, membership| serves(&[("@signal_1:x", "join"), (user, membership)]);
        for membership in ["join", "invite", "knock"] {
            assert!(!with("@herdr_x:x", membership), "{membership}");
            assert!(!with("@herdr:other.org", membership), "{membership}");
        }
        assert!(with("@herdr_x:x", "leave"));
        assert!(with("@herdrx:x", "join"));
    }

    #[test]
    fn times() {
        assert_eq!(moment("7d", NOW), Ok(NOW - 7 * DAY_MS));
        assert_eq!(moment("12h", NOW), Ok(NOW - 12 * HOUR_MS));
        assert_eq!(moment("2w", NOW), Ok(NOW - 14 * DAY_MS));
        assert_eq!(moment("1970-01-01", NOW), Ok(0));
        assert_eq!(
            moment("2026-11-02", NOW).expect("a date") - moment("2026-11-01", NOW).expect("a date"),
            DAY_MS
        );
        assert_eq!(moment("2026-11-16T12:30Z", NOW), Ok(1_794_832_200_000));
        assert_eq!(moment("2026-11-16T12:30:15", NOW), Ok(1_794_832_215_000));
        assert_eq!(moment("2024-02-29", NOW), Ok(1_709_164_800_000));
        for bad in [
            "2026-02-29",
            "2026-13-01",
            "2026-11-00",
            "2026-11-16T24:00",
            "2026-11-16T12:60",
            "2026-11-16 12:30",
            "7x",
            "d",
            "\u{663}d",
            "",
        ] {
            assert!(moment(bad, NOW).is_err(), "{bad}");
        }
        assert_eq!(time(0), "1970-01-01T00:00Z");
        assert_eq!(time(-60_000), "1969-12-31T23:59Z");
        assert_eq!(time(1_794_832_215_000), "2026-11-16T12:30Z");
        assert_eq!(time(1_709_164_800_000), "2024-02-29T00:00Z");
    }

    #[test]
    fn words_match_in_any_case() {
        assert!(matches(
            &["Gift".into(), "idea".into()],
            "an IDEA for a gift"
        ));
        assert!(!matches(
            &["gift".into(), "ring".into()],
            "an idea for a gift"
        ));
    }

    #[test]
    fn runs_cover_hits_and_neighbours() {
        assert_eq!(around(&[0, 1, 9], 10, 1), [vec![0, 1, 2], vec![8, 9]]);
        assert_eq!(around(&[5], 10, 0), [vec![5]]);
        assert_eq!(around(&[3], 4, usize::MAX), [vec![0, 1, 2, 3]]);
        assert!(around(&[], 4, 2).is_empty());
    }

    #[test]
    fn history_pages_and_continues() {
        let mut room = room();
        let last = room.history("!r", 3, None, Range::default()).expect("read");
        assert_eq!(bodies(&json!(last.messages)), ["m248", "m249", "m250"]);
        let first = room
            .history("!r", 120, None, Range::default())
            .expect("read");
        assert_eq!(first.messages.len(), 120);
        assert!(first.truncated);
        let rest = room
            .history("!r", CAP, first.end.as_deref(), Range::default())
            .expect("read");
        assert_eq!(rest.messages.len(), 130);
        assert!(!rest.truncated && rest.end.is_none());
        assert_eq!(bodies(&json!(rest.messages)).last(), Some(&"m130"));
        assert_eq!(rest.messages[0]["sender"], "@signal_1:x");
        assert_eq!(rest.messages[0]["time"], "1970-01-01T00:00Z");
    }

    #[test]
    fn history_takes_a_range_up_to_the_cap() {
        let mut room = room();
        let ranged = room
            .history(
                "!r",
                CAP,
                None,
                Range {
                    since: Some(100),
                    until: Some(110),
                },
            )
            .expect("read");
        let want: Vec<String> = (100..=110).map(|n| format!("m{n}")).collect();
        assert_eq!(bodies(&json!(ranged.messages)), want);
        assert!(!ranged.truncated);
        let capped = room.history("!r", 5, None, since(100)).expect("read");
        assert_eq!(capped.messages.len(), 5);
        assert!(capped.truncated);
    }

    #[test]
    fn search_marks_hits_in_runs() {
        let mut room = room();
        let found = answer(
            &mut room,
            &[
                "search",
                "!r",
                "--context",
                "0",
                "m25",
                "--since",
                "1970-01-01",
            ],
        )
        .expect("search");
        assert_eq!(found["scanned"], 250);
        let runs: Vec<Vec<&str>> = found["hits"]
            .as_array()
            .expect("runs")
            .iter()
            .map(bodies)
            .collect();
        assert_eq!(runs, [["m25"], ["m250"]]);
        let found = room
            .search("!r", &["M17".into()], 1, since(10))
            .expect("search");
        assert_eq!(found["scanned"], 241);
        assert_eq!(found["truncated"], false);
        let hits = found["hits"].as_array().expect("runs");
        assert_eq!(bodies(&hits[0]), ["m16", "m17", "m18"]);
        assert_eq!(hits[0][1]["hit"], true);
        assert!(hits[0][0].get("hit").is_none());
        // m17, then the run m169..m180
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[1].as_array().map(Vec::len), Some(12));
        assert!(found.get("end").is_none() && found.get("messages").is_none());
    }

    #[test]
    fn encrypted_room_is_an_error() {
        let mut secret = fake(vec![event(2, None, "m.room.encrypted"); 3]);
        let err = answer(&mut secret, &["read", "!r"]).expect_err("encrypted");
        assert_eq!(
            err.to_string(),
            "the room is encrypted: 3 messages the gate cannot read"
        );
    }

    #[test]
    fn walk_stops_at_the_cap_of_events() {
        let events: Vec<Value> = (0..12_000)
            .map(|i| event(12_000 - i, None, "m.room.encrypted"))
            .collect();
        let mut secret = fake(events);
        for (argv, page) in [
            (&["read", "!r"][..], COUNT),
            (&["read", "!r", "--since", "1970-01-01"], PAGE),
        ] {
            secret.matrix.paths.clear();
            let err = answer(&mut secret, argv).expect_err("encrypted");
            assert_eq!(
                err.to_string(),
                format!("the room is encrypted: {CAP} messages the gate cannot read")
            );
            let pages = secret
                .matrix
                .paths
                .iter()
                .filter(|p| p.ends_with("/messages"))
                .count();
            assert_eq!(pages, CAP / page, "{argv:?}");
        }
    }

    #[test]
    fn mixed_room_counts_what_it_cannot_read() {
        let mut mixed = fake(vec![
            message(2, Some("plain")),
            event(1, None, "m.room.encrypted"),
            event(1, Some("sticker"), "m.sticker"),
        ]);
        let read = answer(&mut mixed, &["read", "!r"]).expect("read");
        assert_eq!(bodies(&read["messages"]), ["plain"]);
        assert_eq!(read["encrypted"], 1);
        let found = answer(&mut mixed, &["search", "!r", "plain"]).expect("search");
        assert_eq!(found["encrypted"], 1);
    }

    #[test]
    fn bot_room_is_refused() {
        let mut room = room();
        for argv in [
            &["read", "!h"][..],
            &["search", "!h", "x"],
            &["send", "!h", "x"],
        ] {
            let err = answer(&mut room, argv).expect_err("refused");
            assert_eq!(err.to_string(), "not a bridged chat", "{argv:?}");
        }
        assert!(room.matrix.sent.is_empty());
    }

    #[test]
    fn send_posts_one_text_message() {
        let mut room = room();
        assert_eq!(
            answer(&mut room, &["send", "!r", "two", "words"]).expect("sent"),
            json!({"event": "$sent"})
        );
        answer(&mut room, &["send", "!r", "--since", "7d"]).expect("sent");
        let bodies: Vec<&Value> = room.matrix.sent.iter().map(|(_, b)| b).collect();
        assert_eq!(
            bodies,
            [
                &json!({"msgtype": "m.text", "body": "two words"}),
                &json!({"msgtype": "m.text", "body": "--since 7d"})
            ]
        );
        let paths: Vec<&str> = room.matrix.sent.iter().map(|(p, _)| p.as_str()).collect();
        assert!(paths[0].starts_with("/rooms/%21r/send/m.room.message/"));
        assert_ne!(paths[0], paths[1]);
    }

    #[test]
    fn read_gives_the_last_few_or_the_whole_range() {
        let mut room = room();
        let read = answer(&mut room, &["read", "!r"]).expect("read");
        assert_eq!(read["messages"].as_array().map(Vec::len), Some(20));
        assert_eq!(read["truncated"], true);
        let read = answer(&mut room, &["read", "!r", "--since", "1970-01-01"]).expect("read");
        assert_eq!(read["messages"].as_array().map(Vec::len), Some(250));
        assert_eq!(
            (&read["truncated"], &read["end"]),
            (&json!(false), &Value::Null)
        );
        let read = answer(&mut room, &["read", "--from", "40", "!r", "5"]);
        assert!(read.is_err(), "the room comes first");
        let read = answer(&mut room, &["read", "!r", "--from", "40", "5"]).expect("read");
        assert_eq!(
            bodies(&read["messages"]),
            ["m226", "m227", "m228", "m229", "m230"]
        );
    }

    #[test]
    fn rooms_lists_bridged_chats_with_names() {
        let mut gate = fake(Vec::new());
        gate.matrix.rooms.push(("!n", BRIDGED.to_vec()));
        assert_eq!(
            answer(&mut gate, &["rooms"]).expect("rooms"),
            json!([{"room": "!r", "name": "Friend"}, {"room": "!n", "name": "!n"}])
        );
    }

    #[test]
    fn commands_outside_the_list_are_refused() {
        let mut room = room();
        room.rules.commands = Some(vec!["rooms".into(), "read".into(), "search".into()]);
        let err = answer(&mut room, &["send", "!r", "hi"]).expect_err("refused");
        assert_eq!(err.to_string(), "not allowed for this agent");
        assert!(room.matrix.sent.is_empty());
        answer(&mut room, &["read", "!r"]).expect("read");
        answer(&mut room, &["search", "!r", "m1"]).expect("search");
        room.rules.commands = Some(Vec::new());
        let err = answer(&mut room, &["rooms"]).expect_err("refused");
        assert_eq!(err.to_string(), "not allowed for this agent");
    }

    #[test]
    fn rooms_outside_the_list_do_not_exist() {
        let mut gate = fake(Vec::new());
        gate.matrix.rooms.push(("!n", BRIDGED.to_vec()));
        gate.rules.rooms = Some(vec!["!n".into(), "!h".into()]);
        assert_eq!(
            answer(&mut gate, &["rooms"]).expect("rooms"),
            json!([{"room": "!n", "name": "!n"}])
        );
        for argv in [
            &["read", "!r"][..],
            &["search", "!r", "x"],
            &["send", "!r", "x"],
            &["send", "!h", "x"],
        ] {
            let err = answer(&mut gate, argv).expect_err("refused");
            assert_eq!(err.to_string(), "not a bridged chat", "{argv:?}");
        }
        assert!(gate.matrix.sent.is_empty());
        answer(&mut gate, &["send", "!n", "x"]).expect("sent");
    }

    #[test]
    fn rooms_outside_the_list_ask_the_homeserver_nothing() {
        let mut gate = fake(Vec::new());
        gate.rules.rooms = Some(vec!["!r".into()]);
        for room in ["!h", "!gone"] {
            let err = answer(&mut gate, &["read", room]).expect_err("refused");
            assert_eq!(err.to_string(), "not a bridged chat", "{room}");
        }
        assert!(gate.matrix.paths.is_empty(), "{:?}", gate.matrix.paths);
    }

    #[test]
    fn bad_arguments_name_the_usage() {
        let mut room = room();
        for argv in [
            &["delete", "!r"][..],
            &[],
            &["rooms", "x"],
            &["read"],
            &["read", "!r", "0"],
            &["read", "!r", "5", "6"],
            &["read", "!r", "--since"],
            &["read", "!r", "--since", "yesterday"],
            &["read", "!r", "--context", "1"],
            &["search", "!r"],
            &["search", "!r", "x", "--from", "4"],
            &["search", "!r", "x", "--context", "-1"],
            &["send", "!r"],
            &["send", "!r", ""],
        ] {
            let err = answer(&mut room, argv).expect_err("refused").to_string();
            assert!(err.contains(USAGE), "{argv:?}: {err}");
            assert_eq!(err.lines().count(), 1, "{argv:?}");
        }
        assert!(room.matrix.sent.is_empty());
    }

    #[test]
    fn request_is_one_json_line() {
        let env = |_: &str| None;
        let ask = |input: &str| {
            let mut out = Vec::new();
            let lines = std::cell::RefCell::new(Vec::new());
            serve(&env, &mut input.as_bytes(), &mut out, &|l| {
                lines.borrow_mut().push(l.to_owned());
            })
            .expect("served");
            let out = String::from_utf8(out).expect("UTF-8");
            assert_eq!(out.lines().count(), 1);
            (
                serde_json::from_str::<Value>(&out).expect("JSON"),
                lines.into_inner(),
            )
        };
        let (reply, log) = ask("{\"argv\": [\"send\", \"!r\", \"secret words\"]}\n");
        assert_eq!(
            reply,
            json!({"error": "HERDR_MATRIX_GATE_HOMESERVER is not set"})
        );
        assert_eq!(log, ["send: HERDR_MATRIX_GATE_HOMESERVER is not set"]);
        let (reply, log) = ask("{\"argv\": [\"sned\", \"!r\", \"secret\"]}");
        assert!(reply["error"].as_str().is_some_and(|e| e.contains(USAGE)));
        assert!(
            !log[0].contains("secret") && !log[0].contains("sned"),
            "{log:?}"
        );
        let (reply, _) = ask("argv");
        assert_eq!(
            reply,
            json!({"error": "the request is not {\"argv\": [...]}"})
        );
    }

    #[test]
    fn settings_come_from_the_unit() {
        let env = |name: &str| {
            match name {
                "HERDR_MATRIX_GATE_HOMESERVER" => Some("http://m.example.org"),
                "HERDR_MATRIX_GATE_BRIDGES" => Some("signal whatsapp"),
                "HERDR_MATRIX_GATE_BOT" => Some("herdr"),
                _ => None,
            }
            .map(str::to_owned)
        };
        let err = settings(&env).expect_err("plain http");
        assert!(err.to_string().contains("not https://"), "{err}");
        let local = |name: &str| match name {
            "HERDR_MATRIX_GATE_HOMESERVER" => Some("http://127.0.0.1:8008".to_owned()),
            other => env(other),
        };
        assert_eq!(
            settings(&local).expect("settings"),
            (
                "http://127.0.0.1:8008".to_owned(),
                Rules {
                    bridges: vec!["signal".into(), "whatsapp".into()],
                    bot: "herdr".into(),
                    commands: None,
                    rooms: None,
                }
            )
        );
        let agent = |name: &str| match name {
            "HERDR_MATRIX_GATE_COMMANDS" => Some("rooms read  search".to_owned()),
            "HERDR_MATRIX_GATE_ROOMS" => Some("!a:x !b:x".to_owned()),
            other => local(other),
        };
        let (_, rules) = settings(&agent).expect("settings");
        assert_eq!(
            (rules.commands, rules.rooms),
            (
                Some(vec!["rooms".into(), "read".into(), "search".into()]),
                Some(vec!["!a:x".into(), "!b:x".into()])
            )
        );
        let dir = crate::testutil::TempDir::new();
        std::fs::write(dir.path().join("token"), "tok\n").expect("write");
        let creds = dir.path().display().to_string();
        assert_eq!(
            credential(&|_| Some(creds.clone()), "token").expect("token"),
            "tok"
        );
        assert!(credential(&|_| None, "token").is_err());
    }
}
