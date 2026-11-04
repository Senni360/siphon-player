//! A person's account with the group service: who Spotify says they are, the
//! computers signed in as them, and the groups they belong to and what they
//! may do in each. `client` carries a chosen group on every request.

use std::sync::RwLock;

use reqwest::Method;
use serde::{Deserialize, Serialize};

use super::client::{Identity, valid_id};
use super::{Client, NewPost};

/// The group this app is looking at. Every feed, live and stats request is
/// about it, whichever of the runtimes sends one, so it lives in one place.
static CURRENT_GROUP: RwLock<Option<String>> = RwLock::new(None);

pub fn current_group() -> Option<String> {
    CURRENT_GROUP.read().ok().and_then(|group| group.clone())
}

pub fn set_current_group(group: Option<String>) {
    if let Ok(mut current) = CURRENT_GROUP.write() {
        *current = group;
    }
}

/// What a member may do in a group. An admin's rights are all of these.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Rights {
    pub share: bool,
    pub host: bool,
    pub queue: bool,
    pub invite: bool,
}

impl Default for Rights {
    fn default() -> Self {
        Self {
            share: true,
            host: true,
            queue: true,
            invite: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Admin,
    #[default]
    Member,
}

impl Role {
    pub fn manages(self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Member {
    pub user_id: String,
    pub display_name: String,
    #[serde(default)]
    pub role: Role,
    #[serde(default)]
    pub rights: Rights,
    #[serde(default)]
    pub spotify_linked: bool,
    #[serde(default)]
    pub soundcloud: Option<String>,
}

// The invite is a secret to anyone outside the group: no Debug of it.
#[derive(Clone, Deserialize, PartialEq, Eq)]
pub struct Group {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub owner_id: Option<String>,
    #[serde(default)]
    pub role: Role,
    #[serde(default)]
    pub rights: Rights,
    #[serde(default)]
    pub members: Vec<Member>,
    #[serde(default)]
    pub invite: Option<String>,
    #[serde(default)]
    pub pending: Vec<String>,
}

impl std::fmt::Debug for Group {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Group")
            .field("id", &self.id)
            .field("members", &self.members.len())
            .finish_non_exhaustive()
    }
}

impl Group {
    /// The people a share can be sent to, in the shape the feed knows.
    pub fn profiles(&self) -> Vec<super::Profile> {
        self.members
            .iter()
            .map(|member| super::Profile {
                user_id: member.user_id.clone(),
                display_name: member.display_name.clone(),
            })
            .collect()
    }

    pub fn may_invite(&self) -> bool {
        self.role.manages() || self.rights.invite
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub current: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SoundCloudLink {
    pub id: String,
    pub username: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Account {
    pub user_id: String,
    pub display_name: String,
    #[serde(default)]
    pub spotify_id: Option<String>,
    #[serde(default)]
    pub soundcloud: Option<SoundCloudLink>,
    #[serde(default)]
    pub devices: Vec<Device>,
    #[serde(default)]
    pub groups: Vec<Group>,
}

impl Account {
    /// The group to look at: the one chosen, if still theirs, else their first.
    pub fn chosen_group(&self, chosen: Option<&str>) -> Option<&Group> {
        chosen
            .and_then(|id| self.groups.iter().find(|group| group.id == id))
            .or_else(|| self.groups.first())
    }
}

/// A playlist someone shared with the group. Its songs arrive when it is opened.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct SharedPlaylist {
    pub id: String,
    pub name: String,
    pub by: String,
    pub by_name: String,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub count: u32,
    #[serde(default)]
    pub artwork_url: Option<String>,
    #[serde(default)]
    pub to: Vec<String>,
}

/// A change to the account or to a group, answered with the account as it is
/// afterwards, so no answer needs its own merging.
#[derive(Clone)]
pub enum Op {
    NewGroup(String),
    JoinWithCode(String),
    RenameGroup {
        group: String,
        name: String,
    },
    DeleteGroup(String),
    LeaveGroup(String),
    NewInvite(String),
    TurnOffInvite(String),
    /// By Spotify username or profile link. Someone who has not signed in yet
    /// is in the group the moment they do.
    AddMember {
        group: String,
        spotify: String,
    },
    CancelAdd {
        group: String,
        spotify: String,
    },
    RemoveMember {
        group: String,
        user: String,
    },
    SetAdmin {
        group: String,
        user: String,
        admin: bool,
    },
    SetRights {
        group: String,
        user: String,
        rights: Rights,
    },
    HandOver {
        group: String,
        user: String,
    },
    RemoveDevice(String),
    LinkSoundCloud(SoundCloudLink),
    UnlinkSoundCloud,
    UnlinkSpotify,
}

#[derive(Deserialize)]
pub(super) struct SignedIn {
    pub user_id: String,
    pub display_name: String,
    pub secret: String,
}

impl Client {
    /// Proves who this listener is with the app's Spotify token. The service
    /// reads their Spotify id with it once and keeps neither the token nor
    /// anything else it could do. An identity already held comes along, so a
    /// member from before accounts stays themselves.
    pub(super) async fn sign_in(
        &self,
        token: &str,
        device: &str,
        held: Option<&Identity>,
    ) -> Result<(Identity, String), String> {
        let answer: SignedIn = self
            .request(
                Method::POST,
                "/auth/spotify",
                held,
                Some(serde_json::json!({ "token": token, "device": device })),
                None,
            )
            .await?;
        if answer.user_id.is_empty()
            || answer.secret.len() != 64
            || !answer
                .secret
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("Friends returned an invalid identity.".into());
        }
        Ok((
            Identity {
                user_id: answer.user_id,
                secret: answer.secret,
                base_url: self.base_url().into(),
            },
            answer.display_name,
        ))
    }

    pub(super) async fn account(&self, identity: &Identity) -> Result<Account, String> {
        self.request(Method::GET, "/account", Some(identity), None, None)
            .await
    }

    pub(super) async fn shared_songs(
        &self,
        identity: &Identity,
    ) -> Result<(Vec<crate::shared_songs::SharedSong>, u64, u64), String> {
        #[derive(Deserialize)]
        struct Listed {
            songs: Vec<crate::shared_songs::SharedSong>,
            #[serde(default)]
            used: u64,
            #[serde(default)]
            capacity: u64,
        }
        let listed: Listed = self
            .request(Method::GET, "/audio", Some(identity), None, None)
            .await?;
        Ok((listed.songs, listed.used, listed.capacity))
    }

    /// Reserves the song's place, sends the file straight to the file store, and
    /// tells the service it arrived.
    pub(super) async fn upload_song(
        &self,
        identity: &Identity,
        path: std::path::PathBuf,
    ) -> Result<crate::shared_songs::SharedSong, String> {
        #[derive(Deserialize)]
        struct Reserved {
            id: String,
            pathname: String,
            token: String,
            content_type: String,
        }
        let (info, bytes) = tokio::task::spawn_blocking(move || {
            let info = crate::shared_songs::inspect(&path)?;
            let bytes =
                std::fs::read(&path).map_err(|_| "The file could not be read.".to_owned())?;
            Ok::<_, String>((info, bytes))
        })
        .await
        .map_err(|_| "The file could not be read.".to_owned())??;
        // Asked here to say so plainly: this client doesn't show the service's own words.
        let (songs, _, _) = self.shared_songs(identity).await?;
        if songs
            .iter()
            .any(|song| song.title == info.title && song.size == bytes.len() as u64)
        {
            return Err(format!("{} is already shared.", info.title));
        }
        let reserved: Reserved = self
            .request(
                Method::POST,
                "/audio",
                Some(identity),
                Some(serde_json::json!({
                    "title": info.title,
                    "artists": info.artists,
                    "duration_ms": info.duration_ms,
                    "size": bytes.len(),
                    "ext": info.ext,
                })),
                None,
            )
            .await
            .map_err(|error| match error.as_str() {
                super::client::TOO_MANY_REQUESTS => {
                    "You've shared a lot of songs this hour. Try the rest later.".to_owned()
                }
                super::client::NOT_ALLOWED => {
                    "You can't share files in this group. Ask an admin.".to_owned()
                }
                _ => error,
            })?;
        let length = bytes.len();
        let sent = self
            .http
            .client()?
            .put("https://vercel.com/api/blob/")
            .query(&[("pathname", reserved.pathname.as_str())])
            .bearer_auth(&reserved.token)
            .header("x-api-version", "13")
            .header("x-vercel-blob-access", "public")
            .header("x-content-type", &reserved.content_type)
            .header("x-add-random-suffix", "0")
            .header("x-allow-overwrite", "0")
            .header("x-content-length", length)
            .body(bytes)
            .timeout(std::time::Duration::from_secs(600))
            .send()
            .await
            .map_err(|_| "The upload was interrupted. Try again.".to_owned())?;
        if !sent.status().is_success() {
            log::warn!("A shared song's upload was refused: {}", sent.status());
            return Err("The file store refused the upload. Try again later.".into());
        }
        self.request(
            Method::POST,
            &format!("/audio/{}/complete", reserved.id),
            Some(identity),
            Some(serde_json::json!({})),
            None,
        )
        .await
    }

    pub(super) async fn delete_song(&self, identity: &Identity, id: &str) -> Result<(), String> {
        let _: serde_json::Value = self
            .request(
                Method::DELETE,
                &format!("/audio/{id}"),
                Some(identity),
                None,
                None,
            )
            .await?;
        Ok(())
    }

    /// Where to stream a shared song from. Counts against the month's downloads.
    pub async fn song_stream(
        &self,
        identity: &Identity,
        id: &str,
    ) -> Result<(String, String), String> {
        #[derive(Deserialize)]
        struct Stream {
            url: String,
            content_type: String,
        }
        let stream: Stream = self
            .request(
                Method::POST,
                &format!("/audio/{id}/play"),
                Some(identity),
                Some(serde_json::json!({})),
                None,
            )
            .await
            .map_err(|error| {
                if error == super::client::TOO_MANY_REQUESTS {
                    "Shared files have used up this month's downloads. They come back next month."
                        .to_owned()
                } else {
                    error
                }
            })?;
        Ok((stream.url, stream.content_type))
    }

    pub(super) async fn shared_playlists(
        &self,
        identity: &Identity,
    ) -> Result<Vec<SharedPlaylist>, String> {
        #[derive(Deserialize)]
        struct Listed {
            playlists: Vec<SharedPlaylist>,
        }
        let listed: Listed = self
            .request(Method::GET, "/playlists", Some(identity), None, None)
            .await?;
        Ok(listed.playlists)
    }

    pub(super) async fn share_playlist(
        &self,
        identity: &Identity,
        name: &str,
        items: &[NewPost],
        to: &[String],
    ) -> Result<SharedPlaylist, String> {
        self.request(
            Method::POST,
            "/playlists",
            Some(identity),
            Some(serde_json::json!({ "name": name, "items": items, "to": to })),
            None,
        )
        .await
    }

    pub(super) async fn shared_playlist_songs(
        &self,
        identity: &Identity,
        id: &str,
    ) -> Result<(String, Vec<NewPost>), String> {
        #[derive(Deserialize)]
        struct Opened {
            name: String,
            items: Vec<NewPost>,
        }
        valid_id(id)?;
        let opened: Opened = self
            .request(
                Method::GET,
                &format!("/playlists/{id}"),
                Some(identity),
                None,
                None,
            )
            .await?;
        Ok((opened.name, opened.items))
    }

    pub(super) async fn delete_shared_playlist(
        &self,
        identity: &Identity,
        id: &str,
    ) -> Result<(), String> {
        valid_id(id)?;
        let _: serde_json::Value = self
            .request(
                Method::DELETE,
                &format!("/playlists/{id}"),
                Some(identity),
                None,
                None,
            )
            .await?;
        Ok(())
    }

    /// Runs a change. A group's identifiers are checked before they reach a path.
    pub(super) async fn apply(&self, identity: &Identity, op: Op) -> Result<(), String> {
        let call = async |method: Method, path: String, body: Option<serde_json::Value>| {
            let _: serde_json::Value = self
                .request(method, &path, Some(identity), body, None)
                .await?;
            Ok::<(), String>(())
        };
        let group_path = |group: &str, tail: &str| -> Result<String, String> {
            valid_id(group)?;
            Ok(format!("/groups/{group}{tail}"))
        };
        let member_path = |group: &str, user: &str| -> Result<String, String> {
            valid_id(user)?;
            group_path(group, &format!("/members/{user}"))
        };
        match op {
            Op::NewGroup(name) => {
                call(
                    Method::POST,
                    "/groups".into(),
                    Some(serde_json::json!({ "name": name })),
                )
                .await
            }
            Op::JoinWithCode(code) => {
                call(
                    Method::POST,
                    "/groups/join".into(),
                    Some(serde_json::json!({ "code": code })),
                )
                .await
            }
            Op::RenameGroup { group, name } => {
                call(
                    Method::PATCH,
                    group_path(&group, "")?,
                    Some(serde_json::json!({ "name": name })),
                )
                .await
            }
            Op::DeleteGroup(group) => call(Method::DELETE, group_path(&group, "")?, None).await,
            Op::LeaveGroup(group) => {
                call(
                    Method::POST,
                    group_path(&group, "/leave")?,
                    Some(serde_json::json!({})),
                )
                .await
            }
            Op::NewInvite(group) => {
                call(
                    Method::POST,
                    group_path(&group, "/invite")?,
                    Some(serde_json::json!({})),
                )
                .await
            }
            Op::TurnOffInvite(group) => {
                call(Method::DELETE, group_path(&group, "/invite")?, None).await
            }
            Op::AddMember { group, spotify } => {
                call(
                    Method::POST,
                    group_path(&group, "/members")?,
                    Some(serde_json::json!({ "spotify_id": spotify })),
                )
                .await
            }
            Op::CancelAdd { group, spotify } => {
                call(
                    Method::DELETE,
                    group_path(&group, "/members")?,
                    Some(serde_json::json!({ "spotify_id": spotify })),
                )
                .await
            }
            Op::RemoveMember { group, user } => {
                call(Method::DELETE, member_path(&group, &user)?, None).await
            }
            Op::SetAdmin { group, user, admin } => {
                call(
                    Method::PATCH,
                    member_path(&group, &user)?,
                    Some(serde_json::json!({ "role": if admin { "admin" } else { "member" } })),
                )
                .await
            }
            Op::SetRights {
                group,
                user,
                rights,
            } => {
                call(
                    Method::PATCH,
                    member_path(&group, &user)?,
                    Some(serde_json::json!({ "rights": rights })),
                )
                .await
            }
            Op::HandOver { group, user } => {
                valid_id(&user)?;
                call(
                    Method::POST,
                    group_path(&group, "/owner")?,
                    Some(serde_json::json!({ "user_id": user })),
                )
                .await
            }
            Op::RemoveDevice(device) => {
                valid_id(&device)?;
                call(Method::DELETE, format!("/account/devices/{device}"), None).await
            }
            Op::LinkSoundCloud(link) => {
                call(
                    Method::PUT,
                    "/account/soundcloud".into(),
                    Some(serde_json::to_value(link).map_err(|_| "Invalid SoundCloud link.")?),
                )
                .await
            }
            Op::UnlinkSoundCloud => call(Method::DELETE, "/account/soundcloud".into(), None).await,
            Op::UnlinkSpotify => call(Method::DELETE, "/account/spotify".into(), None).await,
        }
    }
}

/// Applied to the account the moment it is asked for: the service's answer
/// then replaces it, or takes it back with the error.
pub(super) fn apply_locally(account: &mut Account, op: &Op) {
    match op {
        Op::RenameGroup { group, name } => {
            if let Some(group) = account.groups.iter_mut().find(|g| g.id == *group) {
                group.name.clone_from(name);
            }
        }
        Op::DeleteGroup(id) | Op::LeaveGroup(id) => account.groups.retain(|g| g.id != *id),
        Op::TurnOffInvite(id) => {
            if let Some(group) = account.groups.iter_mut().find(|g| g.id == *id) {
                group.invite = None;
            }
        }
        Op::CancelAdd { group, spotify } => {
            if let Some(group) = account.groups.iter_mut().find(|g| g.id == *group) {
                group.pending.retain(|pending| pending != spotify);
            }
        }
        Op::RemoveMember { group, user } => {
            if let Some(group) = account.groups.iter_mut().find(|g| g.id == *group) {
                group.members.retain(|member| member.user_id != *user);
            }
        }
        Op::SetAdmin { group, user, admin } => {
            if let Some(member) = account
                .groups
                .iter_mut()
                .find(|g| g.id == *group)
                .and_then(|g| g.members.iter_mut().find(|m| m.user_id == *user))
            {
                member.role = if *admin { Role::Admin } else { Role::Member };
            }
        }
        Op::SetRights {
            group,
            user,
            rights,
        } => {
            if let Some(member) = account
                .groups
                .iter_mut()
                .find(|g| g.id == *group)
                .and_then(|g| g.members.iter_mut().find(|m| m.user_id == *user))
            {
                member.rights = *rights;
            }
        }
        Op::RemoveDevice(id) => account.devices.retain(|device| device.id != *id),
        Op::LinkSoundCloud(link) => account.soundcloud = Some(link.clone()),
        Op::UnlinkSoundCloud => account.soundcloud = None,
        Op::UnlinkSpotify => account.spotify_id = None,
        Op::NewGroup(_)
        | Op::JoinWithCode(_)
        | Op::NewInvite(_)
        | Op::AddMember { .. }
        | Op::HandOver { .. } => {}
    }
}
