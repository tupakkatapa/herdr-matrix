//! Voice messages: whisper.cpp hears them.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::process::{Ran, run_timed};
use crate::{Error, Result};

pub const TOOL_TIMEOUT: Duration = Duration::from_mins(2);

/// The programs that hear, by name on `PATH` unless set.
pub struct Bins {
    pub ffmpeg: PathBuf,
    pub whisper: PathBuf,
    /// How long each may take
    pub timeout: Duration,
}

impl Default for Bins {
    fn default() -> Self {
        Self {
            ffmpeg: "ffmpeg".into(),
            whisper: "whisper-cli".into(),
            timeout: TOOL_TIMEOUT,
        }
    }
}

/// Runs a tool to its end; its failure names the tool and its last
/// complaint.
///
/// # Errors
/// When the tool cannot start, fails or takes longer than `timeout`.
pub fn run_tool(program: &Path, args: &[&str], timeout: Duration) -> Result<Ran> {
    let name = program
        .file_name()
        .map_or_else(|| program.to_string_lossy(), |n| n.to_string_lossy())
        .into_owned();
    let ran = run_timed(program, args, timeout)?
        .ok_or_else(|| Error::Tool(format!("{name} timed out")))?;
    if ran.code != 0 {
        let why = ran
            .stderr
            .trim()
            .lines()
            .last()
            .unwrap_or("failed")
            .to_owned();
        return Err(Error::Tool(format!("{name}: {why}")));
    }
    Ok(ran)
}

/// What is said in a recording, by whisper.cpp in `language`.
///
/// # Errors
/// When ffmpeg or whisper-cli fails.
pub fn whisper(bins: &Bins, audio: &Path, model: &str, language: &str) -> Result<String> {
    let mut wav = audio.as_os_str().to_owned();
    wav.push(".wav");
    let wav = PathBuf::from(wav);
    let audio = audio.to_string_lossy();
    let wav_arg = wav.to_string_lossy().into_owned();
    let threads = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .to_string();
    let ran = run_tool(
        &bins.ffmpeg,
        &[
            "-nostdin",
            "-loglevel",
            "error",
            "-i",
            &audio,
            "-ar",
            "16000",
            "-ac",
            "1",
            "-c:a",
            "pcm_s16le",
            "-y",
            &wav_arg,
        ],
        bins.timeout,
    )
    .and_then(|_| {
        run_tool(
            &bins.whisper,
            &[
                "-m", model, "-f", &wav_arg, "-l", language, "-nt", "-t", &threads,
            ],
            bins.timeout,
        )
    });
    if wav.exists() {
        fs::remove_file(&wav)?;
    }
    let heard = ran?.stdout.replace("[BLANK_AUDIO]", "");
    Ok(heard.split_whitespace().collect::<Vec<_>>().join(" "))
}
