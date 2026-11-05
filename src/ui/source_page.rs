//! Pages of the sources other than Spotify. They share one structure (a profile
//! or a title, tabs, then what the page lists) and differ where their sources do:
//! YouTube is channels and a grid of thumbnails, SoundCloud is people, rows of
//! tracks and a Discover page.

use egui::{Color32, Rect, Sense, Ui, UiBuilder, pos2, vec2};

use super::collection::{self, Hero, HeroImages, Table};
use super::kit::motion;
use super::widgets;
use crate::api::models::{PlayableItem, Track};
use crate::app::App;
use crate::i18n::gettext;
use crate::model::{
    Action, Loadable, Page, RowContext, SearchFilter, SearchSource, SoundCloudCollection,
    SourceCard, SourceProfile, SourceShelf, TableItem,
};
use crate::soundcloud as sc;
use crate::theme::{self, Icon};
use crate::youtube::browse as yt;

/// The narrowest a video's thumbnail gets before a grid takes one column fewer.
const VIDEO_MIN_WIDTH: f32 = 236.0;
const VIDEO_GAP: f32 = 16.0;
/// A video card's words under its thumbnail.
const VIDEO_TEXT: f32 = 76.0;

pub fn show(app: &mut App, ui: &mut Ui, uri: &str) {
    if uri == sc::FEED {
        soundcloud_feed(app, ui);
        return;
    }
    let Some(state) = app.soundcloud_pages.get(uri) else {
        return;
    };
    let content = state.content.get().cloned().unwrap_or_default();
    let loading = state.loading;
    let error = state.error.clone();
    let revision = state.generation;
    let page = Page::SoundCloud(uri.to_owned());

    if uri == sc::HOME {
        soundcloud_home(app, ui, &content, loading, error.as_deref(), revision);
        return;
    }
    if let Some(genre) = sc::charts_genre(uri) {
        ui.add_space(8.0);
        let title = gettext(app.locale, "Trending");
        widgets::title_in_words(ui, &title, theme::semibold(30.0), app.palette.text);
        ui.add_space(12.0);
        genre_chips(app, ui, Some(genre));
        ui.add_space(12.0);
        tracks_section(
            app,
            ui,
            &page,
            &content,
            loading,
            error.as_deref(),
            revision,
        );
        return;
    }
    if uri == yt::HOME {
        youtube_home(app, ui, &content, loading, error.as_deref());
        return;
    }

    let channel = yt::target(uri).is_some_and(|target| {
        matches!(
            target,
            yt::Target::Channel { .. } | yt::Target::VideoChannel(_)
        )
    });
    let profiled = channel || sc::user_tab(uri).is_some();
    if profiled {
        let profile = content
            .profile
            .clone()
            .or_else(|| sibling_profile(app, uri));
        match &profile {
            Some(profile) => profile_header(app, ui, uri, profile, &content),
            None if loading => {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    widgets::skeleton_cards(ui, &app.palette, app.locale, 1, true);
                });
            }
            None => {}
        }
        ui.add_space(14.0);
        if let Some(profile) = &profile {
            profile_tabs(app, ui, uri, profile);
            ui.add_space(14.0);
        }
    } else {
        collection_header(app, ui, uri, &content);
    }
    body(
        app,
        ui,
        &page,
        uri,
        &content,
        loading,
        error.as_deref(),
        revision,
    );
}

/// A profile another tab of the same page already loaded, so the header holds
/// still while a new tab loads.
fn sibling_profile(app: &App, uri: &str) -> Option<SourceProfile> {
    let siblings: Vec<String> = if let Some((id, _)) = sc::user_tab(uri) {
        let base = sc::user_uri(id);
        std::iter::once(base.clone())
            .chain(sc::USER_TABS.iter().map(|tab| format!("{base}:{tab}")))
            .collect()
    } else if let Some(id) = yt::channel_of(uri) {
        let base = yt::channel_uri(id);
        vec![base.clone(), format!("{base}{}", yt::PLAYLISTS_TAB)]
    } else {
        Vec::new()
    };
    siblings.iter().find_map(|sibling| {
        app.soundcloud_pages
            .get(sibling)?
            .content
            .get()?
            .profile
            .clone()
    })
}

#[allow(clippy::too_many_arguments)]
fn body(
    app: &mut App,
    ui: &mut Ui,
    page: &Page,
    uri: &str,
    content: &SoundCloudCollection,
    loading: bool,
    error: Option<&str>,
    revision: u64,
) {
    let youtube = uri.starts_with("youtube:");
    if !content.cards.is_empty() {
        cards_grid(app, ui, &content.cards);
        widgets::load_more_when_near_end(ui, app, page.clone(), content.next.is_some() && !loading);
    } else if youtube && !content.tracks.is_empty() {
        video_grid(
            app,
            ui,
            &content.tracks,
            page,
            content.next.is_some() && !loading,
        );
    } else if !content.tracks.is_empty() {
        tracks_section(app, ui, page, content, loading, error, revision);
        return;
    } else if loading {
        if youtube {
            video_skeleton(app, ui);
        } else {
            widgets::skeleton_rows(ui, &app.palette, app.locale, 8);
        }
    } else if error.is_none() {
        widgets::empty_state(
            ui,
            &app.palette,
            if youtube {
                Icon::ListVideo
            } else {
                Icon::Music
            },
            &gettext(app.locale, "Nothing here yet"),
            &gettext(app.locale, "This page has nothing to show."),
        );
    }
    if let Some(error) = error {
        ui.add_space(8.0);
        widgets::error_row(ui, app, error, Some(page.clone()));
    }
}

/// SoundCloud's list: the same rows, selection and queue as any other list.
fn tracks_section(
    app: &mut App,
    ui: &mut Ui,
    page: &Page,
    content: &SoundCloudCollection,
    loading: bool,
    error: Option<&str>,
    revision: u64,
) {
    let items: Vec<TableItem> = content
        .tracks
        .iter()
        .cloned()
        .map(|track| (PlayableItem::Track(track), None, None))
        .collect();
    if items.is_empty() && loading {
        widgets::skeleton_rows(ui, &app.palette, app.locale, 8);
        return;
    }
    collection::table(
        app,
        ui,
        Table {
            items: &items,
            row_offset: 0,
            pagination: None,
            context: RowContext::Uris(items.iter().map(|row| row.0.uri().to_string()).collect()),
            show_album: false,
            show_cover: true,
            show_added: false,
            show_added_by: false,
            show_plays: true,
            page: page.clone(),
            loading,
            error,
            can_load_more: content.next.is_some() && error.is_none(),
            filter: "",
            items_revision: revision,
            plays_shown_rows: true,
        },
    );
}

fn tracks_uris(tracks: &[Track]) -> Vec<String> {
    tracks.iter().map(|track| track.uri.clone()).collect()
}

/// A set, playlist or mix: the same header as Spotify's, and something to play.
fn collection_header(app: &mut App, ui: &mut Ui, uri: &str, content: &SoundCloudCollection) {
    let locale = app.locale;
    let palette = app.palette;
    let fallback = gettext(locale, "Loading…");
    let title = if content.title.is_empty() {
        fallback.as_ref()
    } else {
        content.title.as_str()
    };
    let (kind, first_track_art) = if uri.starts_with("youtube:") {
        (
            gettext(locale, "YouTube"),
            content.tracks.first().and_then(|track| track.image(500)),
        )
    } else {
        (gettext(locale, "SoundCloud"), None)
    };
    let image = content.image.as_deref().or(first_track_art);
    let count = content.tracks.len();
    collection::hero(
        app,
        ui,
        Hero {
            images: HeroImages {
                image,
                ..Default::default()
            },
            liked: false,
            kind,
            title,
            description: None,
            byline: if count == 0 {
                Vec::new()
            } else {
                vec![(format!("{count} songs"), None)]
            },
            avatar: None,
            round: false,
        },
    );
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        play_all_button(app, ui, &content.tracks);
        if sc::playlist_id(uri).is_some() {
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
                .show(|ui| {
                    widgets::context_menu_items(ui, app, uri, &content.title, None);
                });
        }
    });
    ui.add_space(12.0);
}

fn play_all_button(app: &mut App, ui: &mut Ui, tracks: &[Track]) {
    let palette = app.palette;
    let clicked = ui
        .add_enabled_ui(!tracks.is_empty(), |ui| {
            theme::circle_button(
                ui,
                Icon::PlayFilled,
                44.0,
                palette.accent,
                palette.accent_hover,
                palette.on_accent,
                &gettext(app.locale, "Play"),
            )
            .clicked()
        })
        .inner;
    if clicked {
        app.actions.push(Action::PlayUris {
            uris: tracks_uris(tracks),
            index: 0,
        });
    }
}

/// Who the page is about: their banner, picture, name and what they do.
fn profile_header(
    app: &mut App,
    ui: &mut Ui,
    uri: &str,
    profile: &SourceProfile,
    content: &SoundCloudCollection,
) {
    let palette = app.palette;
    let locale = app.locale;
    let ctx = ui.ctx().clone();
    super::remember_hero(&ctx, &profile.name, profile.image.as_deref());
    let width = ui.available_width();
    ui.add_space(8.0);
    const AVATAR: f32 = 104.0;
    let banner_height = if profile.banner.is_some() {
        (width * 0.17).clamp(96.0, 210.0)
    } else {
        0.0
    };
    if banner_height > 0.0 {
        let (banner, _) = ui.allocate_exact_size(vec2(width, banner_height), Sense::hover());
        // It drifts slower than the page as the page scrolls.
        let scrolled = super::scrolled_page(&ctx).min(banner_height);
        let drift = scrolled * 0.3;
        let mut clipped = ui.new_child(UiBuilder::new().max_rect(banner));
        clipped.set_clip_rect(banner.intersect(ui.clip_rect()));
        let shown = Rect::from_min_size(
            banner.min + vec2(0.0, drift),
            vec2(banner.width(), banner.height() + 24.0),
        );
        widgets::paint_cover(
            &clipped,
            &palette,
            profile.banner.as_deref(),
            shown,
            14.0,
            Icon::Music,
            Some(app.backend.art()),
        );
        widgets::paint_vertical_gradient(
            ui,
            banner,
            Color32::TRANSPARENT,
            palette.window.gamma_multiply(0.75),
        );
    }
    ui.add_space(if banner_height > 0.0 { -36.0 } else { 8.0 });
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.spacing_mut().item_spacing.x = 18.0;
        let (avatar, _) = ui.allocate_exact_size(vec2(AVATAR, AVATAR), Sense::hover());
        // The picture comes up out of where it was pressed, when it was.
        let lift = motion::spring(
            &ctx,
            ui.id().with("profile-avatar-lift"),
            if super::pointer_over(ui, avatar) {
                3.0
            } else {
                0.0
            },
            motion::LIVELY,
        );
        let shown = avatar.expand(lift);
        let handed_over = super::kit::shared::land(ui, &palette, shown, AVATAR / 2.0);
        if !handed_over {
            ui.painter().circle_stroke(
                shown.center(),
                shown.width() / 2.0 + 2.0,
                egui::Stroke::new(3.0, palette.window),
            );
            widgets::paint_cover(
                ui,
                &palette,
                profile.image.as_deref(),
                shown,
                AVATAR / 2.0 + lift,
                Icon::User,
                Some(app.backend.art()),
            );
        }
        ui.vertical(|ui| {
            ui.add_space(if banner_height > 0.0 { 34.0 } else { 6.0 });
            ui.spacing_mut().item_spacing.y = 4.0;
            widgets::title_in_words(ui, &profile.name, theme::semibold(30.0), palette.text);
            if !profile.details.is_empty() {
                theme::text(
                    ui,
                    motion::counting(&profile.details, super::entering(&ctx, 0.08)),
                    theme::regular(13.0),
                    palette.secondary,
                );
            }
        });
    });
    ui.add_space(10.0);
    if !profile.about.trim().is_empty() {
        let line = profile
            .about
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let galley = widgets::ellipsized(
            ui,
            &line,
            theme::regular(13.0),
            palette.secondary,
            (width - 24.0).min(760.0),
            2,
        );
        let (rect, response) = ui.allocate_exact_size(vec2(width, galley.size().y), Sense::hover());
        ui.painter()
            .galley(rect.min + vec2(12.0, 0.0), galley, palette.secondary);
        response.on_hover_text(profile.about.clone());
        ui.add_space(10.0);
    }
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.spacing_mut().item_spacing.x = 10.0;
        play_all_button(app, ui, &content.tracks);
        follow_button(app, ui, uri, profile);
        if let Some(link) = &profile.link {
            let open = if uri.starts_with("youtube:") {
                gettext(locale, "Open on YouTube")
            } else {
                gettext(locale, "Open on SoundCloud")
            };
            if theme::icon_button(
                ui,
                Icon::ExternalLink,
                18.0,
                palette.secondary,
                palette.text,
                &open,
            )
            .clicked()
            {
                app.actions.push(Action::OpenUrl(link.clone()));
            }
        }
    });
}

fn follow_button(app: &mut App, ui: &mut Ui, uri: &str, profile: &SourceProfile) {
    let palette = app.palette;
    let locale = app.locale;
    let youtube = uri.starts_with("youtube:");
    let id = profile.id.parse::<u64>().ok();
    let following = if youtube {
        app.follows_youtube_channel(&profile.id)
    } else {
        id.is_some_and(|id| app.follows_soundcloud_user(id))
    };
    let label = if following {
        gettext(locale, "Following")
    } else {
        gettext(locale, "Follow")
    };
    let icon = if following { Icon::Check } else { Icon::Plus };
    if !theme::soft_button(ui, &palette, Some(icon), &label, following).clicked() {
        return;
    }
    if youtube {
        app.actions.push(Action::ToggleYouTubeFollow(
            crate::youtube::likes::Channel {
                id: profile.id.clone(),
                name: profile.name.clone(),
                image: profile.image.clone(),
            },
        ));
    } else if let Some(id) = id {
        app.actions.push(Action::ToggleSoundCloudFollow {
            id,
            name: profile.name.clone(),
            image: profile.image.clone(),
        });
    }
}

/// A page's tabs: quiet words with a line sliding under the one that's open.
fn tabs(app: &mut App, ui: &mut Ui, all: &[(String, String)], current: &str) {
    let palette = app.palette;
    let origin = ui.max_rect().min;
    let id = ui.id().with("source-tabs");
    let mut line = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 20.0;
        ui.add_space(12.0);
        for (label, uri) in all {
            let galley = ui.painter().layout_no_wrap(
                label.clone(),
                theme::medium(14.0),
                Color32::PLACEHOLDER,
            );
            let (rect, response) =
                ui.allocate_exact_size(vec2(galley.size().x, 30.0), Sense::click());
            let on = uri == current;
            let lit = motion::toward(
                ui.ctx(),
                response.id.with("lit"),
                on || response.hovered(),
                motion::FEEDBACK,
            );
            ui.painter().galley(
                pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
                galley,
                super::blend(palette.secondary, palette.text, lit),
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, label)
            });
            theme::focus_ring(ui, &response);
            if on {
                line = Some(rect);
            }
            if response.clicked() && !on {
                app.actions
                    .push(Action::Open(Page::SoundCloud(uri.clone())));
            }
        }
    });
    if let Some(rect) = line {
        // Measured from the row, so scrolling never leaves it behind.
        let ctx = ui.ctx();
        let left = motion::spring(ctx, id.with("left"), rect.left() - origin.x, motion::LIVELY);
        let right = motion::spring(
            ctx,
            id.with("right"),
            rect.right() - origin.x,
            motion::LIVELY,
        );
        let y = rect.bottom();
        ui.painter().hline(
            origin.x + left..=origin.x + right,
            y,
            egui::Stroke::new(2.0, palette.text),
        );
    }
    ui.painter().hline(
        ui.max_rect().x_range(),
        ui.cursor().top(),
        egui::Stroke::new(1.0, palette.outline),
    );
}

fn profile_tabs(app: &mut App, ui: &mut Ui, uri: &str, profile: &SourceProfile) {
    let locale = app.locale;
    let label = |text: &'static str| gettext(locale, text).into_owned();
    let (all, current) = if uri.starts_with("youtube:") {
        let base = yt::channel_uri(&profile.id);
        let playlists = format!("{base}{}", yt::PLAYLISTS_TAB);
        let current = if uri.ends_with(yt::PLAYLISTS_TAB) {
            playlists.clone()
        } else {
            base.clone()
        };
        (
            vec![(label("Videos"), base), (label("Playlists"), playlists)],
            current,
        )
    } else {
        let Some((id, _)) = sc::user_tab(uri) else {
            return;
        };
        let base = sc::user_uri(id);
        let tab = |name: &str| format!("{base}:{name}");
        (
            vec![
                (label("Tracks"), base.clone()),
                (label("Popular"), tab("popular")),
                (label("Albums"), tab("albums")),
                (label("Playlists"), tab("playlists")),
                (label("Reposts"), tab("reposts")),
                (label("Likes"), tab("likes")),
                (label("Following"), tab("following")),
            ],
            uri.to_owned(),
        )
    };
    tabs(app, ui, &all, &current);
}

fn cards_grid(app: &mut App, ui: &mut Ui, cards: &[SourceCard]) {
    let height = widgets::card_row_height(ui);
    widgets::virtual_wrapped_cards(ui, cards.len(), height, |ui, index| {
        source_card(app, ui, &cards[index]);
    });
}

fn source_card(app: &mut App, ui: &mut Ui, card: &SourceCard) {
    let cover = widgets::CardCover {
        circular: card.round,
        ..Default::default()
    };
    let shown = widgets::card(
        ui,
        app,
        card.image.as_deref(),
        &card.title,
        &card.subtitle,
        cover,
    );
    if shown.clicked {
        app.actions
            .push(Action::Open(Page::SoundCloud(card.uri.clone())));
    }
}

fn shelf(app: &mut App, ui: &mut Ui, shelf: &SourceShelf) {
    let palette = app.palette;
    widgets::shelf(
        ui,
        &palette,
        &format!("source-shelf-{}", shelf.title),
        &shelf.title,
        |ui| {
            for card in &shelf.cards {
                source_card(app, ui, card);
            }
        },
    );
}

fn video_skeleton(app: &App, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        widgets::skeleton_cards(ui, &app.palette, app.locale, 8, false);
    });
}

/// How many videos fit across, and how wide each is.
fn video_columns(width: f32) -> (usize, f32) {
    let columns =
        (((width + VIDEO_GAP) / (VIDEO_MIN_WIDTH + VIDEO_GAP)).floor() as usize).clamp(1, 8);
    let each = (width - VIDEO_GAP * (columns as f32 - 1.0)) / columns as f32;
    (columns, each)
}

/// YouTube's way of showing a list: thumbnails, not rows.
fn video_grid(app: &mut App, ui: &mut Ui, tracks: &[Track], page: &Page, can_load: bool) {
    let (columns, each) = video_columns(ui.available_width());
    let height = each * 9.0 / 16.0 + VIDEO_TEXT + VIDEO_GAP;
    let uris = tracks_uris(tracks);
    let rows = tracks.len().div_ceil(columns);
    widgets::virtual_rows(ui, rows, height, |ui, row| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = VIDEO_GAP;
            let first = row * columns;
            for (offset, track) in tracks.iter().skip(first).take(columns).enumerate() {
                video_card(app, ui, track, &uris, first + offset, each);
            }
        });
    });
    widgets::load_more_when_near_end(ui, app, page.clone(), can_load);
}

/// A compact count, as YouTube shows views.
fn views(count: u64) -> String {
    format!("{} views", crate::util::format_compact(count))
}

fn video_card(
    app: &mut App,
    ui: &mut Ui,
    track: &Track,
    uris: &[String],
    index: usize,
    width: f32,
) {
    let palette = app.palette;
    let locale = app.locale;
    let thumb_height = width * 9.0 / 16.0;
    let (rect, response) =
        ui.allocate_exact_size(vec2(width, thumb_height + VIDEO_TEXT), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &track.name)
    });
    let context = RowContext::Uris(uris.to_vec().into());
    let current = app.current_track_uri().is_some_and(|uri| uri == track.uri);
    let playing = current && app.believed_playing();
    if response.clicked() {
        if current {
            app.actions.push(Action::TogglePlay);
        } else {
            app.actions.push(Action::PlayFromRow {
                context: context.clone(),
                uri: track.uri.clone(),
                index: index as u32,
            });
        }
    }
    let item = PlayableItem::Track(track.clone());
    response.context_menu(|ui| {
        widgets::item_menu(ui, app, &item, Some(&context), Some(index));
    });
    if !ui.is_rect_visible(rect) {
        return;
    }
    let ctx = ui.ctx().clone();
    let hovered = super::pointer_over(ui, rect);
    let lit = motion::toward(&ctx, response.id.with("lit"), hovered, motion::FEEDBACK);
    if let Some((pointer, strength)) = motion::pointer_light(ui, response.id.with("light"), rect) {
        motion::spotlight(
            ui.painter(),
            rect.expand(6.0),
            pointer,
            170.0,
            palette.text.gamma_multiply(0.05 * strength),
        );
    }
    // The thumbnail steps forward under the pointer, and sinks when pressed.
    let lift = motion::spring(
        &ctx,
        response.id.with("lift"),
        if response.is_pointer_button_down_on() {
            -2.0
        } else if hovered {
            3.0
        } else {
            0.0
        },
        motion::LIVELY,
    );
    let thumb = Rect::from_min_size(rect.min, vec2(width, thumb_height)).expand(lift);
    widgets::paint_shadow(ui, &palette, thumb, 10.0);
    widgets::paint_cover(
        ui,
        &palette,
        track.image(640),
        thumb,
        10.0,
        Icon::Music,
        Some(app.backend.art()),
    );
    motion::ripple(ui, &response, thumb, Color32::WHITE);
    // What it would do, over a dimming of the picture.
    let dim = lit.max(if current { 0.7 } else { 0.0 });
    if dim > 0.0 {
        ui.painter()
            .rect_filled(thumb, 10.0, Color32::from_black_alpha((110.0 * dim) as u8));
    }
    if playing && !hovered {
        motion::equalizer(
            ui,
            Rect::from_center_size(thumb.center(), egui::Vec2::splat(30.0)),
            Color32::WHITE,
        );
    } else if lit > 0.01 {
        let disc = Rect::from_center_size(
            thumb.center() + vec2(0.0, (1.0 - lit) * 6.0),
            egui::Vec2::splat(46.0),
        );
        ui.painter()
            .circle_filled(disc.center(), 23.0, Color32::WHITE.gamma_multiply(lit));
        theme::paint_icon(
            ui,
            if playing {
                Icon::PauseFilled
            } else {
                Icon::PlayFilled
            },
            disc,
            22.0,
            Color32::BLACK.gamma_multiply(lit),
        );
    }
    // How long it is, in the corner, as a video's thumbnail says it.
    if track.duration_ms > 0 {
        let text = crate::util::format_duration_ms(track.duration_ms);
        let galley = ui
            .painter()
            .layout_no_wrap(text, theme::medium(11.5), Color32::WHITE);
        let badge = Rect::from_min_size(
            thumb.right_bottom() - galley.size() - vec2(14.0, 12.0),
            galley.size() + vec2(8.0, 4.0),
        );
        ui.painter()
            .rect_filled(badge, 4.0, Color32::from_black_alpha(190));
        ui.painter()
            .galley(badge.min + vec2(4.0, 2.0), galley, Color32::WHITE);
    }
    // The heart comes in on the thumbnail's corner.
    if lit > 0.01 {
        let saved = app.is_saved(&track.uri).unwrap_or(false);
        let corner = Rect::from_center_size(
            thumb.right_top() + vec2(-22.0, 22.0),
            egui::Vec2::splat(34.0),
        );
        let mut child = ui.new_child(UiBuilder::new().max_rect(corner).layout(
            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
        ));
        child.multiply_opacity(lit);
        if theme::icon_button(
            &mut child,
            if saved {
                Icon::HeartFilled
            } else {
                Icon::Heart
            },
            18.0,
            Color32::WHITE,
            Color32::WHITE,
            &if saved {
                gettext(locale, "Remove from YouTube Likes")
            } else {
                gettext(locale, "Add to YouTube Likes")
            },
        )
        .clicked()
        {
            app.actions.push(Action::ToggleSaved(track.uri.clone()));
        }
    }
    // Words: the title in its source's colour, then the channel and its views.
    let words = Rect::from_min_size(
        pos2(rect.left(), rect.top() + thumb_height + 10.0),
        vec2(width, VIDEO_TEXT - 10.0),
    );
    let title_color = crate::ui::kit::Source::of(&track.uri).color(&palette);
    let title = widgets::ellipsized(ui, &track.name, theme::medium(13.5), title_color, width, 2);
    let title_height = title.size().y;
    ui.painter().galley(words.min, title, title_color);
    let below = pos2(words.left(), words.top() + title_height + 3.0);
    let mut line = ui.new_child(
        UiBuilder::new()
            .max_rect(Rect::from_min_max(
                below,
                pos2(words.right(), below.y + 20.0),
            ))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    line.set_clip_rect(line.max_rect().intersect(ui.clip_rect()));
    line.spacing_mut().item_spacing.x = 4.0;
    widgets::artist_links(
        &mut line,
        app,
        &track.artists,
        theme::regular(12.0),
        palette.secondary,
    );
    if let Some(count) = track.play_count {
        theme::text(
            &mut line,
            format!("• {}", views(count)),
            theme::regular(12.0),
            palette.dim,
        );
    }
}

fn genre_chips(app: &mut App, ui: &mut Ui, current: Option<&str>) {
    let palette = app.palette;
    ui.horizontal_wrapped(|ui| {
        theme::pill_group(ui, &palette, |ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for (key, label) in sc::GENRES {
                if theme::soft_button(ui, &palette, None, label, current == Some(key)).clicked()
                    && current != Some(key)
                {
                    app.actions
                        .push(Action::Open(Page::SoundCloud(sc::charts_uri(key))));
                }
            }
        });
    });
}

/// SoundCloud's front door: genres, what's trending, and its own rows.
fn soundcloud_home(
    app: &mut App,
    ui: &mut Ui,
    content: &SoundCloudCollection,
    loading: bool,
    error: Option<&str>,
    revision: u64,
) {
    let palette = app.palette;
    let locale = app.locale;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let title = gettext(locale, "Discover on SoundCloud");
        widgets::title_in_words(ui, &title, theme::semibold(30.0), palette.text);
        if app.can_like_soundcloud() {
            ui.add_space(12.0);
            if theme::soft_button(
                ui,
                &palette,
                Some(Icon::Users),
                &gettext(locale, "Your feed"),
                false,
            )
            .clicked()
            {
                app.actions
                    .push(Action::Open(Page::SoundCloud(sc::FEED.into())));
            }
        }
    });
    ui.add_space(12.0);
    genre_chips(app, ui, None);
    ui.add_space(14.0);
    if let Some(error) = error {
        widgets::error_row(ui, app, error, Some(Page::SoundCloud(sc::HOME.into())));
    }
    if loading && content.tracks.is_empty() && content.shelves.is_empty() {
        ui.horizontal_wrapped(|ui| {
            widgets::skeleton_cards(ui, &palette, locale, 7, false);
        });
        widgets::skeleton_rows(ui, &palette, locale, 6);
        return;
    }
    for each in content.shelves.iter().take(1) {
        shelf(app, ui, each);
    }
    if !content.tracks.is_empty() {
        ui.add_space(8.0);
        super::rise_here(ui, |ui| {
            theme::section_title(ui, &palette, &gettext(locale, "Trending now"))
        });
        ui.add_space(4.0);
        let top = SoundCloudCollection {
            tracks: content.tracks.clone(),
            ..Default::default()
        };
        tracks_section(
            app,
            ui,
            &Page::SoundCloud(sc::HOME.into()),
            &top,
            false,
            None,
            revision,
        );
    }
    for each in content.shelves.iter().skip(1) {
        shelf(app, ui, each);
    }
}

/// What the people you follow posted and reposted.
fn soundcloud_feed(app: &mut App, ui: &mut Ui) {
    use crate::soundcloud::following::{Content, Status};
    let palette = app.palette;
    let locale = app.locale;
    ui.add_space(8.0);
    let title = gettext(locale, "Your feed");
    widgets::title_in_words(ui, &title, theme::semibold(30.0), palette.text);
    ui.add_space(12.0);
    if !app.can_like_soundcloud() {
        widgets::empty_state(
            ui,
            &palette,
            Icon::Users,
            &gettext(locale, "Connect SoundCloud to see your feed"),
            &gettext(locale, "New posts from the people you follow show up here."),
        );
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            if theme::pill_button(ui, &palette, &gettext(locale, "Connect SoundCloud"), true)
                .clicked()
            {
                app.actions
                    .push(Action::ShowDialog(crate::model::Dialog::SoundCloudConnect(
                        Default::default(),
                    )));
            }
        });
        return;
    }
    let following = &app.soundcloud_library.snapshot.following;
    let status = &app.soundcloud_library.following.status;
    if !following.loaded && *status == Status::Idle {
        app.actions
            .push(Action::LoadSoundCloudFollowing { more: false });
    }
    let more = following.next.is_some();
    let tracks: Vec<Track> = following
        .items
        .iter()
        .filter_map(|item| match &item.content {
            Content::Track(track) => Some((**track).clone()),
            Content::Playlist(_) => None,
        })
        .collect();
    let content = SoundCloudCollection {
        tracks,
        ..Default::default()
    };
    let loading = !following.loaded;
    let revision = app.soundcloud_library.revision;
    let page = Page::SoundCloud(sc::FEED.into());
    tracks_section(app, ui, &page, &content, loading, None, revision);
    if more
        && theme::soft_button(ui, &palette, None, &gettext(locale, "Show more"), false).clicked()
    {
        app.actions
            .push(Action::LoadSoundCloudFollowing { more: true });
    }
}

/// YouTube is for finding beats, so there is no page of recommendations here:
/// it's search, the channels followed, and what they uploaded.
fn youtube_home(
    app: &mut App,
    ui: &mut Ui,
    content: &SoundCloudCollection,
    loading: bool,
    error: Option<&str>,
) {
    let palette = app.palette;
    let locale = app.locale;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let title = gettext(locale, "YouTube");
        widgets::title_in_words(ui, &title, theme::semibold(30.0), palette.text);
        ui.add_space(12.0);
        if theme::soft_button(
            ui,
            &palette,
            Some(Icon::Search),
            &gettext(locale, "Search YouTube"),
            false,
        )
        .clicked()
        {
            app.actions
                .push(Action::SetSearchSource(SearchSource::YouTube));
            app.actions.push(Action::FocusSearch);
        }
    });
    ui.add_space(12.0);
    let channels: Vec<SourceCard> = app
        .youtube_channels()
        .iter()
        .map(|channel| SourceCard {
            uri: yt::channel_uri(&channel.id),
            title: channel.name.clone(),
            subtitle: String::new(),
            image: channel.image.clone(),
            round: true,
        })
        .collect();
    if channels.is_empty() {
        widgets::empty_state(
            ui,
            &palette,
            Icon::ListVideo,
            &gettext(locale, "Follow the channels you like"),
            &gettext(
                locale,
                "Search for a beat, open its channel and press Follow. New uploads show up here.",
            ),
        );
        return;
    }
    shelf(
        app,
        ui,
        &SourceShelf {
            title: gettext(locale, "Channels you follow").into_owned(),
            cards: channels,
        },
    );
    ui.add_space(8.0);
    super::rise_here(ui, |ui| {
        theme::section_title(ui, &palette, &gettext(locale, "New uploads"))
    });
    ui.add_space(6.0);
    let page = Page::SoundCloud(yt::HOME.into());
    if let Some(error) = error {
        widgets::error_row(ui, app, error, Some(page.clone()));
    }
    if content.tracks.is_empty() && loading {
        video_skeleton(app, ui);
        return;
    }
    video_grid(app, ui, &content.tracks, &page, false);
}

/// YouTube's part of a search. Beside the other sources (`brief`) it holds a
/// row's worth of videos and channels, and a way to see them all.
pub fn youtube_results(app: &mut App, ui: &mut Ui, brief: bool) {
    let palette = app.palette;
    let locale = app.locale;
    let results = match &app.search.youtube {
        Loadable::Loaded(results) => results.clone(),
        Loadable::Loading | Loadable::NotLoaded => {
            video_skeleton(app, ui);
            return;
        }
        Loadable::Failed(error) => {
            let error = error.clone();
            widgets::error_row(ui, app, &error, None);
            return;
        }
    };
    if let Some(error) = &results.error {
        widgets::error_row(ui, app, error, None);
    }
    if results.is_empty() {
        if results.error.is_none() {
            theme::subtle(ui, &palette, &gettext(locale, "Nothing found on YouTube."));
        }
        return;
    }
    let filter = app.search.filter;
    let videos = matches!(filter, SearchFilter::All | SearchFilter::Songs);
    let channels = matches!(filter, SearchFilter::All | SearchFilter::Artists);
    let playlists = matches!(filter, SearchFilter::All | SearchFilter::Playlists);
    if channels && !results.channels.is_empty() {
        shelf(
            app,
            ui,
            &SourceShelf {
                title: gettext(locale, "Channels").into_owned(),
                cards: results.channels.clone(),
            },
        );
        ui.add_space(8.0);
    }
    if videos && !results.videos.is_empty() {
        theme::section_title(ui, &palette, &gettext(locale, "Videos"));
        ui.add_space(6.0);
        let (columns, _) = video_columns(ui.available_width());
        let shown = if brief {
            &results.videos[..results.videos.len().min(columns * 2)]
        } else {
            &results.videos[..]
        };
        video_grid(app, ui, shown, &Page::Search, false);
        if !brief && results.next.is_some() {
            ui.add_space(8.0);
            ui.add_enabled_ui(!app.search.youtube_pending, |ui| {
                if theme::soft_button(ui, &palette, None, &gettext(locale, "Show more"), false)
                    .clicked()
                {
                    app.actions.push(Action::SearchYouTube { more: true });
                }
            });
        }
    }
    if playlists && !results.playlists.is_empty() {
        ui.add_space(8.0);
        shelf(
            app,
            ui,
            &SourceShelf {
                title: gettext(locale, "Playlists").into_owned(),
                cards: results.playlists.clone(),
            },
        );
    }
    if brief {
        ui.add_space(8.0);
        if theme::soft_button(
            ui,
            &palette,
            None,
            &gettext(locale, "See everything on YouTube"),
            false,
        )
        .clicked()
        {
            app.actions
                .push(Action::SetSearchSource(SearchSource::YouTube));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wider_page_fits_more_videos_without_shrinking_them_below_their_least() {
        for width in [300.0, 800.0, 1200.0, 2400.0] {
            let (columns, each) = video_columns(width);
            let used = columns as f32 * each + (columns as f32 - 1.0) * VIDEO_GAP;
            assert!((used - width).abs() < 0.01, "{width}");
            assert!(each >= VIDEO_MIN_WIDTH, "{width}: {each}");
        }
        assert!(video_columns(1200.0).0 > video_columns(800.0).0);
        assert_eq!(video_columns(100.0).0, 1, "a tiny page still shows one");
    }
}
