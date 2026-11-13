//! Text as the chat shows it.

use std::fmt::Write as _;
use std::sync::LazyLock;

use regex::Regex;

use crate::herdr::Question;

/// Bytes. A long text goes out in parts of about `CHUNK` bytes of body and
/// HTML together, each at most `EVENT_MAX`; an event may hold 65536 in all
pub const CHUNK: usize = 16000;
pub const EVENT_MAX: usize = 30000;
/// Room a cut leaves for the fence it closes
const FENCE_ROOM: usize = 16;

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid pattern")
}

/// Text escaped for HTML, quotes too.
#[must_use]
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
    out
}

/// The little Markdown an agent's answer needs, as Matrix HTML.
#[must_use]
pub fn md(text: &str) -> String {
    static HEADING: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^#{1,6} +(.+)$"));
    static BULLET: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^( *)[-*] "));
    static LINK: LazyLock<Regex> = LazyLock::new(|| re(r"\[([^\]\n]+)\]\((https?://[^)\s]+)\)"));
    static CODE: LazyLock<Regex> = LazyLock::new(|| re(r"`([^`\n]+)`"));
    static BOLD: LazyLock<Regex> = LazyLock::new(|| re(r"\*\*(.+?)\*\*"));
    let mut out = String::new();
    for (i, part) in text.split("```").enumerate() {
        if i % 2 == 1 {
            let (lang, body) = part.split_once('\n').unwrap_or((part, ""));
            let code = if body.is_empty() { lang } else { body };
            let _ = write!(out, "<pre><code>{}</code></pre>", escape(code));
        } else {
            let prose = escape(part.trim_matches('\n'));
            let prose = HEADING.replace_all(&prose, "<b>${1}</b>");
            let prose = BULLET.replace_all(&prose, "${1}\u{2022} ");
            let prose = LINK.replace_all(&prose, "<a href=\"${2}\">${1}</a>");
            let prose = CODE.replace_all(&prose, "<code>${1}</code>");
            let prose = BOLD.replace_all(&prose, "<b>${1}</b>");
            out.push_str(&prose.replace('\n', "<br>"));
        }
    }
    out
}

/// Bytes of a message as body and HTML.
#[must_use]
pub fn size(text: &str) -> usize {
    text.len() + md(text).len()
}

/// Bytes of a screen posted as a `<pre>` block, body and HTML.
#[must_use]
pub fn screen_size(text: &str) -> usize {
    text.len() + escape(text).len() + "\n\n<pre></pre>".len()
}

/// A text in pieces of at most `EVENT_MAX` bytes under `cost`, cut at a
/// line end when one is near.
#[must_use]
pub fn cut(text: &str, cost: fn(&str) -> usize) -> Vec<String> {
    pieces(text, cost, false)
}

/// `cut`; with `fence`, a cut inside a code fence closes and reopens it.
fn pieces(text: &str, cost: fn(&str) -> usize, fence: bool) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut text = text.to_owned();
    while cost(&text) > EVENT_MAX {
        let chars: Vec<char> = text.chars().collect();
        let mut n = chars.len();
        while cost(&chars[..n].iter().collect::<String>()) > EVENT_MAX - FENCE_ROOM {
            n /= 2;
        }
        if let Some(line) = chars[..n].iter().rposition(|&c| c == '\n')
            && line > n / 2
        {
            n = line + 1;
        }
        let mut head: String = chars[..n].iter().collect();
        let mut rest: String = chars[n..].iter().collect();
        if fence && !head.matches("```").count().is_multiple_of(2) {
            head.push_str("\n```");
            rest = format!("```\n{rest}");
        }
        pieces.push(head);
        text = rest;
    }
    pieces.push(text);
    pieces
}

/// A long text in pieces of at most `EVENT_MAX` bytes, split at paragraph
/// breaks, inside a code fence only when it does not fit.
#[must_use]
pub fn chunks(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    for para in text.split("\n\n") {
        let longer = if current.is_empty() {
            para.to_owned()
        } else {
            format!("{current}\n\n{para}")
        };
        if size(&longer) > CHUNK
            && !current.is_empty()
            && current.matches("```").count().is_multiple_of(2)
        {
            parts.push(std::mem::replace(&mut current, para.to_owned()));
        } else {
            current = longer;
        }
    }
    parts.push(current);
    parts
        .iter()
        .flat_map(|part| pieces(part, size, true))
        .collect()
}

/// Questions of the agent with their options numbered.
#[must_use]
pub fn render(questions: &[Question]) -> String {
    let mut out = Vec::new();
    for q in questions {
        let mut lines = vec![format!("**{}**", q.question)];
        for (i, option) in q.options.iter().enumerate() {
            match option.description.as_deref().filter(|d| !d.is_empty()) {
                Some(about) => lines.push(format!("{}. {} - {about}", i + 1, option.label)),
                None => lines.push(format!("{}. {}", i + 1, option.label)),
            }
        }
        out.push(lines.join("\n"));
    }
    if questions.len() > 1 || questions.first().is_some_and(|q| q.multi_select) {
        out.push("Several answers are needed: use !screen and keys.".into());
    } else {
        out.push("Answer with a number, or write your own answer.".into());
    }
    out.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_answer_is_split() {
        let text = ["a", "b", "c"].map(|p| p.repeat(5000)).join("\n\n");
        assert_eq!(
            chunks(&text),
            ["a", "b", "c"].map(|p| p.repeat(5000)).to_vec()
        );
        let fenced = format!("```\n{}\n\n{}\n```", "x".repeat(5000), "y".repeat(5000));
        assert_eq!(chunks(&fenced), vec![fenced.clone()]);
        assert_eq!(chunks(""), vec![String::new()]);
    }

    #[test]
    fn markdown() {
        assert_eq!(
            md("**a** `b<c`\nd\n```sh\nx > y\n```\ne"),
            "<b>a</b> <code>b&lt;c</code><br>d<pre><code>x &gt; y\n</code></pre>e"
        );
    }

    #[test]
    fn markdown_headings_bullets_and_links() {
        assert_eq!(
            md("## Plan\n- one\n* two\n**bold**\n\
                [docs](https://e.org/?a=1&b=\"2\")\n\
                [x](javascript:alert(1))"),
            "<b>Plan</b><br>• one<br>• two<br><b>bold</b><br>\
             <a href=\"https://e.org/?a=1&amp;b=&quot;2&quot;\">docs</a><br>\
             [x](javascript:alert(1))"
        );
    }

    #[test]
    fn long_code_block_is_closed_and_reopened() {
        let code = format!("```sh\n{}```", "echo hello world\n".repeat(4000));
        let parts = chunks(&code);
        assert!(parts.len() > 1);
        for part in &parts {
            assert_eq!(part.matches("```").count() % 2, 0);
            assert!(size(part) <= EVENT_MAX);
        }
        assert_eq!(parts.concat().matches("echo hello world").count(), 4000);
    }

    #[test]
    fn random_texts_split_into_events_that_fit() {
        let atoms = [
            "word ",
            "ä",
            &"€".repeat(30),
            "<",
            "&",
            "\"",
            "\n",
            "\n\n",
            "```",
            "```sh\n",
            "`x`",
            "**b**",
            "- item\n",
            &"x".repeat(400),
        ];
        let mut seed: u64 = 7;
        let mut next = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            usize::try_from(seed % n as u64).expect("below n")
        };
        let bare = |s: &str| {
            s.chars()
                .filter(|c| !"`\n ".contains(*c))
                .collect::<String>()
        };
        for _ in 0..40 {
            let len = 1 + next(899);
            let text: String = (0..len).map(|_| atoms[next(atoms.len())]).collect();
            let pieces = chunks(&text);
            for piece in &pieces {
                assert!(size(piece) <= EVENT_MAX);
                if text.matches("```").count().is_multiple_of(2) {
                    assert!(piece.matches("```").count().is_multiple_of(2));
                }
            }
            assert_eq!(bare(&pieces.concat()), bare(&text));
        }
    }
}
