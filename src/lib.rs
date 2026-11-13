#![forbid(unsafe_code)]
//! herdr <-> Matrix: a chat attached to one agent at a time; agents that
//! need the owner while away post in it too.

pub mod app;
pub mod bot;
pub mod commands;
pub mod error;
pub mod files;
pub mod herdr;
pub mod json;
pub mod matrix;
pub mod process;
pub mod render;
pub mod state;
#[cfg(test)]
mod testutil;
pub mod voice;

pub use error::{Error, Result};
