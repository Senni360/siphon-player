//! Songs people share from their own files. Anyone can add as many as they like:
//! when the group's store fills up, the oldest songs make room.

use egui::{Align, Layout, Ui};

use crate::app::App;
use crate::friends::{Action as Group, Profile};
use crate::i18n::gettext;
use crate::model::Action;
use crate::shared_songs::{LIBRARY, MAX_BYTES, MAX_MS};
use crate::theme::{self, Icon};

use super::group::card;

const MIB: u64 = 1024 * 1024;

pub fn show(app: &mut App, ui: &mut Ui, me: &Profile, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    let feed = &app.friends_feed;
    ui.horizontal(|ui| {
        if theme::soft_button(
            ui,
            &palette,
            Some(Icon::Plus),
            &gettext(locale, "Share songs from this computer"),
            true,
        )
        .clicked()
        {
            actions.push(Action::ChooseSongsToShare);
        }
        if let Some(file) = &feed.uploading {
            ui.spinner();
            let waiting = feed.uploads_waiting();
            let mut line = gettext(locale, "Uploading {file}").replace("{file}", file);
            if waiting > 0 {
                line.push_str(
                    &gettext(locale, ", {count} more waiting")
                        .replace("{count}", &waiting.to_string()),
                );
            }
            theme::text(ui, line, theme::regular(13.0), palette.secondary);
        }
    });
    ui.add_space(6.0);
    theme::text(
        ui,
        gettext(
            locale,
            "Or drop audio files anywhere in Encore. Up to {minutes} minutes and {size} MB a song. When the shared space fills up, the oldest songs are deleted to make room.",
        )
        .replace("{minutes}", &(MAX_MS / 60_000).to_string())
        .replace("{size}", &(MAX_BYTES / MIB).to_string()),
        theme::regular(12.0),
        palette.dim,
    );
    if feed.songs_capacity > 0 {
        theme::text(
            ui,
            gettext(locale, "{used} MB of {total} MB used")
                .replace("{used}", &(feed.songs_used / MIB).to_string())
                .replace("{total}", &(feed.songs_capacity / MIB).to_string()),
            theme::regular(12.0),
            palette.dim,
        );
    }
    ui.add_space(12.0);
    if feed.shared_songs.is_empty() {
        theme::text(
            ui,
            gettext(
                locale,
                "Nothing here yet. Songs you share here can be played by everyone in the group",
            ),
            theme::regular(13.0),
            palette.dim,
        );
        return;
    }
    let manages = feed
        .account
        .as_ref()
        .and_then(|account| {
            account.chosen_group(crate::friends::account::current_group().as_deref())
        })
        .is_some_and(|group| group.role.manages());
    for (index, song) in feed.shared_songs.iter().enumerate() {
        let uri = song.uri();
        let playing = app.now_playing().is_some_and(|now| now.uri == uri);
        card(ui, &palette, |ui| {
            ui.horizontal(|ui| {
                if theme::icon_button(
                    ui,
                    Icon::PlayFilled,
                    18.0,
                    if playing {
                        palette.accent
                    } else {
                        palette.secondary
                    },
                    palette.text,
                    &gettext(locale, "Play"),
                )
                .clicked()
                {
                    actions.push(Action::PlayContext {
                        uri: LIBRARY.into(),
                        offset_uri: Some(uri.clone()),
                        offset_index: u32::try_from(index).ok(),
                    });
                }
                ui.vertical(|ui| {
                    theme::text(ui, song.title.clone(), theme::medium(14.0), palette.text);
                    let artists = song.artists.join(", ");
                    let line = if artists.is_empty() {
                        song.by_name.clone()
                    } else {
                        format!("{artists} · {}", song.by_name)
                    };
                    theme::text(ui, line, theme::regular(12.0), palette.secondary);
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if (song.by == me.user_id || manages)
                        && theme::icon_button(
                            ui,
                            Icon::Minus,
                            16.0,
                            palette.secondary,
                            palette.text,
                            &gettext(locale, "Delete"),
                        )
                        .clicked()
                    {
                        actions.push(Action::Friends(Group::DeleteSong(song.id.clone())));
                    }
                });
            });
        });
        ui.add_space(8.0);
    }
}
