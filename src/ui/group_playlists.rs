//! Playlists shared with the group. A share is a copy of the list as it was:
//! anyone can save it to their own playlists, play it, or make it a party's
//! background, whichever services its songs come from.

use egui::{Id, Ui, Vec2, vec2};

use crate::app::App;
use crate::friends::account::SharedPlaylist;
use crate::friends::{Action as Group, Profile, SongsFor};
use crate::i18n::gettext;
use crate::model::{Action, GroupPlaylistUse};
use crate::theme::{self, Icon};

use super::group::card;

const COVER: f32 = 44.0;

/// The lists this person could choose from: mixed ones and their Spotify library.
pub(super) fn playlist_choices(
    app: &App,
    ui: &mut Ui,
    purpose: GroupPlaylistUse,
    actions: &mut Vec<Action>,
) {
    ui.set_min_width(240.0);
    let palette = app.palette;
    let mut listed = 0;
    egui::ScrollArea::vertical()
        .max_height(320.0)
        .show(ui, |ui| {
            for playlist in &app.mixed_playlists.playlists {
                listed += 1;
                if super::widgets::menu_item(ui, &palette, Some(Icon::ListMusic), &playlist.name) {
                    actions.push(Action::GroupPlaylist {
                        uri: playlist.uri(),
                        name: playlist.name.clone(),
                        purpose: purpose.clone(),
                    });
                }
            }
            for playlist in app.library_playlists() {
                listed += 1;
                if super::widgets::menu_item(ui, &palette, Some(Icon::ListMusic), &playlist.name) {
                    actions.push(Action::GroupPlaylist {
                        uri: playlist.uri.clone(),
                        name: playlist.name.clone(),
                        purpose: purpose.clone(),
                    });
                }
            }
        });
    if listed == 0 {
        theme::text(
            ui,
            gettext(app.locale, "You have no playlists yet"),
            theme::regular(13.0),
            palette.dim,
        );
    }
}

pub fn show(app: &mut App, ui: &mut Ui, me: &Profile, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    ui.horizontal(|ui| {
        let share = theme::soft_button(
            ui,
            &palette,
            Some(Icon::Plus),
            &gettext(locale, "Share a playlist"),
            true,
        );
        egui::Popup::menu(&share).show(|ui| {
            playlist_choices(app, ui, GroupPlaylistUse::Share { to: Vec::new() }, actions);
        });
    });
    ui.add_space(12.0);
    let lists = app.friends_feed.shared_playlists.clone();
    if lists.is_empty() {
        theme::text(
            ui,
            gettext(
                locale,
                "Nothing here yet. A playlist you share lands here for everyone to save or play",
            ),
            theme::regular(13.0),
            palette.dim,
        );
        return;
    }
    let in_party = matches!(app.live.session, crate::friends::live::Session::Channel(_));
    let manages = app
        .friends_feed
        .account
        .as_ref()
        .and_then(|account| {
            account.chosen_group(crate::friends::account::current_group().as_deref())
        })
        .is_some_and(|group| group.role.manages());
    for list in &lists {
        row(app, ui, list, me, in_party, manages, actions);
        ui.add_space(8.0);
    }
}

fn row(
    app: &App,
    ui: &mut Ui,
    list: &SharedPlaylist,
    me: &Profile,
    in_party: bool,
    manages: bool,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    let locale = app.locale;
    let open = |purpose| {
        Action::Friends(Group::OpenSharedPlaylist {
            id: list.id.clone(),
            purpose,
        })
    };
    card(ui, &palette, |ui| {
        ui.push_id(Id::new(("shared-playlist", &list.id)), |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(COVER), egui::Sense::hover());
                super::widgets::paint_cover(
                    ui,
                    &palette,
                    list.artwork_url.as_deref(),
                    rect,
                    8.0,
                    Icon::ListMusic,
                    None,
                );
                ui.allocate_ui(
                    vec2((ui.available_width() - 200.0).max(80.0), COVER),
                    |ui| {
                        ui.vertical(|ui| {
                            theme::text(ui, list.name.clone(), theme::medium(13.5), palette.text);
                            theme::text(
                                ui,
                                format!(
                                    "{} {} · {}",
                                    list.count,
                                    gettext(locale, "songs"),
                                    list.by_name
                                ),
                                theme::regular(12.5),
                                palette.dim,
                            );
                        });
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if (list.by == me.user_id || manages)
                        && theme::icon_button(
                            ui,
                            Icon::Trash,
                            15.0,
                            palette.dim,
                            palette.text,
                            &gettext(locale, "Delete"),
                        )
                        .clicked()
                    {
                        actions.push(Action::ShowDialog(crate::model::Dialog::Confirm {
                            title: gettext(locale, "Remove this playlist from the group?")
                                .into_owned(),
                            message: gettext(
                                locale,
                                "Nobody in the group will see it here anymore.",
                            )
                            .into_owned(),
                            confirm: gettext(locale, "Remove").into_owned(),
                            action: Box::new(Action::Friends(Group::DeleteSharedPlaylist(
                                list.id.clone(),
                            ))),
                        }));
                    }
                    if theme::soft_button(ui, &palette, None, &gettext(locale, "Save"), false)
                        .on_hover_text(gettext(locale, "Keep a copy in your playlists"))
                        .clicked()
                    {
                        actions.push(open(SongsFor::SaveCopy));
                    }
                    if in_party {
                        if theme::soft_button(
                            ui,
                            &palette,
                            None,
                            &gettext(locale, "Use in party"),
                            false,
                        )
                        .on_hover_text(gettext(locale, "Plays when the party's queue runs out"))
                        .clicked()
                        {
                            actions.push(open(SongsFor::PartyBackground));
                        }
                    } else if theme::soft_button(
                        ui,
                        &palette,
                        Some(Icon::PlayFilled),
                        &gettext(locale, "Play"),
                        false,
                    )
                    .clicked()
                    {
                        actions.push(open(SongsFor::Play));
                    }
                });
            });
        });
    });
}
