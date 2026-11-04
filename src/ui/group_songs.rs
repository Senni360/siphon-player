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
/// A list this long is worth searching.
const FILTER_FROM: usize = 8;
/// How long a friend's file counts as new on Home, and how many show there.
const RECENT_MS: u64 = 14 * 24 * 60 * 60 * 1000;
const RECENT_SHOWN: usize = 3;

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

/// A deleted file is gone for everyone, and only its owner has it to put back.
fn confirm_delete(locale: crate::i18n::Locale, song: &SharedSong) -> Action {
    Action::ShowDialog(crate::model::Dialog::Confirm {
        title: gettext(locale, "Delete this file?").into_owned(),
        message: gettext(
            locale,
            "“{title}” stops playing for everyone in the group. This can't be undone.",
        )
        .replace("{title}", &song.title),
        confirm: gettext(locale, "Delete").into_owned(),
        action: Box::new(Action::Friends(Group::DeleteSong(song.id.clone()))),
    })
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

/// Something the group has that answers a search.
struct Hit {
    title: String,
    detail: String,
    color: egui::Color32,
    action: Action,
}

const MOST_HITS: usize = 5;

/// What the group has shared that answers `query`: files first, since nowhere
/// else has them, then songs shared from elsewhere, each song once.
fn hits(
    feed: &crate::friends::Model,
    palette: &theme::Palette,
    locale: crate::i18n::Locale,
    query: &str,
) -> Vec<Hit> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let has = |text: &str| text.to_lowercase().contains(&needle);
    let matches = |title: &str, artists: &[String]| has(title) || artists.iter().any(|a| has(a));
    let files = feed
        .shared_songs
        .iter()
        .enumerate()
        .filter(|(_, song)| matches(&song.title, &song.artists))
        .map(|(index, song)| Hit {
            title: song.title.clone(),
            detail: byline(locale, song),
            color: palette.text,
            action: play(index, song.uri()),
        });
    let mut seen = std::collections::HashSet::new();
    let posts = feed
        .posts
        .iter()
        .filter(|post| matches(&post.item.title, &post.item.artists))
        .filter(|post| seen.insert(post.item.url.clone()))
        .map(|post| {
            let item = &post.item;
            Hit {
                title: item.title.clone(),
                detail: gettext(locale, "{artists}, from {name}")
                    .replace("{artists}", &item.artists.join(", "))
                    .replace("{name}", &post.author_name),
                color: match item.source {
                    crate::friends::Source::Spotify => super::kit::Source::Spotify,
                    crate::friends::Source::Soundcloud => super::kit::Source::SoundCloud,
                    crate::friends::Source::Youtube => super::kit::Source::YouTube,
                }
                .color(palette),
                action: super::group::play_action(&item.url),
            }
        });
    files.chain(posts).take(MOST_HITS).collect()
}

/// The top of a search: what friends shared comes before any service's results.
pub fn search_section(app: &mut App, ui: &mut Ui) {
    let found = hits(
        &app.friends_feed,
        &app.palette,
        app.locale,
        &app.search.committed,
    );
    if found.is_empty() {
        return;
    }
    let palette = app.palette;
    theme::section_title(ui, &palette, &gettext(app.locale, "From your group"));
    for hit in found {
        hit_row(app, ui, hit);
    }
    ui.add_space(12.0);
}

/// Files friends put up lately, so a new one doesn't wait to be found in a tab.
pub fn home_section(app: &mut App, ui: &mut Ui) {
    let Some(me) = app
        .friends_feed
        .profile
        .as_ref()
        .map(|me| me.user_id.clone())
    else {
        return;
    };
    let now = jiff::Timestamp::now().as_millisecond().max(0) as u64;
    let locale = app.locale;
    let found: Vec<Hit> = app
        .friends_feed
        .shared_songs
        .iter()
        .enumerate()
        .filter(|(_, song)| song.by != me && now.saturating_sub(song.created_at) < RECENT_MS)
        .take(RECENT_SHOWN)
        .map(|(index, song)| Hit {
            title: song.title.clone(),
            detail: byline(locale, song),
            color: app.palette.text,
            action: play(index, song.uri()),
        })
        .collect();
    if found.is_empty() {
        return;
    }
    let palette = app.palette;
    ui.horizontal(|ui| {
        theme::section_title(ui, &palette, &gettext(locale, "New files from friends"));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::link(
                ui,
                gettext(locale, "All shared files"),
                theme::regular(13.0),
                palette.secondary,
            )
            .clicked()
            {
                app.actions.push(Action::OpenSharedFiles);
            }
        });
    });
    ui.add_space(4.0);
    // Side by side where they fit, so Home's other shelves stay in view.
    if ui.available_width() >= 720.0 {
        ui.columns(RECENT_SHOWN, |columns| {
            for (column, hit) in columns.iter_mut().zip(found) {
                hit_row(app, column, hit);
            }
        });
    } else {
        for hit in found {
            hit_row(app, ui, hit);
        }
    }
    ui.add_space(16.0);
}

fn hit_row(app: &mut App, ui: &mut Ui, hit: Hit) {
    let palette = app.palette;
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    if response.hovered() {
        ui.painter().rect(
            rect,
            8.0,
            palette.surface_hover,
            Stroke::NONE,
            egui::StrokeKind::Inside,
        );
    }
    let icon = Rect::from_center_size(
        egui::pos2(rect.left() + 22.0, rect.center().y),
        vec2(18.0, 18.0),
    );
    Icon::PlayFilled
        .image(palette.secondary, 18.0)
        .paint_at(ui, icon);
    let painter = ui.painter().with_clip_rect(rect);
    crate::bidi::paint_line(
        &painter,
        rect.left() + 44.0,
        rect.right() - 12.0,
        rect.center().y - 9.0,
        &hit.title,
        theme::medium(14.0),
        hit.color,
    );
    crate::bidi::paint_line(
        &painter,
        rect.left() + 44.0,
        rect.right() - 12.0,
        rect.center().y + 10.0,
        &hit.detail,
        theme::regular(12.5),
        palette.secondary,
    );
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &hit.title));
    if response.clicked() {
        app.actions.push(hit.action);
    }
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
    let filter_id = egui::Id::new("shared-files-filter");
    let mut filter = ui
        .data(|data| data.get_temp::<String>(filter_id))
        .unwrap_or_default();
    if songs.len() >= FILTER_FROM {
        super::widgets::search_field(
            ui,
            &palette,
            locale,
            filter_id.with("field"),
            &mut filter,
            &gettext(locale, "Find a song, artist or person"),
            ui.available_width().min(320.0),
        );
        ui.add_space(8.0);
        ui.data_mut(|data| data.insert_temp(filter_id, filter.clone()));
    }
    let needle = filter.trim().to_lowercase();
    for (index, song) in songs.iter().enumerate() {
        if !needle.is_empty()
            && !song.title.to_lowercase().contains(&needle)
            && !song.by_name.to_lowercase().contains(&needle)
            && !song
                .artists
                .iter()
                .any(|artist| artist.to_lowercase().contains(&needle))
        {
            continue;
        }
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
            actions.push(confirm_delete(locale, song));
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
            actions.push(confirm_delete(locale, song));
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

    #[test]
    fn a_search_finds_a_file_by_title_or_artist_and_nothing_for_no_query() {
        let mut feed = crate::friends::Model::default();
        feed.shared_songs = (0..8)
            .map(|index| SharedSong {
                id: format!("00000000-0000-4000-8000-0000000000{index:02}"),
                title: format!("loop {index}"),
                artists: vec!["Teo".into()],
                by_name: "Teo".into(),
                ..SharedSong::default()
            })
            .collect();
        let palette = theme::Palette::dark();
        let locale = crate::i18n::Locale::English;
        let hits = |query: &str| hits(&feed, &palette, locale, query);
        assert!(hits("  ").is_empty());
        assert_eq!(hits("LOOP 3")[0].title, "loop 3");
        assert_eq!(hits("teo").len(), MOST_HITS, "the section stays short");
        assert!(hits("nobody").is_empty());
    }
}
