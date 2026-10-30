//! Where Siphon keeps its files.
//!
//! Configuration, durable non-secret state, and disposable caches live in the
//! platform's conventional directories, or, for a copy started from the
//! portable Windows zip, in `siphon-data` next to the executable (#703).
//! Spotify grants use the platform store; the token paths below are retained
//! only for migration and sign-out cleanup.

use std::path::PathBuf;

/// Where Siphon's folders are: the per-user folders, or a portable copy's
/// own. A new portable folder copies the per-user settings and state, except
/// the single-instance files a running installed copy holds locked and the
/// logs of its runs.
const ORGANIZATION: &str = if cfg!(windows) { "" } else { "siphon" };

const FOLDERS: fastframe_dirs::App = fastframe_dirs::App {
    not_copied: &["instance", "spotifast.log", "siphon.log", "panic.log"],
    ..fastframe_dirs::App::new(
        crate::brand::SLUG,
        "com",
        ORGANIZATION,
        crate::brand::DATA_NAME,
    )
};
const LEGACY_FOLDERS: fastframe_dirs::App =
    fastframe_dirs::App::new("spotifast", "me", "paolino", "spotifast");

fn rebranded_portable(legacy: fastframe_dirs::Portable) -> fastframe_dirs::Portable {
    match legacy {
        fastframe_dirs::Portable::Folder(folder) => {
            fastframe_dirs::Portable::Folder(folder.with_file_name("siphon-data"))
        }
        other => other,
    }
}

fn portable() -> fastframe_dirs::Portable {
    match FOLDERS.portable() {
        // Replacing just the executable in an old portable zip keeps it portable.
        fastframe_dirs::Portable::No => rebranded_portable(LEGACY_FOLDERS.portable()),
        current => current,
    }
}

pub(crate) fn legacy_flatpak_root() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    if std::env::var("FLATPAK_ID").as_deref() == Ok(crate::brand::APP_ID) {
        return std::env::home_dir().map(|home| home.join(".var/app/rocks.spotifast.Spotifast"));
    }
    None
}

#[cfg(any(target_os = "linux", test))]
fn flatpak_profile(root: &std::path::Path) -> AppDirs {
    AppDirs {
        config: root.join("config/spotifast"),
        state: root.join(".local/state/spotifast"),
        cache: root.join("cache/spotifast"),
    }
}

pub(crate) fn legacy_installed() -> AppDirs {
    #[cfg(target_os = "linux")]
    if let Some(root) = legacy_flatpak_root() {
        return flatpak_profile(&root);
    }
    LEGACY_FOLDERS
        .installed()
        .map_or_else(AppDirs::legacy_fallback, AppDirs::from)
}

#[derive(Clone, Debug)]
pub struct AppDirs {
    pub config: PathBuf,
    pub state: PathBuf,
    pub cache: PathBuf,
}

impl From<fastframe_dirs::Dirs> for AppDirs {
    fn from(dirs: fastframe_dirs::Dirs) -> Self {
        Self {
            config: dirs.config,
            state: dirs.state,
            cache: dirs.cache,
        }
    }
}

/// The folders [`AppDirs::discover`] chose, and what to log about them once
/// logging has started.
pub struct Found {
    pub dirs: AppDirs,
    lines: Vec<(log::Level, String)>,
}

impl Found {
    /// Folders chosen elsewhere, such as a demo's, with nothing to log.
    pub fn given(dirs: AppDirs) -> Self {
        Self {
            dirs,
            lines: Vec::new(),
        }
    }

    pub fn log(&self) {
        for (level, line) in &self.lines {
            log::log!(*level, "{line}");
        }
    }
}

fn discover_with(
    work: impl FnOnce() -> std::io::Result<Found> + Send + 'static,
) -> std::io::Result<Found> {
    std::thread::Builder::new()
        .name("profile-migration".into())
        .spawn(work)?
        .join()
        .map_err(|_| std::io::Error::other("profile migration worker panicked"))?
}

impl AppDirs {
    /// The folders this copy uses: a portable copy's own, made the first
    /// time from the per-user ones, or the per-user ones. Call it once this
    /// process is the running copy, before the window opens.
    pub fn discover() -> std::io::Result<Found> {
        // Startup waits before opening any window, but disk copies and durable
        // writes still belong to a worker, never the future drawing thread.
        discover_with(Self::discover_on_worker)
    }

    fn discover_on_worker() -> std::io::Result<Found> {
        crate::migration::migrate_window_state()?;
        // A portable copy must adopt its adjacent old data before fastframe
        // seeds a new folder from the installed profile.
        if let Some(new) = portable().folder() {
            let old = new.with_file_name("spotifast-data");
            crate::migration::profile(
                &fastframe_dirs::Dirs::inside(&old).into(),
                &fastframe_dirs::Dirs::inside(new).into(),
            )
            .and_then(|()| {
                if matches!(FOLDERS.portable(), fastframe_dirs::Portable::No) {
                    let parent = new.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
                    fastframe_file::write(
                        &parent.join("siphon-portable.txt"),
                        "siphon-portable-v1\n",
                    )?;
                }
                Ok(())
            })?;
        }
        // An existing portable profile does not depend on the installed one.
        // On failure, stop startup: falling back after publishing one tree
        // would let edits to the old tree go missing on the next retry.
        if portable().folder().is_none_or(|new| !new.exists()) {
            crate::migration::profile(&legacy_installed(), &Self::installed())?;
        }
        let found = FOLDERS.discover(|copy, from| {
            // The copy starts with the sign-in it was made from, under
            // credential entries of its own.
            crate::credentials::adopt_profile(&copy.state, &from.state)?;
            // Older ZIPs used the installed profile. Preserve their caches
            // too, inside fastframe's staged portable copy, before publishing.
            crate::migration::copy_cache(&from.cache, &copy.cache)
        });
        let dirs = found.dirs.map_or_else(Self::fallback, Self::from);
        let mut lines: Vec<_> = describe(&dirs, &found.mode).into_iter().collect();
        if matches!(found.mode, fastframe_dirs::Mode::Portable { .. }) {
            // Its sign-in follows the folder when it moves to another drive.
            if let Err(error) = crate::credentials::keep_profile(&dirs.state) {
                lines.push((
                    log::Level::Warn,
                    format!("unable to record the portable credential profile: {error}"),
                ));
            }
        }
        Ok(Found { dirs, lines })
    }

    /// The per-user folders, which an installed copy uses whether or not
    /// this copy is portable.
    pub fn installed() -> Self {
        FOLDERS.installed().map_or_else(Self::fallback, Self::from)
    }

    /// The single-instance slot's scope for a portable copy, so it never
    /// hands its launches to an installed copy, or to a portable copy in
    /// another folder, which keep other settings.
    pub fn instance_scope() -> Option<String> {
        portable().instance_scope()
    }

    pub(crate) fn legacy_instance_scope() -> Option<String> {
        match portable() {
            fastframe_dirs::Portable::Folder(folder) => {
                fastframe_dirs::Portable::Folder(folder.with_file_name("spotifast-data"))
                    .instance_scope()
            }
            _ => None,
        }
    }

    /// No home folder at all: the current directory.
    fn fallback() -> Self {
        let fallback = std::env::current_dir().unwrap_or_default();
        Self {
            config: fallback.join("siphon-config"),
            state: fallback.join("siphon-state"),
            cache: fallback.join("siphon-cache"),
        }
    }

    fn legacy_fallback() -> Self {
        let fallback = std::env::current_dir().unwrap_or_default();
        Self {
            config: fallback.join("spotifast-config"),
            state: fallback.join("spotifast-state"),
            cache: fallback.join("spotifast-cache"),
        }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config.join("settings.json")
    }

    /// Winamp skins the listener has added, as `.wsz` files or folders.
    pub fn skins_dir(&self) -> PathBuf {
        self.config.join("skins")
    }

    pub fn session_file(&self) -> PathBuf {
        self.state.join("session.json")
    }

    /// SoundCloud connection mode and public profile, never its session token.
    pub fn soundcloud_account_file(&self) -> PathBuf {
        self.state.join("soundcloud-account.json")
    }

    /// What was played here, which Spotify never hears about and so
    /// cannot tell us later. See [`crate::history`].
    pub fn history_file(&self) -> PathBuf {
        self.state.join("history.json")
    }

    /// Playlists stored only on this computer, containing either source.
    pub fn mixed_playlists_file(&self) -> PathBuf {
        self.state.join("mixed-playlists.json")
    }

    /// Local SoundCloud insertions into Spotify playlists.
    pub fn playlist_overlay_file(&self) -> PathBuf {
        self.state.join("playlist-overlay.json")
    }

    pub fn shared_web_token_file(&self) -> PathBuf {
        self.state.join("shared_web_api_token.json")
    }

    pub fn personal_web_token_file(&self) -> PathBuf {
        self.state.join("personal_web_api_token.json")
    }

    pub fn legacy_web_token_file(&self) -> PathBuf {
        self.state.join("web_api_token.json")
    }

    /// The log of the current run, replaced at every start.
    pub fn log_file(&self) -> PathBuf {
        self.state.join("siphon.log")
    }

    /// Where a panic is recorded before the process dies of it.
    pub fn panic_log(&self) -> PathBuf {
        self.state.join("panic.log")
    }

    pub fn credentials_dir(&self) -> PathBuf {
        self.state.join("credentials")
    }

    /// Optional proxy password, owner-only, never written to settings.json.
    pub fn proxy_secret_file(&self) -> PathBuf {
        self.state.join("proxy_password")
    }

    pub fn volume_dir(&self) -> PathBuf {
        self.state.join("volume")
    }

    pub fn audio_cache_dir(&self) -> PathBuf {
        self.cache.join("audio")
    }

    pub fn art_cache_dir(&self) -> PathBuf {
        self.cache.join("art")
    }

    pub fn lyrics_cache_dir(&self) -> PathBuf {
        self.cache.join("lyrics")
    }

    pub fn playlist_cache_dir(&self) -> PathBuf {
        self.cache.join("playlists")
    }

    pub fn account_playlist_cache_dir(&self, account_id: &str) -> PathBuf {
        self.playlist_cache_dir().join(account_id)
    }

    pub fn liked_songs_cache_file(&self, account_id: &str) -> PathBuf {
        self.cache
            .join("liked-songs")
            .join(account_file_name(account_id))
    }

    /// The account's last playlist list, shown while it is read again.
    pub fn library_cache_file(&self, account_id: &str) -> PathBuf {
        self.cache
            .join("library")
            .join(account_file_name(account_id))
    }

    /// The account's last Home shelves, shown while Home is read again.
    pub fn home_cache_file(&self, account_id: &str) -> PathBuf {
        self.cache.join("home").join(account_file_name(account_id))
    }

    /// The account snapshots the backend writes as it goes: the playlist
    /// list, Home and Liked Songs.
    pub fn account_snapshot_dirs(&self) -> [PathBuf; 3] {
        [
            self.cache.join("library"),
            self.cache.join("home"),
            self.cache.join("liked-songs"),
        ]
    }

    /// Every cache that holds an account's library: the playlist pages and
    /// the snapshots. Signing out removes them all.
    pub fn account_cache_dirs(&self) -> [PathBuf; 4] {
        let [library, home, liked] = self.account_snapshot_dirs();
        [self.playlist_cache_dir(), library, home, liked]
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        for dir in [&self.config, &self.state, &self.cache] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

/// One account's JSON file name. Hex encoding also keeps unusual account
/// IDs within the cache root.
fn account_file_name(account_id: &str) -> String {
    let account: String = account_id
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{account}.json")
}

/// The log line for the folders a copy uses, `dirs`, chosen for `mode`. An
/// installed copy says nothing.
fn describe(dirs: &AppDirs, mode: &fastframe_dirs::Mode) -> Option<(log::Level, String)> {
    Some(match mode {
        fastframe_dirs::Mode::Installed => return None,
        fastframe_dirs::Mode::Portable {
            folder,
            copied_from: None,
        } => (
            log::Level::Info,
            format!("portable copy: keeping files in {}", folder.display()),
        ),
        fastframe_dirs::Mode::Portable {
            folder,
            copied_from: Some(from),
        } => (
            log::Level::Info,
            format!(
                "portable copy: keeping files in {}, starting from a copy of the settings in {} and the state in {}",
                folder.display(),
                from.config.display(),
                from.state.display()
            ),
        ),
        fastframe_dirs::Mode::Fallback { folder, error } => (
            log::Level::Warn,
            format!(
                "portable copy, but {} is not usable ({error}); keeping files in {} and {}",
                folder.display(),
                dirs.config.display(),
                dirs.state.display()
            ),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn discovery_copies_and_syncs_off_the_drawing_thread_and_propagates_failures() {
        let root = std::env::temp_dir().join(format!(
            "siphon-startup-worker-{:016x}",
            rand::random::<u64>()
        ));
        let old: AppDirs = fastframe_dirs::Dirs::inside(&root.join("old")).into();
        let new: AppDirs = fastframe_dirs::Dirs::inside(&root.join("new")).into();
        old.ensure().unwrap();
        std::fs::write(old.history_file(), "history").unwrap();
        fastframe_file::mark_ui_thread();
        let found = discover_with(move || {
            crate::migration::profile(&old, &new)?;
            Ok(Found::given(new))
        })
        .unwrap();
        assert_eq!(
            std::fs::read(found.dirs.history_file()).unwrap(),
            b"history"
        );
        assert_eq!(
            discover_with(|| Err(std::io::ErrorKind::PermissionDenied.into()))
                .err()
                .unwrap()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn flatpak_adopts_the_original_sandbox_instead_of_the_new_xdg_roots() {
        let root = Path::new("/home/listener/.var/app/rocks.spotifast.Spotifast");
        let old = flatpak_profile(root);
        assert_eq!(old.config, root.join("config/spotifast"));
        assert_eq!(old.state, root.join(".local/state/spotifast"));
        assert_eq!(old.cache, root.join("cache/spotifast"));
    }

    #[test]
    fn an_old_portable_marker_selects_the_new_adjacent_data_and_instance_scope() {
        let root = Path::new("/usb/player");
        let selected = rebranded_portable(fastframe_dirs::Portable::Folder(
            root.join("spotifast-data"),
        ));
        let expected = fastframe_dirs::Portable::Folder(root.join("siphon-data"));
        assert_eq!(selected.folder(), expected.folder());
        assert_eq!(selected.instance_scope(), expected.instance_scope());
        assert!(
            rebranded_portable(fastframe_dirs::Portable::No)
                .folder()
                .is_none()
        );
    }

    #[test]
    fn the_log_says_which_folders_a_portable_copy_uses_and_why() {
        let installed = AppDirs::from(fastframe_dirs::Dirs::inside(Path::new("/user")));
        assert!(describe(&installed, &fastframe_dirs::Mode::Installed).is_none());
        let folder = PathBuf::from("/usb/Siphon/spotifast-data");
        let portable = AppDirs::from(fastframe_dirs::Dirs::inside(&folder));
        let (level, line) = describe(
            &portable,
            &fastframe_dirs::Mode::Portable {
                folder: folder.clone(),
                copied_from: Some(fastframe_dirs::Dirs::inside(Path::new("/user"))),
            },
        )
        .unwrap();
        assert_eq!(level, log::Level::Info);
        assert!(
            line.contains("spotifast-data") && line.contains("/user"),
            "{line}"
        );
        let (level, line) = describe(
            &installed,
            &fastframe_dirs::Mode::Fallback {
                folder,
                error: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            },
        )
        .unwrap();
        assert_eq!(level, log::Level::Warn);
        assert!(
            line.contains("not usable") && line.contains("/user"),
            "{line}"
        );
    }

    /// Installed copies, and every copy on Linux and macOS, keep the folders
    /// they always had.
    #[test]
    fn the_per_user_folders_are_separate_from_the_browser_helper() {
        let Some(project) =
            directories::ProjectDirs::from("com", ORGANIZATION, crate::brand::DATA_NAME)
        else {
            return;
        };
        let dirs = AppDirs::installed();
        assert_eq!(dirs.config, project.config_dir());
        assert_eq!(
            dirs.state,
            project.state_dir().unwrap_or(project.data_local_dir())
        );
        assert_eq!(dirs.cache, project.cache_dir());
        #[cfg(windows)]
        {
            let base = directories::BaseDirs::new().unwrap();
            // The helper removes its entire LocalAppData/Siphon tree on
            // uninstall. The player must be a sibling, never a child of it.
            assert_eq!(dirs.config, base.config_dir().join("Siphon Player/config"));
            assert_eq!(dirs.state, base.data_local_dir().join("Siphon Player/data"));
            assert_eq!(
                dirs.cache,
                base.data_local_dir().join("Siphon Player/cache")
            );
        }
    }

    #[test]
    fn mixed_playlist_files_are_durable_state_in_each_profile() {
        for root in ["installed", "portable/spotifast-data"] {
            let dirs = AppDirs {
                config: PathBuf::from(root).join("config"),
                state: PathBuf::from(root).join("state"),
                cache: PathBuf::from(root).join("cache"),
            };
            assert_eq!(
                dirs.mixed_playlists_file(),
                dirs.state.join("mixed-playlists.json")
            );
            assert_eq!(
                dirs.playlist_overlay_file(),
                dirs.state.join("playlist-overlay.json")
            );
        }
    }
}
