//! Who in the group is listening to what, and listening with them: play the
//! song by yourself, listen along with one person, or join a party, which has
//! a host at the controls and a queue that belongs to everyone in it.

use egui::{Id, RichText, Sense, Ui, Vec2, vec2};

use crate::app::App;
use crate::friends::live::{Action as Live, Channel, Entry, Place, Presence, Session};
use crate::friends::{NewPost, Profile, Source};
use crate::i18n::gettext;
use crate::model::Action;
use crate::theme::{self, Icon, Palette};

use super::group::{card, field, play_action, recognise};

const COVER: f32 = 44.0;
/// Below this a row's buttons go under its song rather than beside it.
const NARROW: f32 = 440.0;
/// The width a queue row keeps for its three buttons.
const QUEUE_BUTTONS: f32 = 112.0;

fn song_line(item: &NewPost) -> String {
    if item.artists.is_empty() {
        item.title.clone()
    } else {
        format!("{} · {}", item.title, item.artists.join(", "))
    }
}

/// One line that shortens rather than wraps or pushes the buttons away.
fn line(ui: &mut Ui, text: impl Into<String>, font: egui::FontId, color: egui::Color32) {
    ui.add(
        egui::Label::new(RichText::new(text.into()).font(font).color(color))
            .truncate()
            .selectable(false),
    );
}

fn name_of(app: &App, user: &str) -> String {
    app.friends_feed
        .members
        .iter()
        .find(|member| member.user_id == user)
        .map_or_else(
            || gettext(app.locale, "Someone").into_owned(),
            |member| member.display_name.clone(),
        )
}

fn cover(ui: &mut Ui, palette: &Palette, url: Option<&str>) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(COVER), Sense::hover());
    super::widgets::paint_cover(ui, palette, url, rect, 8.0, Icon::Music, None);
}

/// A cover and two lines beside the buttons `buttons` adds, or above them
/// when the column is narrow.
fn row(
    ui: &mut Ui,
    palette: &Palette,
    art: Option<&str>,
    title: &str,
    detail: &str,
    buttons_width: f32,
    buttons: impl FnOnce(&mut Ui),
) {
    let narrow = ui.available_width() < NARROW;
    let reserve = if narrow { 0.0 } else { buttons_width + 12.0 };
    let mut buttons = Some(buttons);
    ui.horizontal(|ui| {
        cover(ui, palette, art);
        let text_width = (ui.available_width() - reserve).max(40.0);
        ui.allocate_ui(vec2(text_width, COVER), |ui| {
            ui.set_width(text_width);
            ui.vertical(|ui| {
                ui.add_space(3.0);
                line(ui, title, theme::medium(13.5), palette.text);
                line(ui, detail, theme::regular(12.5), palette.dim);
            });
        });
        if !narrow {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(buttons) = buttons.take() {
                    buttons(ui);
                }
            });
        }
    });
    if let Some(buttons) = buttons.take() {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), buttons);
        });
    }
}

/// Everything live, above the composer.
pub fn section(app: &mut App, ui: &mut Ui, me: &Profile, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    if app.settings.group_sharing.is_none() {
        card(ui, &palette, |ui| {
            theme::text(
                ui,
                gettext(locale, "Let the group see what you're playing?"),
                theme::medium(14.0),
                palette.text,
            );
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(
                    RichText::new(gettext(
                        locale,
                        "Friends see your song while you play it and your listening stats, and can listen along with you. You can change this any time in your group settings.",
                    ))
                    .font(theme::regular(13.0))
                    .color(palette.secondary),
                )
                .wrap()
                .selectable(false),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if theme::pill_button(ui, &palette, &gettext(locale, "Share my listening"), true)
                    .clicked()
                {
                    actions.push(Action::Live(Live::SetSharing(true)));
                }
                if theme::soft_button(
                    ui,
                    &palette,
                    None,
                    &gettext(locale, "Keep it private"),
                    false,
                )
                .clicked()
                {
                    actions.push(Action::Live(Live::SetSharing(false)));
                }
            });
        });
        ui.add_space(16.0);
    }

    let others: Vec<Presence> = app
        .live
        .snapshot
        .presence
        .iter()
        .filter(|p| p.user_id != me.user_id && p.item.is_some())
        // Those in your own party are on its card already.
        .filter(|p| {
            app.live
                .my_channel()
                .is_none_or(|channel| !channel.members.contains(&p.user_id))
        })
        .cloned()
        .collect();
    let channels = app.live.snapshot.channels.clone();
    theme::text(
        ui,
        gettext(locale, "Listening now"),
        theme::medium(16.0),
        palette.text,
    );
    ui.add_space(8.0);

    let followers = app.live.followers_of(&me.user_id).len();
    if followers > 0 {
        let text = if followers == 1 {
            gettext(locale, "1 person is listening along with you").into_owned()
        } else {
            format!(
                "{followers} {}",
                gettext(locale, "people are listening along with you")
            )
        };
        theme::text(ui, text, theme::regular(13.0), palette.secondary);
        ui.add_space(8.0);
    }

    for channel in &channels {
        channel_card(app, ui, channel, me, actions);
        ui.add_space(8.0);
    }
    for person in &others {
        person_row(app, ui, person, actions);
        ui.add_space(8.0);
    }
    if others.is_empty() && channels.is_empty() {
        theme::text(
            ui,
            gettext(locale, "Nobody in the group is playing anything right now"),
            theme::regular(13.0),
            palette.dim,
        );
        ui.add_space(8.0);
    }
    let may_host = app
        .friends_feed
        .account
        .as_ref()
        .and_then(|account| {
            account.chosen_group(crate::friends::account::current_group().as_deref())
        })
        .is_none_or(|group| group.rights.host);
    if !matches!(app.live.session, Session::Channel(_)) && may_host {
        start_channel(app, ui, actions);
    }
    ui.add_space(20.0);
}

fn person_row(app: &App, ui: &mut Ui, person: &Presence, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    let Some(item) = person.item.as_ref() else {
        return;
    };
    let name = name_of(app, &person.user_id);
    let channel = app.live.channel_of(&person.user_id).cloned();
    let with_them = app.live.session == Session::Following(person.user_id.clone());
    let in_their_channel = channel
        .as_ref()
        .is_some_and(|c| app.live.session == Session::Channel(c.id.clone()));
    let status = match (&channel, &person.following, person.playing) {
        (Some(channel), _, _) => format!("{name} · {}", channel.name),
        (None, Some(leader), _) => format!(
            "{name} · {} {}",
            gettext(locale, "listening along with"),
            name_of(app, leader)
        ),
        (None, None, false) => format!("{name} · {}", gettext(locale, "paused")),
        (None, None, true) => name.clone(),
    };
    let play_label = gettext(locale, "Play song");
    let join_label = if with_them || in_their_channel {
        gettext(locale, "Leave")
    } else {
        gettext(locale, "Join")
    };
    let width =
        theme::soft_button_width(ui, &play_label) + theme::soft_button_width(ui, &join_label) + 8.0;
    row(
        ui,
        &palette,
        item.artwork_url.as_deref(),
        &status,
        &song_line(item),
        width,
        |ui| {
            // Right to left beside the song, so the order reads the same.
            let joining = !(with_them || in_their_channel);
            let join = |ui: &mut Ui, actions: &mut Vec<Action>| {
                if theme::soft_button(ui, &palette, None, &join_label, joining).clicked() {
                    actions.push(Action::Live(if !joining {
                        Live::Leave
                    } else if let Some(channel) = &channel {
                        Live::JoinChannel(channel.id.clone())
                    } else {
                        Live::ListenAlong(person.user_id.clone())
                    }));
                }
            };
            let play = |ui: &mut Ui, actions: &mut Vec<Action>| {
                if theme::soft_button(ui, &palette, Some(Icon::PlayFilled), &play_label, false)
                    .clicked()
                {
                    actions.push(play_action(&item.url));
                }
            };
            if ui.layout().prefer_right_to_left() {
                join(ui, actions);
                play(ui, actions);
            } else {
                play(ui, actions);
                join(ui, actions);
            }
        },
    );
}

fn channel_card(
    app: &mut App,
    ui: &mut Ui,
    channel: &Channel,
    me: &Profile,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    let locale = app.locale;
    let mine = app.live.session == Session::Channel(channel.id.clone());
    let count = if channel.members.len() == 1 {
        gettext(locale, "1 listening").into_owned()
    } else {
        format!("{} {}", channel.members.len(), gettext(locale, "listening"))
    };
    let names: Vec<String> = channel.members.iter().map(|m| name_of(app, m)).collect();
    let mut add = None;
    card(ui, &palette, |ui| {
        ui.horizontal(|ui| {
            theme::icon(ui, Icon::Users, 16.0, palette.secondary);
            line_fit(ui, &channel.name, theme::medium(14.0), palette.text, 180.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if mine {
                    gettext(locale, "Leave")
                } else {
                    gettext(locale, "Join")
                };
                if mine
                    && channel.host_id == me.user_id
                    && theme::soft_button(ui, &palette, None, &gettext(locale, "End party"), false)
                        .on_hover_text(gettext(locale, "Stops it for everyone"))
                        .clicked()
                {
                    actions.push(Action::Live(Live::End));
                }
                if theme::soft_button(ui, &palette, None, &label, !mine).clicked() {
                    actions.push(Action::Live(if mine {
                        Live::Leave
                    } else {
                        Live::JoinChannel(channel.id.clone())
                    }));
                }
                let host = if channel.locked {
                    format!(
                        "{} {} · ",
                        gettext(locale, "Hosted by"),
                        name_of(app, &channel.host_id)
                    )
                } else {
                    String::new()
                };
                let detail = format!("{host}{count} · {}", names.join(", "));
                line(ui, detail, theme::regular(12.5), palette.dim);
            });
        });
        ui.add_space(8.0);
        match &channel.current {
            Some(current) => {
                ui.horizontal(|ui| {
                    cover(ui, &palette, current.item.artwork_url.as_deref());
                    ui.vertical(|ui| {
                        ui.add_space(3.0);
                        line(ui, &current.item.title, theme::medium(13.5), palette.text);
                        let state = if channel.playing {
                            current.item.artists.join(", ")
                        } else {
                            format!(
                                "{} · {}",
                                gettext(locale, "paused"),
                                current.item.artists.join(", ")
                            )
                        };
                        line(ui, state, theme::regular(12.5), palette.dim);
                    });
                });
            }
            None => {
                theme::text(
                    ui,
                    gettext(
                        locale,
                        "Nothing is playing. Add a song and it starts for everyone",
                    ),
                    theme::regular(13.0),
                    palette.dim,
                );
            }
        }
        if mine {
            queue(app, ui, channel, me, actions);
            background(app, ui, channel, me, actions);
            add = add_song(app, ui, channel, actions);
            guests(app, ui, channel, me, actions);
        } else if !channel.queue.is_empty() {
            ui.add_space(6.0);
            theme::text(
                ui,
                format!(
                    "{} {}",
                    channel.queue.len(),
                    gettext(locale, "songs queued")
                ),
                theme::regular(12.5),
                palette.dim,
            );
        }
    });
    if let Some(item) = add {
        actions.push(Action::Live(Live::Add(item, Place::End)));
    }
}

fn line_fit(ui: &mut Ui, text: &str, font: egui::FontId, color: egui::Color32, width: f32) {
    ui.allocate_ui(vec2(width, 20.0), |ui| line(ui, text, font, color));
}

/// The shared queue. Anyone in the channel may reorder it, remove from it, or
/// jump to a row.
fn queue(app: &App, ui: &mut Ui, channel: &Channel, me: &Profile, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    if channel.queue.is_empty() {
        return;
    }
    ui.add_space(10.0);
    theme::text(
        ui,
        gettext(locale, "Up next, for everyone"),
        theme::medium(12.5),
        palette.secondary,
    );
    ui.add_space(4.0);
    let last = channel.queue.len() - 1;
    let width = ui.available_width();
    let at_controls = channel.controlled_by(&me.user_id);
    for (index, entry) in channel.queue.iter().enumerate() {
        queue_row(app, ui, entry, (index, last), width, at_controls, actions);
    }
}

fn queue_row(
    app: &App,
    ui: &mut Ui,
    entry: &Entry,
    (index, last): (usize, usize),
    width: f32,
    at_controls: bool,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    let locale = app.locale;
    let pending = entry.id.starts_with("pending-");
    ui.horizontal(|ui| {
        ui.set_max_width(width);
        let text_width = (width - QUEUE_BUTTONS).max(40.0);
        let (rect, response) = ui.allocate_exact_size(vec2(text_width, 34.0), Sense::click());
        if response.hovered() && !pending {
            ui.painter().rect_filled(rect, 6.0, palette.surface_hover);
        }
        let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(6.0, 1.0))));
        inner.vertical(|ui| {
            line(ui, &entry.item.title, theme::regular(13.0), palette.text);
            let detail = format!(
                "{} · {} {}",
                entry.item.artists.join(", "),
                gettext(locale, "added by"),
                name_of(app, &entry.added_by)
            );
            line(ui, detail, theme::regular(11.5), palette.dim);
        });
        // Jumping to a row is moving the music, so it is for whoever is at the controls.
        let response = if at_controls {
            response.on_hover_text(gettext(locale, "Play now, for everyone"))
        } else {
            response
        };
        if response.clicked() && !pending && at_controls {
            actions.push(Action::Live(Live::PlayEntry(entry.id.clone())));
        }
        ui.add_enabled_ui(!pending, |ui| {
            ui.add_enabled_ui(index > 0, |ui| {
                if theme::icon_button(
                    ui,
                    Icon::ChevronUp,
                    16.0,
                    palette.secondary,
                    palette.text,
                    &gettext(locale, "Move up"),
                )
                .clicked()
                {
                    actions.push(Action::Live(Live::Move {
                        entry: entry.id.clone(),
                        index: index - 1,
                    }));
                }
            });
            ui.add_enabled_ui(index < last, |ui| {
                if theme::icon_button(
                    ui,
                    Icon::ChevronDown,
                    16.0,
                    palette.secondary,
                    palette.text,
                    &gettext(locale, "Move down"),
                )
                .clicked()
                {
                    actions.push(Action::Live(Live::Move {
                        entry: entry.id.clone(),
                        index: index + 1,
                    }));
                }
            });
            if theme::icon_button(
                ui,
                Icon::X,
                16.0,
                palette.secondary,
                palette.text,
                &gettext(locale, "Remove from queue"),
            )
            .clicked()
            {
                actions.push(Action::Live(Live::Remove(entry.id.clone())));
            }
        });
    });
}

/// What plays when the queue runs out.
fn background(app: &App, ui: &mut Ui, channel: &Channel, me: &Profile, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    ui.add_space(10.0);
    match &channel.background {
        Some(list) => {
            ui.horizontal(|ui| {
                theme::icon(ui, Icon::ListMusic, 14.0, palette.secondary);
                let next = list
                    .upcoming
                    .first()
                    .map(|song| format!(" · {} {}", gettext(locale, "next:"), song.title))
                    .unwrap_or_default();
                line(
                    ui,
                    format!(
                        "{} · {} {}{next}",
                        list.name,
                        list.total,
                        gettext(locale, "songs in the background")
                    ),
                    theme::regular(12.5),
                    palette.secondary,
                );
                if (list.by == me.user_id || channel.controlled_by(&me.user_id))
                    && theme::icon_button(
                        ui,
                        Icon::X,
                        14.0,
                        palette.dim,
                        palette.text,
                        &gettext(locale, "Remove background playlist"),
                    )
                    .clicked()
                {
                    actions.push(Action::Live(Live::ClearBackground));
                }
            });
        }
        None => {
            theme::text(
                ui,
                gettext(
                    locale,
                    "No background playlist. When the queue runs out, the music stops",
                ),
                theme::regular(12.5),
                palette.dim,
            );
        }
    }
}

/// The host's view of who is here: who else may hold the controls, and who takes over.
fn guests(app: &App, ui: &mut Ui, channel: &Channel, me: &Profile, actions: &mut Vec<Action>) {
    if !channel.locked || channel.host_id != me.user_id || channel.members.len() < 2 {
        return;
    }
    let palette = app.palette;
    let locale = app.locale;
    ui.add_space(10.0);
    theme::text(
        ui,
        gettext(locale, "Guests"),
        theme::medium(12.5),
        palette.secondary,
    );
    for guest in channel.members.iter().filter(|id| **id != me.user_id) {
        ui.horizontal(|ui| {
            line(ui, name_of(app, guest), theme::regular(13.0), palette.text);
            let mut allowed = channel.controllers.contains(guest);
            if ui
                .checkbox(&mut allowed, gettext(locale, "Can control"))
                .changed()
            {
                actions.push(Action::Live(Live::SetController {
                    user: guest.clone(),
                    allowed,
                }));
            }
            if theme::soft_button(ui, &palette, None, &gettext(locale, "Make host"), false)
                .clicked()
            {
                actions.push(Action::Live(Live::HandOver(guest.clone())));
            }
        });
    }
}

/// Everything a person might add: a song found by searching, a whole playlist
/// as the background, or, for those who have one to hand, a link.
fn add_song(
    app: &mut App,
    ui: &mut Ui,
    channel: &Channel,
    actions: &mut Vec<Action>,
) -> Option<NewPost> {
    let palette = app.palette;
    let locale = app.locale;
    let id = Id::new(("group-channel-add", &channel.id));
    let mut text = ui
        .data(|data| data.get_temp::<String>(id))
        .unwrap_or_default();
    ui.add_space(10.0);
    let mut added = None;
    ui.horizontal(|ui| {
        if theme::soft_button(
            ui,
            &palette,
            Some(Icon::Search),
            &gettext(locale, "Find a song"),
            true,
        )
        .on_hover_text(gettext(
            locale,
            "Search as usual. Any song you press joins the queue",
        ))
        .clicked()
        {
            actions.push(Action::FocusSearch);
        }
        let button = theme::soft_button(
            ui,
            &palette,
            Some(Icon::ListMusic),
            &gettext(locale, "Use a playlist"),
            false,
        )
        .on_hover_text(gettext(locale, "Plays when the queue runs out"));
        egui::Popup::menu(&button).show(|ui| {
            super::group_playlists::playlist_choices(
                app,
                ui,
                crate::model::GroupPlaylistUse::PartyBackground,
                actions,
            );
        });
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 72.0).max(80.0);
        let submitted = ui
            .allocate_ui(vec2(width, 36.0), |ui| {
                let response = field(
                    ui,
                    &palette,
                    &mut text,
                    &gettext(locale, "Or paste a song link"),
                    false,
                    4096,
                );
                response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
            })
            .inner;
        let clicked = ui
            .add_enabled_ui(!text.trim().is_empty(), |ui| {
                theme::soft_button(ui, &palette, None, &gettext(locale, "Add"), false)
            })
            .inner
            .clicked();
        if (submitted || clicked) && !text.trim().is_empty() {
            added = one_song(app, text.trim());
            if added.is_some() {
                text.clear();
            } else {
                app.toast(gettext(locale, "That link isn't one song"));
            }
        }
    });
    ui.data_mut(|data| data.insert_temp(id, text));
    added
}

/// A link's song, named well enough to show. Albums and playlists have no
/// place in a queue of songs.
fn one_song(app: &mut App, text: &str) -> Option<NewPost> {
    let mut post = recognise(app, text)?;
    let single = match post.source {
        Source::Spotify => post.url.contains("/track/"),
        Source::Soundcloud => !post.url.contains("/sets/"),
        Source::Youtube => true,
    };
    if !single {
        return None;
    }
    if post.title.trim().is_empty() {
        post.title = match post.source {
            Source::Spotify => "Spotify song",
            Source::Soundcloud => "SoundCloud track",
            Source::Youtube => "YouTube video",
        }
        .to_owned();
    }
    Some(post)
}

fn start_channel(app: &App, ui: &mut Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    let open_id = Id::new("group-channel-start-open");
    let name_id = Id::new("group-channel-start-name");
    let open = ui
        .data(|data| data.get_temp::<bool>(open_id))
        .unwrap_or(false);
    if !open {
        if theme::soft_button(
            ui,
            &palette,
            Some(Icon::Users),
            &gettext(locale, "Start a party"),
            false,
        )
        .on_hover_text(gettext(
            locale,
            "You run the music. Everyone who joins hears the same thing, and can add to and reorder the queue",
        ))
        .clicked()
        {
            ui.data_mut(|data| data.insert_temp(open_id, true));
        }
        return;
    }
    let mut name = ui
        .data(|data| data.get_temp::<String>(name_id))
        .unwrap_or_default();
    let mut close = false;
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 150.0).max(80.0);
        let submitted = ui
            .allocate_ui(vec2(width, 36.0), |ui| {
                let response = field(
                    ui,
                    &palette,
                    &mut name,
                    &gettext(locale, "Party name"),
                    false,
                    60,
                );
                response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
            })
            .inner;
        let ready = !name.trim().is_empty();
        let start = ui
            .add_enabled_ui(ready, |ui| {
                theme::soft_button(ui, &palette, None, &gettext(locale, "Start"), true)
            })
            .inner
            .clicked();
        if (start || submitted) && ready {
            actions.push(Action::Live(Live::StartChannel(name.trim().to_owned())));
            close = true;
        }
        if theme::soft_button(ui, &palette, None, &gettext(locale, "Cancel"), false).clicked() {
            close = true;
        }
    });
    if close {
        name.clear();
        ui.data_mut(|data| data.insert_temp(open_id, false));
    }
    ui.data_mut(|data| data.insert_temp(name_id, name));
}
