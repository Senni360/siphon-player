//! Optional SoundCloud connections. Anonymous search and playback need none.
//!
//! Implement [`Resolver`] on the API client. Capture a [`Connection`] when
//! dispatching each connect or restore action, then await it on the runtime.
//! Accept its result only while [`Connection::current`] is true. Apply that
//! result after drawing, snapshot
//! [`State::from_account`], and hand it to the app's file writer. This module
//! does not depend on the API client's wire types or perform HTTP itself.

use std::future::Future;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::credentials::{self, Lease, Slot, Store};

/// Public identity. `username` is the display name; `permalink` is the slug.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub user_id: u64,
    pub username: String,
    pub permalink: String,
    #[serde(default)]
    pub avatar: Option<String>,
}

impl Profile {
    fn valid(&self) -> bool {
        self.user_id != 0 && !self.username.trim().is_empty() && valid_permalink(&self.permalink)
    }
}

/// Usable only in memory or the protected credential entry. No Serialize or
/// Debug implementation, so ordinary state and diagnostics cannot expose it.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionToken(String);

impl SessionToken {
    /// Accept a cookie value or `OAuth <value>`, never an entire cookie/header
    /// dump. This checks syntax only; `/me` decides whether the token works.
    pub fn parse(input: &str) -> Result<Self, InputError> {
        let input = input.trim();
        let value = match input.split_once(char::is_whitespace) {
            Some((prefix, rest)) if prefix.eq_ignore_ascii_case("OAuth") => rest.trim(),
            _ => input,
        };
        let body = value.trim_end_matches('=');
        if !(16..=4096).contains(&value.len())
            || body.is_empty()
            || !body.bytes().any(|b| b.is_ascii_alphabetic())
            || !body.bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'+' | b'/')
            })
            || value.eq_ignore_ascii_case("your_session_token")
            || value.eq_ignore_ascii_case("your_access_token")
        {
            return Err(InputError::Token);
        }
        Ok(Self(value.to_owned()))
    }

    /// For the API client's sensitive Authorization header only. Never log it
    /// or put it in a URL, error, ordinary state file or request diagnostics.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// No serialization or Debug: a session contains a usable secret.
#[derive(Clone, Default, PartialEq, Eq)]
pub enum Account {
    #[default]
    Disconnected,
    Profile(Profile),
    /// The identity must come from `/me`, never from a pasted profile name.
    Session {
        user: Profile,
        token: SessionToken,
    },
}

impl Account {
    pub fn user(&self) -> Option<&Profile> {
        match self {
            Self::Disconnected => None,
            Self::Profile(user) | Self::Session { user, .. } => Some(user),
        }
    }

    pub fn token(&self) -> Option<&SessionToken> {
        match self {
            Self::Session { token, .. } => Some(token),
            _ => None,
        }
    }

    /// Immediately forget the in-memory account and invalidate pending work.
    /// Queue `State::from_account(self).save(...)` on the UI thread afterwards,
    /// and await this future on the runtime. It records a durable revocation
    /// marker before deleting the protected entry. Report any returned error.
    /// No remote sign-out: that would also end the shared browser session.
    pub fn sign_out(
        &mut self,
        store: &Store,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), credentials::Error>> + Send>> {
        *self = Self::Disconnected;
        store.invalidate(Slot::SoundCloud);
        store.lease(Slot::SoundCloud).clear_secret(false)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileInput {
    Permalink(String),
    /// Resolve on the runtime, then require a SoundCloud profile as the target.
    /// Do not send credentials to the short-link host or arbitrary redirects.
    ShortLink(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InputError {
    #[error("Enter a SoundCloud profile URL or username.")]
    Profile,
    #[error("Paste only the SoundCloud oauth_token cookie value.")]
    Token,
}

impl ProfileInput {
    pub fn parse(input: &str) -> Result<Self, InputError> {
        let input = input.trim();
        let bare = input.strip_prefix('@').unwrap_or(input);
        if valid_permalink(bare) {
            return Ok(Self::Permalink(bare.to_ascii_lowercase()));
        }
        let with_scheme;
        let address = if input.contains("://") {
            input
        } else {
            with_scheme = format!("https://{}", input.trim_start_matches("//"));
            &with_scheme
        };
        let url = reqwest::Url::parse(address).map_err(|_| InputError::Profile)?;
        if !matches!(url.scheme(), "https" | "http")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
        {
            return Err(InputError::Profile);
        }
        let segments: Vec<_> = url
            .path_segments()
            .ok_or(InputError::Profile)?
            .filter(|part| !part.is_empty())
            .collect();
        match url.host_str() {
            Some("on.soundcloud.com") if segments.len() == 1 => {
                if !segments[0].bytes().all(|b| b.is_ascii_alphanumeric()) {
                    return Err(InputError::Profile);
                }
                Ok(Self::ShortLink(format!(
                    "https://on.soundcloud.com/{}",
                    segments[0]
                )))
            }
            Some("soundcloud.com" | "www.soundcloud.com" | "m.soundcloud.com") => {
                let slug = segments.first().ok_or(InputError::Profile)?;
                let profile_tab = segments.len() == 2
                    && matches!(
                        segments[1],
                        "tracks"
                            | "albums"
                            | "sets"
                            | "reposts"
                            | "likes"
                            | "following"
                            | "followers"
                    );
                if !valid_permalink(slug) || (segments.len() != 1 && !profile_tab) {
                    return Err(InputError::Profile);
                }
                Ok(Self::Permalink(slug.to_ascii_lowercase()))
            }
            _ => Err(InputError::Profile),
        }
    }
}

fn valid_permalink(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "you"
                | "discover"
                | "search"
                | "stream"
                | "charts"
                | "upload"
                | "settings"
                | "signin"
                | "logout"
                | "terms"
                | "pages"
        )
}

/// The API client supplies these two operations. Resolve public profiles with
/// the anonymous client ID even if the client currently has a session. For
/// `me`, send `Authorization: OAuth <token>` only to the SoundCloud API origin.
/// Map wire errors to these fixed variants; never include response bodies.
pub trait Resolver: Sync {
    fn resolve_profile(
        &self,
        input: &ProfileInput,
    ) -> impl Future<Output = Result<Profile, ResolveError>> + Send;

    fn me(
        &self,
        token: &SessionToken,
    ) -> impl Future<Output = Result<Profile, ResolveError>> + Send;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    #[error("SoundCloud could not find that profile.")]
    NotFound,
    #[error(
        "SoundCloud rejected this session. Paste a current session token or connect by profile."
    )]
    Unauthorized,
    #[error("SoundCloud could not be reached. Try again.")]
    Unavailable,
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    #[error("Could not update the protected SoundCloud session.")]
    Storage(#[from] credentials::Error),
    #[error("SoundCloud returned an invalid profile.")]
    InvalidProfile,
    #[error("The saved SoundCloud session is invalid. Connect again.")]
    InvalidSession,
    #[error("The saved SoundCloud session belongs to a different account. Connect again.")]
    DifferentAccount,
}

/// A public profile needs no protected store. Apply `Account::Profile(user)`
/// even if old session cleanup failed, and show `cleanup_warning` when present.
/// The public account carries no token; its saved mode never restores one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectedProfile {
    pub user: Profile,
    pub cleanup_warning: Option<credentials::Error>,
}

/// One authorization generation, captured before network work starts. Keep
/// this beside its result so the event consumer can reject stale completions.
#[derive(Clone)]
pub struct Connection {
    lease: Lease,
}

impl Connection {
    /// Call for a new connect/restore action, not for ordinary API requests.
    pub fn begin(store: &Store) -> Self {
        store.invalidate(Slot::SoundCloud);
        Self {
            lease: store.lease(Slot::SoundCloud),
        }
    }

    pub fn current(&self) -> bool {
        self.lease.current()
    }

    fn check(&self) -> Result<(), ConnectError> {
        if self.current() {
            Ok(())
        } else {
            Err(credentials::Error::Stale.into())
        }
    }

    pub async fn connect_profile(
        &self,
        resolver: &impl Resolver,
        input: &ProfileInput,
    ) -> Result<ConnectedProfile, ConnectError> {
        self.check()?;
        let user = resolver.resolve_profile(input).await?;
        if !user.valid() {
            return Err(ConnectError::InvalidProfile);
        }
        // A profile connection must not silently retain a previous account's
        // privileges, including when switching out of session mode.
        // Public information must remain usable without an unlocked keyring.
        // Surface failed cleanup, while still rejecting superseded lookups.
        let cleanup_warning = self.lease.clear_secret(true).await.err();
        self.check()?;
        Ok(ConnectedProfile {
            user,
            cleanup_warning,
        })
    }

    pub async fn connect_session(
        &self,
        resolver: &impl Resolver,
        token: SessionToken,
    ) -> Result<Account, ConnectError> {
        self.check()?;
        let user = resolver.me(&token).await?;
        if !user.valid() {
            return Err(ConnectError::InvalidProfile);
        }
        self.lease
            .save_secret(token.expose().as_bytes().to_vec())
            .await?;
        self.check()?;
        Ok(Account::Session { user, token })
    }

    /// Read `State` off the UI thread first. Session restoration verifies the
    /// protected token through `/me` and requires the saved account identity.
    /// An absent or locally revoked token restores as disconnected. A rejected
    /// token returns an error, leaving anonymous search and playback available.
    pub async fn restore(
        &self,
        state: &State,
        resolver: &impl Resolver,
    ) -> Result<Account, ConnectError> {
        self.check()?;
        if state.version != 1 {
            return Err(ConnectError::InvalidProfile);
        }
        let result = match &state.account {
            SavedAccount::Disconnected => Account::Disconnected,
            SavedAccount::Profile { user } => {
                if !user.valid() {
                    return Err(ConnectError::InvalidProfile);
                }
                Account::Profile(user.clone())
            }
            SavedAccount::Session { user: saved } => {
                let Some(bytes) = self.lease.load_secret().await? else {
                    return Ok(Account::Disconnected);
                };
                let text = std::str::from_utf8(&bytes).map_err(|_| ConnectError::InvalidSession)?;
                let token = SessionToken::parse(text).map_err(|_| ConnectError::InvalidSession)?;
                let user = resolver.me(&token).await?;
                if !user.valid() {
                    return Err(ConnectError::InvalidProfile);
                }
                if saved.user_id != user.user_id {
                    return Err(ConnectError::DifferentAccount);
                }
                Account::Session { user, token }
            }
        };
        self.check()?;
        Ok(result)
    }
}

/// The entire ordinary JSON format. It cannot hold a token. Missing fields
/// retain defaults, unknown fields are ignored, and unknown versions fail.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    version: u32,
    account: SavedAccount,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
enum SavedAccount {
    #[default]
    Disconnected,
    Profile {
        user: Profile,
    },
    Session {
        user: Profile,
    },
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: 1,
            account: SavedAccount::Disconnected,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StateError {
    #[error("The saved SoundCloud profile could not be read.")]
    Read,
    #[error("The saved SoundCloud profile is invalid or uses a newer format.")]
    Invalid,
}

impl State {
    pub fn from_account(account: &Account) -> Self {
        Self {
            account: match account {
                Account::Disconnected => SavedAccount::Disconnected,
                Account::Profile(user) => SavedAccount::Profile { user: user.clone() },
                Account::Session { user, .. } => SavedAccount::Session { user: user.clone() },
            },
            ..Self::default()
        }
    }

    pub fn decode(text: &str) -> Result<Self, StateError> {
        let state: Self = serde_json::from_str(text).map_err(|_| StateError::Invalid)?;
        let valid = match &state.account {
            SavedAccount::Disconnected => true,
            SavedAccount::Profile { user } | SavedAccount::Session { user } => user.valid(),
        };
        if state.version != 1 || !valid {
            return Err(StateError::Invalid);
        }
        Ok(state)
    }

    /// Read during startup or on a blocking worker, never while drawing.
    pub fn load(path: &Path) -> Result<Self, StateError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::decode(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(_) => Err(StateError::Read),
        }
    }

    pub fn encode(&self) -> Result<String, StateError> {
        serde_json::to_string_pretty(self).map_err(|_| StateError::Invalid)
    }

    /// Encode on the UI thread; the app's writer replaces the file atomically
    /// off it. Use the failure callback to report the error and retry later.
    pub fn save(
        &self,
        files: &fastframe_file::Writer,
        path: &Path,
        on_failure: impl FnOnce() + Send + 'static,
    ) -> Result<(), StateError> {
        files.write_or(
            path,
            self.encode()?,
            fastframe_file::Access::Private,
            on_failure,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::AppDirs;
    use std::sync::atomic::{AtomicU64, Ordering};

    const TOKEN: &str = "2-000000-0000000000-dummy_session_token";

    fn user() -> Profile {
        Profile {
            user_id: 42,
            username: "A Display Name".into(),
            permalink: "a-name".into(),
            avatar: Some("https://i1.sndcdn.com/avatars-example-large.jpg".into()),
        }
    }

    struct FakeResolver;
    impl Resolver for FakeResolver {
        async fn resolve_profile(&self, _: &ProfileInput) -> Result<Profile, ResolveError> {
            Ok(user())
        }
        async fn me(&self, token: &SessionToken) -> Result<Profile, ResolveError> {
            if token.expose() == TOKEN {
                Ok(user())
            } else {
                Err(ResolveError::Unauthorized)
            }
        }
    }

    struct Fixture {
        dirs: AppDirs,
        store: Store,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::current_dir()
                .unwrap()
                .join(".cache")
                .join(format!(
                    "soundcloud-account-tests-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
            let dirs = AppDirs {
                config: root.join("config"),
                state: root.join("state"),
                cache: root.join("cache"),
            };
            dirs.ensure().unwrap();
            let store = Store::in_memory(dirs.clone());
            Self { dirs, store }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.dirs.state.parent().unwrap());
        }
    }

    #[test]
    fn profile_inputs_accept_urls_mobile_tabs_and_usernames() {
        for input in [
            "a-name",
            " @a-name ",
            "https://soundcloud.com/a-name",
            "http://www.soundcloud.com/a-name/",
            "m.soundcloud.com/a-name?utm_source=test",
            "//soundcloud.com/A-Name/likes#top",
        ] {
            assert_eq!(
                ProfileInput::parse(input),
                Ok(ProfileInput::Permalink("a-name".into())),
                "{input}"
            );
        }
        assert_eq!(
            ProfileInput::parse("https://on.soundcloud.com/AbCd123?si=tracking"),
            Ok(ProfileInput::ShortLink(
                "https://on.soundcloud.com/AbCd123".into()
            ))
        );
        for input in [
            "",
            "a name",
            "https://example.com/a-name",
            "https://soundcloud.com.evil.test/a-name",
            "https://soundcloud.com@evil.test/a-name",
            "https://name@soundcloud.com/a-name",
            "https://soundcloud.com:444/a-name",
            "https://soundcloud.com/",
            "https://soundcloud.com/you/likes",
            "https://soundcloud.com/a-name/a-track",
            "https://soundcloud.com/a-name/sets/a-playlist",
            "https://on.soundcloud.com/",
            "file:///a-name",
            "https://soundcloud.com/a%2fname",
        ] {
            assert!(ProfileInput::parse(input).is_err(), "{input}");
        }
    }

    #[test]
    fn session_input_accepts_only_a_token_value_or_oauth_prefix() {
        for input in [
            TOKEN.to_owned(),
            format!("  {TOKEN}\n"),
            format!("OAuth {TOKEN}"),
            format!("oauth   {TOKEN}"),
        ] {
            assert!(SessionToken::parse(&input).unwrap().expose() == TOKEN);
        }
        for input in [
            "",
            "OAuth ",
            "undefined",
            "null",
            "your_session_token",
            "12345678901234567890",
            "oauth_token=dummy_session_token",
            "Authorization: OAuth dummy_session_token",
            "https://soundcloud.com/name",
            "dummy_session_token; Path=/",
            "dummy_session_token\r\nInjected: header",
            "Bearer dummy_session_token",
            "dummy=token_value",
        ] {
            assert!(SessionToken::parse(input).is_err());
        }
    }

    #[test]
    fn state_round_trips_all_modes_without_secrets() {
        for account in [
            Account::Disconnected,
            Account::Profile(user()),
            Account::Session {
                user: user(),
                token: SessionToken::parse(TOKEN).unwrap(),
            },
        ] {
            let state = State::from_account(&account);
            let json = state.encode().unwrap();
            assert!(!json.contains(TOKEN));
            assert!(!json.contains("token"));
            assert_eq!(State::decode(&json).unwrap(), state);
        }
    }

    #[test]
    fn state_accepts_missing_optional_fields_and_rejects_invalid_versions() {
        assert_eq!(State::decode("{}").unwrap(), State::default());
        let state = State::decode(r#"{"account":{"mode":"profile","user":{"user_id":42,"username":"Name","permalink":"a-name"}},"future_field":true}"#).unwrap();
        assert!(matches!(
            state.account,
            SavedAccount::Profile {
                user: Profile { avatar: None, .. }
            }
        ));
        for json in [
            r#"{"version":2}"#,
            r#"{"account":{"mode":"new-mode"}}"#,
            r#"{"account":{"mode":"profile","user":{"user_id":0,"username":"Name","permalink":"a-name"}}}"#,
            "not json",
        ] {
            assert_eq!(State::decode(json), Err(StateError::Invalid));
        }
        // An older or hand-edited file cannot sneak a secret into later saves.
        let state = State::decode(
            r#"{"token":"old-secret","account":{"mode":"disconnected","token":"old-secret"}}"#,
        )
        .unwrap();
        assert!(!state.encode().unwrap().contains("old-secret"));
    }

    #[tokio::test]
    async fn session_is_verified_stored_restored_and_forgotten() {
        let f = Fixture::new();
        let connection = Connection::begin(&f.store);
        let mut account = connection
            .connect_session(&FakeResolver, SessionToken::parse(TOKEN).unwrap())
            .await
            .unwrap();
        let state = State::from_account(&account);
        let files = fastframe_file::Writer::new();
        state
            .save(&files, &f.dirs.soundcloud_account_file(), || {
                panic!("state write failed")
            })
            .unwrap();
        assert!(files.flush(fastframe_file::FLUSH_TIMEOUT));
        assert_eq!(
            State::load(&f.dirs.soundcloud_account_file()).unwrap(),
            state
        );
        let restored = Connection::begin(&f.store)
            .restore(&state, &FakeResolver)
            .await
            .unwrap();
        assert!(restored == account);
        let pending = Connection::begin(&f.store);
        let clearing = account.sign_out(&f.store);
        assert!(account == Account::Disconnected);
        assert!(!pending.current());
        clearing.await.unwrap();
        assert!(
            f.store
                .lease(Slot::SoundCloud)
                .load_secret()
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            Connection::begin(&f.store)
                .restore(&state, &FakeResolver)
                .await
                .unwrap()
                == Account::Disconnected
        );
        State::from_account(&account)
            .save(&files, &f.dirs.soundcloud_account_file(), || {
                panic!("state write failed")
            })
            .unwrap();
        assert!(files.flush(fastframe_file::FLUSH_TIMEOUT));
        assert_eq!(
            State::load(&f.dirs.soundcloud_account_file()).unwrap(),
            State::default()
        );
    }

    #[tokio::test]
    async fn switching_to_a_public_profile_removes_session_privileges() {
        let f = Fixture::new();
        Connection::begin(&f.store)
            .connect_session(&FakeResolver, SessionToken::parse(TOKEN).unwrap())
            .await
            .unwrap();
        let connected = Connection::begin(&f.store)
            .connect_profile(&FakeResolver, &ProfileInput::parse("a-name").unwrap())
            .await
            .unwrap();
        assert_eq!(connected.cleanup_warning, None);
        let account = Account::Profile(connected.user);
        assert!(matches!(account, Account::Profile(_)));
        assert!(account.token().is_none());
        assert!(
            f.store
                .lease(Slot::SoundCloud)
                .load_secret()
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_saved_public_profile_restores_without_a_session_or_network() {
        struct Offline;
        impl Resolver for Offline {
            async fn resolve_profile(&self, _: &ProfileInput) -> Result<Profile, ResolveError> {
                panic!("a saved public profile needs no lookup");
            }
            async fn me(&self, _: &SessionToken) -> Result<Profile, ResolveError> {
                panic!("a public profile must not use /me");
            }
        }
        let f = Fixture::new();
        let account = Account::Profile(user());
        let state = State::decode(&State::from_account(&account).encode().unwrap()).unwrap();
        let restored = Connection::begin(&f.store)
            .restore(&state, &Offline)
            .await
            .unwrap();
        assert!(restored == account);
        assert!(restored.token().is_none());
    }

    #[tokio::test]
    async fn restoring_a_session_does_not_reuse_another_accounts_profile() {
        let f = Fixture::new();
        Connection::begin(&f.store)
            .connect_session(&FakeResolver, SessionToken::parse(TOKEN).unwrap())
            .await
            .unwrap();
        let mut other = user();
        other.user_id += 1;
        let state = State {
            account: SavedAccount::Session { user: other },
            ..State::default()
        };
        assert!(matches!(
            Connection::begin(&f.store)
                .restore(&state, &FakeResolver)
                .await,
            Err(ConnectError::DifferentAccount)
        ));
    }

    #[tokio::test]
    async fn invalid_sessions_and_superseded_connections_cannot_be_saved() {
        let f = Fixture::new();
        let old = Connection::begin(&f.store);
        let current = Connection::begin(&f.store);
        assert!(matches!(
            old.connect_session(&FakeResolver, SessionToken::parse(TOKEN).unwrap())
                .await,
            Err(ConnectError::Storage(credentials::Error::Stale))
        ));
        assert!(matches!(
            current
                .connect_session(
                    &FakeResolver,
                    SessionToken::parse("invalid_session_token").unwrap()
                )
                .await,
            Err(ConnectError::Resolve(ResolveError::Unauthorized))
        ));
        assert!(
            f.store
                .lease(Slot::SoundCloud)
                .load_secret()
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            State::load(&f.dirs.soundcloud_account_file()).unwrap(),
            State::default()
        );
    }

    struct PendingResolver {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    impl Resolver for PendingResolver {
        async fn resolve_profile(&self, _: &ProfileInput) -> Result<Profile, ResolveError> {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(user())
        }

        async fn me(&self, _: &SessionToken) -> Result<Profile, ResolveError> {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(user())
        }
    }

    #[tokio::test]
    async fn a_late_me_response_cannot_undo_sign_out() {
        let f = Fixture::new();
        let pending = PendingResolver {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        };
        let connection = Connection::begin(&f.store);
        let mut account = Account::Disconnected;
        let (result, ()) = tokio::join!(
            connection.connect_session(&pending, SessionToken::parse(TOKEN).unwrap()),
            async {
                pending.entered.notified().await;
                account.sign_out(&f.store).await.unwrap();
                pending.release.notify_one();
            }
        );
        assert!(matches!(
            result,
            Err(ConnectError::Storage(credentials::Error::Stale))
        ));
        assert!(account == Account::Disconnected);
        assert!(
            f.store
                .lease(Slot::SoundCloud)
                .load_secret()
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_late_public_profile_cannot_clear_a_new_session() {
        let f = Fixture::new();
        let pending = PendingResolver {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        };
        let connection = Connection::begin(&f.store);
        let input = ProfileInput::parse("a-name").unwrap();
        let (result, ()) = tokio::join!(connection.connect_profile(&pending, &input), async {
            pending.entered.notified().await;
            Connection::begin(&f.store)
                .connect_session(&FakeResolver, SessionToken::parse(TOKEN).unwrap())
                .await
                .unwrap();
            pending.release.notify_one();
        });
        assert!(matches!(
            result,
            Err(ConnectError::Storage(credentials::Error::Stale))
        ));
        assert!(
            f.store
                .lease(Slot::SoundCloud)
                .load_secret()
                .await
                .unwrap()
                .as_deref()
                == Some(TOKEN.as_bytes())
        );
    }
}
