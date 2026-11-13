//! A temporary directory for the tests, of its own, removed when dropped.

use std::hash::{BuildHasher as _, Hasher as _, RandomState};
use std::path::{Path, PathBuf};

pub struct TempDir(PathBuf);

impl TempDir {
    /// # Panics
    /// When no new directory can be made.
    #[must_use]
    pub fn new() -> Self {
        let n = RandomState::new().build_hasher().finish();
        let path =
            std::env::temp_dir().join(format!("herdr-matrix-test-{}-{n:016x}", std::process::id()));
        std::fs::create_dir(&path).expect("a new temporary directory");
        Self(path)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Default for TempDir {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
