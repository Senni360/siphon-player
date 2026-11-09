//! Where unit tests put their scratch folders.
//!
//! Every test folder goes under one `siphon-tests` folder in the system's
//! temporary folder instead of loose beside everything else there. Most tests
//! never remove their folder, and each run names its folders afresh, so the
//! first test of a run removes what earlier runs left behind: anything there
//! untouched for an hour. A run going on at the same time is newer than that.

use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::{Duration, SystemTime};

const STALE: Duration = Duration::from_secs(60 * 60);

/// The folder tests use in place of `std::env::temp_dir()`.
pub(crate) fn temp_dir() -> PathBuf {
    static SWEEP: Once = Once::new();
    let root = std::env::temp_dir().join("siphon-tests");
    SWEEP.call_once(|| {
        remove_stale(&root, SystemTime::now());
    });
    let _ = std::fs::create_dir_all(&root);
    root
}

fn remove_stale(root: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > STALE);
        if stale {
            let path = entry.path();
            let _ = if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_left_by_earlier_runs_are_removed_and_new_ones_kept() {
        let root = temp_dir().join(format!("sweep-{:016x}", rand::random::<u64>()));
        let old = root.join("old");
        let new = root.join("new");
        std::fs::create_dir_all(old.join("state")).unwrap();
        std::fs::create_dir_all(&new).unwrap();

        // An hour and a minute after the folders were made, both are stale.
        remove_stale(&root, SystemTime::now() + STALE + Duration::from_secs(60));
        assert!(!old.exists());
        assert!(!new.exists());

        std::fs::create_dir_all(&new).unwrap();
        remove_stale(&root, SystemTime::now());
        assert!(new.exists());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_test_folder_is_inside_the_system_temporary_folder() {
        let dir = temp_dir();
        assert!(dir.starts_with(std::env::temp_dir()));
        assert!(dir.is_dir());
    }
}
