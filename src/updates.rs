//! Self-update from GitHub releases, through fastframe-update.
//!
//! The previous release's helper installs the first release built on the
//! crate, so what it relies on stays the same: `--version` prints
//! `<command> <version>`, the new app accepts `--update-receipt` and
//! `--update-error` (stripped by [`fastframe_update::intercept`] before the
//! argument parser), and the handoff and receipt files keep their format.

pub use fastframe_update::{
    CHECK_INTERVAL, DownloadState, Installation, Kind, Prepared, Release, Source, Unsupported,
    Updater,
};
use fastframe_update::{MacConfig, Prereleases, ReqwestTransport, UpdateConfig};

pub const CONFIG: UpdateConfig = UpdateConfig {
    macos: MacConfig {
        bundle_ids: &["com.siphon.player"],
        executable_names: &["Encore"],
        legacy_bundle_names: &[],
    },
    // Siphon's own release key, which signs every release's checksums.txt.
    // Verification stays mandatory. See private-updates.md.
    publisher_key: Some(include_str!("../assets/update-public-key.hex")),
    // A preview hears about the next preview, and a stable copy only about
    // stable releases.
    prereleases: Prereleases::WhenRunningPrerelease,
    // Releases build their AppImages before publishing, so the signed
    // checksums.txt lists them (#737): an AppImage replaces itself with the
    // next one.
    appimage: true,
    // Only siphon.exe updates itself. Without this, any program run from
    // beside the portable marker (a tray or yt-dlp) would have siphon.exe
    // installed over it.
    portable_executable: Some("siphon"),
    ..UpdateConfig::new(
        "Senni360/siphon-player",
        "Encore",
        "siphon",
        env!("CARGO_PKG_VERSION"),
    )
};

/// What the update engine asks of its client: 15 seconds to connect, and 15
/// minutes for the whole download.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// An updater on Siphon's HTTP client, through the configured proxy.
pub fn updater(proxy: &crate::settings::ProxyConfig) -> anyhow::Result<Updater> {
    // A dropped connection must end the download, or the dialog stays on
    // Downloading and refuses every retry until the app restarts.
    let builder = crate::http::blocking_builder(proxy)
        .map_err(anyhow::Error::msg)?
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(DOWNLOAD_TIMEOUT);
    Ok(Updater::new(CONFIG, ReqwestTransport::new(builder)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_config_is_valid() {
        CONFIG.validate().unwrap();
        assert_eq!(CONFIG.current_version, env!("CARGO_PKG_VERSION"));
        // Only the app itself may update, never a program beside it.
        assert_eq!(CONFIG.portable_executable, Some("siphon"));
    }

    #[test]
    fn updates_require_the_release_signing_key() {
        assert!(
            CONFIG.publisher_key.is_some(),
            "updates must require a publisher signature"
        );
        CONFIG.validate().unwrap();
    }

    #[test]
    fn update_checks_use_the_private_repository_without_upstream_fallback() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct MissingPrivateRelease(Arc<AtomicUsize>);
        impl fastframe_update::Transport for MissingPrivateRelease {
            fn get(
                &self,
                request: &fastframe_update::Request<'_>,
            ) -> anyhow::Result<fastframe_update::Response> {
                // A preview lists every release; a stable build reads the latest.
                let expected = if env!("CARGO_PKG_VERSION").contains('-') {
                    "https://api.github.com/repos/Senni360/siphon-player/releases?per_page=100&page=1"
                } else {
                    "https://api.github.com/repos/Senni360/siphon-player/releases/latest"
                };
                assert_eq!(request.url, expected);
                self.0.fetch_add(1, Ordering::Relaxed);
                Ok(fastframe_update::Response {
                    status: 404,
                    location: None,
                    body: Box::new(std::io::empty()),
                })
            }
        }

        let requests = Arc::new(AtomicUsize::new(0));
        let updater = Updater::new(CONFIG, MissingPrivateRelease(Arc::clone(&requests)));
        assert!(updater.check().is_err());
        assert_eq!(requests.load(Ordering::Relaxed), 1);
        assert_eq!(
            env!("CARGO_PKG_REPOSITORY"),
            format!("https://github.com/{}", CONFIG.repository)
        );
    }

    #[test]
    fn previews_follow_the_next_preview() {
        assert_eq!(CONFIG.prereleases, Prereleases::WhenRunningPrerelease);
    }

    #[test]
    fn appimages_update_themselves() {
        const { assert!(CONFIG.appimage) };
    }
}
