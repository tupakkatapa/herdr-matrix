//! What the owner may write: commands, keys and reactions, typed or spoken.

use std::sync::LazyLock;

use regex::Regex;

pub const KEYS: [&str; 19] = [
    "1", "2", "3", "4", "5", "6", "7", "8", "9", "y", "n", "esc", "enter", "tab", "space", "up",
    "down", "left", "right",
];

macro_rules! key_hint {
    () => {
        "1-9, y, n, enter, esc, tab, space, up, down, left or right"
    };
}

pub const KEY_HINT: &str = key_hint!();

/// What a reaction is: a thumbs-up picks the first option.
pub const THUMBS_UP: &str = "\u{1F44D}";

pub const HELP: &str = concat!(
    "This chat is attached to one agent at a time: what you write goes to it, \
     and what it says comes back as it works. A picture or file you send is \
     handed to it too. A voice message counts as written: the bot shows what \
     it heard, and the key or command it became.\n\n\
     When the agent asks a question, answer with the number of an option or \
     write your own answer. When it waits on another choice, answer with one \
     key: ",
    key_hint!(),
    ". A \u{1F44D} on the message that shows the choice picks the first option.\n\n\
     After !away, every agent that stops or asks posts here under its \
     name, until !here. Reply to such a message to answer that agent without \
     switching. A command acts on the agent of the message you reply to, \
     else on the attached one."
);

/// The help of an agent's own chat, after its name.
#[must_use]
pub fn contact(name: &str) -> String {
    format!(
        "This chat is {name}'s own: what you write here goes to it alone, and \
         what it says comes back here as it works. Pictures, files, voice \
         messages and answers work as in the main chat. !leave closes this \
         chat; the agent keeps running."
    )
}

/// What a command does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    Agents,
    Attach,
    New,
    Dm,
    Leave,
    Last,
    Screen,
    Stop,
    Exit,
    Away,
    Here,
    Help,
}

impl Cmd {
    /// True when it acts on the agent of the chat.
    #[must_use]
    pub fn acts(self) -> bool {
        matches!(self, Self::Last | Self::Screen | Self::Stop | Self::Exit)
    }
}

/// The chat a command is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Main,
    /// An agent's own chat
    Own,
}

/// A command: what may follow it when spoken, None when it cannot be
/// spoken, and its help in the main chat and in an agent's own chat; None
/// where it does not work.
pub struct Command {
    pub cmd: Cmd,
    pub name: &'static str,
    pub spoken: Option<&'static str>,
    pub main: Option<&'static str>,
    pub contact: Option<&'static str>,
}

impl Command {
    /// Its help where it is written; None where it does not work.
    #[must_use]
    pub fn help(&self, place: Place) -> Option<&'static str> {
        match place {
            Place::Main => self.main,
            Place::Own => self.contact,
        }
    }
}

const fn both(cmd: Cmd, name: &'static str, spoken: &'static str, help: &'static str) -> Command {
    Command {
        cmd,
        name,
        spoken: Some(spoken),
        main: Some(help),
        contact: Some(help),
    }
}

pub const COMMANDS: [Command; 12] = [
    Command {
        cmd: Cmd::Agents,
        name: "agents",
        spoken: Some(""),
        main: Some("!agents - list the agents"),
        contact: None,
    },
    Command {
        cmd: Cmd::Attach,
        name: "attach",
        spoken: Some(r"\S*"),
        main: Some("!attach <name> - talk to that agent"),
        contact: None,
    },
    Command {
        cmd: Cmd::New,
        name: "new",
        spoken: Some(r"\S*"),
        main: Some("!new [folder] - start an agent there, or list folders"),
        contact: None,
    },
    Command {
        cmd: Cmd::Dm,
        name: "dm",
        spoken: None,
        main: Some("!dm <name> - give it its own chat"),
        contact: None,
    },
    Command {
        cmd: Cmd::Leave,
        name: "leave",
        spoken: None,
        main: Some("!leave <name> - close its own chat; it keeps running"),
        contact: Some("!leave - close this chat; the agent keeps running"),
    },
    both(Cmd::Last, "last", "", "!last - what it said last"),
    both(
        Cmd::Screen,
        "screen",
        r"\d*",
        "!screen [n] - its screen, n <= 200 lines",
    ),
    both(Cmd::Stop, "stop", "", "!stop - interrupt what it is doing"),
    Command {
        cmd: Cmd::Exit,
        name: "exit",
        spoken: None,
        main: Some("!exit - quit the agent and close its pane (asks first)"),
        contact: Some("!exit - quit the agent and close its pane (asks first)"),
    },
    both(
        Cmd::Away,
        "away",
        "",
        "!away - every agent posts here when it stops or asks",
    ),
    both(
        Cmd::Here,
        "here",
        "",
        "!here - only answers to your messages come here",
    ),
    both(Cmd::Help, "help", "", "!help - this text"),
];

/// Other names of commands, and the command each stands for.
const ALIASES: [(&str, &str); 9] = [
    ("list", "agents"),
    ("spawn", "dm"),
    ("chat", "dm"),
    ("kill", "leave"),
    ("unchat", "leave"),
    ("close", "exit"),
    ("afk", "away"),
    ("back", "here"),
    ("auto", "here"),
];

/// A command by its name or alias.
#[must_use]
pub fn command(word: &str) -> Option<&'static Command> {
    let name = ALIASES
        .iter()
        .find(|(alias, _)| *alias == word)
        .map_or(word, |(_, name)| name);
    COMMANDS.iter().find(|c| c.name == name)
}

/// The command or alias that works in `place` and `word` is a slip of:
/// the nearest within two edits, and at most half its letters.
#[must_use]
pub fn suggest(word: &str, place: Place) -> Option<&'static str> {
    let most = (word.chars().count() / 2).min(2);
    COMMANDS
        .iter()
        .map(|c| c.name)
        .chain(ALIASES.iter().map(|(alias, _)| *alias))
        .filter(|name| command(name).is_some_and(|c| c.help(place).is_some()))
        .map(|name| (edits(word, name), name))
        .filter(|(n, _)| *n <= most)
        .min_by_key(|(n, _)| *n)
        .map(|(_, name)| name)
}

/// The Levenshtein distance between two words.
fn edits(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let next = (diagonal + usize::from(x != *y))
                .min(row[j] + 1)
                .min(row[j + 1] + 1);
            diagonal = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

/// The commands of a chat.
#[must_use]
pub fn helps(place: Place) -> String {
    COMMANDS
        .iter()
        .filter_map(|c| c.help(place))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The key a reaction stands for: a thumbs-up, the first option.
#[must_use]
pub fn reaction(key: &str) -> Option<&'static str> {
    (key.replace('\u{fe0f}', "") == THUMBS_UP).then_some("1")
}

fn spoken_key(word: &str) -> Option<&'static str> {
    Some(match word {
        "yes" | "yeah" => "y",
        "no" => "n",
        "one" => "1",
        "two" => "2",
        "three" => "3",
        "four" => "4",
        "five" => "5",
        "six" => "6",
        "seven" => "7",
        "eight" => "8",
        "nine" => "9",
        "enter" => "enter",
        "escape" | "cancel" => "esc",
        _ => return None,
    })
}

/// The key a typed answer to a choice on screen names: a key, or a word
/// for one, spaces and trailing punctuation aside.
#[must_use]
pub fn typed_key(text: &str) -> Option<&'static str> {
    let word = text
        .trim()
        .trim_end_matches(|c: char| c.is_ascii_punctuation())
        .trim_end()
        .to_lowercase();
    KEYS.iter()
        .find(|k| **k == word)
        .copied()
        .or_else(|| spoken_key(&word))
}

/// A transcript as the owner would have typed it: a short utterance that
/// names a key, or a command that can be spoken with what it takes, becomes
/// that key or command; anything else stays as heard. `auto` is a word of
/// speech, not the command.
#[must_use]
pub fn spoken(heard: &str) -> String {
    static NOISE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[^\w\s:-]").expect("a valid pattern"));
    let cleaned = NOISE.replace_all(heard, "").to_lowercase();
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    if let [word] = words[..] {
        if let Some(key) = spoken_key(word) {
            return key.into();
        }
        if KEYS.contains(&word) {
            return word.into();
        }
    }
    if let Some((first, rest)) = words.split_first()
        && *first != "auto"
        && let Some(entry) = command(first)
        && let Some(takes) = entry.spoken
        && words.len() <= 3
        && Regex::new(&format!("^(?:{takes})$")).is_ok_and(|re| re.is_match(&rest.join(" ")))
    {
        let mut out = vec![format!("!{first}")];
        out.extend(rest.iter().map(|w| (*w).to_owned()));
        return out.join(" ");
    }
    heard.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_hint_names_every_key() {
        for key in KEYS.iter().filter(|k| k.parse::<u8>().is_err()) {
            assert!(KEY_HINT.contains(key), "{key}");
        }
        assert!(KEY_HINT.starts_with("1-9"));
        assert!(HELP.contains(KEY_HINT));
    }

    #[test]
    fn spoken_new_and_attach_take_one_word() {
        assert_eq!(spoken("New tests, please."), "New tests, please.");
        assert_eq!(spoken("Attach the parser one."), "Attach the parser one.");
        assert_eq!(spoken("New beta."), "!new beta");
        assert_eq!(spoken("Attach repo."), "!attach repo");
        assert_eq!(spoken("New."), "!new");
        assert_eq!(spoken("Auto."), "Auto.");
        assert_eq!(spoken("Back."), "!back");
    }

    #[test]
    fn typed_answer_names_a_key() {
        for (typed, key) in [
            ("2.", Some("2")),
            ("y ", Some("y")),
            ("Yes!", Some("y")),
            (" No.", Some("n")),
            ("Esc", Some("esc")),
            ("two", Some("2")),
            ("yes please", None),
            ("...", None),
        ] {
            assert_eq!(typed_key(typed), key, "{typed}");
        }
    }

    #[test]
    fn slip_of_a_command_is_named() {
        assert_eq!(suggest("atach", Place::Main), Some("attach"));
        assert_eq!(suggest("stpo", Place::Main), Some("stop"));
        assert_eq!(suggest("kil", Place::Main), Some("kill"));
        assert_eq!(suggest("x", Place::Main), None);
        assert_eq!(suggest("banana", Place::Main), None);
        assert_eq!(suggest("atach", Place::Own), None);
        assert_eq!(edits("kitten", "sitting"), 3);
    }
}
