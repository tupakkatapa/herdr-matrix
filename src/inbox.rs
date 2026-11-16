//! Prompts from the machine for the running bot: one JSON file each in a
//! private directory, taken in the order they came.

use std::ffi::OsString;
use std::fs;
use std::io::{ErrorKind, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::files::{private_dir, random_name};

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

/// Queues a prompt in `dir`, a private directory; the path of its file.
///
/// # Errors
/// When `dir` is not private or the file cannot be written.
pub fn queue(dir: &Path, prompt: &Prompt) -> Result<PathBuf> {
    private_dir(dir)?;
    let name = random_name();
    let tmp = dir.join(format!(".{name}.tmp"));
    let path = dir.join(format!(
        "{:013.0}-{name}.json",
        (prompt.at * 1000.0).floor()
    ));
    let written = write(&tmp, &serde_json::to_vec(prompt)?).and_then(|()| {
        fs::rename(&tmp, &path)?;
        Ok(())
    });
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written.map(|()| path)
}

fn write(path: &Path, data: &[u8]) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(data)?;
    Ok(())
}

/// The prompts queued in `dir`, oldest first, each removed before it is
/// returned, so a crash cannot prompt twice.
///
/// # Errors
/// When `dir` is not private or cannot be read.
pub fn take(dir: &Path, log: &dyn Fn(&str)) -> Result<Vec<Prompt>> {
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
    let mut prompts = Vec::new();
    for name in names {
        let path = dir.join(&name);
        let taken = fs::read(&path).and_then(|data| fs::remove_file(&path).map(|()| data));
        let parsed = match taken {
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) => Err(err.to_string()),
            Ok(data) => serde_json::from_slice(&data).map_err(|err| err.to_string()),
        };
        match parsed {
            Ok(prompt) => prompts.push(prompt),
            Err(err) => log(&format!("error: prompt {}: {err}", name.to_string_lossy())),
        }
    }
    Ok(prompts)
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
    fn unreadable_file_stays_and_the_others_are_taken() {
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
        let taken = take(&inbox, &|line| lines.borrow_mut().push(line.to_owned())).expect("take");
        assert_eq!(taken, [prompt("a", 1000.0)]);
        let lines = lines.into_inner();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("error: prompt 0000000000001-x.json: "));
        assert!(locked.exists());
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
        }
        assert_eq!(fs::read_dir(&private).expect("list").count(), 0);
    }
}
