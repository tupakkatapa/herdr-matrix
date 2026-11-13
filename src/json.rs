//! Fields of JSON from herdr and Matrix.

use serde_json::Value;

use crate::{Error, Result};

/// A field of a JSON object.
///
/// # Errors
/// When it is absent.
pub fn key<'a>(value: &'a Value, name: &str) -> Result<&'a Value> {
    value
        .get(name)
        .ok_or_else(|| Error::Malformed(format!("no {name}")))
}

/// A string field of a JSON object.
///
/// # Errors
/// When it is absent or not a string.
pub fn key_str<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    key(value, name)?
        .as_str()
        .ok_or_else(|| Error::Malformed(format!("{name} is not a string")))
}
