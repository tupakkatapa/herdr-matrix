//! Files from the chat in a private directory, and the folders `!new`
//! starts agents in.

use std::fs;
use std::hash::{BuildHasher as _, Hasher as _, RandomState};
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::{
    DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _,
};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, UNIX_EPOCH};

use regex::Regex;

use crate::{Error, Result};

/// How long a file from the chat is kept
pub const FILE_TTL: Duration = Duration::from_hours(30 * 24);

/// Eight random characters for a file name.
pub(crate) fn random_name() -> String {
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
/// older than `FILE_TTL`: `fill` writes it. Its path; nothing when `fill`
/// fails. `now` is in seconds since the epoch.
///
/// # Errors
/// When `dir` is not private, or the file cannot be made or filled.
pub fn store(
    dir: &Path,
    now: f64,
    name: &str,
    fill: &mut dyn FnMut(&mut fs::File) -> Result<()>,
) -> Result<PathBuf> {
    static UNSAFE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[^A-Za-z0-9._-]").expect("a valid pattern"));
    private_dir(dir)?;
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
            Ok(mut made) => {
                let filled = fill(&mut made);
                if filled.is_err() {
                    let _ = fs::remove_file(&path);
                }
                return filled.map(|()| path);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err.into()),
        }
    }
}

/// Makes `dir` when it is missing, and checks that it is private: a
/// directory of this user, mode `0700`, not a symlink.
///
/// # Errors
/// When `dir` cannot be made or is not private.
pub fn private_dir(dir: &Path) -> Result<()> {
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
    Ok(())
}

/// Removes the files in `dir` older than `old`, seconds since the epoch.
/// One that vanishes or cannot be read meanwhile stays out of it.
pub(crate) fn purge(dir: &Path, old: f64) -> Result<()> {
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

/// Opens `path`, a regular file under `root`, to read, when no part of the
/// path below `root` is a symlink: a confined agent can write there, and
/// a link may point anywhere this user can read.
///
/// # Errors
/// When the path is not a regular file under `root` by itself, or cannot
/// be opened.
pub fn open_inside(root: &Path, path: &Path) -> std::io::Result<fs::File> {
    let refused = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{}: not a regular file", path.display()),
        )
    };
    // Opening a FIFO or a device may wake who holds its other end
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(refused());
    }
    // A FIFO opens at once, and a symlink as the last part does not
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_CLOEXEC)
        .open(path)
        .map_err(|err| {
            if err.raw_os_error() == Some(libc::ELOOP) {
                refused()
            } else {
                err
            }
        })?;
    // Where the open file is, as the kernel resolved it
    let opened = fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
    let below = path.strip_prefix(root).map_err(|_| refused())?;
    if !file.metadata()?.is_file() || opened != fs::canonicalize(root)?.join(below) {
        return Err(refused());
    }
    Ok(file)
}

/// The user this process runs as.
pub(crate) fn own_uid() -> Result<u32> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use std::io::Write as _;

    fn aged(path: &Path, now: f64, age: Duration) {
        let at = UNIX_EPOCH + Duration::from_secs_f64(now) - age;
        fs::File::options()
            .write(true)
            .open(path)
            .expect("open")
            .set_modified(at)
            .expect("utime");
    }

    #[test]
    fn stored_file_is_private_and_old_ones_go() {
        let dir = TempDir::new();
        let files = dir.path().join("files");
        let now = 2_000_000_000.0;
        private_dir(&files).expect("private");
        for (name, age) in [
            ("old", FILE_TTL + Duration::from_secs(1)),
            ("young", FILE_TTL.saturating_sub(Duration::from_mins(1))),
        ] {
            fs::write(files.join(name), name).expect("write");
            aged(&files.join(name), now, age);
        }
        let path = store(&files, now, "a/b c.png", &mut |file| {
            file.write_all(b"data")?;
            Ok(())
        })
        .expect("stored");
        assert_eq!(fs::read(&path).expect("read"), b"data");
        let name = path.file_name().expect("a name").to_string_lossy();
        assert!(
            name.starts_with("2000000000-") && name.ends_with("-b_c.png"),
            "{name}"
        );
        assert_eq!(
            fs::metadata(&path).expect("stat").permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&files).expect("stat").permissions().mode() & 0o7777,
            0o700
        );
        assert!(!files.join("old").exists());
        assert!(files.join("young").exists());
    }

    #[test]
    fn only_a_regular_file_under_the_root_opens() {
        let dir = TempDir::new();
        let root = dir.path().join("root");
        let outside = dir.path().join("outside");
        for d in [root.join("real"), outside.clone()] {
            fs::create_dir_all(d).expect("mkdir");
        }
        fs::write(root.join("real/a.jsonl"), "mine").expect("write");
        fs::write(outside.join("a.jsonl"), "secret").expect("write");
        std::os::unix::fs::symlink(outside.join("a.jsonl"), root.join("real/b.jsonl"))
            .expect("symlink");
        std::os::unix::fs::symlink(&outside, root.join("link")).expect("symlink");
        let mut read = String::new();
        std::io::Read::read_to_string(
            &mut open_inside(&root, &root.join("real/a.jsonl")).expect("open"),
            &mut read,
        )
        .expect("read");
        assert_eq!(read, "mine");
        for path in [
            root.join("real/b.jsonl"),
            root.join("link/a.jsonl"),
            root.join("real"),
            outside.join("a.jsonl"),
        ] {
            assert!(open_inside(&root, &path).is_err(), "{}", path.display());
        }
        assert_eq!(
            open_inside(&root, &root.join("real/none"))
                .expect_err("missing")
                .kind(),
            std::io::ErrorKind::NotFound
        );
    }

    #[test]
    fn fifo_is_refused_at_once() {
        let dir = TempDir::new();
        let root = dir.path().to_owned();
        let fifo = root.join("a.jsonl");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo");
        assert!(made.success());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(open_inside(&root, &fifo).is_err()));
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)), Ok(true));
    }

    #[test]
    fn fifo_behind_a_linked_folder_is_not_opened() {
        let dir = TempDir::new();
        let root = dir.path().join("root");
        let real = dir.path().join("real");
        for d in [&root, &real] {
            fs::create_dir_all(d).expect("mkdir");
        }
        let fifo = real.join("a.json");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo");
        assert!(made.success());
        std::os::unix::fs::symlink(&real, root.join("link")).expect("symlink");
        let (tx, rx) = std::sync::mpsc::channel();
        let writer = fifo.clone();
        std::thread::spawn(move || {
            tx.send(fs::OpenOptions::new().write(true).open(writer).is_ok())
        });
        std::thread::sleep(Duration::from_millis(100));
        assert!(open_inside(&root, &root.join("link/a.json")).is_err());
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "opened"
        );
        drop(fs::OpenOptions::new().read(true).open(&fifo));
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)), Ok(true));
    }

    #[test]
    fn failed_fill_leaves_no_file() {
        let dir = TempDir::new();
        let files = dir.path().join("files");
        let err = store(&files, 1000.0, "a.png", &mut |file| {
            file.write_all(b"part")?;
            Err(crate::Error::Transport("gone".into()))
        })
        .expect_err("failed");
        assert_eq!(err.to_string(), "cannot reach the homeserver: gone");
        assert_eq!(fs::read_dir(&files).expect("list").count(), 0);
    }
}
