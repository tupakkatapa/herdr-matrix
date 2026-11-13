//! Pictures and files from the chat.

use crate::fakes::T;
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;

fn image(url: &str) -> serde_json::Value {
    json!({"msgtype": "m.image", "url": url})
}

#[test]
fn picture_is_handed_to_the_agent() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let files = t.files().to_string_lossy().into_owned();
    t.tell_with("shot.png", &image("mxc://x/y"));
    let prompt = t.herdr.did("prompt").last().expect("a prompt")[3].clone();
    assert!(
        prompt.starts_with(&format!("See the file {files}/1000-")),
        "{prompt}"
    );
    let path = prompt.strip_prefix("See the file ").expect("a path");
    assert!(path.ends_with("-shot.png"));
    assert_eq!(fs::read(path).expect("read"), b"img mxc://x/y");
    t.tell_with(
        "what is this",
        &json!({"msgtype": "m.image", "url": "mxc://x/z", "filename": "../a b.png"}),
    );
    let prompt = t.herdr.did("prompt").last().expect("a prompt")[3].clone();
    let (caption, path) = prompt.split_once("\n\n").expect("caption and path");
    assert_eq!(caption, "what is this");
    assert_eq!(
        std::path::Path::new(path)
            .parent()
            .expect("dir")
            .to_string_lossy(),
        files
    );
    assert!(path.ends_with("-a_b.png"));
}

#[test]
fn files_of_one_name_do_not_overwrite() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.tell_with("shot.png", &image("mxc://x/1"));
    t.tell_with("shot.png", &image("mxc://x/2"));
    let paths: Vec<String> = t
        .herdr
        .did("prompt")
        .into_iter()
        .map(|c| {
            c[3].strip_prefix("See the file ")
                .expect("a path")
                .to_owned()
        })
        .collect();
    assert_ne!(paths[0], paths[1]);
    for (path, mxc) in paths.iter().zip(["mxc://x/1", "mxc://x/2"]) {
        assert_eq!(
            fs::read(path).expect("read"),
            format!("img {mxc}").into_bytes()
        );
    }
}

#[test]
fn files_go_only_to_a_private_directory() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    let files = t.files();
    fs::create_dir(&files).expect("mkdir");
    fs::set_permissions(&files, fs::Permissions::from_mode(0o755)).expect("chmod");
    t.tell_with("shot.png", &image("mxc://x/y"));
    fs::remove_dir(&files).expect("rmdir");
    let elsewhere = t.dir.path().join("elsewhere");
    fs::create_dir(&elsewhere).expect("mkdir");
    fs::set_permissions(&elsewhere, fs::Permissions::from_mode(0o700)).expect("chmod");
    std::os::unix::fs::symlink(&elsewhere, &files).expect("symlink");
    t.tell_with("shot.png", &image("mxc://x/y"));
    assert!(t.herdr.did("prompt").is_empty());
    assert_eq!(fs::read_dir(&elsewhere).expect("list").count(), 0);
    assert!(t.new_msgs().iter().all(|m| m.starts_with("Failed")));
    assert_eq!(t.new_msgs().len(), 2);
}

#[test]
fn old_files_are_removed() {
    let mut t = T::new();
    t.set("wA:p1", "idle");
    t.clock.set(200_000.0);
    let files = t.files();
    fs::create_dir(&files).expect("mkdir");
    fs::set_permissions(&files, fs::Permissions::from_mode(0o700)).expect("chmod");
    for (name, age) in [("old", 86401.0), ("young", 86000.0)] {
        let path = files.join(name);
        fs::write(&path, name).expect("write");
        let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs_f64(t.now() - age);
        fs::File::options()
            .write(true)
            .open(&path)
            .expect("open")
            .set_modified(at)
            .expect("utime");
    }
    t.tell_with("shot.png", &image("mxc://x/y"));
    let mut left: Vec<String> = fs::read_dir(&files)
        .expect("list")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left.len(), 2);
    assert!(!left.contains(&"old".to_owned()));
    assert!(left.contains(&"young".to_owned()));
}
