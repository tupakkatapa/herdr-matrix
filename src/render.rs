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

static HEADING: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^#{1,6} +(.+)$"));
static LINK: LazyLock<Regex> = LazyLock::new(|| re(r"\[([^\]\n]+)\]\((https?://[^)\s<]+)\)"));
static CODE: LazyLock<Regex> = LazyLock::new(|| re(r"`([^`\n]+)`"));
static BOLD: LazyLock<Regex> = LazyLock::new(|| re(r"\*\*(.+?)\*\*"));
/// A code span in prose while the rest is changed, by its number
static SPAN: LazyLock<Regex> = LazyLock::new(|| re(r"\x00(\d+)\x00"));

/// The host of a URL, its port too, without who logs in.
fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    rest[..end].rsplit('@').next().unwrap_or_default()
}

/// Prose with its code spans set aside, `change` made, and the spans put
/// back as `wrap` gives them.
fn around_code(
    prose: &str,
    change: impl FnOnce(&str) -> String,
    wrap: impl Fn(&str) -> String,
) -> String {
    let mut spans = Vec::new();
    let marked = CODE.replace_all(prose, |c: &regex::Captures| {
        spans.push(wrap(&c[1]));
        format!("\x00{}\x00", spans.len() - 1)
    });
    let changed = change(&marked);
    SPAN.replace_all(&changed, |c: &regex::Captures| {
        c[1].parse::<usize>()
            .ok()
            .and_then(|i| spans.get(i).cloned())
            .unwrap_or_default()
    })
    .into_owned()
}

/// A message as plain text, for clients that show no HTML: no bold,
/// heading or link marks; code is left as written.
#[must_use]
pub fn plain(text: &str) -> String {
    let mut out = String::new();
    for (i, part) in text.split("```").enumerate() {
        if i > 0 {
            out.push_str("```");
        }
        if i % 2 == 1 {
            out.push_str(part);
            continue;
        }
        let prose = around_code(
            part,
            |prose| {
                let prose = HEADING.replace_all(prose, "${1}");
                let prose = LINK.replace_all(&prose, |c: &regex::Captures| {
                    if c[1] == c[2] {
                        c[2].to_owned()
                    } else {
                        format!("{} ({})", &c[1], &c[2])
                    }
                });
                BOLD.replace_all(&prose, "${1}").into_owned()
            },
            |code| format!("`{code}`"),
        );
        out.push_str(&prose);
    }
    out
}

/// The little Markdown an agent's answer needs, as Matrix HTML. A link
/// whose text is not its address is followed by its host.
#[must_use]
pub fn md(text: &str) -> String {
    static BULLET: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^( *)[-*] "));
    let mut out = String::new();
    for (i, part) in text.split("```").enumerate() {
        if i % 2 == 1 {
            let code = part.split_once('\n').map_or(part, |(_, body)| body);
            let _ = write!(out, "<pre><code>{}</code></pre>", escape(code));
        } else {
            let prose = escape(part.trim_matches('\n'));
            let prose = HEADING.replace_all(&prose, "<b>${1}</b>");
            let prose = BULLET.replace_all(&prose, |c: &regex::Captures| {
                format!("{}\u{2022} ", "&nbsp;".repeat(c[1].len()))
            });
            let prose = around_code(
                &prose,
                |prose| {
                    let prose = LINK.replace_all(prose, |c: &regex::Captures| {
                        let (label, url) = (&c[1], &c[2]);
                        if label == url {
                            format!("<a href=\"{url}\">{label}</a>")
                        } else {
                            format!("<a href=\"{url}\">{label}</a> ({})", host(url))
                        }
                    });
                    BOLD.replace_all(&prose, "<b>${1}</b>").into_owned()
                },
                |code| format!("<code>{code}</code>"),
            );
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
        out.push(
            "Several answers are needed. Send one key at a time: up or down to move, \
             space to tick, enter to confirm. !screen shows where you are."
                .into(),
        );
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
             <a href=\"https://e.org/?a=1&amp;b=&quot;2&quot;\">docs</a> (e.org)<br>\
             [x](javascript:alert(1))"
        );
    }

    #[test]
    fn link_text_is_followed_by_its_host() {
        for shown in [
            "https://good.example",
            "www.good.example",
            "good.example/a",
            "HTTPS://good.example",
            "github.com",
            " good.example/a",
        ] {
            assert_eq!(
                md(&format!("[{shown}](https://evil.example)")),
                format!("<a href=\"https://evil.example\">{shown}</a> (evil.example)")
            );
        }
        assert_eq!(
            md("[`good.example/a`](https://evil.example)"),
            "<a href=\"https://evil.example\"><code>good.example/a</code></a> (evil.example)"
        );
        assert_eq!(
            md("[**good.example/a**](https://evil.example)"),
            "<a href=\"https://evil.example\"><b>good.example/a</b></a> (evil.example)"
        );
        assert_eq!(
            md("[https://x.example](https://x.example) [the docs](https://x.example)"),
            "<a href=\"https://x.example\">https://x.example</a> \
             <a href=\"https://x.example\">the docs</a> (x.example)"
        );
        assert_eq!(
            md("[a](https://me@evil.example:8443/x) [b](http://evil.example?q#h)"),
            "<a href=\"https://me@evil.example:8443/x\">a</a> (evil.example:8443) \
             <a href=\"http://evil.example?q#h\">b</a> (evil.example)"
        );
    }

    #[test]
    fn plain_text_has_no_marks() {
        assert_eq!(
            plain(
                "## Plan\n**repo** is done: [docs](https://e.org/a) \
                 [https://e.org](https://e.org) [good.example/a](https://evil.example)\n\
                 `a**b**c` and **x**\n```\n**kept**\n```\n- one"
            ),
            "Plan\nrepo is done: docs (https://e.org/a) https://e.org good.example/a (https://evil.example)\n\
             `a**b**c` and x\n```\n**kept**\n```\n- one"
        );
    }

    #[test]
    fn plain_bold_spans_a_code_span() {
        assert_eq!(plain("**a `b` c** `**d**`"), "a `b` c `**d**`");
    }

    #[test]
    fn code_span_keeps_its_markup() {
        assert_eq!(md("`a**b**c`"), "<code>a**b**c</code>");
        assert_eq!(
            md("`[x](https://e.org)` **a `b` c**"),
            "<code>[x](https://e.org)</code> <b>a <code>b</code> c</b>"
        );
    }

    #[test]
    fn empty_fence_shows_no_language() {
        assert_eq!(md("```rust\n```"), "<pre><code></code></pre>");
        assert_eq!(md("```x```"), "<pre><code>x</code></pre>");
    }

    #[test]
    fn nested_list_keeps_its_depth() {
        assert_eq!(md("- a\n  - b"), "• a<br>&nbsp;&nbsp;• b");
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
