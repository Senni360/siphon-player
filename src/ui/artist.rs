//! The artist page.

use std::sync::Arc;

use crate::api::models::{Artist, PlayableItem, pick_image};
use crate::app::App;
use crate::artist_sources::{Account, Confidence, Finding, Side};
use crate::i18n::{gettext, ngettext, pgettext};
use crate::model::{
    Action, ArtistChoice, ArtistPage, DiscographyFilter, Loadable, Page, RowContext,
};
use crate::theme::{self, Icon};
use crate::ui::kit::Source;
use crate::util;

use super::collection::{Hero, HeroArt, hero, hero_images};
use super::widgets::{self, TrackRow};

pub fn show(app: &mut App, ui: &mut egui::Ui, id: &str) {
    if !app.artist_pages.contains_key(id) {
        app.ensure_loaded(Page::Artist(id.to_string()));
    }
    let Some(page) = app.artist_pages.remove(id) else {
        return;
    };
    let preview =
        super::loading_preview(ui.ctx(), id, &page.artist, || app.known_artist(id).cloned());
    let palette = app.palette;
    let locale = app.locale;
    match &page.artist {
        Loadable::Loaded(artist) => {
            artist_hero(app, ui, artist, preview.as_deref());
            artist_actions(app, ui, artist, Some(&page));
            accounts_row(app, ui, id, artist, &page);

            // Popular.
            theme::section_title(ui, &palette, &gettext(locale, "Popular"));
            ui.add_space(4.0);
            match &page.top_tracks {
                Loadable::Loaded(tracks) if !tracks.is_empty() => {
                    let uris: Arc<[String]> = tracks
                        .iter()
                        .map(|track| track.uri.clone())
                        .collect::<Vec<_>>()
                        .into();
                    let context = RowContext::Uris(Arc::clone(&uris));
                    let items: Vec<PlayableItem> =
                        tracks.iter().cloned().map(PlayableItem::Track).collect();
                    let limit = if page.show_all_top { items.len() } else { 5 };
                    app.request_play_counts(&artist.uri);
                    let show_plays = app.has_play_counts(&artist.uri);
                    let render_top = |ui: &mut egui::Ui, app: &mut App| {
                        for (index, item) in items.iter().take(limit).enumerate() {
                            widgets::track_row(
                                ui,
                                app,
                                TrackRow {
                                    index,
                                    number: Some(index + 1),
                                    item,
                                    context: &context,
                                    show_cover: !app.settings.tracklist_compact,
                                    show_album: false,
                                    added_at: None,
                                    added_by: None,
                                    show_added_by: false,
                                    show_plays,
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
                    };
                    if !app.network_reachable {
                        ui.add_enabled_ui(false, |ui| {
                            ui.multiply_opacity(0.55);
                            render_top(ui, app);
                        });
                    } else {
                        render_top(ui, app);
                    }
                    if items.len() > 5 {
                        ui.add_space(6.0);
                        if theme::soft_button(
                            ui,
                            &palette,
                            None,
                            &if page.show_all_top {
                                gettext(locale, "Show less")
                            } else {
                                gettext(locale, "See more")
                            },
                            false,
                        )
                        .clicked()
                        {
                            app.actions.push(Action::ToggleShowAllTop(id.to_string()));
                        }
                    }
                }
                Loadable::Loaded(_) => {
                    theme::subtle(ui, &palette, &gettext(locale, "No popular songs to show."));
                }
                Loadable::Loading | Loadable::NotLoaded => {
                    widgets::skeleton_rows(ui, &palette, app.locale, 5)
                }
                Loadable::Failed(error) => {
                    let error = error.clone();
                    widgets::error_row(ui, app, &error, None);
                }
            }
            ui.add_space(20.0);

            elsewhere_songs(app, ui, id, &page);

            // Discography.
            theme::section_title(ui, &palette, &gettext(locale, "Discography"));
            ui.add_space(6.0);
            let labels: Vec<_> = DiscographyFilter::ALL
                .iter()
                .map(|f| (*f, f.label(locale)))
                .collect();
            let options: Vec<(DiscographyFilter, &str)> = labels
                .iter()
                .map(|(filter, label)| (*filter, label.as_ref()))
                .collect();
            if let Some(filter) = widgets::chips(ui, &palette, &options, page.filter) {
                app.actions.push(Action::SetDiscographyFilter {
                    artist_id: id.to_string(),
                    filter,
                });
            }
            ui.add_space(10.0);
            match page.albums.get(page.filter.groups()) {
                Some(list) => {
                    let mut seen = std::collections::HashSet::new();
                    let albums: Vec<_> = list
                        .items
                        .iter()
                        .filter(|album| seen.insert(album.name.to_lowercase()))
                        .collect();
                    widgets::grid(ui, |ui| {
                        for album in &albums {
                            let subtitle = format!(
                                "{} • {}",
                                album.year().unwrap_or(""),
                                app.album_kind_label(album)
                            );
                            let context_here =
                                app.playing_context_uri().as_deref() == Some(album.uri.as_str());
                            let playing_here = context_here && app.believed_playing();
                            let card_art = app.art_px(widgets::CARD_ART);
                            let card = widgets::card(
                                ui,
                                app,
                                pick_image(&album.images, card_art),
                                &album.name,
                                subtitle.trim_start_matches(" • "),
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
                                .id(ui.make_persistent_id(("discography-menu", &album.uri)))
                                .frame(widgets::menu_frame(&palette))
                                .show(|ui| {
                                    widgets::context_menu_items(
                                        ui,
                                        app,
                                        &album.uri,
                                        &album.name,
                                        None,
                                    )
                                });
                        }
                    });
                    if list.loading {
                        widgets::loading_row(ui, &palette, app.locale);
                    } else if let Some(error) = &list.error {
                        let error = error.clone();
                        widgets::error_row(ui, app, &error, None);
                    } else if list.items.is_empty() {
                        theme::subtle(ui, &palette, &gettext(locale, "Nothing in this category."));
                    } else if list.can_load_more() {
                        ui.add_space(8.0);
                        if theme::soft_button(
                            ui,
                            &palette,
                            None,
                            &gettext(locale, "Load more"),
                            false,
                        )
                        .clicked()
                        {
                            app.actions
                                .push(Action::LoadMoreArtistAlbums(id.to_string()));
                        }
                    }
                }
                None => widgets::grid(ui, |ui| {
                    widgets::skeleton_cards(ui, &palette, app.locale, 6, false)
                }),
            }
            ui.add_space(20.0);

            elsewhere_edits(app, ui, id, &page);

            // Related.
            if let Loadable::Loaded(related) = &page.related
                && !related.is_empty()
            {
                let artist_label = gettext(locale, "Artist");
                let title = gettext(locale, "Fans also like");
                widgets::shelf(ui, &palette, "related", &title, |ui| {
                    for artist in related {
                        let context_here =
                            app.playing_context_uri().as_deref() == Some(artist.uri.as_str());
                        let playing_here = context_here && app.believed_playing();
                        let card_art = app.art_px(widgets::CARD_ART);
                        let card = widgets::card(
                            ui,
                            app,
                            pick_image(&artist.images, card_art),
                            &artist.name,
                            &artist_label,
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
                            .id(ui.make_persistent_id(("related-artist-menu", &artist.uri)))
                            .frame(widgets::menu_frame(&palette))
                            .show(|ui| {
                                widgets::context_menu_items(
                                    ui,
                                    app,
                                    &artist.uri,
                                    &artist.name,
                                    None,
                                )
                            });
                    }
                });
            }
        }
        Loadable::Loading | Loadable::NotLoaded => {
            if let Some(artist) = &preview {
                artist_hero(app, ui, artist, None);
            } else {
                ui.add_space(40.0);
            }
            if !app.network_reachable {
                widgets::empty_state(
                    ui,
                    &palette,
                    Icon::WifiOff,
                    &gettext(app.locale, "You're offline"),
                    &gettext(
                        app.locale,
                        "Encore will reconnect automatically when your connection returns.",
                    ),
                );
            } else {
                if let Some(artist) = &preview {
                    ui.add_enabled_ui(false, |ui| artist_actions(app, ui, artist, None));
                }
                widgets::skeleton_rows(ui, &palette, app.locale, 5);
            }
        }
        Loadable::Failed(error) => {
            let error = error.clone();
            if let Some(artist) = &preview {
                artist_hero(app, ui, artist, None);
            } else {
                ui.add_space(40.0);
            }
            if !app.network_reachable
                || error.contains("network error")
                || error.contains("Network connection error")
            {
                widgets::empty_state(
                    ui,
                    &palette,
                    Icon::WifiOff,
                    &gettext(app.locale, "You're offline"),
                    &gettext(
                        app.locale,
                        "Encore will reconnect automatically when your connection returns.",
                    ),
                );
            } else {
                if let Some(artist) = &preview {
                    ui.add_enabled_ui(false, |ui| artist_actions(app, ui, artist, None));
                }
                widgets::error_row(ui, app, &error, Some(Page::Artist(id.to_string())));
            }
        }
    }
    app.artist_pages.insert(id.to_string(), page);
    app.ask_artist_sources(id);
}

fn artist_hero(app: &mut App, ui: &mut egui::Ui, artist: &Artist, preview: Option<&Artist>) {
    let locale = app.locale;
    let mut byline = Vec::new();
    if let Some(followers) = &artist.followers {
        byline.push((
            ngettext(
                locale,
                // Translators: {count} is the number of people who follow an artist.
                "{count} follower",
                "{count} followers",
                u32::try_from(followers.total).unwrap_or(u32::MAX),
            )
            .replace("{count}", &util::format_count(followers.total)),
            None,
        ));
    }
    if !artist.genres.is_empty() {
        byline.push((
            artist
                .genres
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            None,
        ));
    }
    let images = hero_images(
        &artist.images,
        preview.map(|artist| artist.images.as_slice()),
        false,
        HeroArt::of(app),
    );
    hero(
        app,
        ui,
        Hero {
            images,
            liked: false,
            kind: gettext(locale, "Artist"),
            title: &artist.name,
            description: None,
            byline,
            avatar: None,
            round: true,
        },
    );
}

fn artist_actions(app: &mut App, ui: &mut egui::Ui, artist: &Artist, page: Option<&ArtistPage>) {
    let palette = app.palette;
    let locale = app.locale;
    let following = app.is_saved(&artist.uri).unwrap_or(false);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        if app.play_pending(&artist.uri) {
            theme::circle_spinner(
                ui,
                56.0,
                palette.accent,
                palette.on_accent,
                &gettext(locale, "Starting…"),
            );
        } else if theme::circle_button(
            ui,
            Icon::PlayFilled,
            56.0,
            palette.accent,
            palette.accent_hover,
            palette.on_accent,
            &gettext(locale, "Play"),
        )
        .clicked()
        {
            app.actions.push(Action::PlayContext {
                uri: artist.uri.clone(),
                offset_uri: None,
                offset_index: None,
            });
        }
        if util::uri_kind(&artist.uri) == Some("artist")
            && theme::pill_button(
                ui,
                &palette,
                &if following {
                    pgettext(locale, "artist", "Following")
                } else {
                    pgettext(locale, "artist", "Follow")
                },
                false,
            )
            .clicked()
        {
            app.actions.push(Action::ToggleSaved(artist.uri.clone()));
        }
        // Spotify's songs and what's only elsewhere, as one queue.
        if let Some(page) = page
            && !page.sources.songs.is_empty()
            && theme::pill_button(ui, &palette, &gettext(locale, "Play everything"), false)
                .clicked()
        {
            let mut uris: Vec<String> = page
                .top_tracks
                .get()
                .into_iter()
                .flatten()
                .map(|track| track.uri.clone())
                .collect();
            uris.extend(page.sources.songs.iter().map(|track| track.uri.clone()));
            app.actions.push(Action::PlayUris { uris, index: 0 });
        }
        let more = theme::icon_button(
            ui,
            Icon::Ellipsis,
            26.0,
            palette.secondary,
            palette.text,
            &gettext(locale, "More"),
        );
        egui::Popup::menu(&more)
            .frame(widgets::menu_frame(&palette))
            .show(|ui| widgets::context_menu_items(ui, app, &artist.uri, &artist.name, None));
    });
    ui.add_space(20.0);
}

const SIDES: [Side; 2] = [Side::SoundCloud, Side::YouTube];
const ACCOUNT_HEIGHT: f32 = 34.0;
const SHOWN_SONGS: usize = 5;

fn source_of(side: Side) -> Source {
    match side {
        Side::SoundCloud => Source::SoundCloud,
        Side::YouTube => Source::YouTube,
    }
}

fn followers_line(locale: crate::i18n::Locale, account: &Account) -> Option<String> {
    let followers = account.followers?;
    Some(
        ngettext(
            locale,
            "{count} follower",
            "{count} followers",
            u32::try_from(followers).unwrap_or(u32::MAX),
        )
        .replace("{count}", &util::format_count(followers)),
    )
}

/// The artist on the other sources: a chip for each account that opens it, or a
/// question when the page can't tell which account is theirs.
fn accounts_row(app: &mut App, ui: &mut egui::Ui, id: &str, artist: &Artist, page: &ArtistPage) {
    let palette = app.palette;
    let locale = app.locale;
    let sources = &page.sources;
    let any = SIDES.iter().any(|side| match sources.finding(*side) {
        Loadable::Loaded(found) => {
            found.account.is_some() || !found.candidates.is_empty() || found.not_there
        }
        Loadable::Loading => true,
        _ => false,
    });
    if !any {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        for side in SIDES {
            match sources.finding(side) {
                Loadable::Loading => {
                    let looking = gettext(locale, "Looking on {source}…")
                        .replace("{source}", source_of(side).name());
                    theme::text(ui, &looking, theme::regular(13.0), palette.dim);
                }
                Loadable::Loaded(found) => match &found.account {
                    Some(account) => account_chip(app, ui, id, side, account, found),
                    None if found.not_there => {
                        let again = gettext(locale, "Find on {source}")
                            .replace("{source}", source_of(side).name());
                        if theme::soft_button(ui, &palette, Some(Icon::Search), &again, false)
                            .clicked()
                        {
                            app.actions.push(Action::ChooseArtistAccount {
                                artist_id: id.to_string(),
                                side,
                                choice: ArtistChoice::SearchAgain,
                            });
                        }
                    }
                    None => {}
                },
                _ => {}
            }
        }
    });
    for side in SIDES {
        if let Loadable::Loaded(found) = sources.finding(side)
            && found.account.is_none()
            && !found.not_there
            && !found.candidates.is_empty()
        {
            ui.add_space(8.0);
            which_account(app, ui, id, side, artist, found);
        }
    }
    ui.add_space(16.0);
}

/// Paints a picture, a name and a line of small print on a pill, and says if it's
/// pressed.
fn account_pill(
    app: &App,
    ui: &mut egui::Ui,
    side: Side,
    account: &Account,
    reserve: f32,
) -> (egui::Response, egui::Rect) {
    let palette = app.palette;
    let name = ui
        .painter()
        .layout_no_wrap(account.name.clone(), theme::medium(13.0), palette.text);
    let source = source_of(side);
    let mark = ui.painter().layout_no_wrap(
        source.name().to_owned(),
        theme::regular(11.0),
        source.color(&palette),
    );
    let text_width = name.size().x.max(mark.size().x).min(180.0);
    let width = 6.0 + 24.0 + 8.0 + text_width + 12.0 + reserve;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(width, ACCOUNT_HEIGHT + 6.0),
        egui::Sense::click(),
    );
    let lit = super::kit::motion::toward(
        ui.ctx(),
        response.id.with("lit"),
        response.hovered(),
        super::kit::motion::POPOVER,
    );
    let radius = rect.height() / 2.0;
    ui.painter().rect_filled(
        rect,
        radius,
        palette.surface.lerp_to_gamma(palette.surface_hover, lit),
    );
    ui.painter().rect_stroke(
        rect,
        radius,
        theme::field_stroke(&palette),
        egui::StrokeKind::Inside,
    );
    let picture = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 6.0 + 12.0 + 2.0, rect.center().y),
        egui::Vec2::splat(26.0),
    );
    widgets::paint_cover(
        ui,
        &palette,
        account.image.as_deref(),
        picture,
        13.0,
        Icon::User,
        Some(app.backend.art()),
    );
    let text_left = picture.right() + 8.0;
    let clip = egui::Rect::from_min_size(
        egui::pos2(text_left, rect.top()),
        egui::vec2(text_width, rect.height()),
    );
    let painter = ui.painter().with_clip_rect(clip.intersect(ui.clip_rect()));
    painter.galley(
        egui::pos2(text_left, rect.center().y - name.size().y + 1.0),
        name,
        palette.text,
    );
    painter.galley(
        egui::pos2(text_left, rect.center().y + 1.0),
        mark,
        source.color(&palette),
    );
    (response, rect)
}

/// An account of theirs: pressing it opens its page, and the arrow beside it says
/// it's the wrong one or follows it.
fn account_chip(
    app: &mut App,
    ui: &mut egui::Ui,
    id: &str,
    side: Side,
    account: &Account,
    found: &Finding,
) {
    const ARROW: f32 = 26.0;
    let palette = app.palette;
    let (response, rect) = account_pill(app, ui, side, account, ARROW);
    let arrow_rect =
        egui::Rect::from_min_max(egui::pos2(rect.right() - ARROW - 4.0, rect.top()), rect.max);
    let arrow = ui.interact(arrow_rect, response.id.with("arrow"), egui::Sense::click());
    Icon::ChevronDown
        .image(
            if arrow.hovered() {
                palette.text
            } else {
                palette.secondary
            },
            14.0,
        )
        .paint_at(
            ui,
            egui::Rect::from_center_size(arrow_rect.center(), egui::Vec2::splat(14.0)),
        );
    if response.clicked() && !arrow.hovered() {
        app.actions
            .push(Action::Open(Page::SoundCloud(account.uri.clone())));
    }
    let locale = app.locale;
    let details = followers_line(locale, account);
    let response = response.on_hover_text(match (&details, found.confidence) {
        (Some(line), Some(Confidence::Likely)) => format!(
            "{line}\n{}",
            gettext(locale, "Probably them. The arrow changes it if not.")
        ),
        (Some(line), _) => line.clone(),
        (None, _) => account.name.clone(),
    });
    let _ = response;
    egui::Popup::menu(&arrow)
        .frame(widgets::menu_frame(&palette))
        .show(|ui| {
            let following = match side {
                Side::SoundCloud => account
                    .id
                    .parse()
                    .is_ok_and(|user| app.follows_soundcloud_user(user)),
                Side::YouTube => app.follows_youtube_channel(&account.id),
            };
            let follow = if following {
                gettext(locale, "Following")
            } else {
                gettext(locale, "Follow")
            };
            let icon = if following { Icon::Check } else { Icon::Plus };
            if widgets::menu_item(ui, &palette, Some(icon), &follow) {
                match side {
                    Side::SoundCloud => {
                        if let Ok(user) = account.id.parse() {
                            app.actions.push(Action::ToggleSoundCloudFollow {
                                id: user,
                                name: account.name.clone(),
                                image: account.image.clone(),
                            });
                        }
                    }
                    Side::YouTube => app.actions.push(Action::ToggleYouTubeFollow(
                        crate::youtube::likes::Channel {
                            id: account.id.clone(),
                            name: account.name.clone(),
                            image: account.image.clone(),
                            subscription: None,
                        },
                    )),
                }
            }
            if widgets::menu_item(
                ui,
                &palette,
                Some(Icon::User),
                &gettext(locale, "Open profile"),
            ) {
                app.actions
                    .push(Action::Open(Page::SoundCloud(account.uri.clone())));
            }
            ui.separator();
            for other in &found.candidates {
                let label = gettext(locale, "It's {name} instead").replace("{name}", &other.name);
                let label = match followers_line(locale, other) {
                    Some(line) => format!("{label} · {line}"),
                    None => label,
                };
                if widgets::menu_item(ui, &palette, None, &label) {
                    app.actions.push(Action::ChooseArtistAccount {
                        artist_id: id.to_string(),
                        side,
                        choice: ArtistChoice::Account(other.clone()),
                    });
                }
            }
            let none =
                gettext(locale, "Not on {source}").replace("{source}", source_of(side).name());
            if widgets::menu_item(ui, &palette, Some(Icon::X), &none) {
                app.actions.push(Action::ChooseArtistAccount {
                    artist_id: id.to_string(),
                    side,
                    choice: ArtistChoice::NotThere,
                });
            }
        });
}

/// Asks which of a few accounts of that name is the artist's, rather than guess.
fn which_account(
    app: &mut App,
    ui: &mut egui::Ui,
    id: &str,
    side: Side,
    artist: &Artist,
    found: &Finding,
) {
    let palette = app.palette;
    let locale = app.locale;
    let question = gettext(locale, "Which of these is {artist} on {source}?")
        .replace("{artist}", &artist.name)
        .replace("{source}", source_of(side).name());
    theme::text(ui, &question, theme::regular(13.0), palette.secondary);
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        for account in &found.candidates {
            let (response, _) = account_pill(app, ui, side, account, 0.0);
            let response = match followers_line(locale, account) {
                Some(line) => response.on_hover_text(line),
                None => response,
            };
            if response.clicked() {
                app.actions.push(Action::ChooseArtistAccount {
                    artist_id: id.to_string(),
                    side,
                    choice: ArtistChoice::Account(account.clone()),
                });
            }
        }
        if theme::soft_button(ui, &palette, None, &gettext(locale, "None of these"), false)
            .clicked()
        {
            app.actions.push(Action::ChooseArtistAccount {
                artist_id: id.to_string(),
                side,
                choice: ArtistChoice::NotThere,
            });
        }
    });
}

/// Rows of songs from anywhere, with their source on them, that play in this order.
fn elsewhere_rows(
    app: &mut App,
    ui: &mut egui::Ui,
    songs: &[crate::api::models::Track],
    limit: usize,
) {
    let uris: Arc<[String]> = songs
        .iter()
        .map(|track| track.uri.clone())
        .collect::<Vec<_>>()
        .into();
    let context = RowContext::Uris(uris);
    for (index, track) in songs.iter().take(limit).enumerate() {
        let item = PlayableItem::Track(track.clone());
        widgets::track_row(
            ui,
            app,
            TrackRow {
                index,
                number: Some(index + 1),
                item: &item,
                context: &context,
                show_cover: !app.settings.tracklist_compact,
                show_album: false,
                added_at: None,
                added_by: None,
                show_added_by: false,
                show_plays: track.play_count.is_some(),
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
}

/// How many songs came from where, under a section's title.
fn from_where(locale: crate::i18n::Locale, songs: &[crate::api::models::Track]) -> String {
    let mut parts = Vec::new();
    for source in [Source::SoundCloud, Source::YouTube] {
        let count = songs
            .iter()
            .filter(|track| Source::of(&track.uri) == source)
            .count();
        if count > 0 {
            parts.push(
                gettext(locale, "{count} on {source}")
                    .replace("{count}", &count.to_string())
                    .replace("{source}", source.name()),
            );
        }
    }
    parts.join(" · ")
}

/// Their songs that Spotify doesn't have, from wherever they are.
fn elsewhere_songs(app: &mut App, ui: &mut egui::Ui, id: &str, page: &ArtistPage) {
    let palette = app.palette;
    let locale = app.locale;
    let sources = &page.sources;
    if sources.songs.is_empty() {
        if sources.looking() {
            theme::section_title(ui, &palette, &gettext(locale, "Not on Spotify"));
            ui.add_space(4.0);
            widgets::skeleton_rows(ui, &palette, locale, 3);
            ui.add_space(20.0);
        }
        return;
    }
    theme::section_title(ui, &palette, &gettext(locale, "Not on Spotify"));
    theme::subtle(ui, &palette, &from_where(locale, &sources.songs));
    ui.add_space(4.0);
    let limit = if sources.show_all {
        sources.songs.len()
    } else {
        SHOWN_SONGS
    };
    elsewhere_rows(app, ui, &sources.songs, limit);
    see_more(app, ui, id, sources.songs.len(), sources.show_all, false);
    ui.add_space(20.0);
}

/// Other people's versions of their songs.
fn elsewhere_edits(app: &mut App, ui: &mut egui::Ui, id: &str, page: &ArtistPage) {
    let palette = app.palette;
    let locale = app.locale;
    let sources = &page.sources;
    if sources.edits.is_empty() {
        return;
    }
    theme::section_title(ui, &palette, &gettext(locale, "Edits and remixes"));
    theme::subtle(ui, &palette, &from_where(locale, &sources.edits));
    ui.add_space(4.0);
    let limit = if sources.show_all_edits {
        sources.edits.len()
    } else {
        SHOWN_SONGS
    };
    elsewhere_rows(app, ui, &sources.edits, limit);
    see_more(
        app,
        ui,
        id,
        sources.edits.len(),
        sources.show_all_edits,
        true,
    );
    ui.add_space(20.0);
}

fn see_more(app: &mut App, ui: &mut egui::Ui, id: &str, count: usize, open: bool, edits: bool) {
    if count <= SHOWN_SONGS {
        return;
    }
    let palette = app.palette;
    let locale = app.locale;
    ui.add_space(6.0);
    let label = if open {
        gettext(locale, "Show less")
    } else {
        gettext(locale, "See more")
    };
    if theme::soft_button(ui, &palette, None, &label, false).clicked() {
        app.actions.push(Action::ToggleShowAllElsewhere {
            artist_id: id.to_string(),
            edits,
        });
    }
}
