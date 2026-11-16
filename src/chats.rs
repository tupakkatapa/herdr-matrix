//! The client of the chat gate: `herdr-matrix chats …`.

use std::fs;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::files::own_uid;
use crate::{Error, Result};

/// Where the gate module puts a socket per user
const SOCKETS: &str = "/run/herdr-matrix-gate";

/// The socket of the gate for this user: `HERDR_MATRIX_GATE_SOCKET`, else
/// the socket in `/run/herdr-matrix-gate` that this user owns.
fn socket(env: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf> {
    if let Some(path) = env("HERDR_MATRIX_GATE_SOCKET") {
        return Ok(path.into());
    }
    let uid = own_uid()?;
    fs::read_dir(SOCKETS)
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .find(|entry| {
            entry
                .metadata()
                .is_ok_and(|meta| meta.file_type().is_socket() && meta.uid() == uid)
        })
        .map(|entry| entry.path())
        .ok_or_else(|| Error::Setting("the gate is not set up for this user".into()))
}

/// Sends the arguments of `herdr-matrix chats` to the gate; its answer.
///
/// # Errors
/// When the gate cannot be reached, or answers with an error.
pub fn ask(env: &dyn Fn(&str) -> Option<String>, argv: &[String]) -> Result<Value> {
    let reach = |err: std::io::Error| Error::Setting(format!("cannot reach the gate: {err}"));
    let mut stream = UnixStream::connect(socket(env)?).map_err(reach)?;
    writeln!(stream, "{}", json!({ "argv": argv })).map_err(reach)?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).map_err(reach)?;
    let reply: Value = serde_json::from_str(&reply).unwrap_or_default();
    if let Some(ok) = reply.get("ok") {
        return Ok(ok.clone());
    }
    Err(Error::Refused(
        reply
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("the gate gave no answer")
            .to_owned(),
    ))
}
