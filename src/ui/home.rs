//! The Home page.

use std::sync::Arc;

use egui::{CornerRadius, Rect, Sense, Vec2, pos2, vec2};

use crate::api::models::{Album, PlayableItem, Playlist, pick_image};
use crate::app::App;
use crate::i18n::gettext;
use crate::model::{Action, Loadable, Page, RowContext, TopPeriod};
use crate::theme::{self, Icon};

use super::widgets::{self, TrackRow};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    if !app.network_reachable {
        let space = ((ui.available_height() - 160.0) / 2.0).max(48.0);
        ui.add_space(space);
        ui.vertical_centered(|ui| {
            theme::icon(ui, Icon::WifiOff, 54.0, palette.dim);
            ui.add_space(16.0);
            theme::text(
                ui,
                gettext(app.locale, "You're offline"),
                theme::bold(28.0),
                palette.text,
            );
            ui.add_space(8.0);
            theme::text(
                ui,
                gettext(
                    app.locale,
                    "Encore will reconnect automatically when your connection returns.",
                ),
                theme::regular(14.0),
                palette.secondary,
            );
        });
        return;
    }
    ui.add_space(6.0);
    let greeting = crate::util::greeting(app.locale);
    widgets::title_in_words(ui, greeting.as_ref(), theme::medium(28.0), palette.text);
    ui.add_space(12.0);
    // Home never scrolls sideways: its rows hold what fits.
    widgets::fit_shelves(ui.ctx(), true);
    use super::rise;
    let picks = app
        .for_you
        .get()
        .map_or_else(Vec::new, |shown| shown.picks.clone());
    let following = app
        .for_you
        .get()
        .map_or_else(Vec::new, |shown| shown.following.clone());
    rise(ui, 0, |ui| featured(app, ui, &picks));
    ui.add_space(20.0);
    // What's new from the group comes before anything a service suggests.
    rise(ui, 1, |ui| {
        super::group_shelf::show(app, ui);
        super::group_songs::home_section(app, ui);
    });
    rise(ui, 2, |ui| quick_access(app, ui));
    ui.add_space(16.0);
    rise(ui, 3, |ui| more_for_you(app, ui, &picks));
    if app.settings.home.made_for_you.visible {
        rise(ui, 4, |ui| {
            made_for_you(app, ui);
            top_mixes(app, ui);
        });
    }
    rise(ui, 5, |ui| new_from_followed(app, ui, &following));
    rise(ui, 5, |ui| source_picks(app, ui, &picks));
    rise(ui, 6, |ui| new_releases(app, ui));
    rise(ui, 6, |ui| recently_played(app, ui));
    rise(ui, 7, |ui| top_artists(app, ui));
    rise(ui, 7, |ui| top_tracks(app, ui));
    widgets::fit_shelves(ui.ctx(), false);
}

/// The first thing on Home: the best pick, large, in its own colour, with the next
/// few beside it. It says why it's here.
fn featured(app: &mut App, ui: &mut egui::Ui, picks: &[crate::for_you::Pick]) {
    use super::kit::motion;
    let palette = app.palette;
    let width = ui.available_width();
    const HEIGHT: f32 = 268.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, HEIGHT), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let Some(first) = picks.first() else {
        ui.painter().rect_filled(rect, 18.0, palette.surface);
        if app.for_you.is_loading() {
            motion::glint(
                ui.painter(),
                rect,
                (ui.input(|input| input.time) * 0.55).fract() as f32,
                palette.text.gamma_multiply(0.05),
            );
            ui.ctx().request_repaint();
        }
        return;
    };
    let uris: Vec<String> = picks.iter().map(|pick| pick.track.uri.clone()).collect();
    let art = first.track.image(500);
    let tint = app
        .tint_for(art)
        .map(super::glow_color)
        .unwrap_or(palette.surface_active);
    let glow = super::entering(ui.ctx(), 0.05);
    ui.painter().rect_filled(rect, 18.0, palette.surface);
    motion::glow_in_card(
        ui.painter(),
        rect,
        18.0,
        rect.left_center() + egui::vec2(150.0, 0.0),
        width * 0.75,
        tint.gamma_multiply(0.75 * glow),
    );
    let cover = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 28.0 + 106.0, rect.center().y),
        egui::Vec2::splat(212.0),
    );
    widgets::paint_shadow(ui, &palette, cover, 12.0);
    widgets::paint_cover(
        ui,
        &palette,
        art,
        cover,
        12.0,
        Icon::Music,
        Some(app.backend.art()),
    );
    let side = if width > 1000.0 { 320.0 } else { 0.0 };
    let words = egui::Rect::from_min_max(
        egui::pos2(cover.right() + 28.0, rect.top() + 34.0),
        egui::pos2(rect.right() - side - 24.0, rect.bottom() - 24.0),
    );
    let mut block = ui.new_child(egui::UiBuilder::new().max_rect(words));
    block.spacing_mut().item_spacing.y = 8.0;
    theme::text(
        &mut block,
        gettext(app.locale, "Picked for you"),
        theme::medium(13.0),
        palette.secondary,
    );
    let title_color = super::kit::Source::of(&first.track.uri).color(&palette);
    let title = widgets::ellipsized(
        &block,
        &first.track.name,
        theme::semibold(30.0),
        title_color,
        words.width(),
        2,
    );
    block.add(egui::Label::new(title).selectable(false));
    widgets::artist_links(
        &mut block,
        app,
        &first.track.artists,
        theme::regular(14.0),
        palette.text,
    );
    theme::text(
        &mut block,
        first.reason.as_str(),
        theme::regular(13.0),
        palette.secondary,
    );
    block.add_space(4.0);
    block.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let current = app
            .current_track_uri()
            .is_some_and(|uri| uri == first.track.uri);
        let playing = current && app.believed_playing();
        if theme::circle_button(
            ui,
            if playing {
                Icon::PauseFilled
            } else {
                Icon::PlayFilled
            },
            48.0,
            palette.accent,
            palette.accent_hover,
            palette.on_accent,
            &gettext(app.locale, if playing { "Pause" } else { "Play" }),
        )
        .clicked()
        {
            app.actions.push(if current {
                Action::TogglePlay
            } else {
                Action::PlayUris {
                    uris: uris.clone(),
                    index: 0,
                }
            });
        }
    });
    if side > 0.0 {
        let list = egui::Rect::from_min_max(
            egui::pos2(rect.right() - side, rect.top() + 18.0),
            egui::pos2(rect.right() - 16.0, rect.bottom() - 18.0),
        );
        let row_height = list.height() / 4.0;
        for (index, pick) in picks.iter().skip(1).take(4).enumerate() {
            let row = egui::Rect::from_min_size(
                list.min + egui::vec2(0.0, index as f32 * row_height),
                egui::vec2(list.width(), row_height),
            );
            // They follow it in, one after another.
            let rose = super::entering(ui.ctx(), 0.1 + motion::stagger(index));
            let row = row.translate(egui::vec2((1.0 - rose) * 14.0, 0.0));
            let response = ui.interact(
                row,
                ui.id().with(("featured-next", index)),
                egui::Sense::click(),
            );
            let lit = motion::toward(
                ui.ctx(),
                response.id.with("lit"),
                response.hovered(),
                motion::FEEDBACK,
            );
            if lit > 0.0 {
                ui.painter().rect_filled(
                    row.shrink2(egui::vec2(0.0, 2.0)),
                    8.0,
                    palette.surface_hover.gamma_multiply(lit),
                );
            }
            let art_rect = egui::Rect::from_min_size(
                row.min + egui::vec2(6.0, 6.0),
                egui::Vec2::splat(row_height - 12.0),
            );
            widgets::paint_cover(
                ui,
                &palette,
                pick.track.image(120),
                art_rect,
                6.0,
                Icon::Music,
                Some(app.backend.art()),
            );
            let left = art_rect.right() + 10.0;
            let room = (row.right() - left - 6.0).max(10.0);
            let color = super::kit::Source::of(&pick.track.uri).color(&palette);
            let name =
                widgets::ellipsized(ui, &pick.track.name, theme::medium(13.5), color, room, 1);
            ui.painter()
                .galley(egui::pos2(left, row.center().y - 18.0), name, color);
            let artist = pick
                .track
                .artists
                .first()
                .map_or("", |artist| artist.name.as_str());
            let by =
                widgets::ellipsized(ui, artist, theme::regular(12.0), palette.secondary, room, 1);
            ui.painter().galley(
                egui::pos2(left, row.center().y + 2.0),
                by,
                palette.secondary,
            );
            if response.clicked() {
                app.actions.push(Action::PlayUris {
                    uris: uris.clone(),
                    index: index as u32 + 1,
                });
            }
        }
    }
}

/// More of what's on the featured card, across the sources.
fn more_for_you(app: &mut App, ui: &mut egui::Ui, picks: &[crate::for_you::Pick]) {
    let tracks: Vec<_> = picks
        .iter()
        .skip(5)
        .map(|pick| pick.track.clone())
        .collect();
    if tracks.is_empty() {
        return;
    }
    theme::section_title(ui, &app.palette, &gettext(app.locale, "More for you"));
    ui.add_space(6.0);
    super::source_page::song_cards(app, ui, &tracks, 2);
    ui.add_space(12.0);
}

/// New from the people and channels followed on SoundCloud and YouTube.
fn new_from_followed(app: &mut App, ui: &mut egui::Ui, tracks: &[crate::api::models::Track]) {
    if tracks.is_empty() {
        return;
    }
    theme::section_title(
        ui,
        &app.palette,
        &gettext(app.locale, "New from who you follow"),
    );
    ui.add_space(6.0);
    super::source_page::song_cards(app, ui, tracks, 1);
    ui.add_space(12.0);
}

/// Each source's own picks: SoundCloud's as cards, YouTube's as thumbnails.
fn source_picks(app: &mut App, ui: &mut egui::Ui, picks: &[crate::for_you::Pick]) {
    let of = |wanted: fn(&str) -> bool| -> Vec<crate::api::models::Track> {
        picks
            .iter()
            .filter(|pick| wanted(&pick.track.uri))
            .map(|pick| pick.track.clone())
            .collect()
    };
    let soundcloud = of(crate::soundcloud::is_track_uri);
    if !soundcloud.is_empty() {
        theme::section_title(ui, &app.palette, &gettext(app.locale, "SoundCloud for you"));
        ui.add_space(6.0);
        super::source_page::song_cards(app, ui, &soundcloud, 1);
        ui.add_space(12.0);
    }
    let youtube = of(crate::youtube::is_track_uri);
    if !youtube.is_empty() {
        theme::section_title(ui, &app.palette, &gettext(app.locale, "YouTube for you"));
        ui.add_space(6.0);
        super::source_page::video_cards(app, ui, &youtube, 1);
        ui.add_space(12.0);
    }
}

struct Tile {
    image: Option<String>,
    name: String,
    page: Page,
    uri: Option<String>,
    liked: bool,
    owned_playlist: Option<Playlist>,
    /// A song played on its own, which the tile plays and whose menu it
    /// offers, rather than a playlist, album or artist.
    song: Option<crate::api::models::Track>,
}

/// The height of a quick-access tile, and so the side of its cover.
const TILE_HEIGHT: f32 = 60.0;
/// How many quick-access tiles Home shows, as Spotify does.
const TILES: usize = 8;

/// What was played most recently, as Spotify's Home shows it: the playlists,
/// albums, artists and podcasts songs were played from, Liked Songs, and
/// songs played on their own, newest first (#942). Until Spotify's play
/// history arrives, or when it holds fewer, Liked Songs and the first
/// playlists in the Library fill the rest.
fn quick_access_tiles(app: &App) -> Vec<Tile> {
    let tile_art = app.art_px(TILE_HEIGHT);
    let collection = app
        .user
        .as_ref()
        .map(|user| format!("spotify:user:{}:collection", user.id));
    let liked_tile = || Tile {
        image: None,
        name: gettext(app.locale, "Liked Songs").into_owned(),
        page: Page::LikedSongs,
        uri: collection.clone(),
        liked: true,
        owned_playlist: None,
        song: None,
    };
    let playlist_tile = |playlist: &Playlist| Tile {
        image: pick_image(&playlist.images, tile_art).map(str::to_string),
        name: playlist.name.clone(),
        page: Page::from_uri(&playlist.uri).unwrap_or(Page::Search),
        uri: Some(playlist.uri.clone()),
        liked: false,
        owned_playlist: app
            .user_id()
            .is_some_and(|id| playlist.owned_by(id))
            .then(|| playlist.clone()),
        song: None,
    };
    let mut tiles: Vec<Tile> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for play in app.home.recently_played.get().into_iter().flatten() {
        if tiles.len() == TILES {
            break;
        }
        // Home is for music.
        if play
            .context
            .as_ref()
            .is_some_and(|context| context.uri.starts_with("spotify:show:"))
            || play.track.uri.starts_with("spotify:episode:")
        {
            continue;
        }
        let track = &play.track;
        let context = play.context.as_ref().map(|context| context.uri.as_str());
        let tile = match context.map(|uri| (uri, uri.rsplit(':').next().unwrap_or(""))) {
            Some((uri, _)) if uri.ends_with(":collection") => Some(liked_tile()),
            Some((uri, id)) if uri.starts_with("spotify:playlist:") => {
                app.known_playlist(id).map(playlist_tile)
            }
            Some((uri, id)) if uri.starts_with("spotify:album:") => app
                .known_album(id)
                .or(track.album.as_ref().filter(|album| album.id == id))
                .map(|album| Tile {
                    image: pick_image(&album.images, tile_art).map(str::to_string),
                    name: album.name.clone(),
                    page: Page::Album(album.id.clone()),
                    uri: Some(uri.to_string()),
                    liked: false,
                    owned_playlist: None,
                    song: None,
                }),
            Some((uri, id)) if uri.starts_with("spotify:artist:") => {
                app.known_artist(id).map(|artist| Tile {
                    image: pick_image(&artist.images, tile_art).map(str::to_string),
                    name: artist.name.clone(),
                    page: Page::from_uri(&artist.uri).unwrap_or(Page::Search),
                    uri: Some(uri.to_string()),
                    liked: false,
                    owned_playlist: None,
                    song: None,
                })
            }
            Some((uri, id)) if uri.starts_with("spotify:show:") => {
                app.known_show(id).map(|show| Tile {
                    image: pick_image(&show.images, tile_art).map(str::to_string),
                    name: show.name.clone(),
                    page: Page::Show(show.id.clone()),
                    uri: Some(uri.to_string()),
                    liked: false,
                    owned_playlist: None,
                    song: None,
                })
            }
            // A song played on its own, or from somewhere Home cannot name.
            _ => track
                .album
                .as_ref()
                .filter(|album| !album.id.is_empty())
                .map(|album| Tile {
                    image: track.image(tile_art).map(str::to_string),
                    name: track.name.clone(),
                    page: Page::from_uri(&album.uri).unwrap_or(Page::Search),
                    uri: Some(track.uri.clone()),
                    liked: false,
                    owned_playlist: None,
                    song: Some(track.clone()),
                }),
        };
        if let Some(tile) = tile
            && seen.insert(tile.page.clone())
        {
            tiles.push(tile);
        }
    }
    let library = app.library.playlists.get().into_iter().flatten();
    for tile in std::iter::once(liked_tile()).chain(library.map(playlist_tile)) {
        if tiles.len() == TILES {
            break;
        }
        if seen.insert(tile.page.clone()) {
            tiles.push(tile);
        }
    }
    tiles
}

fn quick_access(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let mut tiles = quick_access_tiles(app);
    let available = ui.available_width();
    let columns = ((available / 300.0).floor() as usize).clamp(2, 4);
    // A last row with a gap in it looks like something failed to load.
    if tiles.len() > columns {
        tiles.truncate(tiles.len() / columns * columns);
    }
    let gap = 10.0;
    let tile_width = (available - gap * (columns as f32 - 1.0)) / columns as f32;
    let rows = tiles.len().div_ceil(columns);
    for row in 0..rows {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let opacity = ui.opacity();
            for column in 0..columns {
                // They light up across and down, as the page comes in.
                let across = (row + column) as f32 * super::kit::motion::STAGGER;
                ui.set_opacity(opacity * super::entering(ui.ctx(), across).max(0.02));
                let Some(Tile {
                    image,
                    name,
                    page,
                    uri,
                    liked,
                    owned_playlist,
                    song,
                }) = tiles.get(row * columns + column)
                else {
                    break;
                };
                let (rect, response) =
                    ui.allocate_exact_size(vec2(tile_width, TILE_HEIGHT), Sense::click());
                if ui.is_rect_visible(rect) {
                    let hovered = super::pointer_over(ui, rect);
                    let lit = super::kit::motion::toward(
                        ui.ctx(),
                        response.id.with("lit"),
                        hovered,
                        super::kit::motion::FEEDBACK,
                    );
                    let fill = super::blend(palette.surface, palette.surface_hover, lit);
                    ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
                    if let Some((pointer, strength)) =
                        super::kit::motion::pointer_light(ui, response.id.with("light"), rect)
                    {
                        super::kit::motion::spotlight(
                            ui.painter(),
                            rect,
                            pointer,
                            130.0,
                            palette.text.gamma_multiply(0.07 * strength),
                        );
                    }
                    super::kit::motion::ripple(ui, &response, rect, palette.text);
                    let cover = Rect::from_min_size(rect.min, Vec2::splat(TILE_HEIGHT));
                    if response.clicked() {
                        super::kit::shared::launch(ui.ctx(), image.as_deref(), cover, 6.0);
                    }
                    if *liked {
                        super::sidebar::liked_cover(ui, cover, 6.0);
                    } else {
                        widgets::paint_cover(
                            ui,
                            &palette,
                            image.as_deref(),
                            cover,
                            6.0,
                            Icon::Music,
                            Some(app.backend.art()),
                        );
                    }
                    let play_room = if hovered && uri.is_some() { 52.0 } else { 12.0 };
                    let text_rect = Rect::from_min_max(
                        pos2(cover.right() + 12.0, rect.top()),
                        pos2(rect.right() - play_room, rect.bottom()),
                    );
                    crate::bidi::paint_line(
                        &ui.painter().with_clip_rect(text_rect),
                        text_rect.left(),
                        text_rect.right(),
                        rect.center().y,
                        name,
                        theme::bold(14.5),
                        palette.text,
                    );
                    if lit > 0.01
                        && let Some(uri) = uri
                    {
                        let context_here =
                            app.playing_context_uri().as_deref() == Some(uri.as_str());
                        let playing_here = context_here && app.believed_playing();
                        let button = Rect::from_center_size(
                            pos2(rect.right() - 28.0 + (1.0 - lit) * 10.0, rect.center().y),
                            Vec2::splat(40.0),
                        );
                        let mut child =
                            ui.new_child(egui::UiBuilder::new().max_rect(button).layout(
                                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                            ));
                        child.multiply_opacity(lit);
                        if theme::circle_button(
                            &mut child,
                            if playing_here {
                                Icon::PauseFilled
                            } else {
                                Icon::PlayFilled
                            },
                            40.0,
                            palette.accent,
                            palette.accent_hover,
                            palette.on_accent,
                            &gettext(app.locale, if playing_here { "Pause" } else { "Play" }),
                        )
                        .clicked()
                        {
                            if context_here {
                                app.actions.push(Action::TogglePlay);
                            } else if song.is_some() {
                                app.actions.push(Action::PlayUris {
                                    uris: vec![uri.clone()],
                                    index: 0,
                                });
                            } else {
                                app.actions.push(Action::PlayContext {
                                    uri: uri.clone(),
                                    offset_uri: None,
                                    offset_index: None,
                                });
                            }
                        }
                    }
                }
                if response.clicked() {
                    app.actions.push(Action::Open(page.clone()));
                }
                if !liked && let Some(uri) = uri {
                    egui::Popup::context_menu(&response)
                        .id(ui.make_persistent_id(("quick-access-menu", uri)))
                        .frame(widgets::menu_frame(&palette))
                        .show(|ui| match song {
                            Some(track) => widgets::item_menu(
                                ui,
                                app,
                                &PlayableItem::Track(track.clone()),
                                None,
                                None,
                            ),
                            None => widgets::context_menu_items(
                                ui,
                                app,
                                uri,
                                name,
                                owned_playlist.as_ref(),
                            ),
                        });
                }
            }
        });
    }
}

/// Renders session Home cards or approximate fallback cards in the same shelf.
fn made_for_you(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let (playlists, loading, failure) = match &app.home.made_for_you {
        Loadable::Loaded(playlists) => (playlists.clone(), false, None),
        Loadable::Loading | Loadable::NotLoaded => (Vec::new(), true, None),
        Loadable::Failed(error) => (Vec::new(), false, Some(error.clone())),
    };
    if playlists.is_empty() && !loading && failure.is_none() {
        return;
    }
    widgets::shelf(
        ui,
        &palette,
        "made-for-you",
        &gettext(app.locale, "Made for you"),
        |ui| {
            if playlists.is_empty() && loading {
                widgets::skeleton_cards(ui, &palette, app.locale, 7, false);
            } else if playlists.is_empty()
                && let Some(message) = &failure
            {
                widgets::error_row(ui, app, message, Some(Page::Home));
            }
            playlist_cards(app, ui, "home-made_for_you-menu", &playlists);
        },
    );
}

fn top_mixes(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let Some(playlists) = app
        .home
        .top_mixes
        .get()
        .filter(|playlists| !playlists.is_empty())
        .cloned()
    else {
        return;
    };
    widgets::shelf(
        ui,
        &palette,
        "top-mixes",
        &gettext(app.locale, "Your top mixes"),
        |ui| playlist_cards(app, ui, "home-top_mixes-menu", &playlists),
    );
}

/// The latest releases from followed artists. A card opens its release;
/// the title opens the page that plays all their songs, newest first.
fn new_releases(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let (albums, loading, failure) = match &app.home.new_releases {
        Loadable::Loaded(albums) => (albums.clone(), false, None),
        Loadable::Loading | Loadable::NotLoaded => (Vec::new(), true, None),
        Loadable::Failed(error) => (Vec::new(), false, Some(error.clone())),
    };
    if albums.is_empty() && !loading && failure.is_none() {
        return;
    }
    let open = widgets::linked_shelf(
        ui,
        &palette,
        "new-releases",
        &gettext(app.locale, "New releases"),
        |ui| {
            if albums.is_empty() && loading {
                widgets::loading_row(ui, &palette, app.locale);
            } else if albums.is_empty()
                && let Some(message) = &failure
            {
                widgets::error_row(ui, app, message, Some(Page::Home));
            }
            for album in &albums {
                release_card(app, ui, album);
            }
        },
    );
    if open {
        app.actions.push(Action::Open(Page::NewReleases));
    }
}

fn release_card(app: &mut App, ui: &mut egui::Ui, album: &Album) {
    let palette = app.palette;
    let artists =
        crate::api::models::join_names(album.artists.iter().map(|artist| artist.name.as_str()));
    let subtitle = if artists.is_empty() {
        album.kind_label(app.locale).into_owned()
    } else {
        format!("{} • {artists}", album.kind_label(app.locale))
    };
    let context_here = app.playing_context_uri().as_deref() == Some(album.uri.as_str());
    let playing_here = context_here && app.believed_playing();
    let card_art = app.art_px(widgets::CARD_ART);
    let card = widgets::card(
        ui,
        app,
        pick_image(&album.images, card_art),
        &album.name,
        &subtitle,
        widgets::CardCover::square(playing_here),
    );
    if card.play {
        if context_here {
            app.actions.push(Action::TogglePlay);
        } else {
            app.actions.push(Action::PlayContext {
                uri: album.uri.clone(),
                offset_uri: None,
                offset_index: None,
            });
        }
    }
    if card.clicked {
        app.actions
            .push(Action::Open(Page::Album(album.id.clone())));
    }
    egui::Popup::context_menu(&card.response)
        .id(ui.make_persistent_id(("home-new-releases-menu", &album.uri)))
        .frame(widgets::menu_frame(&palette))
        .show(|ui| {
            widgets::context_menu_items(ui, app, &album.uri, &album.name, None);
        });
}

fn playlist_cards(app: &mut App, ui: &mut egui::Ui, menu: &str, playlists: &[Playlist]) {
    let palette = app.palette;
    for playlist in playlists {
        let subtitle = playlist
            .description
            .as_deref()
            .map(crate::util::strip_html)
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| {
                // Translators: {owner} is the name of the playlist's owner.
                gettext(app.locale, "By {owner}").replace("{owner}", app.playlist_owner(playlist))
            });
        let context_here = app.playing_context_uri().as_deref() == Some(playlist.uri.as_str());
        let playing_here = context_here && app.believed_playing();
        let card_art = app.art_px(widgets::CARD_ART);
        let card = widgets::card(
            ui,
            app,
            pick_image(&playlist.images, card_art),
            &playlist.name,
            &subtitle,
            widgets::CardCover::square(playing_here),
        );
        if card.play {
            if context_here {
                app.actions.push(Action::TogglePlay);
            } else {
                app.actions.push(Action::PlayContext {
                    uri: playlist.uri.clone(),
                    offset_uri: None,
                    offset_index: None,
                });
            }
        }
        if card.clicked {
            app.actions.push(Action::Open(
                Page::from_uri(&playlist.uri).unwrap_or(Page::Search),
            ));
        }
        egui::Popup::context_menu(&card.response)
            .id(ui.make_persistent_id((menu, &playlist.uri)))
            .frame(widgets::menu_frame(&palette))
            .show(|ui| {
                let owned = app.user_id().is_some_and(|id| playlist.owned_by(id));
                widgets::context_menu_items(
                    ui,
                    app,
                    &playlist.uri,
                    &playlist.name,
                    owned.then_some(playlist),
                );
            });
    }
}

fn recently_played(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let history = match app.home.recently_played.clone() {
        Loadable::Loaded(history) => history,
        Loadable::Loading | Loadable::NotLoaded => {
            widgets::shelf(
                ui,
                &palette,
                "recent",
                &gettext(app.locale, "Recently played"),
                |ui| widgets::skeleton_cards(ui, &palette, app.locale, 7, false),
            );
            return;
        }
        Loadable::Failed(message) => {
            widgets::shelf(
                ui,
                &palette,
                "recent",
                &gettext(app.locale, "Recently played"),
                |ui| {
                    widgets::error_row(ui, app, &message, Some(Page::Home));
                },
            );
            return;
        }
    };
    let mut seen = std::collections::HashSet::new();
    let tracks: Vec<_> = history
        .into_iter()
        .filter(|entry| !entry.track.uri.starts_with("spotify:episode:"))
        .filter(|entry| {
            entry
                .track
                .id
                .as_ref()
                .is_some_and(|id| seen.insert(id.clone()))
        })
        .take(16)
        .collect();
    if tracks.is_empty() {
        return;
    }
    widgets::shelf(
        ui,
        &palette,
        "recent",
        &gettext(app.locale, "Recently played"),
        |ui| {
            for entry in &tracks {
                let track = &entry.track;
                let card_art = app.art_px(widgets::CARD_ART);
                let card = widgets::card(
                    ui,
                    app,
                    track.image(card_art),
                    &track.name,
                    &track.artist_names(),
                    widgets::CardCover::square(false),
                );
                if card.play {
                    app.actions.push(Action::PlayUris {
                        uris: vec![track.uri.clone()],
                        index: 0,
                    });
                }
                if card.clicked
                    && let Some(album) = &track.album
                    && !album.id.is_empty()
                {
                    app.actions
                        .push(Action::Open(Page::Album(album.id.clone())));
                }
                egui::Popup::context_menu(&card.response)
                    .id(ui.make_persistent_id(("home-recently_played-menu", &track.uri)))
                    .frame(widgets::menu_frame(&palette))
                    .show(|ui| {
                        widgets::item_menu(
                            ui,
                            app,
                            &PlayableItem::Track(track.clone()),
                            None,
                            None,
                        );
                    });
            }
        },
    );
}

fn top_artists(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let artists = match app.home.top_artists.clone() {
        Loadable::Loaded(artists) => artists,
        Loadable::Loading | Loadable::NotLoaded => {
            widgets::shelf(
                ui,
                &palette,
                "top-artists",
                &gettext(app.locale, "Your top artists"),
                |ui| widgets::skeleton_cards(ui, &palette, app.locale, 7, true),
            );
            return;
        }
        Loadable::Failed(message) => {
            widgets::shelf(
                ui,
                &palette,
                "top-artists",
                &gettext(app.locale, "Your top artists"),
                |ui| {
                    widgets::error_row(ui, app, &message, Some(Page::Home));
                },
            );
            return;
        }
    };
    if artists.is_empty() {
        return;
    }
    widgets::shelf(
        ui,
        &palette,
        "top-artists",
        &gettext(app.locale, "Your top artists"),
        |ui| {
            for artist in &artists {
                let context_here =
                    app.playing_context_uri().as_deref() == Some(artist.uri.as_str());
                let playing_here = context_here && app.believed_playing();
                let card_art = app.art_px(widgets::CARD_ART);
                let card = widgets::card(
                    ui,
                    app,
                    pick_image(&artist.images, card_art),
                    &artist.name,
                    &gettext(app.locale, "Artist"),
                    widgets::CardCover::portrait(playing_here),
                );
                if card.play {
                    if context_here {
                        app.actions.push(Action::TogglePlay);
                    } else {
                        app.actions.push(Action::PlayContext {
                            uri: artist.uri.clone(),
                            offset_uri: None,
                            offset_index: None,
                        });
                    }
                }
                if card.clicked {
                    app.actions.push(Action::Open(
                        Page::from_uri(&artist.uri).unwrap_or(Page::Search),
                    ));
                }
                egui::Popup::context_menu(&card.response)
                    .id(ui.make_persistent_id(("home-top_artists-menu", &artist.uri)))
                    .frame(widgets::menu_frame(&palette))
                    .show(|ui| {
                        widgets::context_menu_items(ui, app, &artist.uri, &artist.name, None);
                    });
            }
        },
    );
}

fn top_tracks(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let title = gettext(app.locale, "Your top songs");
    let more = gettext(app.locale, "Show more top songs");
    let tracks = app.home.top_tracks.clone();

    if theme::link(ui, &*title, theme::bold(17.0), palette.text).clicked() {
        app.actions.push(Action::Open(Page::TopSongs));
    }
    ui.add_space(4.0);

    let labels: Vec<_> = TopPeriod::ALL
        .iter()
        .map(|p| (*p, p.label(app.locale)))
        .collect();
    let options: Vec<(TopPeriod, &str)> = labels
        .iter()
        .map(|(period, label)| (*period, label.as_ref()))
        .collect();
    if let Some(period) = widgets::chips(ui, &palette, &options, app.home.top_period) {
        app.actions.push(Action::SetTopPeriod(period));
    }
    ui.add_space(8.0);

    let tracks = match tracks {
        Loadable::Loaded(tracks) => tracks,
        Loadable::Loading | Loadable::NotLoaded => {
            widgets::loading_row(ui, &palette, app.locale);
            ui.add_space(12.0);
            return;
        }
        Loadable::Failed(message) => {
            widgets::error_row(ui, app, &message, Some(Page::TopSongs));
            ui.add_space(12.0);
            return;
        }
    };

    let uris: Arc<[String]> = tracks
        .iter()
        .map(|track| track.uri.clone())
        .collect::<Vec<_>>()
        .into();
    let context = RowContext::Uris(Arc::clone(&uris));
    for (index, track) in tracks.iter().take(10).enumerate() {
        let item = PlayableItem::Track(track.clone());
        widgets::track_row(
            ui,
            app,
            TrackRow {
                index,
                number: None,
                item: &item,
                context: &context,
                show_cover: !app.settings.tracklist_compact,
                show_album: true,
                added_at: None,
                added_by: None,
                show_added_by: false,
                show_plays: false,
                compact: false,
                thin: app.settings.tracklist_compact,
                shift: 0.0,
                picked: false,
                picked_songs: &[],
                picked_positions: &[],
                position: None,
            },
        );
    }
    if tracks.len() > 10
        && theme::link(ui, &*more, theme::semibold(14.0), palette.secondary).clicked()
    {
        app.actions.push(Action::Open(Page::TopSongs));
    }
    ui.add_space(16.0);
}

#[cfg(test)]
mod tests {

    /// A tile's cover is 60 points. Spotify's 64 pixel rendition would be
    /// blurry even at 100%, so the tile takes the 300 pixel one and scales
    /// it down; at 300% the 300 pixel one still has a quarter more pixels
    /// than the tile shows.
    #[cfg(feature = "demo")]
    #[test]
    fn quick_access_tiles_ask_for_art_the_display_can_show() {
        let root = std::env::temp_dir().join(format!("spotifast-tiles-{}", std::process::id()));
        let mut app = App::new(
            &crate::backend::Waker::default(),
            crate::paths::AppDirs {
                config: root.join("config"),
                state: root.join("state"),
                cache: root.join("cache"),
            },
            crate::settings::Settings::default(),
            crate::app::AppOptions {
                media_controls: false,
                restore_sign_in: false,
                tray: false,
                autostart: false,
            },
        );
        crate::demo::populate(&mut app);
        // Only the Library's playlists, so the second tile is the first one.
        app.home.recently_played = Loadable::Loaded(Vec::new());
        let first = app.library.playlists.get().unwrap()[0].images.clone();
        let width = |url: Option<&str>| {
            first
                .iter()
                .find(|image| Some(image.url.as_str()) == url)
                .and_then(|image| image.width)
        };
        app.pixels_per_point = 1.0;
        assert_eq!(
            width(quick_access_tiles(&app)[1].image.as_deref()),
            Some(300)
        );
        app.pixels_per_point = 3.0;
        assert_eq!(
            width(quick_access_tiles(&app)[1].image.as_deref()),
            Some(300)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// Home's tiles follow what was played, newest first: the playlist,
    /// album or Liked Songs a song came from, or the song itself, each once.
    /// The Library's playlists fill what history leaves, and stand in until
    /// it arrives (#942).
    #[cfg(feature = "demo")]
    #[test]
    fn quick_access_tiles_follow_what_was_played() {
        let root =
            std::env::temp_dir().join(format!("spotifast-recent-tiles-{}", std::process::id()));
        let mut app = App::new(
            &crate::backend::Waker::default(),
            crate::paths::AppDirs {
                config: root.join("config"),
                state: root.join("state"),
                cache: root.join("cache"),
            },
            crate::settings::Settings::default(),
            crate::app::AppOptions {
                media_controls: false,
                restore_sign_in: false,
                tray: false,
                autostart: false,
            },
        );
        crate::demo::populate(&mut app);
        let history = app.home.recently_played.get().unwrap().clone();
        let tiles = quick_access_tiles(&app);
        assert_eq!(tiles.len(), TILES);
        assert_eq!(
            tiles[0].page,
            Page::Playlist("pl1".into()),
            "played from it"
        );
        let song = &history[2].track;
        assert_eq!(
            tiles[1].uri.as_deref(),
            Some(song.uri.as_str()),
            "played on its own"
        );
        assert!(tiles[1].song.is_some());
        let pages: Vec<_> = tiles.iter().map(|tile| tile.page.clone()).collect();
        assert_eq!(
            pages
                .iter()
                .filter(|page| **page == Page::LikedSongs)
                .count(),
            1
        );
        let unique: std::collections::HashSet<_> = pages.iter().collect();
        assert_eq!(unique.len(), pages.len(), "each once");

        app.home.recently_played = Loadable::Loading;
        let tiles = quick_access_tiles(&app);
        assert_eq!(
            tiles[0].page,
            Page::LikedSongs,
            "the Library until history arrives"
        );
        let first = app.library.playlists.get().unwrap()[0].id.clone();
        assert_eq!(tiles[1].page, Page::Playlist(first));
        assert_eq!(tiles.len(), TILES);
        app.backend.shutdown();
        let _ = std::fs::remove_dir_all(root);
    }
}
