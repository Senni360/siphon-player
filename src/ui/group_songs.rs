//! Songs people share from their own files. Anyone can add as many as they like:
//! when the group's store fills up, the oldest songs make room.

use egui::{Align, Layout, Rect, Sense, Stroke, Ui, UiBuilder, vec2};

use crate::app::App;
use crate::friends::{Action as Group, Profile};
use crate::i18n::gettext;
use crate::model::Action;
use crate::shared_songs::{LIBRARY, MAX_BYTES, MAX_MS, SharedSong};
use crate::theme::{self, Icon};

const MIB: u64 = 1024 * 1024;
const ROW: f32 = 54.0;

fn play(index: usize, uri: String) -> Action {
    Action::PlayContext {
        uri: LIBRARY.into(),
        offset_uri: Some(uri),
        offset_index: u32::try_from(index).ok(),
    }
}

fn wrapped(ui: &mut Ui, text: String, size: f32, color: egui::Color32) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .font(theme::regular(size))
                .color(color),
        )
        .wrap(),
    );
}

/// Who it is by and who put it here, without saying the same name twice.
fn byline(locale: crate::i18n::Locale, song: &SharedSong) -> String {
    let artists = song.artists.join(", ");
    if artists.is_empty() || artists == song.by_name {
        return gettext(locale, "from {name}").replace("{name}", &song.by_name);
    }
    gettext(locale, "{artists}, from {name}")
        .replace("{artists}", &artists)
        .replace("{name}", &song.by_name)
}

/// The room left in the store: the thing that decides when old songs go.
fn meter(app: &App, ui: &mut Ui, used: u64, capacity: u64) {
    let palette = app.palette;
    let width = ui.available_width().min(240.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(width, 4.0), Sense::hover());
        ui.painter().rect_filled(rect, 2.0, palette.surface_active);
        let share = (used as f32 / capacity as f32).clamp(0.0, 1.0);
        let filled = Rect::from_min_size(rect.min, vec2(rect.width() * share, rect.height()));
        ui.painter().rect_filled(filled, 2.0, palette.secondary);
        theme::text(
            ui,
            gettext(app.locale, "{used} MB of {total} MB used")
                .replace("{used}", &(used / MIB).to_string())
                .replace("{total}", &(capacity / MIB).to_string()),
            theme::regular(12.0),
            palette.dim,
        );
    });
}

pub fn show(app: &mut App, ui: &mut Ui, me: &Profile, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    let songs = app.friends_feed.shared_songs.clone();
    ui.horizontal(|ui| {
        if !songs.is_empty()
            && theme::soft_button(
                ui,
                &palette,
                Some(Icon::PlayFilled),
                &gettext(locale, "Play all"),
                false,
            )
            .clicked()
        {
            actions.push(Action::PlayContext {
                uri: LIBRARY.into(),
                offset_uri: None,
                offset_index: None,
            });
        }
        if theme::soft_button(
            ui,
            &palette,
            Some(Icon::Plus),
            &gettext(locale, "Share songs from this computer"),
            songs.is_empty(),
        )
        .clicked()
        {
            actions.push(Action::ChooseSongsToShare);
        }
        if let Some(file) = &app.friends_feed.uploading {
            ui.spinner();
            let waiting = app.friends_feed.uploads_waiting();
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
    ui.add_space(8.0);
    wrapped(
        ui,
        gettext(
            locale,
            "Or drop audio files anywhere in Encore. Up to {minutes} minutes and {size} MB a song. When the shared space fills up, the oldest songs are deleted to make room.",
        )
        .replace("{minutes}", &(MAX_MS / 60_000).to_string())
        .replace("{size}", &(MAX_BYTES / MIB).to_string()),
        12.0,
        palette.dim,
    );
    if app.friends_feed.songs_capacity > 0 {
        ui.add_space(6.0);
        meter(
            app,
            ui,
            app.friends_feed.songs_used,
            app.friends_feed.songs_capacity,
        );
    }
    ui.add_space(12.0);
    if songs.is_empty() {
        wrapped(
            ui,
            gettext(
                locale,
                "Nothing here yet. Songs you share here can be played by everyone in the group",
            )
            .into_owned(),
            13.0,
            palette.dim,
        );
        return;
    }
    let manages = app
        .friends_feed
        .account
        .as_ref()
        .and_then(|account| {
            account.chosen_group(crate::friends::account::current_group().as_deref())
        })
        .is_some_and(|group| group.role.manages());
    for (index, song) in songs.iter().enumerate() {
        row(
            app,
            ui,
            song,
            index,
            song.by == me.user_id || manages,
            actions,
        );
    }
}

fn row(
    app: &mut App,
    ui: &mut Ui,
    song: &SharedSong,
    index: usize,
    deletable: bool,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    let locale = app.locale;
    let uri = song.uri();
    let playing = app.now_playing().is_some_and(|now| now.uri == uri);
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    let inside = ui.rect_contains_pointer(rect);
    if inside || playing {
        ui.painter().rect(
            rect,
            8.0,
            palette.surface_hover,
            Stroke::NONE,
            egui::StrokeKind::Inside,
        );
    }
    if response.double_clicked() {
        actions.push(play(index, uri.clone()));
    }
    response.context_menu(|ui| {
        ui.set_min_width(200.0);
        if super::widgets::menu_item(ui, &palette, Some(Icon::Play), &gettext(locale, "Play")) {
            actions.push(play(index, uri.clone()));
        }
        if super::widgets::menu_item(
            ui,
            &palette,
            Some(Icon::ListEnd),
            &gettext(locale, "Add to queue"),
        ) {
            actions.push(Action::QueueMany {
                songs: vec![(uri.clone(), song.title.clone())],
            });
        }
        if deletable
            && super::widgets::menu_item(
                ui,
                &palette,
                Some(Icon::Trash),
                &gettext(locale, "Delete"),
            )
        {
            actions.push(Action::Friends(Group::DeleteSong(song.id.clone())));
        }
    });
    let mut line = ui.new_child(
        UiBuilder::new()
            .max_rect(rect.shrink2(vec2(8.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    if theme::icon_button(
        &mut line,
        Icon::PlayFilled,
        18.0,
        if playing {
            palette.text
        } else {
            palette.secondary
        },
        palette.text,
        &gettext(locale, "Play"),
    )
    .clicked()
    {
        actions.push(play(index, uri.clone()));
    }
    line.add_space(6.0);
    line.with_layout(Layout::right_to_left(Align::Center), |line| {
        if deletable
            && inside
            && theme::icon_button(
                line,
                Icon::Trash,
                16.0,
                palette.secondary,
                palette.text,
                &gettext(locale, "Delete"),
            )
            .clicked()
        {
            actions.push(Action::Friends(Group::DeleteSong(song.id.clone())));
        }
        line.add_space(6.0);
        theme::text(
            line,
            crate::util::format_duration_ms(song.duration_ms),
            theme::regular(12.5),
            palette.secondary,
        );
        line.add_space(12.0);
        line.with_layout(Layout::top_down(Align::Min), |line| {
            line.add_space(9.0);
            line.spacing_mut().item_spacing.y = 2.0;
            theme::text(line, song.title.clone(), theme::medium(14.0), palette.text);
            theme::text(
                line,
                byline(locale, song),
                theme::regular(12.0),
                palette.secondary,
            );
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_producer_sharing_their_own_song_is_named_once() {
        let locale = crate::i18n::Locale::English;
        let mut song = SharedSong {
            by_name: "Maya".into(),
            artists: vec!["Maya".into()],
            ..SharedSong::default()
        };
        assert_eq!(byline(locale, &song), "from Maya");
        song.artists = vec!["Bonobo".into()];
        assert_eq!(byline(locale, &song), "Bonobo, from Maya");
    }
}
