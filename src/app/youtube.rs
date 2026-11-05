//! Local YouTube library state and stale-result guards for the existing views.
use super::*;
use crate::backend::youtube_account::{Request as AccountRequest, Response as AccountResponse};
use crate::youtube::likes::{Likes, Restored, Writes};

#[derive(Default)]
pub(super) struct State {
    pub generation: u64,
    account: Option<String>,
    likes: Loadable<Likes>,
    writes: std::sync::Arc<Writes>,
    pub(super) write_revision: u64,
    save_error: bool,
    restore_warning: bool,
    /// The connected YouTube account, if there is one.
    connected: Option<crate::youtube::account::Profile>,
    /// Asks of the connection go stale past a sign-out.
    connection: u64,
    restore_asked: bool,
    last_sync: Option<std::time::Instant>,
    /// Changes the listener made, and those not answered yet. A sync read before
    /// one of them would undo it on screen, so it is asked again instead.
    edits: u64,
    writes_in_flight: u32,
    sync_again: bool,
    /// Follows pressed before the library was read, applied when it is.
    queued_follows: Vec<crate::youtube::likes::Channel>,
}

/// How often the account is read again when its pages are opened.
const SYNC_EVERY: std::time::Duration = std::time::Duration::from_secs(5 * 60);
/// How many local follows and likes one press sends up. Each costs 50 of the
/// project's 10,000 daily units.
const MOST_SENT_AT_ONCE: usize = 40;

impl App {
    /// Install explicitly invented library states without disk or helper work.
    #[cfg(any(test, feature = "demo"))]
    pub fn set_demo_youtube_likes(&mut self, likes: Loadable<Likes>) {
        self.youtube.generation = self.youtube.generation.wrapping_add(1);
        self.youtube.account = self.user_id().map(str::to_owned);
        self.youtube.likes = likes;
        self.youtube.save_error = false;
        self.youtube.restore_warning = false;
        self.refresh_youtube_likes_page();
    }

    pub fn youtube_likes(&self) -> Option<&[Track]> {
        if self.youtube.account.as_deref() != self.user_id() {
            return None;
        }
        self.youtube
            .likes
            .get()
            .map(|likes| likes.tracks.as_slice())
    }

    /// The channels followed here, newest first.
    pub fn youtube_channels(&self) -> &[crate::youtube::likes::Channel] {
        if self.youtube.account.as_deref() != self.user_id() {
            return &[];
        }
        self.youtube
            .likes
            .get()
            .map_or(&[], |likes| likes.channels.as_slice())
    }

    pub fn follows_youtube_channel(&self, id: &str) -> bool {
        self.youtube_channels()
            .iter()
            .any(|channel| channel.id == id)
            || self
                .youtube
                .queued_follows
                .iter()
                .any(|channel| channel.id == id)
    }

    /// The connected YouTube account, if there is one.
    pub fn youtube_account(&self) -> Option<&crate::youtube::account::Profile> {
        self.youtube.connected.as_ref()
    }

    /// How many follows and likes are kept only here, while connected.
    pub fn youtube_local_only(&self) -> usize {
        self.youtube.likes.get().map_or(0, |likes| {
            likes
                .channels
                .iter()
                .filter(|each| each.subscription.is_none())
                .count()
                + likes.local_likes().len()
        })
    }

    pub(super) fn connect_youtube(&mut self) {
        let Some(Dialog::YouTubeConnect(draft)) = &mut self.dialog else {
            return;
        };
        let Some(keys) =
            crate::youtube::account::ClientKeys::parse(&draft.client_id, &draft.client_secret)
        else {
            draft.error = Some(
                "Paste the Client ID, which ends in .apps.googleusercontent.com, and the Client secret."
                    .into(),
            );
            return;
        };
        draft.client_secret.clear();
        draft.busy = true;
        draft.error = None;
        self.youtube.connection = self.youtube.connection.wrapping_add(1);
        self.backend
            .send(Command::YouTubeAccount(AccountRequest::Connect {
                generation: self.youtube.connection,
                keys,
            }));
    }

    pub(super) fn disconnect_youtube(&mut self) {
        self.youtube.connection = self.youtube.connection.wrapping_add(1);
        self.youtube.connected = None;
        self.youtube.last_sync = None;
        self.backend
            .send(Command::YouTubeAccount(AccountRequest::CancelConnect));
        self.backend
            .send(Command::YouTubeAccount(AccountRequest::Disconnect));
        // What was synced stays as local follows and likes, not tied to the account.
        if let Some(likes) = self.youtube.likes.get_mut() {
            for channel in &mut likes.channels {
                channel.subscription = None;
            }
            likes.remote_likes.clear();
        }
        self.save_youtube_likes();
    }

    /// Reads the account, unless that was done a moment ago or `force`d.
    pub(super) fn sync_youtube(&mut self, force: bool) {
        if self.youtube.connected.is_none() || self.youtube.likes.get().is_none() {
            return;
        }
        if !force
            && self
                .youtube
                .last_sync
                .is_some_and(|last| last.elapsed() < SYNC_EVERY)
        {
            return;
        }
        if self.youtube.writes_in_flight > 0 {
            self.youtube.sync_again = true;
            return;
        }
        self.youtube.last_sync = Some(std::time::Instant::now());
        self.backend
            .send(Command::YouTubeAccount(AccountRequest::Sync {
                generation: self.youtube.connection,
                serial: self.youtube.edits,
            }));
    }

    /// Sends up what is kept only here: an explicit press, as it changes the
    /// account and spends the project's daily quota.
    pub(super) fn send_youtube_local_up(&mut self) {
        let Some(likes) = self.youtube.likes.get() else {
            return;
        };
        if self.youtube.connected.is_none() {
            return;
        }
        let channels: Vec<_> = likes
            .channels
            .iter()
            .filter(|each| each.subscription.is_none())
            .take(MOST_SENT_AT_ONCE)
            .cloned()
            .collect();
        let tracks: Vec<_> = likes
            .local_likes()
            .into_iter()
            .take(MOST_SENT_AT_ONCE - channels.len())
            .collect();
        for channel in channels {
            self.send_follow(channel, true);
        }
        for track in tracks {
            self.send_rating(track, true);
        }
    }

    fn send_follow(&mut self, channel: crate::youtube::likes::Channel, subscribe: bool) {
        self.youtube.edits = self.youtube.edits.wrapping_add(1);
        self.youtube.writes_in_flight += 1;
        self.backend
            .send(Command::YouTubeAccount(AccountRequest::Subscribe {
                generation: self.youtube.connection,
                channel,
                subscribe,
            }));
    }

    fn send_rating(&mut self, track: Track, liked: bool) {
        let Some(video) = crate::youtube::video_id(&track.uri).map(str::to_owned) else {
            return;
        };
        self.youtube.edits = self.youtube.edits.wrapping_add(1);
        self.youtube.writes_in_flight += 1;
        self.backend
            .send(Command::YouTubeAccount(AccountRequest::Rate {
                generation: self.youtube.connection,
                track: Box::new(track),
                video,
                liked,
            }));
    }

    fn youtube_write_answered(&mut self) {
        self.youtube.writes_in_flight = self.youtube.writes_in_flight.saturating_sub(1);
        if self.youtube.writes_in_flight == 0 && std::mem::take(&mut self.youtube.sync_again) {
            self.sync_youtube(true);
        }
    }

    fn youtube_account_failed(&mut self, error: &crate::youtube::account::Error) {
        if *error == crate::youtube::account::Error::Revoked {
            self.disconnect_youtube();
        }
        self.toast_error(error.to_string());
    }

    pub(super) fn handle_youtube_account(&mut self, response: AccountResponse) {
        use crate::youtube::likes::Channel;
        let generation = match &response {
            AccountResponse::Connected { generation, .. }
            | AccountResponse::Restored { generation, .. }
            | AccountResponse::Synced { generation, .. }
            | AccountResponse::Subscribed { generation, .. }
            | AccountResponse::Rated { generation, .. } => *generation,
        };
        if generation != self.youtube.connection {
            return;
        }
        match response {
            AccountResponse::Connected { result, .. } => match result {
                Ok(profile) => {
                    self.toast(format!("Connected to YouTube as {}", profile.name));
                    self.youtube.connected = Some(profile);
                    if matches!(self.dialog, Some(Dialog::YouTubeConnect(_))) {
                        self.dialog = None;
                    }
                    self.sync_youtube(true);
                }
                Err(error) => {
                    if let Some(Dialog::YouTubeConnect(draft)) = &mut self.dialog {
                        draft.busy = false;
                        draft.error = Some(error);
                    } else {
                        self.toast_error(error);
                    }
                }
            },
            AccountResponse::Restored { result, .. } => match result {
                Ok(profile) => {
                    self.youtube.connected = profile;
                    self.sync_youtube(true);
                }
                Err(error) => self.toast_error(error),
            },
            AccountResponse::Synced { serial, result, .. } => {
                // Read before something the listener did: holding what they see
                // and asking again keeps it from flickering back.
                if serial != self.youtube.edits {
                    if self.youtube.writes_in_flight > 0 {
                        self.youtube.sync_again = true;
                    } else {
                        self.sync_youtube(true);
                    }
                    return;
                }
                match result {
                    Ok((channels, tracks)) => {
                        self.cache_soundcloud_tracks(&tracks);
                        let Some(likes) = self.youtube.likes.get_mut() else {
                            return;
                        };
                        let moved = likes.apply_remote(channels, tracks);
                        self.save_youtube_likes();
                        self.refresh_youtube_likes_page();
                        if moved {
                            self.soundcloud_pages.remove(crate::youtube::browse::HOME);
                            if self.page() == &Page::SoundCloud(crate::youtube::browse::HOME.into())
                            {
                                self.load_youtube_home();
                            }
                        }
                    }
                    Err(error) => self.youtube_account_failed(&error),
                }
            }
            AccountResponse::Subscribed {
                channel,
                subscribe,
                result,
                ..
            } => {
                match result {
                    Ok(subscription) => {
                        if let Some(likes) = self.youtube.likes.get_mut()
                            && let Some(shown) =
                                likes.channels.iter_mut().find(|each| each.id == channel.id)
                        {
                            shown.subscription = subscription;
                        }
                    }
                    Err(error) => {
                        // It did not happen: stop showing that it did.
                        if let Some(likes) = self.youtube.likes.get_mut() {
                            if subscribe {
                                likes.channels.retain(|each| each.id != channel.id);
                            } else if !likes.channels.iter().any(|each| each.id == channel.id) {
                                let restored: Channel = channel;
                                likes.channels.insert(0, restored);
                            }
                        }
                        self.youtube_account_failed(&error);
                    }
                }
                self.save_youtube_likes();
                self.youtube_write_answered();
            }
            AccountResponse::Rated {
                track,
                liked,
                result,
                ..
            } => {
                match result {
                    Ok(()) => {
                        if let Some(likes) = self.youtube.likes.get_mut() {
                            likes.remote_likes.retain(|uri| *uri != track.uri);
                            if liked {
                                likes.remote_likes.push(track.uri.clone());
                            }
                        }
                    }
                    Err(error) => {
                        if let Some(likes) = self.youtube.likes.get_mut() {
                            likes.tracks.retain(|each| each.uri != track.uri);
                            if !liked {
                                likes.tracks.insert(0, *track);
                            }
                        }
                        self.refresh_youtube_likes_page();
                        self.youtube_account_failed(&error);
                    }
                }
                self.save_youtube_likes();
                self.youtube_write_answered();
            }
        }
    }

    /// Follows the channel or stops. It shows at once and is saved behind it, and
    /// the feed asks again next time it opens. Connected, the account changes too.
    pub(super) fn toggle_youtube_follow(&mut self, channel: crate::youtube::likes::Channel) {
        self.request_youtube_likes();
        let connected = self.youtube.connected.is_some();
        if self.youtube.likes.is_loading() {
            self.youtube.queued_follows.push(channel);
            return;
        }
        let Some(likes) = self.youtube.likes.get_mut() else {
            self.toast_error("Sign in to Spotify to follow YouTube channels here.");
            return;
        };
        if let Some(index) = likes.channels.iter().position(|each| each.id == channel.id) {
            let removed = likes.channels.remove(index);
            if connected && removed.subscription.is_some() {
                self.send_follow(removed, false);
            }
        } else {
            likes.channels.insert(0, channel.clone());
            if connected {
                self.send_follow(channel, true);
            }
        }
        self.save_youtube_likes();
        self.soundcloud_pages.remove(crate::youtube::browse::HOME);
        if self.page() == &Page::SoundCloud(crate::youtube::browse::HOME.into()) {
            self.load_youtube_home();
        }
    }

    /// Asks for the channels' newest uploads, once the followed channels are known.
    pub(super) fn load_youtube_home(&mut self) {
        self.sync_youtube(false);
        self.soundcloud_pages
            .entry(crate::youtube::browse::HOME.into())
            .or_default();
        self.request_youtube_likes();
        if self.youtube.likes.is_loading() {
            return;
        }
        let channels: Vec<String> = self
            .youtube_channels()
            .iter()
            .map(|channel| channel.id.clone())
            .collect();
        let page = self
            .soundcloud_pages
            .entry(crate::youtube::browse::HOME.into())
            .or_default();
        if page.loading || page.content.get().is_some() {
            return;
        }
        if channels.is_empty() {
            page.content = Loadable::Loaded(SoundCloudCollection::default());
            return;
        }
        page.generation = page.generation.wrapping_add(1);
        page.loading = true;
        page.error = None;
        self.backend.send(Command::YouTubeFeed {
            generation: page.generation,
            channels,
        });
    }

    /// Starts a YouTube search for the committed query, or its next page.
    pub(super) fn start_youtube_search(&mut self, more: bool) {
        let cursor = if more {
            let Some(cursor) = self
                .search
                .youtube
                .get()
                .and_then(|value| value.next.clone())
            else {
                return;
            };
            Some(cursor)
        } else {
            self.search.youtube = Loadable::Loading;
            None
        };
        self.search.youtube_pending = true;
        self.backend.send(Command::YouTubeSearch {
            query: self.search.committed.clone(),
            serial: self.search.serial,
            cursor,
        });
    }

    pub(super) fn youtube_search_finished(
        &mut self,
        serial: u64,
        more: bool,
        mut results: YouTubeSearchResults,
    ) {
        if serial != self.search.serial {
            return;
        }
        self.search.youtube_pending = false;
        self.cache_soundcloud_tracks(&results.videos);
        if more && let Some(shown) = self.search.youtube.get_mut() {
            shown.videos.append(&mut results.videos);
            shown.next = results.next;
            shown.error = results.error;
        } else {
            self.search.youtube = Loadable::Loaded(results);
        }
    }

    pub(super) fn reset_youtube(&mut self) {
        self.youtube.generation = self.youtube.generation.wrapping_add(1);
        self.youtube.account = None;
        self.youtube.likes = Loadable::NotLoaded;
        self.youtube.write_revision = 0;
        self.youtube.save_error = false;
        self.youtube.restore_warning = false;
        self.soundcloud_pages.remove("youtube:likes");
        self.soundcloud_pages.remove(crate::youtube::browse::HOME);
        self.table_rows
            .remove(&Page::SoundCloud("youtube:likes".into()));
        self.track_cache.retain(|uri, track| {
            !crate::youtube::is_track_uri(uri) && !crate::youtube::is_track_uri(&track.uri)
        });
        self.backend.send(Command::CancelYouTubeMetadata);
    }

    pub(super) fn request_youtube_likes(&mut self) {
        let Some(account) = self.user_id().map(str::to_owned) else {
            return;
        };
        if self.youtube.account.as_ref() != Some(&account) {
            self.reset_youtube();
            self.youtube.account = Some(account.clone());
        }
        if !self.youtube.likes.needs_load() {
            return;
        }
        // The demo reads YouTube but keeps nothing: its follows last as it runs.
        #[cfg(all(feature = "demo", not(test)))]
        if self.offline {
            self.youtube.likes = Loadable::Loaded(Likes::default());
            return;
        }
        self.youtube.likes = Loadable::Loading;
        self.backend.send(Command::YouTubeLikes {
            account,
            generation: self.youtube.generation,
            files: self.files.clone(),
            writes: self.youtube.writes.clone(),
        });
    }

    pub(super) fn youtube_likes_received(
        &mut self,
        account: &str,
        generation: u64,
        result: Result<Restored, String>,
    ) {
        if generation != self.youtube.generation
            || self.user_id() != Some(account)
            || self.youtube.account.as_deref() != Some(account)
            || !self.youtube.likes.is_loading()
        {
            return;
        }
        self.youtube.restore_warning = result.as_ref().is_ok_and(|restored| restored.missed_save);
        self.youtube.likes = Loadable::from_result(result.map(|restored| restored.likes));
        self.refresh_youtube_likes_page();
        for channel in std::mem::take(&mut self.youtube.queued_follows) {
            self.toggle_youtube_follow(channel);
        }
        if self.page() == &Page::SoundCloud(crate::youtube::browse::HOME.into()) {
            self.load_youtube_home();
        }
        // Once the library is here, see whether a YouTube account is connected.
        if !std::mem::replace(&mut self.youtube.restore_asked, true) {
            self.backend
                .send(Command::YouTubeAccount(AccountRequest::Restore {
                    generation: self.youtube.connection,
                }));
        } else {
            self.sync_youtube(false);
        }
    }

    pub(super) fn load_youtube_likes_page(&mut self) {
        self.sync_youtube(false);
        if self.youtube.save_error && self.youtube.likes.get().is_some() {
            self.save_youtube_likes();
        }
        self.request_youtube_likes();
        self.refresh_youtube_likes_page();
    }

    fn refresh_youtube_likes_page(&mut self) {
        let tracks = self.youtube_likes().unwrap_or_default().to_vec();
        let page = self
            .soundcloud_pages
            .entry("youtube:likes".into())
            .or_default();
        page.generation = page.generation.wrapping_add(1);
        page.loading = self.youtube.likes.is_loading();
        page.error = match &self.youtube.likes {
            Loadable::Failed(error) => Some(error.clone()),
            _ if self.user.is_none() => Some("Sign in to see your local YouTube likes.".into()),
            _ if self.youtube.save_error => Some(crate::youtube::likes::SAVE_ERROR.into()),
            _ if self.youtube.restore_warning => {
                Some(crate::youtube::likes::RESTORE_WARNING.into())
            }
            _ => None,
        };
        page.content = Loadable::Loaded(SoundCloudCollection {
            title: "YouTube Likes".into(),
            tracks,
            ..Default::default()
        });
        self.table_rows
            .remove(&Page::SoundCloud("youtube:likes".into()));
    }

    pub(super) fn toggle_youtube_like(&mut self, uri: &str) {
        let Some(account) = self.user_id().map(str::to_owned) else {
            return;
        };
        if self.youtube.account.as_ref() != Some(&account) {
            return;
        }
        let track = self.hybrid_cached_track(uri);
        let connected = self.youtube.connected.is_some();
        let Some(likes) = self.youtube.likes.get_mut() else {
            self.toast_error("YouTube likes are not ready. Open YouTube Likes and try again.");
            return;
        };
        if let Some(index) = likes.tracks.iter().position(|track| track.uri == uri) {
            let removed = likes.tracks.remove(index);
            if connected {
                self.send_rating(removed, false);
            }
        } else if let Some(track) = track {
            likes.tracks.insert(0, track.clone());
            if connected {
                self.send_rating(track, true);
            }
        } else {
            return;
        }
        self.save_youtube_likes();
        self.refresh_youtube_likes_page();
    }

    fn save_youtube_likes(&mut self) {
        #[cfg(all(feature = "demo", not(test)))]
        if self.offline {
            return;
        }
        let Some(account) = self.user_id().map(str::to_owned) else {
            return;
        };
        if self.youtube.account.as_deref() != Some(account.as_str()) {
            return;
        }
        let Some(likes) = self.youtube.likes.get() else {
            return;
        };
        self.youtube.save_error = false;
        self.youtube.restore_warning = false;
        if self.offline {
            return;
        }
        let revision = self.youtube.writes.begin(&account);
        self.youtube.write_revision = revision;
        let Ok(bytes) = likes.encode() else {
            self.youtube.writes.failed(&account, revision);
            self.youtube.save_error = true;
            self.toast_error(crate::youtube::likes::SAVE_ERROR);
            return;
        };
        self.files.write_or(
            &crate::youtube::likes::path(&self.dirs.state, &account),
            bytes,
            fastframe_file::Access::Private,
            self.backend.youtube_likes_write_failure(
                account,
                self.youtube.generation,
                revision,
                self.youtube.writes.clone(),
            ),
        );
    }

    pub(super) fn youtube_likes_write_failed(
        &mut self,
        account: &str,
        generation: u64,
        revision: u64,
    ) {
        if generation != self.youtube.generation
            || self.user_id() != Some(account)
            || self.youtube.account.as_deref() != Some(account)
            || revision != self.youtube.write_revision
        {
            return;
        }
        // Keep the listener's edits visible and retry the same snapshot when
        // they reopen Likes. Never turn a failed save into an empty library.
        self.youtube.save_error = true;
        self.toast_error(crate::youtube::likes::SAVE_ERROR);
        self.refresh_youtube_likes_page();
    }

    pub(super) fn youtube_track_received(
        &mut self,
        generation: u64,
        uri: &str,
        result: Result<Box<Track>, String>,
    ) {
        if generation != self.youtube.generation {
            return;
        }
        if !self.youtube_playlist_link_pending(uri)
            && !self
                .pending_pastes
                .iter()
                .any(|paste| paste.uris.iter().any(|pasted| pasted == uri))
        {
            return;
        }
        self.youtube_playlist_link_received(uri, &result);
        match result {
            Ok(track) => {
                self.cache_soundcloud_tracks(std::slice::from_ref(track.as_ref()));
                self.resolve_pasted_song(uri, Some(PlayableItem::Track(*track)));
            }
            Err(error) => {
                self.resolve_pasted_song(uri, None);
                self.toast_error(error);
            }
        }
    }
}
