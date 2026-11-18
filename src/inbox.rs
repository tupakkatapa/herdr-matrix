//! Prompts and files from the machine for the running bot: one JSON file
//! each in a private directory, taken in the order they came.

use std::ffi::OsString;
use std::fs;
use std::io::{self, ErrorKind, Read as _, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::files::{FILE_TTL, open_inside, private_dir, purge, random_name};

/// Bytes of an entry at most; the rest of a longer one is not read
const ENTRY_MAX: u64 = 1 << 20;
/// Files taken from a queue at once; the rest wait for the next take
const UPLOADS_TAKEN: usize = 5;
/// Bytes of a file's name at most; the rest of a longer one is cut
const NAME_MAX: usize = 255;

/// A prompt for the agent `agent` names, or with `cwd`, for the agent
/// working there; `at` is in seconds since the epoch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prompt {
    pub agent: String,
    pub text: String,
    pub at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// A file the agent in `pane` posts as `name`; `at` is in seconds since
/// the epoch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Upload {
    pub pane: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    pub at: f64,
}

/// Queues a prompt in `dir`, a private directory; the path of its file.
///
/// # Errors
/// When `dir` is not private or the file cannot be written.
pub fn queue(dir: &Path, prompt: &Prompt) -> Result<PathBuf> {
    private_dir(dir)?;
    let path = dir.join(format!("{}.json", stem(prompt.at)));
    place(dir, &path, prompt).map(|()| path)
}

/// Queues a file in `dir`, a private directory: a copy of `source` beside
/// its entry. The path of the entry.
///
/// # Errors
/// When `dir` is not private, or the copy or the entry cannot be written.
pub fn queue_upload(dir: &Path, upload: &Upload, source: &mut fs::File) -> Result<PathBuf> {
    private_dir(dir)?;
    let stem = stem(upload.at);
    let data = dir.join(format!("{stem}.data"));
    let path = dir.join(format!("{stem}.json"));
    let copied = create(&data).and_then(|mut file| {
        io::copy(source, &mut file)?;
        place(dir, &path, upload)
    });
    if copied.is_err() {
        let _ = fs::remove_file(&data);
    }
    copied.map(|()| path)
}

/// The start of a queued file's name: the time in milliseconds, so names
/// sort in the order they came, and a random part.
fn stem(at: f64) -> String {
    format!("{:013.0}-{}", (at * 1000.0).floor(), random_name())
}

/// Writes an entry under a hidden name, then renames it into place, so it
/// is never taken in part.
fn place(dir: &Path, path: &Path, entry: &impl Serialize) -> Result<()> {
    let tmp = dir.join(format!(".{}.tmp", random_name()));
    let written = create(&tmp)
        .and_then(|mut file| Ok(file.write_all(&serde_json::to_vec(entry)?)?))
        .and_then(|()| Ok(fs::rename(&tmp, path)?));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

fn create(path: &Path) -> Result<fs::File> {
    Ok(fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?)
}

/// The prompts queued in `dir`, oldest first, each removed before it is
/// returned, so a crash cannot prompt twice.
///
/// # Errors
/// When `dir` is not private or cannot be read.
pub fn take(dir: &Path, log: &dyn Fn(&str)) -> Result<Vec<Prompt>> {
    Ok(take_all(dir, "prompt", usize::MAX, log)?
        .into_iter()
        .map(|(prompt, _)| prompt)
        .collect())
}

/// The oldest `UPLOADS_TAKEN` files queued in `dir`, oldest first, each
/// with the path of its copy, its name cut to `NAME_MAX` bytes and its time
/// no later than `now`. Each
/// entry is removed before it is returned, so a crash cannot post twice;
/// copies older than `FILE_TTL` are removed. `now` is in seconds since the
/// epoch.
///
/// # Errors
/// When `dir` is not private or cannot be read.
pub fn take_uploads(dir: &Path, now: f64, log: &dyn Fn(&str)) -> Result<Vec<(Upload, PathBuf)>> {
    let uploads = take_all(dir, "upload", UPLOADS_TAKEN, log)?;
    if fs::symlink_metadata(dir).is_ok() {
        purge(dir, now - FILE_TTL.as_secs_f64())?;
    }
    Ok(uploads
        .into_iter()
        .map(|(mut upload, path): (Upload, PathBuf)| {
            let cut = upload.name.floor_char_boundary(NAME_MAX);
            upload.name.truncate(cut);
            upload.at = upload.at.min(now);
            (upload, path.with_extension("data"))
        })
        .collect())
}

/// True when taking from `dir` is worth it: an entry is there, or `dir`
/// is there and not private, which taking reports.
#[must_use]
pub fn queued(dir: &Path) -> bool {
    if fs::symlink_metadata(dir).is_err_and(|err| err.kind() == ErrorKind::NotFound) {
        return false;
    }
    private_dir(dir).is_err()
        || fs::read_dir(dir).map_or(true, |entries| {
            entries
                .filter_map(std::result::Result::ok)
                .any(|e| e.file_name().to_string_lossy().ends_with(".json"))
        })
}

/// Queues again a file taken from the queue, beside its copy `data`.
///
/// # Errors
/// When the entry cannot be written.
pub fn requeue(upload: &Upload, data: &Path) -> Result<()> {
    place(
        data.parent().unwrap_or(data),
        &data.with_extension("json"),
        upload,
    )
}

/// The oldest `max` entries queued in `dir`, oldest first, with their
/// paths, each removed before it is returned; one that cannot be read is
/// removed and logged as a `what`.
fn take_all<T: DeserializeOwned>(
    dir: &Path,
    what: &str,
    max: usize,
    log: &dyn Fn(&str),
) -> Result<Vec<(T, PathBuf)>> {
    match fs::symlink_metadata(dir) {
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        _ => private_dir(dir)?,
    }
    let mut names: Vec<OsString> = fs::read_dir(dir)?
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.file_name())
        .filter(|n| n.to_string_lossy().ends_with(".json"))
        .collect();
    names.sort();
    let mut entries = Vec::new();
    for name in names {
        if entries.len() == max {
            break;
        }
        let path = dir.join(&name);
        let taken = open_inside(dir, &path)
            .and_then(|file| {
                let mut data = Vec::new();
                file.take(ENTRY_MAX).read_to_end(&mut data).map(|_| data)
            })
            .and_then(|data| fs::remove_file(&path).map(|()| data));
        let parsed = match taken {
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) => {
                let _ = fs::remove_file(&path);
                Err(err.to_string())
            }
            Ok(data) => serde_json::from_slice(&data).map_err(|err| err.to_string()),
        };
        match parsed {
            Ok(entry) => entries.push((entry, path)),
            Err(err) => log(&format!("error: {what} {}: {err}", name.to_string_lossy())),
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use std::cell::RefCell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;

    fn prompt(agent: &str, at: f64) -> Prompt {
        Prompt {
            agent: agent.into(),
            text: "Check the mail".into(),
            at,
            cwd: None,
        }
    }

    #[test]
    fn queued_prompts_are_taken_once_in_order() {
        let dir = TempDir::new();
        let inbox = dir.path().join("prompts");
        let late = queue(&inbox, &prompt("b", 1_800_000_000.5)).expect("queue");
        queue(&inbox, &prompt("a", 999_999_999.0)).expect("queue");
        let name = late.file_name().expect("a name").to_string_lossy();
        assert!(
            name.starts_with("1800000000500-") && name.ends_with(".json"),
            "{name}"
        );
        let mode = fs::metadata(&late).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let taken = take(&inbox, &|line| panic!("{line}")).expect("take");
        assert_eq!(
            taken,
            [prompt("a", 999_999_999.0), prompt("b", 1_800_000_000.5)]
        );
        assert_eq!(fs::read_dir(&inbox).expect("list").count(), 0);
        assert!(take(&inbox, &|_| {}).expect("take").is_empty());
    }

    #[test]
    fn broken_file_is_removed_and_logged() {
        let dir = TempDir::new();
        let inbox = dir.path().join("prompts");
        queue(&inbox, &prompt("a", 1000.0)).expect("queue");
        fs::write(inbox.join("0000000000001-x.json"), "{").expect("write");
        let lines = RefCell::new(Vec::new());
        let taken = take(&inbox, &|line| lines.borrow_mut().push(line.to_owned())).expect("take");
        assert_eq!(taken, [prompt("a", 1000.0)]);
        let lines = lines.into_inner();
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].starts_with("error: prompt 0000000000001-x.json: "),
            "{}",
            lines[0]
        );
        assert_eq!(fs::read_dir(&inbox).expect("list").count(), 0);
    }

    #[test]
    fn unreadable_file_is_removed_and_logged_once() {
        let dir = TempDir::new();
        let inbox = dir.path().join("prompts");
        queue(&inbox, &prompt("a", 1000.0)).expect("queue");
        let locked = inbox.join("0000000000001-x.json");
        fs::write(&locked, "{}").expect("write");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("chmod");
        if fs::read(&locked).is_ok() {
            return;
        }
        let lines = RefCell::new(Vec::new());
        let log = |line: &str| lines.borrow_mut().push(line.to_owned());
        let taken = take(&inbox, &log).expect("take");
        assert_eq!(taken, [prompt("a", 1000.0)]);
        assert!(take(&inbox, &log).expect("take").is_empty());
        let lines = lines.into_inner();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("error: prompt 0000000000001-x.json: "));
        assert!(!locked.exists());
    }

    fn upload(at: f64) -> Upload {
        Upload {
            pane: "w1:p1".into(),
            name: "shot.png".into(),
            caption: Some("Look".into()),
            at,
        }
    }

    #[test]
    fn queued_file_is_a_private_copy_taken_once() {
        let dir = TempDir::new();
        let uploads = dir.path().join("uploads");
        let source = dir.path().join("shot.png");
        fs::write(&source, "picture").expect("write");
        fs::set_permissions(&source, fs::Permissions::from_mode(0o444)).expect("chmod");
        let mut file = fs::File::open(&source).expect("open");
        queue_upload(&uploads, &upload(1000.0), &mut file).expect("queue");
        let mut file = fs::File::open(&source).expect("open");
        queue_upload(&uploads, &upload(999.0), &mut file).expect("queue");
        let taken = take_uploads(&uploads, 1000.0, &|line| panic!("{line}")).expect("take");
        assert_eq!(
            taken.iter().map(|(u, _)| u.clone()).collect::<Vec<_>>(),
            [upload(999.0), upload(1000.0)]
        );
        for (_, data) in &taken {
            assert_eq!(data.parent(), Some(uploads.as_path()));
            assert_eq!(fs::read(data).expect("read"), b"picture");
            let mode = fs::metadata(data).expect("stat").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(fs::read_dir(&uploads).expect("list").count(), 2);
        assert!(
            take_uploads(&uploads, 1000.0, &|_| {})
                .expect("take")
                .is_empty()
        );
        let none = dir.path().join("none");
        assert!(
            take_uploads(&none, 1000.0, &|_| {})
                .expect("take")
                .is_empty()
        );
    }

    #[test]
    fn copies_older_than_the_files_go() {
        let dir = TempDir::new();
        let uploads = dir.path().join("uploads");
        let source = dir.path().join("a.txt");
        fs::write(&source, "a").expect("write");
        let mut file = fs::File::open(&source).expect("open");
        queue_upload(&uploads, &upload(1000.0), &mut file).expect("queue");
        fs::write(uploads.join("0000000000001-x.json"), "{").expect("write");
        let lines = RefCell::new(Vec::new());
        let log = |line: &str| lines.borrow_mut().push(line.to_owned());
        let taken = take_uploads(&uploads, 1000.0, &log).expect("take");
        assert_eq!(taken.len(), 1);
        let lines = lines.into_inner();
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].starts_with("error: upload 0000000000001-x.json: "),
            "{lines:?}"
        );
        let made = fs::metadata(&taken[0].1)
            .and_then(|m| m.modified())
            .expect("mtime")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("since")
            .as_secs_f64();
        let ttl = FILE_TTL.as_secs_f64();
        assert!(
            take_uploads(&uploads, made + ttl - 60.0, &|_| {})
                .expect("take")
                .is_empty()
        );
        assert_eq!(fs::read_dir(&uploads).expect("list").count(), 1);
        assert!(
            take_uploads(&uploads, made + ttl + 1.0, &|_| {})
                .expect("take")
                .is_empty()
        );
        assert_eq!(fs::read_dir(&uploads).expect("list").count(), 0);
    }

    #[test]
    fn missing_directory_holds_nothing() {
        let dir = TempDir::new();
        let inbox = dir.path().join("prompts");
        assert!(take(&inbox, &|_| {}).expect("take").is_empty());
        assert!(!inbox.exists());
    }

    #[test]
    fn directory_that_is_not_private_is_refused() {
        let dir = TempDir::new();
        let open = dir.path().join("open");
        fs::create_dir(&open).expect("mkdir");
        fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).expect("chmod");
        let link = dir.path().join("link");
        let private = dir.path().join("private");
        fs::create_dir(&private).expect("mkdir");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("chmod");
        std::os::unix::fs::symlink(&private, &link).expect("symlink");
        for inbox in [&open, &link] {
            let err = take(inbox, &|_| {}).expect_err("refused");
            assert!(err.to_string().contains("not a private directory"), "{err}");
            assert!(queue(inbox, &prompt("a", 1000.0)).is_err());
            let mut source = fs::File::open(dir.path()).expect("open");
            assert!(queue_upload(inbox, &upload(1000.0), &mut source).is_err());
        }
        assert_eq!(fs::read_dir(&private).expect("list").count(), 0);
    }
}
