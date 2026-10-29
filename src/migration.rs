//! One-time directory adoption. Copy to a sibling, then publish by rename;
//! the original profile remains available if copying fails or is interrupted.

use std::fs;
use std::io;
use std::path::Path;

use crate::paths::AppDirs;

#[cfg(any(windows, target_os = "linux"))]
pub(crate) mod startup;

const COMPLETE: &str = "siphon-migration-v1";

/// Adopt each empty destination. Existing Siphon profiles always win.
pub(crate) fn profile(old: &AppDirs, new: &AppDirs) -> io::Result<()> {
    if new.state.join(COMPLETE).try_exists()? {
        return Ok(());
    }
    let mut found = false;
    for source in [&old.config, &old.state, &old.cache] {
        found |= source.try_exists()?;
    }
    if !found {
        return Ok(());
    }
    // Installed and portable copies have different instance slots, but can
    // both adopt the installed profile. Serialize that one-time copy too.
    let parent = new.state.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let name = new.state.file_name().ok_or(io::ErrorKind::InvalidInput)?;
    fs::create_dir_all(parent)?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(parent.join(format!(".siphon-migration-{}.lock", name.to_string_lossy())))?;
    lock.lock()?;
    if new.state.join(COMPLETE).try_exists()? {
        return Ok(());
    }
    for (source, destination, state) in [
        // macOS uses Application Support for both config and durable state.
        (&old.config, &new.config, old.config == old.state),
        (&old.state, &new.state, true),
        (&old.cache, &new.cache, false),
    ] {
        directory(source, destination, state, |copy| {
            if state {
                crate::credentials::preserve_profile(copy, source)?;
            }
            Ok(())
        })?;
    }
    fs::create_dir_all(&new.state)?;
    // Clearing a cache or signing out later must not reimport old files.
    fastframe_file::write(&new.state.join(COMPLETE), "complete\n")?;
    Ok(())
}

fn empty(path: &Path) -> io::Result<bool> {
    match fs::read_dir(path) {
        Ok(mut entries) => entries.next().transpose().map(|entry| entry.is_none()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    }
}

pub(crate) fn copy_cache(source: &Path, destination: &Path) -> io::Result<()> {
    directory(source, destination, false, |_| Ok(()))
}

fn directory(
    source: &Path,
    destination: &Path,
    skip_instance: bool,
    adapt: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    if !source.try_exists()? || !empty(destination)? {
        return Ok(());
    }
    let parent = destination.parent().ok_or(io::ErrorKind::InvalidInput)?;
    fs::create_dir_all(parent)?;
    let name = destination.file_name().ok_or(io::ErrorKind::InvalidInput)?;
    let staging = parent.join(format!(
        ".siphon-migration-{}.partial",
        name.to_string_lossy()
    ));
    // Only this destination's sibling staging directory is ours. A previous
    // interrupted copy is discarded and rebuilt from the untouched source.
    if staging.try_exists()? {
        fs::remove_dir_all(&staging)?;
    }
    // create_dir, rather than create_dir_all, refuses an existing staging path.
    fs::create_dir(&staging)?;
    let result = (|| {
        copy_tree(source, &staging, skip_instance)?;
        adapt(&staging)?;
        if destination.exists() {
            // Only an empty directory may be replaced, including after a race.
            fs::remove_dir(destination)?;
        }
        fs::rename(&staging, destination)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn copy_tree(source: &Path, destination: &Path, skip_instance: bool) -> io::Result<()> {
    fs::set_permissions(destination, fs::metadata(source)?.permissions())?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        // A live instance's locks and socket must never become this profile's.
        if skip_instance && entry.file_name() == "instance" {
            continue;
        }
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            fs::create_dir(&target)?;
            copy_tree(&entry.path(), &target, false)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// eframe used a separate data directory for window geometry.
pub(crate) fn migrate_window_state() -> io::Result<()> {
    let Some(new) = eframe::storage_dir(crate::brand::DATA_NAME) else {
        return Ok(());
    };
    if let Some(old) = legacy_window_dir() {
        directory(&old, &new, false, |_| Ok(()))?;
    }
    Ok(())
}

fn legacy_window_dir() -> Option<std::path::PathBuf> {
    crate::paths::legacy_flatpak_root()
        .map(|root| root.join("data/spotifast"))
        .or_else(|| eframe::storage_dir("spotifast"))
}

/// The saved window remains usable if migration failed on a read-only folder.
pub fn window_state() -> Option<std::path::PathBuf> {
    let new = eframe::storage_dir(crate::brand::DATA_NAME)?.join("app.ron");
    if !new.exists()
        && let Some(old) = legacy_window_dir().map(|dir| dir.join("app.ron"))
        && old.exists()
    {
        return Some(old);
    }
    Some(new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "siphon-migration-test-{:016x}",
                rand::random::<u64>()
            )))
        }
        fn dirs(&self, name: &str) -> AppDirs {
            fastframe_dirs::Dirs::inside(&self.0.join(name)).into()
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn migrates_all_profile_files_and_preserves_the_credential_identity() {
        let scratch = Scratch::new();
        let old = scratch.dirs("spotifast-data");
        let new = scratch.dirs("siphon-data");
        old.ensure().unwrap();
        fs::write(
            old.settings_file(),
            br#"{"device_name":"Spotifast","local_files_folder":"D:/Music"}"#,
        )
        .unwrap();
        for file in [
            old.session_file(),
            old.soundcloud_account_file(),
            old.mixed_playlists_file(),
            old.playlist_overlay_file(),
            old.history_file(),
            old.state.join("soundcloud-client.json"),
            old.state.join("credential-storage"),
        ] {
            fs::write(file, "unchanged").unwrap();
        }
        fs::write(old.cache.join("audio"), "cached").unwrap();
        fs::create_dir_all(old.config.join("skins/instance")).unwrap();
        fs::write(old.config.join("skins/instance/user-art"), "keep this too").unwrap();
        fs::create_dir(old.state.join("instance")).unwrap();
        // The empty destination case is common after an installer creates it.
        new.ensure().unwrap();
        profile(&old, &new).unwrap();
        assert_eq!(
            fs::read(new.settings_file()).unwrap(),
            fs::read(old.settings_file()).unwrap()
        );
        for file in [
            "session.json",
            "soundcloud-account.json",
            "mixed-playlists.json",
            "playlist-overlay.json",
            "history.json",
            "soundcloud-client.json",
            "credential-storage",
        ] {
            assert_eq!(fs::read(new.state.join(file)).unwrap(), b"unchanged");
            assert!(old.state.join(file).exists());
        }
        assert_eq!(fs::read(new.cache.join("audio")).unwrap(), b"cached");
        let settings = crate::settings::Settings::load(&new.settings_file());
        assert_eq!(settings.device_name, "Spotifast");
        assert_eq!(
            serde_json::to_value(&settings).unwrap()["local_files_folder"],
            "D:/Music"
        );
        assert!(!new.state.join("instance").exists());
        assert_eq!(
            fs::read(new.config.join("skins/instance/user-art")).unwrap(),
            b"keep this too"
        );
        crate::credentials::keep_profile(&old.state).unwrap();
        assert_eq!(
            fs::read(new.state.join("credential-profile")).unwrap(),
            fs::read(old.state.join("credential-profile")).unwrap()
        );
        fs::write(new.mixed_playlists_file(), "new edits").unwrap();
        profile(&old, &new).unwrap();
        assert_eq!(fs::read(new.mixed_playlists_file()).unwrap(), b"new edits");
    }

    #[test]
    fn missing_old_profile_leaves_destination_absent() {
        let scratch = Scratch::new();
        let new = scratch.dirs("new");
        profile(&scratch.dirs("absent"), &new).unwrap();
        assert!(!new.config.exists());
        assert!(!new.state.exists());
        assert!(!new.cache.exists());
    }

    #[test]
    fn a_new_portable_profile_keeps_the_installed_audio_and_artwork_cache() {
        let scratch = Scratch::new();
        let old = scratch.dirs("installed");
        let new = scratch.dirs("siphon-data.partial");
        old.ensure().unwrap();
        new.ensure().unwrap();
        for folder in ["audio", "art"] {
            fs::create_dir(old.cache.join(folder)).unwrap();
            fs::write(old.cache.join(folder).join("cached"), folder).unwrap();
        }
        copy_cache(&old.cache, &new.cache).unwrap();
        for folder in ["audio", "art"] {
            assert_eq!(
                fs::read(new.cache.join(folder).join("cached")).unwrap(),
                folder.as_bytes()
            );
            assert!(old.cache.join(folder).join("cached").exists());
        }
    }

    #[test]
    fn an_intentionally_cleared_cache_is_not_reimported_on_later_starts() {
        let scratch = Scratch::new();
        let old = scratch.dirs("old");
        let new = scratch.dirs("new");
        old.ensure().unwrap();
        fs::write(old.cache.join("audio"), "cached").unwrap();
        profile(&old, &new).unwrap();
        fs::remove_dir_all(&new.cache).unwrap();
        profile(&old, &new).unwrap();
        assert!(!new.cache.exists());
    }

    #[test]
    fn an_interrupted_staging_copy_is_rebuilt_from_the_original_files() {
        let scratch = Scratch::new();
        let old = scratch.dirs("old");
        let new = scratch.dirs("new");
        old.ensure().unwrap();
        fs::write(old.settings_file(), "complete settings").unwrap();
        let staging = new
            .config
            .parent()
            .unwrap()
            .join(".siphon-migration-config.partial");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("settings.json"), "partial").unwrap();
        profile(&old, &new).unwrap();
        assert_eq!(fs::read(new.settings_file()).unwrap(), b"complete settings");
        assert!(!staging.exists());
    }

    #[test]
    fn shared_config_and_state_roots_keep_the_credential_profile() {
        let scratch = Scratch::new();
        let mut old = scratch.dirs("old-macos");
        let mut new = scratch.dirs("new-macos");
        old.state = old.config.clone();
        new.state = new.config.clone();
        old.ensure().unwrap();
        fs::write(old.session_file(), "session").unwrap();
        profile(&old, &new).unwrap();
        crate::credentials::keep_profile(&old.state).unwrap();
        assert_eq!(
            fs::read(new.state.join("credential-profile")).unwrap(),
            fs::read(old.state.join("credential-profile")).unwrap()
        );
        assert_eq!(fs::read(new.session_file()).unwrap(), b"session");
    }

    #[test]
    fn failed_adaptation_never_publishes_partial_data_and_can_retry() {
        let scratch = Scratch::new();
        let old = scratch.dirs("old");
        let new = scratch.dirs("new");
        old.ensure().unwrap();
        fs::write(old.session_file(), "session").unwrap();
        assert!(
            directory(&old.state, &new.state, true, |_| Err(
                io::ErrorKind::PermissionDenied.into()
            ))
            .is_err()
        );
        assert!(!new.state.exists());
        assert!(old.session_file().exists());
        profile(&old, &new).unwrap();
        assert_eq!(fs::read(new.session_file()).unwrap(), b"session");
    }

    #[test]
    fn a_late_failure_can_retry_without_overwriting_published_or_newer_data() {
        let scratch = Scratch::new();
        let old = scratch.dirs("old");
        let new = scratch.dirs("new");
        old.ensure().unwrap();
        fs::write(old.settings_file(), "original settings").unwrap();
        fs::write(old.history_file(), "history").unwrap();
        fs::create_dir_all(new.cache.parent().unwrap()).unwrap();
        fs::write(&new.cache, "blocks the cache copy").unwrap();
        assert!(profile(&old, &new).is_err());
        assert!(!new.state.join(COMPLETE).exists());
        assert_eq!(fs::read(new.history_file()).unwrap(), b"history");
        fs::write(new.settings_file(), "newer settings").unwrap();
        fs::remove_file(&new.cache).unwrap();
        profile(&old, &new).unwrap();
        assert_eq!(fs::read(new.settings_file()).unwrap(), b"newer settings");
        assert!(new.state.join(COMPLETE).exists());
        assert_eq!(fs::read(old.settings_file()).unwrap(), b"original settings");
    }
}
