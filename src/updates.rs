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
use fastframe_update::{MacConfig, ReqwestTransport, UpdateConfig};

pub const CONFIG: UpdateConfig = UpdateConfig {
    macos: MacConfig {
        bundle_ids: &["com.siphon.player"],
        executable_names: &["Siphon"],
        legacy_bundle_names: &[],
    },
    // Keep signature verification mandatory. The inherited key remains until
    // the owner configures a signed private release channel and key rotation;
    // candidate artifacts are not update releases. See private-updates.md.
    publisher_key: Some(include_str!("../assets/update-public-key.hex")),
    // Releases build their AppImages before publishing, so the signed
    // checksums.txt lists them (#737): an AppImage replaces itself with the
    // next one.
    appimage: true,
    ..UpdateConfig::new(
        "Senni360/siphon-player",
        "Siphon",
        "siphon",
        env!("CARGO_PKG_VERSION"),
    )
};

/// An updater on Siphon's HTTP client, through the configured proxy.
pub fn updater(proxy: &crate::settings::ProxyConfig) -> anyhow::Result<Updater> {
    let builder = crate::http::blocking_builder(proxy).map_err(anyhow::Error::msg)?;
    Ok(Updater::new(CONFIG, ReqwestTransport::new(builder)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_config_is_valid() {
        CONFIG.validate().unwrap();
        assert_eq!(CONFIG.current_version, env!("CARGO_PKG_VERSION"));
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
                assert_eq!(
                    request.url,
                    "https://api.github.com/repos/Senni360/siphon-player/releases/latest"
                );
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

    /// An AppImage may replace itself only because the release workflow
    /// builds the AppImages into the signed checksums.txt;
    /// packaging/test-release-names.py checks that side.
    #[test]
    fn appimages_update_themselves() {
        const { assert!(CONFIG.appimage) };
    }
}
