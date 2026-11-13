//! Files from the chat in a private directory, and the folders `!new`
//! starts agents in.

use std::fs;
use std::hash::{BuildHasher as _, Hasher as _, RandomState};
use std::io::Write as _;
use std::os::unix::fs::{
    DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _,
};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, UNIX_EPOCH};

use regex::Regex;

use crate::{Error, Result};

/// How long a file from the chat is kept
pub const FILE_TTL: Duration = Duration::from_hours(24);

/// Eight random characters for a file name.
fn random_name() -> String {
    let mut n = RandomState::new().build_hasher().finish();
    let alphabet = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    (0..8)
        .map(|_| {
            let c = alphabet[usize::try_from(n % 37).unwrap_or_default()];
            n /= 37;
            char::from(c)
        })
        .collect()
}

/// Saves a file from the chat in `dir`, a private directory, after files
/// older than `FILE_TTL`; its path. `now` is in seconds since the epoch.
///
/// # Errors
/// When `dir` is not private or the file cannot be written.
pub fn store(dir: &Path, now: f64, name: &str, data: &[u8]) -> Result<PathBuf> {
    static UNSAFE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[^A-Za-z0-9._-]").expect("a valid pattern"));
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    // The directory may sit in a shared /tmp
    let st = fs::symlink_metadata(dir)?;
    if !st.is_dir() || st.uid() != own_uid()? || st.permissions().mode() & 0o7777 != 0o700 {
        return Err(Error::Refused(format!(
            "{} is not a private directory",
            dir.display()
        )));
    }
    purge(dir, now - FILE_TTL.as_secs_f64())?;
    let base = name.rsplit('/').next().unwrap_or_default();
    let suffix = UNSAFE.replace_all(base, "_");
    let stamp = now.floor();
    loop {
        let path = dir.join(format!("{stamp:.0}-{}-{suffix}", random_name()));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(data)?;
                return Ok(path);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err.into()),
        }
    }
}

/// Removes the files in `dir` older than `old`, seconds since the epoch.
/// One that vanishes or cannot be read meanwhile stays out of it.
fn purge(dir: &Path, old: f64) -> Result<()> {
    for entry in fs::read_dir(dir)?.filter_map(std::result::Result::ok) {
        let mtime = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
            .map_or(f64::INFINITY, |d| d.as_secs_f64());
        if entry.file_type().is_ok_and(|t| t.is_file()) && mtime < old {
            match fs::remove_file(entry.path()) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err.into()),
                _ => {}
            }
        }
    }
    Ok(())
}

/// The user this process runs as.
fn own_uid() -> Result<u32> {
    Ok(fs::metadata("/proc/self")?.uid())
}

/// Folders one or two levels under the workspace that match a query, or
/// the top ones without a query.
#[must_use]
pub fn folders(workspace: &Path, query: Option<&str>) -> Vec<String> {
    let list = |dir: &Path| -> Vec<PathBuf> {
        fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(std::result::Result::ok)
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .map(|e| e.path())
            .filter(|d| d.is_dir())
            .collect()
    };
    let top = list(workspace);
    let Some(query) = query else {
        let mut dirs: Vec<String> = top
            .iter()
            .map(|d| d.to_string_lossy().into_owned())
            .collect();
        dirs.sort();
        return dirs;
    };
    let found: Vec<String> = top
        .iter()
        .chain(top.iter().flat_map(|d| list(d)).collect::<Vec<_>>().iter())
        .map(|d| d.to_string_lossy().into_owned())
        .collect();
    let query = query.to_lowercase();
    let query = query.trim_matches('/');
    let exact: Vec<String> = found
        .iter()
        .filter(|d| d.to_lowercase().ends_with(&format!("/{query}")))
        .cloned()
        .collect();
    let mut dirs = if exact.is_empty() {
        found
            .into_iter()
            .filter(|d| {
                let lower = d.to_lowercase();
                let parts: Vec<&str> = lower.split('/').collect();
                parts[parts.len().saturating_sub(2)..]
                    .join("/")
                    .contains(query)
            })
            .collect()
    } else {
        exact
    };
    dirs.sort();
    dirs
}
