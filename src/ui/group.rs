//! The Siphon group as a page: what friends shared, newest first, grouped
//! by day like a conversation, each share with its thread. A composer at
//! the top takes the song that is playing or any pasted Spotify,
//! SoundCloud or YouTube link, a note, and who it is for. Every share can
//! be played, queued, downloaded through the helper, and once downloaded
//! its file can be dragged straight into a DAW.
//!
//! Everything the person does shows at once (the model is optimistic) and
//! moves to say what happened: a sent share drops into the feed, hearts
//! pop and their counts roll, threads unfold, a finished download slides
//! its file in under the song.

use egui::{Align2, Color32, CornerRadius, Id, Key, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};

use crate::app::App;
use crate::friends::{
    Action as Group, NewPost, PENDING_REPLY, PENDING_SHARE, Post, Profile, Reply,
    Source as PostSource,
};
use crate::i18n::gettext;
use crate::model::Action;
use crate::theme::{self, Icon, Palette};

use super::kit::{Source, motion};

/// The widest the feed runs; past it, the page keeps a reading column.
const COLUMN: f32 = 760.0;
const AVATAR: f32 = 36.0;
const REPLY_AVATAR: f32 = 24.0;
const COVER: f32 = 56.0;
/// Room left of a message's body for its avatar.
const GUTTER: f32 = AVATAR + 12.0;
/// Replies shown before the rest fold behind "Show all".
const FOLDED_REPLIES: usize = 2;

/// Who a share is about to go to, what is attached and what it says,
/// kept between frames while the person writes it.
#[derive(Clone, Default)]
struct Draft {
    link: String,
    title: String,
    note: String,
    to: Vec<String>,
    attached: Option<NewPost>,
    /// The link the attachment was recognised from, to tell an edit apart.
    recognised: String,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Filter {
    #[default]
    Everyone,
    ToYou,
    FromYou,
}

impl Filter {
    const ALL: [Self; 3] = [Self::Everyone, Self::ToYou, Self::FromYou];

    fn label(self) -> &'static str {
        match self {
            Self::Everyone => "Everything",
            Self::ToYou => "Sent to you",
            Self::FromYou => "From you",
        }
    }

    fn keeps(self, post: &Post, me: &str) -> bool {
        match self {
            Self::Everyone => true,
            Self::ToYou => post.author_id != me && post.item.to.iter().any(|id| id == me),
            Self::FromYou => post.author_id == me,
        }
    }
}

fn draft_id() -> Id {
    Id::new("group-draft")
}

/// When the person last saw the group, in the service's milliseconds,
/// per member so another identity starts unread-free.
fn seen_id(user: &str) -> Id {
    Id::new(("group-seen", user))
}

/// What the "New" line in the feed marks: the seen time as it was when the
/// page opened, so the line stays put while the page is read.
fn divider_id() -> Id {
    Id::new("group-seen-at-open")
}

/// How many shares and replies arrived since the person last looked.
pub fn unread(app: &App, ctx: &egui::Context) -> usize {
    let Some(profile) = &app.friends_feed.profile else {
        return 0;
    };
    if app.page() == &crate::model::Page::Group {
        return 0;
    }
    let seen = ctx
        .data_mut(|data| data.get_persisted::<u64>(seen_id(&profile.user_id)))
        .unwrap_or(0);
    // A first visit counts nothing as new: the badge is for what changed.
    if seen == 0 {
        return 0;
    }
    app.friends_feed.unread_since(seen)
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let palette = app.palette;
    let locale = app.locale;
    let ctx = ui.ctx().clone();
    let width = ui.available_width().min(COLUMN);
    let mut actions = Vec::new();

    let margin = ((ui.available_width() - width) / 2.0).max(0.0).floor();
    ui.horizontal_top(|ui| {
    ui.add_space(margin);
    ui.vertical(|ui| {
        ui.set_width(width);
        super::rise(ui, 0, |ui| header(app, ui));
        ui.add_space(16.0);

        let feed = &app.friends_feed;
        if feed.profile.is_none() {
            join(app, ui, &mut actions);
            return;
        }
        let me = feed.profile.clone().unwrap_or_else(|| Profile {
            user_id: String::new(),
            display_name: String::new(),
        });
        // The feed is read: the newest thing in it is now seen. The line
        // marking what was new keeps the time from when the page opened.
        let newest = feed.newest();
        let seen = ctx
            .data_mut(|data| data.get_persisted::<u64>(seen_id(&me.user_id)))
            .unwrap_or(0);
        let divider = ctx.data_mut(|data| *data.get_temp_mut_or_insert_with(divider_id(), || seen));
        if newest > seen {
            ctx.data_mut(|data| data.insert_persisted(seen_id(&me.user_id), newest));
        }

        if let Some(secret) = feed.recovery_secret.clone() {
            recovery(ui, &palette, &secret, &mut actions);
            ui.add_space(16.0);
        }
        super::rise(ui, 1, |ui| composer(app, ui, &me, &mut actions));
        ui.add_space(20.0);

        let filter = super::rise(ui, 2, |ui| filters(ui, &palette));
        ui.add_space(12.0);

        let feed = &app.friends_feed;
        if let Some(error) = &feed.error {
            notice(
                ui,
                &palette,
                Icon::CircleAlert,
                error,
                Some((gettext(locale, "Try again").into_owned(), Group::Refresh)),
                &mut actions,
            );
            ui.add_space(12.0);
        }
        let posts: Vec<Post> = feed
            .posts
            .iter()
            .filter(|post| filter.keeps(post, &me.user_id))
            .cloned()
            .collect();
        if posts.is_empty() {
            if feed.busy {
                skeleton(ui, &palette);
            } else {
                let text = match filter {
                    Filter::Everyone => "Nothing shared yet. Paste a link above, or share what's playing, and it lands here for everyone",
                    Filter::ToYou => "Songs a friend sends only to you show up here",
                    Filter::FromYou => "What you share shows up here",
                };
                notice(ui, &palette, Icon::Users, &gettext(locale, text), None, &mut actions);
            }
        } else {
            feed_list(app, ui, &posts, &me, divider, &mut actions);
        }
        let feed = &app.friends_feed;
        if feed.next_cursor.is_some() && filter == Filter::Everyone {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(GUTTER);
                if theme::soft_button(ui, &palette, None, &gettext(locale, "Older shares"), false)
                    .clicked()
                {
                    actions.push(Action::Friends(Group::More));
                }
            });
        }
        ui.add_space(32.0);
        identity(app, ui, &me, &mut actions);
    });
    });
    app.actions.extend(actions);
}

/// The page title, and the members as a stack of initials that spreads
/// out when pointed at so each name can be read.
fn header(app: &App, ui: &mut Ui) {
    let palette = app.palette;
    let locale = app.locale;
    let members = &app.friends_feed.members;
    ui.horizontal(|ui| {
        ui.set_min_height(40.0);
        theme::text(
            ui,
            gettext(locale, "Group"),
            theme::medium(28.0),
            palette.text,
        );
        if members.is_empty() {
            return;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let count = format!(
                "{} {}",
                members.len(),
                if members.len() == 1 {
                    "member"
                } else {
                    "members"
                }
            );
            theme::text(ui, count, theme::regular(13.0), palette.dim);
            ui.add_space(8.0);
            let shown = members.len().min(6);
            let (rect, response) =
                ui.allocate_exact_size(vec2(28.0 + 30.0 * shown as f32, 28.0), Sense::hover());
            let spread = motion::toward(
                ui.ctx(),
                Id::new("group-members-spread"),
                response.hovered(),
                motion::POPOVER,
            );
            let step = 22.0 + 10.0 * spread;
            let start = rect.right() - 28.0 - step * (shown as f32 - 1.0);
            for (index, member) in members.iter().take(shown).enumerate() {
                let center = pos2(start + 14.0 + step * index as f32, rect.center().y);
                avatar(ui, &palette, center, 28.0, &member.display_name, true);
                let hit = Rect::from_center_size(center, Vec2::splat(28.0));
                if spread > 0.5 && ui.rect_contains_pointer(hit) {
                    response
                        .clone()
                        .on_hover_text_at_pointer(member.display_name.clone());
                }
            }
        });
    });
}

/// An initial in a circle, quiet: the shade only tells people apart.
fn avatar(ui: &Ui, palette: &Palette, center: egui::Pos2, size: f32, name: &str, ring: bool) {
    let initial = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_else(|| "?".into());
    // Four neutral steps, picked by the name, so the same person always
    // looks the same without adding hues to the chrome.
    let shade = name.bytes().fold(0u32, |hash, b| {
        hash.wrapping_mul(31).wrapping_add(u32::from(b))
    }) % 4;
    let fill = shade_of(palette, shade);
    let painter = ui.painter();
    if ring {
        painter.circle_filled(center, size / 2.0 + 2.0, palette.window);
    }
    painter.circle_filled(center, size / 2.0, fill);
    painter.text(
        center,
        Align2::CENTER_CENTER,
        initial,
        theme::medium(size * 0.42),
        palette.text,
    );
}

fn shade_of(palette: &Palette, step: u32) -> Color32 {
    let toward = if palette.dark {
        Color32::WHITE
    } else {
        Color32::BLACK
    };
    super::blend(palette.surface_active, toward, 0.04 * step as f32)
}

/// Before joining: what the group is, and the invite and name to join with.
fn join(app: &App, ui: &mut Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    let feed = &app.friends_feed;
    let id = Id::new("group-join");
    let (mut invite, mut name) = ui
        .data(|data| data.get_temp::<(String, String)>(id))
        .unwrap_or_default();
    card(ui, &palette, |ui| {
        theme::text(
            ui,
            gettext(locale, "Join your group"),
            theme::medium(17.0),
            palette.text,
        );
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(gettext(
                    locale,
                    "Your friends' shares from Spotify, SoundCloud and YouTube in one feed, with download and drag-to-DAW on every song. Ask whoever runs the group for the invite code.",
                ))
                .font(theme::regular(14.0))
                .color(palette.secondary),
            )
            .wrap(),
        );
        ui.add_space(16.0);
        ui.add_enabled_ui(!feed.busy, |ui| {
            field(
                ui,
                &palette,
                &mut invite,
                &gettext(locale, "Invite code"),
                true,
                256,
            );
            ui.add_space(8.0);
            field(
                ui,
                &palette,
                &mut name,
                &gettext(locale, "Your name in the group"),
                false,
                40,
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let ready = !invite.trim().is_empty() && !name.trim().is_empty();
                let label = if feed.busy {
                    gettext(locale, "Joining…")
                } else {
                    gettext(locale, "Join")
                };
                if ui
                    .add_enabled_ui(ready, |ui| theme::pill_button(ui, &palette, &label, true))
                    .inner
                    .clicked()
                {
                    actions.push(Action::Friends(Group::Join {
                        invite_code: std::mem::take(&mut invite).trim().to_owned(),
                        display_name: name.trim().to_owned(),
                    }));
                }
                if feed.error.is_some() {
                    ui.add_space(8.0);
                    if theme::pill_button(
                        ui,
                        &palette,
                        &gettext(locale, "Retry saved identity"),
                        false,
                    )
                    .clicked()
                    {
                        actions.push(Action::Friends(Group::Open));
                    }
                }
            });
        });
        if let Some(error) = &feed.error {
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                theme::icon(ui, Icon::CircleAlert, 16.0, palette.danger);
                theme::text(ui, error, theme::regular(13.0), palette.secondary);
            });
        }
    });
    ui.data_mut(|data| data.insert_temp(id, (invite, name)));
}

/// A one-line text field in the page's style.
fn field(
    ui: &mut Ui,
    palette: &Palette,
    text: &mut String,
    hint: &str,
    password: bool,
    limit: usize,
) -> egui::Response {
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(text)
                    .hint_text(hint)
                    .password(password)
                    .char_limit(limit)
                    .frame(egui::Frame::NONE)
                    .font(theme::regular(14.0))
                    .desired_width(f32::INFINITY),
            )
        })
        .inner
}

/// A plain raised surface holding a group of controls.
fn card<R>(ui: &mut Ui, palette: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(palette.panel)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// The one-time secret after joining, until the person says it is saved.
fn recovery(ui: &mut Ui, palette: &Palette, secret: &str, actions: &mut Vec<Action>) {
    card(ui, palette, |ui| {
        ui.horizontal(|ui| {
            theme::icon(ui, Icon::Lock, 16.0, palette.warning);
            theme::text(
                ui,
                "Save your group secret",
                theme::medium(14.0),
                palette.text,
            );
        });
        ui.add_space(4.0);
        theme::text(
            ui,
            "It is shown once. It brings this identity back if this computer forgets it",
            theme::regular(13.0),
            palette.secondary,
        );
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if theme::soft_button(ui, palette, Some(Icon::Copy), "Copy secret", false).clicked() {
                ui.ctx().copy_text(secret.to_owned());
            }
            if theme::soft_button(ui, palette, Some(Icon::Check), "I saved it", false).clicked() {
                actions.push(Action::Friends(Group::DismissSecret));
            }
        });
    });
}

/// A quiet line with an icon and at most one action.
fn notice(
    ui: &mut Ui,
    palette: &Palette,
    icon: Icon,
    text: &str,
    action: Option<(String, Group)>,
    actions: &mut Vec<Action>,
) {
    egui::Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::icon(ui, icon, 16.0, palette.dim);
                ui.add_space(4.0);
                let reserve = if action.is_some() { 110.0 } else { 0.0 };
                ui.allocate_ui(
                    vec2((ui.available_width() - reserve).max(80.0), 0.0),
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(text)
                                    .font(theme::regular(13.5))
                                    .color(palette.secondary),
                            )
                            .wrap(),
                        );
                    },
                );
                if let Some((label, group)) = action {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::soft_button(ui, palette, None, &label, false).clicked() {
                            actions.push(Action::Friends(group));
                        }
                    });
                }
            });
        });
}

/// Three still placeholder rows while the first read runs. Nothing
/// shimmers while it waits.
fn skeleton(ui: &mut Ui, palette: &Palette) {
    for index in 0..3 {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 104.0), Sense::hover());
        let alpha = motion::appear(
            ui.ctx(),
            Id::new(("group-skeleton", index)),
            motion::ENTER,
            motion::stagger(index),
        );
        let painter = ui.painter();
        let fill = palette.surface.gamma_multiply(alpha);
        painter.circle_filled(
            pos2(rect.left() + AVATAR / 2.0, rect.top() + AVATAR / 2.0),
            AVATAR / 2.0,
            fill,
        );
        painter.rect_filled(
            Rect::from_min_size(
                pos2(rect.left() + GUTTER, rect.top() + 4.0),
                vec2(120.0, 12.0),
            ),
            4.0,
            fill,
        );
        painter.rect_filled(
            Rect::from_min_size(
                pos2(rect.left() + GUTTER, rect.top() + 26.0),
                vec2(rect.width() - GUTTER, 68.0),
            ),
            12.0,
            fill,
        );
        ui.add_space(8.0);
    }
}

/// The source a pasted link belongs to.
fn link_source(url: &str) -> Option<PostSource> {
    let uri = crate::link::parse(url)?;
    if uri.starts_with("youtube:") {
        Some(PostSource::Youtube)
    } else if uri.starts_with("spotify:") {
        matches!(
            crate::util::uri_kind(&uri),
            Some("track" | "album" | "playlist")
        )
        .then_some(PostSource::Spotify)
    } else {
        Some(PostSource::Soundcloud)
    }
}

/// What a pasted link can be shared as, filled in from what the app
/// already knows about it: the playing song, a cached song, album or
/// playlist. A link nothing is known about keeps an empty title for the
/// person to type, except SoundCloud's, whose path names the song.
fn recognise(app: &mut App, text: &str) -> Option<NewPost> {
    let text = text.trim();
    let source = link_source(text)?;
    let uri = crate::link::parse(text)?;
    if let Some(item) = app.now_playing_item().filter(|item| item.uri() == uri)
        && let Ok(post) = NewPost::from_item(&item)
    {
        return Some(post);
    }
    match source {
        PostSource::Spotify => {
            let kind = crate::util::uri_kind(&uri)?.to_owned();
            let id = crate::util::uri_id(&uri)?.to_owned();
            let url = format!("https://open.spotify.com/{kind}/{id}");
            match kind.as_str() {
                "track" => {
                    if let Some(track) = app.track_cache.get(&id).cloned()
                        && let Ok(post) =
                            NewPost::from_item(&crate::api::models::PlayableItem::Track(track))
                    {
                        return Some(post);
                    }
                    Some(NewPost::from_url(url, source, String::new()))
                }
                "album" => {
                    let album = app.known_album(&id).cloned();
                    let mut post = NewPost::from_url(
                        url,
                        source,
                        album.as_ref().map(|a| a.name.clone()).unwrap_or_default(),
                    );
                    if let Some(album) = album {
                        post.artists = album.artists.iter().map(|a| a.name.clone()).collect();
                        post.artwork_url =
                            crate::api::models::pick_image(&album.images, 300).map(str::to_owned);
                    }
                    Some(post)
                }
                _ => {
                    let playlist = app.known_playlist(&id).cloned();
                    let mut post = NewPost::from_url(
                        url,
                        source,
                        playlist
                            .as_ref()
                            .map(|p| p.name.clone())
                            .unwrap_or_default(),
                    );
                    if let Some(playlist) = playlist {
                        post.artwork_url = crate::api::models::pick_image(&playlist.images, 300)
                            .map(str::to_owned);
                    }
                    Some(post)
                }
            }
        }
        PostSource::Youtube => {
            let id = uri.strip_prefix("youtube:video:")?;
            Some(NewPost::from_url(
                format!("https://www.youtube.com/watch?v={id}"),
                source,
                String::new(),
            ))
        }
        PostSource::Soundcloud => {
            let mut post = NewPost::from_url(text.to_owned(), source, String::new());
            let (artist, title) = soundcloud_names(text);
            post.title = title;
            post.artists = artist.into_iter().collect();
            Some(post)
        }
    }
}

/// A guess at a SoundCloud link's artist and title from its path:
/// `soundcloud.com/some-artist/a-song-name` reads "some artist", "a song
/// name". Private `s-` segments and query tokens are left out.
fn soundcloud_names(url: &str) -> (Option<String>, String) {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return (None, String::new());
    };
    let words = |segment: &str| segment.replace(['-', '_'], " ").trim().to_owned();
    let segments: Vec<&str> = parsed
        .path_segments()
        .map(|s| {
            s.filter(|s| !s.is_empty() && !s.starts_with("s-"))
                .collect()
        })
        .unwrap_or_default();
    match segments.as_slice() {
        [artist, "sets", set, ..] => (Some(words(artist)), words(set)),
        [artist, track, ..] => (Some(words(artist)), words(track)),
        _ => (None, String::new()),
    }
}

/// The composer: attach the playing song or paste a link, add a note,
/// pick who it is for, send.
fn composer(app: &mut App, ui: &mut Ui, me: &Profile, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    let ctx = ui.ctx().clone();
    let mut draft = ui
        .data(|data| data.get_temp::<Draft>(draft_id()))
        .unwrap_or_default();
    let playing = app
        .now_playing_item()
        .and_then(|item| NewPost::from_item(&item).ok());

    card(ui, &palette, |ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        // The link field, or what is attached.
        match draft.attached.clone() {
            None => {
                let response = field(
                    ui,
                    &palette,
                    &mut draft.link,
                    &gettext(locale, "Paste a Spotify, SoundCloud or YouTube link"),
                    false,
                    4096,
                );
                if response.changed()
                    && draft.link.trim() != draft.recognised
                    && let Some(post) = recognise(app, &draft.link)
                {
                    draft.recognised = draft.link.trim().to_owned();
                    draft.title.clone_from(&post.title);
                    draft.attached = Some(post);
                    motion::begin(&ctx, Id::new("group-attach"), motion::ENTER);
                }
                let typed = !draft.link.trim().is_empty();
                if typed && draft.attached.is_none() {
                    theme::text(
                        ui,
                        gettext(
                            locale,
                            "That isn't a song, album, playlist or set link Siphon can share",
                        ),
                        theme::regular(12.5),
                        palette.dim,
                    );
                } else if let Some(post) = playing.clone().filter(|_| !typed) {
                    ui.horizontal(|ui| {
                        let label = format!(
                            "{} {}",
                            gettext(locale, "Share what's playing:"),
                            post.title
                        );
                        if theme::soft_button(ui, &palette, Some(Icon::AudioLines), &label, false)
                            .clicked()
                        {
                            draft.title.clone_from(&post.title);
                            draft.attached = Some(post);
                            motion::begin(&ctx, Id::new("group-attach"), motion::ENTER);
                        }
                    });
                }
            }
            Some(post) => {
                let t = motion::running(&ctx, Id::new("group-attach")).unwrap_or(1.0);
                let remove = attachment(ui, &palette, &post, t, locale);
                if post.title.is_empty() || draft.title != post.title {
                    field(
                        ui,
                        &palette,
                        &mut draft.title,
                        &gettext(locale, "Title"),
                        false,
                        300,
                    );
                }
                if remove {
                    draft.attached = None;
                    draft.link.clear();
                    draft.recognised.clear();
                    draft.title.clear();
                }
            }
        }

        if draft.attached.is_some() {
            field(
                ui,
                &palette,
                &mut draft.note,
                &gettext(locale, "Add a note (optional)"),
                false,
                280,
            );
            recipients(ui, &palette, &app.friends_feed.members, me, &mut draft.to);
            ui.horizontal(|ui| {
                let ready = !draft.title.trim().is_empty();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if draft.to.is_empty() {
                        gettext(locale, "Share with the group").into_owned()
                    } else {
                        gettext(locale, "Send").into_owned()
                    };
                    if send_button(ui, &palette, &label, ready).clicked() && ready {
                        if let Some(mut post) = draft.attached.take() {
                            post.title = draft.title.trim().to_owned();
                            post.note = draft.note.trim().to_owned();
                            post.to = draft.to.clone();
                            actions.push(Action::Friends(Group::Share(post)));
                        }
                        draft = Draft::default();
                        motion::begin(&ctx, Id::new("group-sent"), motion::FLY);
                    }
                    if theme::soft_button(ui, &palette, None, &gettext(locale, "Cancel"), false)
                        .clicked()
                    {
                        draft = Draft::default();
                    }
                });
            });
        }
    });
    ui.data_mut(|data| data.insert_temp(draft_id(), draft));
}

/// The attached song: its cover, its title in its source's colour and its
/// artists, sliding in from the field it came from. Returns whether its
/// remove button was clicked.
fn attachment(
    ui: &mut Ui,
    palette: &Palette,
    post: &NewPost,
    t: f32,
    locale: crate::i18n::Locale,
) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::hover());
    let rect = rect.translate(vec2(-12.0 * (1.0 - t), 0.0));
    let painter = ui.painter();
    painter.rect_filled(rect, 10.0, palette.surface.gamma_multiply(t));
    let cover = Rect::from_min_size(rect.min + vec2(8.0, 8.0), Vec2::splat(48.0));
    super::widgets::paint_cover(
        ui,
        palette,
        post.artwork_url.as_deref(),
        cover,
        6.0,
        Icon::Music,
        None,
    );
    let left = cover.right() + 12.0;
    let width = (rect.right() - left - 44.0).max(40.0);
    let title = if post.title.is_empty() {
        "Untitled"
    } else {
        post.title.as_str()
    };
    let title = super::widgets::ellipsized(
        ui,
        title,
        theme::medium(14.0),
        source_of(post.source).color(palette).gamma_multiply(t),
        width,
        1,
    );
    painter.galley(pos2(left, cover.top() + 4.0), title, Color32::WHITE);
    let meta = if post.artists.is_empty() {
        source_name(post.source).to_owned()
    } else {
        format!("{} · {}", post.artists.join(", "), source_name(post.source))
    };
    let meta = super::widgets::ellipsized(
        ui,
        &meta,
        theme::regular(12.5),
        palette.secondary.gamma_multiply(t),
        width,
        1,
    );
    painter.galley(pos2(left, cover.top() + 26.0), meta, Color32::WHITE);
    let close = Rect::from_center_size(
        pos2(rect.right() - 22.0, rect.center().y),
        Vec2::splat(28.0),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(close));
    theme::icon_button(
        &mut child,
        Icon::X,
        16.0,
        palette.dim,
        palette.text,
        &gettext(locale, "Remove"),
    )
    .clicked()
}

fn source_of(source: PostSource) -> Source {
    match source {
        PostSource::Spotify => Source::Spotify,
        PostSource::Soundcloud => Source::SoundCloud,
        PostSource::Youtube => Source::YouTube,
    }
}

fn source_name(source: PostSource) -> &'static str {
    match source {
        PostSource::Spotify => "Spotify",
        PostSource::Soundcloud => "SoundCloud",
        PostSource::Youtube => "YouTube",
    }
}

/// Who a share goes to: everyone, or the friends picked. A picked chip
/// fills in and its check grows in; Everyone clears the picks.
fn recipients(
    ui: &mut Ui,
    palette: &Palette,
    members: &[Profile],
    me: &Profile,
    to: &mut Vec<String>,
) {
    let others: Vec<&Profile> = members.iter().filter(|m| m.user_id != me.user_id).collect();
    if others.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        theme::text(ui, "To", theme::regular(13.0), palette.dim);
        if chip(
            ui,
            palette,
            "Everyone",
            to.is_empty(),
            Id::new("group-to-everyone"),
        )
        .clicked()
        {
            to.clear();
        }
        for member in others {
            let picked = to.contains(&member.user_id);
            if chip(
                ui,
                palette,
                &member.display_name,
                picked,
                Id::new(("group-to", &member.user_id)),
            )
            .clicked()
            {
                if picked {
                    to.retain(|id| *id != member.user_id);
                } else {
                    to.push(member.user_id.clone());
                }
            }
        }
    });
}

/// A small toggle with a check that grows in when it is on.
fn chip(ui: &mut Ui, palette: &Palette, label: &str, on: bool, id: Id) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), theme::medium(12.5), palette.text);
    let check = 14.0;
    let size = vec2(galley.size().x + 20.0 + check + 4.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let t = motion::toward(ui.ctx(), id, on, motion::FEEDBACK);
    let scale = motion::press_scale(ui.ctx(), &response);
    let rect = Rect::from_center_size(rect.center(), rect.size() * scale);
    let fill = super::blend(
        if response.hovered() {
            palette.surface_hover
        } else {
            palette.surface
        },
        palette.text,
        0.9 * t,
    );
    let text = super::blend(palette.secondary, palette.window, t);
    let painter = ui.painter();
    painter.rect(
        rect,
        8.0,
        fill,
        Stroke::new(1.0, palette.outline.gamma_multiply(1.0 - t)),
        egui::StrokeKind::Inside,
    );
    // The check takes its room as it grows, pushing the label right.
    let check_rect = Rect::from_center_size(
        pos2(rect.left() + 10.0 + check * t / 2.0, rect.center().y),
        Vec2::splat(check * t),
    );
    if t > 0.01 {
        theme::paint_icon(ui, Icon::Check, check_rect, check * t, text);
    }
    let label_left = rect.left() + 10.0 + (check + 4.0) * t;
    painter.galley(
        pos2(label_left, rect.center().y - galley.size().y / 2.0),
        galley,
        text,
    );
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, label));
    theme::focus_ring(ui, &response);
    response
}

/// The send button: the primary action, whose paper plane flies off when
/// a share goes out.
fn send_button(ui: &mut Ui, palette: &Palette, label: &str, enabled: bool) -> egui::Response {
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), theme::medium(13.5), palette.on_accent);
    let size = vec2(galley.size().x + 52.0, 36.0);
    let (rect, response) = ui.allocate_exact_size(
        size,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let scale = motion::press_scale(ui.ctx(), &response);
    let rect = Rect::from_center_size(rect.center(), rect.size() * scale);
    let fill = if !enabled {
        palette.accent.gamma_multiply(0.4)
    } else if response.hovered() {
        palette.accent_hover
    } else {
        palette.accent
    };
    ui.painter().rect_filled(rect, 10.0, fill);
    let icon_center = pos2(rect.left() + 22.0, rect.center().y);
    let (offset, icon_scale, alpha) = motion::running(ui.ctx(), Id::new("group-sent"))
        .map_or((Vec2::ZERO, 1.0, 1.0), motion::fly_off);
    theme::paint_icon(
        ui,
        Icon::Send,
        Rect::from_center_size(icon_center + offset, Vec2::splat(16.0)),
        16.0 * icon_scale,
        palette.on_accent.gamma_multiply(alpha),
    );
    ui.painter().galley(
        pos2(rect.left() + 36.0, rect.center().y - galley.size().y / 2.0),
        galley,
        palette.on_accent,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    theme::focus_ring(ui, &response);
    response
}

/// Everything, sent to you, from you: a selection that slides between
/// them, the feed following.
fn filters(ui: &mut Ui, palette: &Palette) -> Filter {
    let id = Id::new("group-filter");
    let mut filter = ui
        .data(|data| data.get_temp::<Filter>(id))
        .unwrap_or_default();
    let labels: Vec<_> = Filter::ALL
        .iter()
        .map(|f| {
            ui.painter()
                .layout_no_wrap(f.label().to_owned(), theme::medium(13.0), palette.text)
        })
        .collect();
    let widths: Vec<f32> = labels.iter().map(|g| g.size().x + 24.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 8.0;
    let (rect, _) = ui.allocate_exact_size(vec2(total, 34.0), Sense::hover());
    ui.painter().rect_filled(rect, 10.0, palette.surface);
    let mut x = rect.left() + 4.0;
    let mut rects = Vec::new();
    for width in &widths {
        rects.push(Rect::from_min_size(
            pos2(x, rect.top() + 4.0),
            vec2(*width, 26.0),
        ));
        x += width;
    }
    let selected = Filter::ALL.iter().position(|f| *f == filter).unwrap_or(0);
    let left = motion::glide(
        ui.ctx(),
        id.with("left"),
        rects[selected].left(),
        motion::POPOVER,
    );
    let width = motion::glide(
        ui.ctx(),
        id.with("width"),
        rects[selected].width(),
        motion::POPOVER,
    );
    ui.painter().rect(
        Rect::from_min_size(pos2(left, rects[selected].top()), vec2(width, 26.0)),
        7.0,
        palette.panel,
        Stroke::new(1.0, palette.outline),
        egui::StrokeKind::Inside,
    );
    for ((option, galley), slot) in Filter::ALL.iter().zip(labels).zip(&rects) {
        let response = ui.interact(*slot, id.with(option.label()), Sense::click());
        let on = *option == filter;
        let color = if on || response.hovered() {
            palette.text
        } else {
            palette.secondary
        };
        ui.painter()
            .galley(slot.center() - galley.size() / 2.0, galley, color);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, on, option.label())
        });
        theme::focus_ring(ui, &response);
        if response.clicked() && !on {
            filter = *option;
            motion::begin(ui.ctx(), Id::new("group-feed-enter"), motion::ENTER);
        }
    }
    ui.data_mut(|data| data.insert_temp(id, filter));
    filter
}

/// A day in the feed: Today, Yesterday, or the date.
fn day_label(created_at: u64) -> (jiff::civil::Date, String) {
    let zone = jiff::tz::TimeZone::system();
    let at =
        jiff::Timestamp::from_millisecond(created_at as i64).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    let day = at.to_zoned(zone.clone()).date();
    let today = jiff::Timestamp::now().to_zoned(zone).date();
    let label = if day == today {
        "Today".to_owned()
    } else if day.checked_add(jiff::Span::new().days(1)).ok() == Some(today) {
        "Yesterday".to_owned()
    } else if day.year() == today.year() {
        day.strftime("%A %-d %B").to_string()
    } else {
        day.strftime("%-d %B %Y").to_string()
    };
    (day, label)
}

fn feed_list(
    app: &mut App,
    ui: &mut Ui,
    posts: &[Post],
    me: &Profile,
    divider: u64,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    // Switching the filter brings the feed in from below, once.
    let enter = motion::running(ui.ctx(), Id::new("group-feed-enter")).unwrap_or(1.0);
    let mut last_day = None;
    let mut marked_new = divider == 0;
    for (index, post) in posts.iter().enumerate() {
        let (day, label) = day_label(post.created_at);
        if last_day != Some(day) {
            last_day = Some(day);
            ui.add_space(if index == 0 { 0.0 } else { 8.0 });
            separator(ui, &palette, &label, false);
        }
        // Everything above the line arrived since the last visit.
        if !marked_new && post.created_at <= divider {
            marked_new = true;
            if index > 0 {
                separator(ui, &palette, "Earlier", true);
            }
        }
        let fresh = post.created_at > divider && divider != 0 && post.author_id != me.user_id;
        let delay = motion::stagger(index);
        let arrive = motion::appear(
            ui.ctx(),
            Id::new(("group-post-arrive", &post.id)),
            motion::ENTER,
            delay,
        );
        let t = arrive.min(if index < motion::MAX_STAGGERED {
            ((enter - delay / motion::ENTER).max(0.0) / (1.0 - delay / motion::ENTER)).min(1.0)
        } else {
            enter
        });
        message(app, ui, post, me, fresh, t, actions);
        ui.add_space(14.0);
    }
}

/// A line across the feed with a label on it: a day, or where the new
/// shares end.
fn separator(ui: &mut Ui, palette: &Palette, label: &str, subtle: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        theme::medium(12.0),
        if subtle {
            palette.dim
        } else {
            palette.secondary
        },
    );
    let text_left = rect.left() + GUTTER;
    let y = rect.center().y;
    let line = Stroke::new(1.0, palette.outline);
    ui.painter().hline(rect.left()..=text_left - 8.0, y, line);
    ui.painter().galley(
        pos2(text_left, y - galley.size().y / 2.0),
        galley.clone(),
        Color32::WHITE,
    );
    ui.painter()
        .hline(text_left + galley.size().x + 8.0..=rect.right(), y, line);
}

/// The play action for a share: Spotify songs, albums and playlists play
/// here; SoundCloud and YouTube links go through the app's link handling,
/// which plays them; anything else opens in the browser.
pub(crate) fn play_action(url: &str) -> Action {
    match crate::link::parse(url) {
        Some(uri) if uri.starts_with("spotify:track:") => Action::PlayUris {
            uris: vec![uri],
            index: 0,
        },
        Some(uri) if uri.starts_with("spotify:album:") || uri.starts_with("spotify:playlist:") => {
            Action::PlayContext {
                uri,
                offset_uri: None,
                offset_index: None,
            }
        }
        Some(uri) => Action::OpenLink(uri),
        None => Action::OpenUrl(url.to_owned()),
    }
}

/// One share: who sent it and when, their note, the song as an attachment
/// with its tools, the file once downloaded, and the thread.
fn message(
    app: &mut App,
    ui: &mut Ui,
    post: &Post,
    me: &Profile,
    fresh: bool,
    t: f32,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    let locale = app.locale;
    let pending = post.id == PENDING_SHARE;
    // Rises 10 points into place as it arrives; a share still on its way
    // to the service stays a little faded until it lands.
    let landed = motion::toward(
        ui.ctx(),
        Id::new(("group-landed", &post.item.url, post.created_at)),
        !pending,
        motion::ENTER,
    );
    let alpha = t * (0.55 + 0.45 * landed);
    let rise = (1.0 - t) * 10.0;
    let top = ui.cursor().top();
    let left = ui.max_rect().left();
    let width = ui.available_width();

    ui.scope(|ui| {
        ui.set_opacity(alpha);
        let body_left = left + GUTTER;
        let body_width = width - GUTTER;
        // Header: avatar, name, time and who it was for.
        let (head, _) = ui.allocate_exact_size(vec2(width, 22.0), Sense::hover());
        let head = head.translate(vec2(0.0, rise));
        avatar(
            ui,
            &palette,
            pos2(left + AVATAR / 2.0, top + AVATAR / 2.0 + rise),
            AVATAR,
            &post.author_name,
            false,
        );
        let painter = ui.painter();
        let name =
            painter.layout_no_wrap(post.author_name.clone(), theme::medium(14.0), palette.text);
        let name_width = name.size().x;
        painter.galley(
            pos2(body_left, head.center().y - name.size().y / 2.0),
            name,
            Color32::WHITE,
        );
        let mut meta = if pending {
            gettext(locale, "Sending…").into_owned()
        } else {
            super::group_shelf::ago(locale, post.created_at)
        };
        if !post.item.to.is_empty() {
            let names: Vec<String> = post
                .item
                .to
                .iter()
                .map(|id| {
                    if *id == me.user_id {
                        "you".to_owned()
                    } else {
                        app.friends_feed
                            .members
                            .iter()
                            .find(|m| m.user_id == *id)
                            .map_or_else(
                                || "a former member".to_owned(),
                                |m| m.display_name.clone(),
                            )
                    }
                })
                .collect();
            meta = format!("{meta} · to {}", names.join(", "));
        }
        painter.text(
            pos2(body_left + name_width + 8.0, head.center().y),
            Align2::LEFT_CENTER,
            meta,
            theme::regular(12.5),
            palette.dim,
        );
        if fresh {
            // A small dot beside the name of what is new since last time.
            painter.circle_filled(
                pos2(left + AVATAR - 3.0, top + 3.0 + rise),
                4.0,
                palette.text,
            );
        }

        // The note, as the message itself.
        if !post.item.note.is_empty() {
            wrapped(
                ui,
                &post.item.note,
                theme::regular(14.5),
                palette.text,
                body_left,
                body_width,
                rise,
            );
        }
        ui.add_space(2.0);
        song(app, ui, post, me, body_left, body_width, pending, actions);
        file_chip(app, ui, post, body_left, body_width, actions);
        thread(app, ui, post, me, body_left, body_width, pending, actions);
    });
    // The avatar takes its own height even for a share with no note.
    let used = ui.cursor().top() - top;
    if used < AVATAR {
        ui.add_space(AVATAR - used);
    }
}

/// Text wrapped to `width`, laid out once and painted at `left` (shifted
/// down by `rise` while its message arrives), taking only its own height.
fn wrapped(
    ui: &mut Ui,
    text: &str,
    font: egui::FontId,
    color: Color32,
    left: f32,
    width: f32,
    rise: f32,
) {
    let galley = crate::bidi::layout(ui.painter(), text, font, color, width, 12, None);
    let (row, _) = ui.allocate_exact_size(
        vec2(
            left + galley.size().x - ui.max_rect().left(),
            galley.size().y,
        ),
        Sense::hover(),
    );
    ui.painter()
        .galley(pos2(left, row.top() + rise), galley, color);
}

/// The shared song as an attachment: cover with a play button on hover,
/// title in its source's colour, artists and length, and its tools.
#[allow(clippy::too_many_arguments)]
fn song(
    app: &mut App,
    ui: &mut Ui,
    post: &Post,
    me: &Profile,
    left: f32,
    width: f32,
    pending: bool,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    let locale = app.locale;
    let item = &post.item;
    let (row, _) = ui.allocate_exact_size(
        vec2(left + width - ui.max_rect().left(), COVER + 16.0),
        Sense::hover(),
    );
    let rect = Rect::from_min_max(pos2(left, row.top()), row.max);
    let id = Id::new(("group-song", &post.id));
    let response = ui.interact(rect, id, Sense::click());
    let hover = motion::toward(
        ui.ctx(),
        id.with("hover"),
        response.hovered(),
        motion::FEEDBACK,
    );
    let painter = ui.painter().clone();
    painter.rect(
        rect,
        12.0,
        super::blend(palette.surface, palette.surface_hover, hover).gamma_multiply(0.8),
        Stroke::new(1.0, palette.outline),
        egui::StrokeKind::Inside,
    );
    // A band of light crosses the card when its song starts from here.
    if let Some(sweep) = motion::running(ui.ctx(), id.with("sweep")) {
        let x = rect.left() + (rect.width() + 160.0) * sweep - 80.0;
        let band = Rect::from_center_size(pos2(x, rect.center().y), vec2(120.0, rect.height()));
        painter
            .with_clip_rect(rect.intersect(ui.clip_rect()))
            .rect_filled(band, 0.0, palette.text.gamma_multiply(0.05 * (1.0 - sweep)));
    }
    let cover = Rect::from_min_size(rect.min + vec2(8.0, 8.0), Vec2::splat(COVER));
    super::widgets::paint_cover(
        ui,
        &palette,
        item.artwork_url.as_deref(),
        cover,
        8.0,
        Icon::Music,
        None,
    );
    // Play grows in over the cover as the card is pointed at.
    if hover > 0.01 && !pending {
        painter.rect_filled(cover, 8.0, Color32::from_black_alpha((110.0 * hover) as u8));
        theme::paint_icon(
            ui,
            Icon::PlayFilled,
            Rect::from_center_size(
                cover.center() + theme::play_glyph_offset(Icon::PlayFilled, 18.0),
                Vec2::splat(18.0),
            ),
            14.0 + 4.0 * hover,
            Color32::WHITE.gamma_multiply(hover),
        );
    }
    let tools_width = 4.0 * 32.0 + 12.0;
    let text_left = cover.right() + 12.0;
    let text_width = (rect.right() - tools_width - text_left).max(60.0);
    let title = if item.title.is_empty() {
        "Untitled"
    } else {
        item.title.as_str()
    };
    let galley = super::widgets::ellipsized(
        ui,
        title,
        theme::medium(14.5),
        source_of(item.source).color(&palette),
        text_width,
        1,
    );
    painter.galley(pos2(text_left, cover.top() + 8.0), galley, Color32::WHITE);
    let mut meta = item.artists.join(", ");
    if item.duration_ms > 0 {
        let seconds = item.duration_ms / 1000;
        let length = format!("{}:{:02}", seconds / 60, seconds % 60);
        meta = if meta.is_empty() {
            length
        } else {
            format!("{meta} · {length}")
        };
    }
    if meta.is_empty() {
        meta = source_name(item.source).to_owned();
    }
    let galley = super::widgets::ellipsized(
        ui,
        &meta,
        theme::regular(13.0),
        palette.secondary,
        text_width,
        1,
    );
    painter.galley(pos2(text_left, cover.top() + 30.0), galley, Color32::WHITE);

    // Clicking the card (not its tools) plays it.
    let play = play_action(&item.url);
    let cover_hit = ui
        .interact(cover, id.with("cover"), Sense::click())
        .on_hover_text(gettext(locale, "Play"));
    if !pending && (response.double_clicked() || cover_hit.clicked()) {
        actions.push(play.clone());
        motion::begin(ui.ctx(), id.with("sweep"), motion::SWEEP);
    }

    // Tools, right to left: more, replies, heart, download.
    if pending {
        return;
    }
    let mut x = rect.right() - 8.0 - 16.0;
    let cy = rect.center().y;
    let mut tool = |ui: &mut Ui,
                    icon: Icon,
                    tip: &str,
                    color: Color32,
                    salt: &str|
     -> (egui::Response, Rect) {
        let r = Rect::from_center_size(pos2(x, cy), Vec2::splat(32.0));
        x -= 34.0;
        let response = ui
            .interact(r, id.with(salt), Sense::click())
            .on_hover_text(tip);
        let lift = motion::toward(
            ui.ctx(),
            id.with((salt, "hover")),
            response.hovered(),
            motion::FEEDBACK,
        );
        if lift > 0.0 {
            ui.painter()
                .rect_filled(r, 8.0, palette.surface_active.gamma_multiply(lift));
        }
        let scale = motion::press_scale(ui.ctx(), &response);
        theme::paint_icon(
            ui,
            icon,
            r,
            16.0 * scale,
            if response.hovered() {
                palette.text
            } else {
                color
            },
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tip));
        theme::focus_ring(ui, &response);
        (response, r)
    };

    // More: queue, open, copy link, delete.
    let (more, _) = tool(
        ui,
        Icon::Ellipsis,
        &gettext(locale, "More"),
        palette.dim,
        "more",
    );
    egui::Popup::menu(&more).show(|ui| {
        ui.set_min_width(200.0);
        if let Some(uri) =
            crate::link::parse(&item.url).filter(|uri| uri.starts_with("spotify:track:"))
            && ui.button(gettext(locale, "Add to queue")).clicked()
        {
            actions.push(Action::AddToQueue {
                uri,
                label: item.title.clone(),
            });
        }
        if ui.button(gettext(locale, "Open in browser")).clicked() {
            actions.push(Action::OpenUrl(item.url.clone()));
        }
        if ui.button(gettext(locale, "Copy link")).clicked() {
            ui.ctx().copy_text(item.url.clone());
        }
        if post.author_id == me.user_id && ui.button(gettext(locale, "Delete share")).clicked() {
            actions.push(Action::Friends(Group::Delete(post.id.clone())));
        }
    });

    // Replies, with their count.
    let open_id = Id::new(("group-thread-open", &post.id));
    let open = ui
        .data(|data| data.get_temp::<bool>(open_id))
        .unwrap_or(false);
    let (replies, replies_rect) = tool(
        ui,
        Icon::MessageCircle,
        &gettext(locale, "Reply"),
        if open { palette.text } else { palette.dim },
        "reply",
    );
    if !post.replies.is_empty() {
        motion::rolling_count(
            ui,
            id.with("reply-count"),
            pos2(replies_rect.right() - 6.0, replies_rect.top() + 7.0),
            post.replies.len() as u64,
            theme::medium(10.5),
            palette.secondary,
        );
    }
    if replies.clicked() {
        ui.data_mut(|data| data.insert_temp(open_id, !open));
        if !open {
            ui.memory_mut(|memory| memory.request_focus(Id::new(("group-reply-field", &post.id))));
        }
    }

    // Heart: pops when given, the count rolls.
    let liked = post.liked;
    let label = if liked {
        gettext(locale, "Remove heart")
    } else {
        gettext(locale, "Heart")
    };
    let r = Rect::from_center_size(pos2(x, cy), Vec2::splat(32.0));
    x -= 34.0;
    let heart = ui
        .interact(r, id.with("heart"), Sense::click())
        .on_hover_text(label.as_ref());
    let pop = ui.ctx().animate_bool_with_time(
        id.with("pop"),
        liked,
        crate::motion::seconds(ui.ctx(), motion::POP),
    );
    let swell = if liked { motion::pop(pop) } else { 1.0 } * motion::press_scale(ui.ctx(), &heart);
    if heart.hovered() {
        ui.painter().rect_filled(r, 8.0, palette.surface_active);
    }
    theme::paint_icon(
        ui,
        if liked {
            Icon::HeartFilled
        } else {
            Icon::Heart
        },
        r,
        16.0 * swell,
        if liked || heart.hovered() {
            palette.text
        } else {
            palette.dim
        },
    );
    // Six dots burst out of a heart as it is given.
    if liked {
        motion::burst(ui.painter(), r.center(), pop, palette.text);
    }
    if post.like_count > 0 {
        motion::rolling_count(
            ui,
            id.with("heart-count"),
            pos2(r.right() - 6.0, r.top() + 7.0),
            u64::from(post.like_count),
            theme::medium(10.5),
            palette.secondary,
        );
    }
    heart.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, liked, label.as_ref())
    });
    theme::focus_ring(ui, &heart);
    if heart.clicked() {
        actions.push(Action::Friends(Group::Like {
            id: post.id.clone(),
            liked: !liked,
        }));
    }

    // Download, or its progress while it runs.
    let progress = app
        .download_keys
        .iter()
        .find(|(_, key)| **key == item.url)
        .and_then(|(id, _)| app.downloads.active.get(id))
        .map(|progress| progress.fraction.unwrap_or(0.0));
    let r = Rect::from_center_size(pos2(x, cy), Vec2::splat(32.0));
    match progress {
        Some(fraction) => {
            let shown = motion::glide(ui.ctx(), id.with("download-fill"), fraction, motion::ENTER);
            ring(ui, r.center(), 9.0, shown, &palette);
            ui.interact(r, id.with("download"), Sense::hover())
                .on_hover_text(format!(
                    "{} {:.0}%",
                    gettext(locale, "Downloading"),
                    fraction * 100.0
                ));
        }
        None => {
            let available = app.downloads.available;
            let tip = if available {
                gettext(locale, "Download, ready to drag into your DAW").into_owned()
            } else {
                crate::downloads::INSTALL_HELP.to_owned()
            };
            let response = ui
                .interact(
                    r,
                    id.with("download"),
                    if available {
                        Sense::click()
                    } else {
                        Sense::hover()
                    },
                )
                .on_hover_text(tip.as_str());
            if response.hovered() && available {
                ui.painter().rect_filled(r, 8.0, palette.surface_active);
            }
            // The arrow dips as it is pressed.
            let dip = if response.is_pointer_button_down_on() {
                2.0
            } else {
                0.0
            };
            theme::paint_icon(
                ui,
                Icon::Download,
                r.translate(vec2(0.0, dip)),
                16.0,
                if !available {
                    palette.dim.gamma_multiply(0.5)
                } else if response.hovered() {
                    palette.text
                } else {
                    palette.dim
                },
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, available, tip.as_str())
            });
            theme::focus_ring(ui, &response);
            if response.clicked() {
                actions.push(Action::DownloadShare(post.id.clone()));
            }
        }
    }
}

/// A progress ring with real progress and a stop square in it.
fn ring(ui: &Ui, center: egui::Pos2, radius: f32, fraction: f32, palette: &Palette) {
    let painter = ui.painter();
    painter.circle_stroke(center, radius, Stroke::new(2.0, palette.outline));
    let steps = 40;
    let shown = (fraction.clamp(0.0, 1.0) * steps as f32).ceil() as usize;
    if shown > 0 {
        let points: Vec<egui::Pos2> = (0..=shown)
            .map(|step| {
                let angle = step as f32 / steps as f32 * std::f32::consts::TAU
                    - std::f32::consts::FRAC_PI_2;
                center + Vec2::angled(angle) * radius
            })
            .collect();
        painter.add(egui::Shape::line(points, Stroke::new(2.0, palette.text)));
    }
    painter.rect_filled(
        Rect::from_center_size(center, Vec2::splat(6.0)),
        1.0,
        palette.text,
    );
}

/// The downloaded file under its song: drag it into FL Studio or any DAW,
/// or click to see it in its folder. It slides in when the download ends.
fn file_chip(
    app: &mut App,
    ui: &mut Ui,
    post: &Post,
    left: f32,
    width: f32,
    actions: &mut Vec<Action>,
) {
    let Some(path) = app.downloaded_files.get(&post.item.url).cloned() else {
        return;
    };
    let palette = app.palette;
    let locale = app.locale;
    let id = Id::new(("group-file", &post.id));
    let t = motion::appear(ui.ctx(), id.with("arrive"), motion::ENTER, 0.0);
    ui.add_space(6.0 * t);
    let height = 40.0 * t;
    let (row, _) = ui.allocate_exact_size(
        vec2(left + width - ui.max_rect().left(), height),
        Sense::hover(),
    );
    if t <= 0.01 {
        return;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let galley = ui
        .painter()
        .layout_no_wrap(name, theme::medium(13.0), palette.text);
    let hint = if crate::drag_out::SUPPORTED {
        gettext(locale, "Drag into FL Studio or any DAW")
    } else {
        gettext(locale, "Show in folder")
    };
    let hint = ui
        .painter()
        .layout_no_wrap(hint.into_owned(), theme::regular(12.0), palette.dim);
    let chip_width = (galley.size().x + hint.size().x + 64.0).min(width);
    let rect = Rect::from_min_size(pos2(left, row.top()), vec2(chip_width, 40.0))
        .translate(vec2(0.0, -6.0 * (1.0 - t)));
    let response = ui
        .interact(rect, id, Sense::click_and_drag())
        .on_hover_cursor(egui::CursorIcon::Grab)
        .on_hover_text(path.display().to_string());
    let lifted = motion::toward(
        ui.ctx(),
        id.with("lift"),
        response.hovered() || response.dragged(),
        motion::FEEDBACK,
    );
    let painter = ui.painter();
    if lifted > 0.0 {
        super::widgets::paint_shadow(ui, &palette, rect.translate(vec2(0.0, 2.0 * lifted)), 10.0);
    }
    painter.rect(
        rect.translate(vec2(0.0, -lifted)),
        10.0,
        super::blend(palette.surface, palette.surface_hover, lifted),
        Stroke::new(1.0, palette.outline),
        egui::StrokeKind::Inside,
    );
    let rect = rect.translate(vec2(0.0, -lifted));
    theme::paint_icon(
        ui,
        Icon::FileAudio,
        Rect::from_center_size(pos2(rect.left() + 20.0, rect.center().y), Vec2::splat(16.0)),
        16.0,
        palette.secondary,
    );
    let text_left = rect.left() + 38.0;
    let clip = painter.with_clip_rect(rect.shrink(4.0).intersect(ui.clip_rect()));
    clip.galley(
        pos2(text_left, rect.center().y - galley.size().y / 2.0),
        galley.clone(),
        Color32::WHITE,
    );
    clip.galley(
        pos2(
            text_left + galley.size().x + 10.0,
            rect.center().y - hint.size().y / 2.0,
        ),
        hint,
        Color32::WHITE,
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Downloaded file")
    });
    theme::focus_ring(ui, &response);
    if response.drag_started() {
        actions.push(Action::DragFileOut(path));
    } else if response.clicked() {
        actions.push(Action::RevealFile(path));
    }
}

/// The replies under a share, and the field to add one. Long threads fold
/// to their latest replies; the thread unfolds when opened.
#[allow(clippy::too_many_arguments)]
fn thread(
    app: &mut App,
    ui: &mut Ui,
    post: &Post,
    me: &Profile,
    left: f32,
    width: f32,
    pending: bool,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    let locale = app.locale;
    let open_id = Id::new(("group-thread-open", &post.id));
    let open = ui
        .data(|data| data.get_temp::<bool>(open_id))
        .unwrap_or(false);
    let unfold = motion::toward(ui.ctx(), open_id.with("unfold"), open, motion::ENTER);
    if post.replies.is_empty() && unfold <= 0.0 || pending {
        return;
    }
    ui.add_space(8.0);
    let all = open || post.replies.len() <= FOLDED_REPLIES + 1;
    let shown: Vec<&Reply> = if all {
        post.replies.iter().collect()
    } else {
        post.replies
            .iter()
            .skip(post.replies.len() - FOLDED_REPLIES)
            .collect()
    };
    let indent = left - ui.max_rect().left();
    if !all {
        ui.horizontal(|ui| {
            ui.add_space(indent);
            let more = post.replies.len() - FOLDED_REPLIES;
            let label = if more == 1 {
                gettext(locale, "Show 1 earlier reply").into_owned()
            } else {
                gettext(locale, "Show {n} earlier replies").replace("{n}", &more.to_string())
            };
            if theme::link(ui, label, theme::medium(12.5), palette.secondary).clicked() {
                ui.data_mut(|data| data.insert_temp(open_id, true));
            }
        });
        ui.add_space(4.0);
    }
    for reply in shown {
        let pending_reply = reply.id.starts_with(PENDING_REPLY);
        // A new reply slides in from the left of its place.
        let arrive = motion::appear(
            ui.ctx(),
            Id::new(("group-reply-arrive", &reply.id, reply.created_at)),
            motion::ENTER,
            0.0,
        );
        let top = ui.cursor().top();
        ui.scope(|ui| {
            ui.set_opacity(arrive * if pending_reply { 0.6 } else { 1.0 });
            let text_left = left + REPLY_AVATAR + 8.0 - 8.0 * (1.0 - arrive);
            let text_width = width - REPLY_AVATAR - 8.0;
            let (head, _) =
                ui.allocate_exact_size(vec2(width + indent, REPLY_AVATAR), Sense::hover());
            let name = ui.painter().layout_no_wrap(
                reply.author_name.clone(),
                theme::medium(13.0),
                palette.text,
            );
            let name_width = name.size().x;
            ui.painter().galley(
                pos2(text_left, head.center().y - name.size().y / 2.0),
                name,
                Color32::WHITE,
            );
            let when = if pending_reply {
                gettext(locale, "Sending…").into_owned()
            } else {
                super::group_shelf::ago(locale, reply.created_at)
            };
            ui.painter().text(
                pos2(text_left + name_width + 8.0, head.center().y),
                Align2::LEFT_CENTER,
                when,
                theme::regular(12.0),
                palette.dim,
            );
            wrapped(
                ui,
                &reply.text,
                theme::regular(14.0),
                palette.text,
                text_left,
                text_width,
                0.0,
            );
            // Your own reply can be taken back from its row.
            let row = Rect::from_min_max(
                pos2(left, head.top()),
                pos2(left + width, ui.cursor().top()),
            );
            if reply.author_id == me.user_id && !pending_reply && ui.rect_contains_pointer(row) {
                let bin = Rect::from_center_size(
                    pos2(left + width - 14.0, head.center().y),
                    Vec2::splat(24.0),
                );
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bin));
                if theme::icon_button(
                    &mut child,
                    Icon::Trash,
                    13.0,
                    palette.dim,
                    palette.text,
                    &gettext(locale, "Delete reply"),
                )
                .clicked()
                {
                    actions.push(Action::Friends(Group::DeleteReply {
                        post: post.id.clone(),
                        reply: reply.id.clone(),
                    }));
                }
            }
        });
        avatar(
            ui,
            &palette,
            pos2(left + REPLY_AVATAR / 2.0, top + REPLY_AVATAR / 2.0),
            REPLY_AVATAR,
            &reply.author_name,
            false,
        );
        ui.add_space(6.0);
    }

    // The reply field unfolds when the thread is opened.
    if unfold > 0.0 {
        let field_id = Id::new(("group-reply-field", &post.id));
        let text_id = Id::new(("group-reply-text", &post.id));
        let mut text = ui
            .data(|data| data.get_temp::<String>(text_id))
            .unwrap_or_default();
        ui.add_space(2.0 * unfold);
        ui.horizontal(|ui| {
            ui.set_opacity(unfold);
            ui.add_space(indent);
            ui.set_max_height(36.0 * unfold);
            let hint = gettext(locale, "Reply to {name}").replace("{name}", &post.author_name);
            let mut send = false;
            egui::Frame::new()
                .fill(palette.surface)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(theme::RADIUS))
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut text)
                                .id(field_id)
                                .hint_text(hint)
                                .char_limit(500)
                                .frame(egui::Frame::NONE)
                                .font(theme::regular(14.0))
                                .desired_width(width - 64.0),
                        );
                        if response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter))
                        {
                            send = true;
                        }
                        if theme::icon_button(
                            ui,
                            Icon::Send,
                            14.0,
                            palette.dim,
                            palette.text,
                            &gettext(locale, "Send reply"),
                        )
                        .clicked()
                        {
                            send = true;
                        }
                    });
                });
            if send && !text.trim().is_empty() {
                actions.push(Action::Friends(Group::Reply {
                    post: post.id.clone(),
                    text: text.trim().to_owned(),
                }));
                text.clear();
                ui.memory_mut(|memory| memory.request_focus(field_id));
            }
        });
        ui.data_mut(|data| data.insert_temp(text_id, text));
    }
}

/// The member's own settings, folded away at the end of the page.
fn identity(app: &App, ui: &mut Ui, me: &Profile, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    let id = Id::new("group-identity-open");
    let open = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(false);
    ui.horizontal(|ui| {
        let chevron = motion::toward(ui.ctx(), id.with("turn"), open, motion::FEEDBACK);
        let response = theme::link(
            ui,
            gettext(locale, "Your group settings"),
            theme::medium(13.0),
            palette.secondary,
        );
        let icon = if chevron > 0.5 {
            Icon::ChevronUp
        } else {
            Icon::ChevronDown
        };
        theme::icon(ui, icon, 14.0, palette.dim);
        if response.clicked() {
            ui.data_mut(|data| data.insert_temp(id, !open));
        }
    });
    if !open {
        return;
    }
    let name_id = Id::new("group-rename");
    let mut name = ui
        .data(|data| data.get_temp::<String>(name_id))
        .unwrap_or_else(|| me.display_name.clone());
    ui.add_space(8.0);
    card(ui, &palette, |ui| {
        theme::text(
            ui,
            gettext(locale, "Your name in the group"),
            theme::medium(13.0),
            palette.text,
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.allocate_ui(vec2(260.0, 36.0), |ui| {
                field(ui, &palette, &mut name, &gettext(locale, "Name"), false, 40);
            });
            let changed = !name.trim().is_empty() && name.trim() != me.display_name;
            if ui
                .add_enabled_ui(changed, |ui| {
                    theme::soft_button(ui, &palette, None, &gettext(locale, "Rename"), false)
                })
                .inner
                .clicked()
            {
                actions.push(Action::Friends(Group::Rename(name.trim().to_owned())));
            }
        });
        ui.add_space(12.0);
        theme::text(
            ui,
            gettext(
                locale,
                "Leaving revokes your secret. Your shares stay, as Former member",
            ),
            theme::regular(12.5),
            palette.dim,
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if theme::soft_button(
                ui,
                &palette,
                Some(Icon::LogOut),
                &gettext(locale, "Leave group"),
                false,
            )
            .clicked()
            {
                actions.push(Action::Friends(Group::LeaveGroup));
            }
            if theme::soft_button(
                ui,
                &palette,
                None,
                &gettext(locale, "Forget this computer"),
                false,
            )
            .clicked()
            {
                actions.push(Action::Friends(Group::Leave));
            }
        });
    });
    ui.data_mut(|data| data.insert_temp(name_id, name));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_links_are_recognised_by_source() {
        assert_eq!(
            link_source("https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC"),
            Some(PostSource::Spotify)
        );
        assert_eq!(
            link_source("https://open.spotify.com/album/4uLU6hMCjMI75M1A2tKUQC"),
            Some(PostSource::Spotify)
        );
        assert_eq!(
            link_source("https://open.spotify.com/artist/4uLU6hMCjMI75M1A2tKUQC"),
            None
        );
        assert_eq!(
            link_source("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            Some(PostSource::Youtube)
        );
        assert_eq!(
            link_source("https://soundcloud.com/some-artist/a-song/s-AbC123"),
            Some(PostSource::Soundcloud)
        );
        assert_eq!(link_source("not a link"), None);
    }

    #[test]
    fn a_soundcloud_path_names_its_artist_and_song_without_its_secret() {
        let (artist, title) =
            soundcloud_names("https://soundcloud.com/some-artist/late-night-flip/s-AbC123?si=x");
        assert_eq!(artist.as_deref(), Some("some artist"));
        assert_eq!(title, "late night flip");
        let (artist, title) = soundcloud_names("https://soundcloud.com/crew/sets/beat-tape_2");
        assert_eq!(artist.as_deref(), Some("crew"));
        assert_eq!(title, "beat tape 2");
    }

    #[test]
    fn spotify_songs_play_here_and_other_links_go_through_link_handling() {
        assert!(matches!(
            play_action("https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC"),
            Action::PlayUris { ref uris, index: 0 } if uris == &["spotify:track:4uLU6hMCjMI75M1A2tKUQC".to_owned()]
        ));
        assert!(matches!(
            play_action("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M"),
            Action::PlayContext { .. }
        ));
        assert!(matches!(
            play_action("https://youtu.be/dQw4w9WgXcQ"),
            Action::OpenLink(_)
        ));
        assert!(matches!(
            play_action("https://example.com/x"),
            Action::OpenUrl(_)
        ));
    }

    #[test]
    fn filters_split_the_feed_by_who_it_is_for() {
        let post = |author: &str, to: &[&str]| Post {
            id: "x".into(),
            author_id: author.into(),
            author_name: author.into(),
            created_at: 0,
            like_count: 0,
            liked: false,
            replies: Vec::new(),
            affinity: None,
            item: NewPost {
                to: to.iter().map(|s| (*s).to_owned()).collect(),
                ..NewPost::from_url(
                    "https://youtu.be/dQw4w9WgXcQ".into(),
                    PostSource::Youtube,
                    "t".into(),
                )
            },
        };
        let to_me = post("maya", &["me"]);
        let open = post("maya", &[]);
        let mine = post("me", &["maya"]);
        assert!(Filter::ToYou.keeps(&to_me, "me"));
        assert!(!Filter::ToYou.keeps(&open, "me"));
        assert!(!Filter::ToYou.keeps(&mine, "me"));
        assert!(Filter::FromYou.keeps(&mine, "me"));
        assert!(Filter::Everyone.keeps(&open, "me"));
    }
}
