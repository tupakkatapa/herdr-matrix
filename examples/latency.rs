//! Bot-side latency of a turn's end: from a pi agent going idle with its
//! answer written to the `PUT /send` of that answer. herdr and the
//! homeserver are fakes; the homeserver holds `/sync` for its timeout and
//! answers early after any `PUT`, as Synapse does for the bot's own typing.
//!
//! `cargo run --example latency -- [turns]`

use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use herdr_matrix::Result;
use herdr_matrix::bot::{Bot, Deps};
use herdr_matrix::herdr::Herdr;
use herdr_matrix::kind;
use herdr_matrix::matrix::{Matrix, Source};
use herdr_matrix::state::{State, Store};
use serde_json::{Value, json};

const OWNER: &str = "@o:e.org";

#[derive(Default)]
struct Shared {
    status: String,
    prompted: usize,
    inbox: Vec<Value>,
    put_since_sync: bool,
    sends: Vec<(Instant, String)>,
    herdr_calls: usize,
    syncs: usize,
    typing: usize,
}

#[derive(Clone, Default)]
struct S(Arc<Mutex<Shared>>);

impl S {
    fn get(&self) -> MutexGuard<'_, Shared> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

struct FakeHerdr(S);

impl Herdr for FakeHerdr {
    fn run(&mut self, args: &[&str]) -> Result<(i32, String)> {
        let mut s = self.0.get();
        s.herdr_calls += 1;
        Ok(match (args[0], args.get(1).copied().unwrap_or_default()) {
            ("agent", "list") => (
                0,
                json!({"result": {"agents": [{"pane_id": "w:p", "workspace_id": "w",
                    "agent_status": s.status, "agent": "pi", "cwd": "/w/a"}]}})
                .to_string(),
            ),
            ("workspace", "list") => (
                0,
                json!({"result": {"workspaces": [{"workspace_id": "w", "label": "w"}]}})
                    .to_string(),
            ),
            ("agent", "prompt") => {
                s.status = "working".into();
                s.prompted += 1;
                (0, "{}".into())
            }
            _ => (0, "{}".into()),
        })
    }
}

struct FakeMatrix(S);

impl FakeMatrix {
    fn sync(&self, wait: Duration) -> Value {
        let start = Instant::now();
        loop {
            {
                let mut s = self.0.get();
                if !s.inbox.is_empty() || s.put_since_sync || start.elapsed() >= wait {
                    s.put_since_sync = false;
                    s.syncs += 1;
                    let events: Vec<Value> = s.inbox.drain(..).collect();
                    return json!({"next_batch": "n",
                        "rooms": {"join": {"!r:e.org": {"timeline": {"events": events}}}}});
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
    }
}

impl Matrix for FakeMatrix {
    fn call(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        query: &[(&str, &str)],
        _: Duration,
    ) -> Result<Value> {
        if path == "/sync" {
            let wait = query
                .iter()
                .find(|(k, _)| *k == "timeout")
                .and_then(|(_, v)| v.parse().ok())
                .unwrap_or_default();
            return Ok(self.sync(Duration::from_millis(wait)));
        }
        let mut s = self.0.get();
        if method == "PUT" {
            s.put_since_sync = true;
        }
        if path.contains("/typing/") {
            s.typing += 1;
        }
        if path.contains("/send/") {
            let text = body.and_then(|b| b["body"].as_str()).unwrap_or_default();
            s.sends.push((Instant::now(), text.to_owned()));
            return Ok(json!({"event_id": format!("$e{}", s.sends.len())}));
        }
        Ok(match path {
            "/createRoom" => json!({"room_id": "!r:e.org"}),
            "/account/whoami" => json!({"user_id": "@b:e.org"}),
            _ => json!({}),
        })
    }

    fn upload(&mut self, _: &mut dyn Source, _: u64, _: &str) -> Result<String> {
        Ok("mxc://x/y".into())
    }

    fn download(&mut self, _: &str, _: &mut dyn std::io::Write) -> Result<()> {
        Ok(())
    }

    fn upload_limit(&mut self) -> Result<Option<u64>> {
        Ok(None)
    }

    fn next_txn(&mut self) -> String {
        format!("{:?}", Instant::now())
    }
}

/// A line of a pi session file.
fn line(role: &str, id: &str, text: &str) -> String {
    json!({"type": "message", "id": id, "parentId": null, "timestamp": "2026-11-18T18:06:58.776Z",
           "message": {"role": role, "content": [{"type": "text", "text": text}]}})
    .to_string()
        + "\n"
}

fn append(path: &Path, text: &str) {
    fs::File::options()
        .append(true)
        .open(path)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .expect("append");
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

/// Runs `turns` turns of a pi agent, each after a random 0.8 to 4.3 s of
/// work, and prints the latency and the calls of each.
fn drive(s: &S, session: &Path, turns: usize) {
    thread::sleep(Duration::from_secs(4));
    let mut latencies = Vec::new();
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    for k in 1..=turns {
        s.get()
            .inbox
            .push(json!({"type": "m.room.message", "sender": OWNER,
            "event_id": format!("$in{k}"), "origin_server_ts": now_ms(),
            "content": {"msgtype": "m.text", "body": format!("go {k}")}}));
        while s.get().prompted < k {
            thread::sleep(Duration::from_millis(2));
        }
        append(session, &line("user", &format!("u{k}"), "go"));
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        thread::sleep(Duration::from_millis(800 + seed % 3500));
        let want = format!("answer {k}");
        let (idle, calls, syncs, typing) = {
            let mut s = s.get();
            append(session, &line("assistant", &format!("a{k}"), &want));
            s.status = "idle".into();
            (Instant::now(), s.herdr_calls, s.syncs, s.typing)
        };
        let posted = loop {
            if let Some((at, _)) = s.get().sends.iter().find(|(_, b)| *b == want) {
                break *at;
            }
            thread::sleep(Duration::from_millis(1));
        };
        let ms = (posted - idle).as_millis();
        let now = s.get();
        println!(
            "turn {k}: idle -> PUT /send {ms:>5} ms (herdr calls {}, syncs {}, typing PUTs {})",
            now.herdr_calls - calls,
            now.syncs - syncs,
            now.typing - typing,
        );
        latencies.push(ms);
    }
    latencies.sort_unstable();
    let mean = latencies.iter().sum::<u128>() / latencies.len().max(1) as u128;
    println!(
        "latency over {turns} turns: min {} mean {mean} max {} ms",
        latencies.first().unwrap_or(&0),
        latencies.last().unwrap_or(&0),
    );
    let s = s.get();
    println!(
        "totals: herdr calls {}, syncs {}, typing PUTs {}",
        s.herdr_calls, s.syncs, s.typing
    );
}

fn main() {
    let turns: usize = std::env::args()
        .nth(1)
        .map_or(10, |n| n.parse().expect("a number of turns"));
    let dir = std::env::temp_dir().join(format!("herdr-matrix-latency-{}", std::process::id()));
    let home = dir.join("home");
    let session = home.join(".pi/agent/sessions/--w-a--/s.jsonl");
    fs::create_dir_all(session.parent().expect("a directory")).expect("mkdir");
    fs::write(
        &session,
        line("user", "u0", "hi") + &line("assistant", "a0", "old"),
    )
    .expect("write");
    let s = S::default();
    s.get().status = "idle".into();
    let deps = Deps {
        owner: OWNER.into(),
        herdr: Box::new(FakeHerdr(s.clone())),
        answer: Box::new(move |a| kind::turn(a, Some(&home))),
        log: Box::new(|_| {}),
        files: dir.join("files"),
        ..Deps::default()
    };
    let mut bot = Bot::new(
        Box::new(FakeMatrix(s.clone())),
        Store::new(dir.join("state.json")),
        State::default(),
        deps,
    );
    let driver = {
        let s = s.clone();
        thread::spawn(move || drive(&s, &session, turns))
    };
    herdr_matrix::app::serve(
        &mut || bot.tick(),
        &|line| eprintln!("{line}"),
        &mut |wait| {
            if driver.is_finished() {
                return false;
            }
            thread::sleep(wait);
            true
        },
    );
    driver.join().expect("the driver");
    let _ = fs::remove_dir_all(&dir);
}
