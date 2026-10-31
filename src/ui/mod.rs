//! Window layout: panels, overlays, keyboard shortcuts.

use std::sync::Arc;

pub mod artist;
pub mod collection;
pub(crate) mod devices;
mod dialogs;
mod downloads;
pub(crate) mod friends;
pub mod group;
mod group_shelf;
pub mod home;
mod keys;
pub mod kit;
pub mod library;
pub mod login;
mod lyrics;
pub mod new_releases;
pub mod now_playing;
pub mod player_bar;
mod prerelease;
pub mod queue;
pub mod radio;
pub(crate) mod row_tools;
pub mod search;
pub mod settings;
pub mod show;
pub mod sidebar;
mod soloist;
pub(crate) mod soundcloud;
pub mod topbar;
mod update;
pub mod widgets;
pub mod winamp;

use egui::{Align2, Color32, Context, CornerRadius, Frame, Id, Margin, Rect, Stroke, vec2};

use crate::app::App;
use crate::backend::AuthStatus;
use crate::model::{Action, Loadable, Page, ToastKind};
use crate::theme::{self, Icon};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let ctx = &ctx;
    keys::handle(app, ctx);
    for path in winamp::dropped_skins(ctx) {
        app.actions.push(Action::InstallSkin(path));
    }
    let signed_in = app.is_connected() && app.user.is_some();
    let connecting = matches!(app.auth, AuthStatus::Connecting | AuthStatus::Starting)
        || (app.is_connected() && app.user.is_none());
    if !signed_in {
        player_bar::end_tint_session(ctx);
        // Nothing an account saw survives into the next one.
        player_bar::forget_island(ctx);
        row_tools::forget(ctx);
        now_playing::close(ctx);
        login::show(app, ui, connecting);
        update::show(app, ctx);
        toasts(app, ctx, 20.0);
        caption_buttons(ui, &app.palette, app.locale);
        fastframe_titlebar::resize(ui);
        return;
    }
    if titlebar_spans_window(cfg!(target_os = "macos"), fastframe_titlebar::enabled()) {
        fastframe_titlebar::drag(
            ui,
            Rect::from_min_size(
                ui.max_rect().min,
                vec2(
                    ui.max_rect().width(),
                    theme::top_bar_height() + theme::titlebar_inset(ui.ctx()),
                ),
            ),
        );
    }
    let window = ui.max_rect();
    backdrop(app, ctx);
    player_bar::show(app, ui);
    let content = ui.available_rect_before_wrap();
    if app.lyrics_fullscreen.is_some() {
        lyrics::fullscreen(app, ui);
    } else {
        if app.settings.sidebar_visible {
            sidebar::show(app, ui);
        }
        if app.show_queue_panel {
            queue::side_panel(app, ui);
        }
        if app.show_lyrics_panel {
            lyrics::side_panel(app, ui);
        }
        if app.show_friends_panel {
            friends::side_panel(app, ui);
        }
        let page = ui.available_rect_before_wrap();
        central(app, ui);
        let strip = Rect::from_min_max(
            egui::pos2(window.left(), window.bottom() - theme::PLAYER_BAR_HEIGHT),
            window.max,
        );
        now_playing::show(app, ui, page, player_bar::island_rect(strip));
        if !app.settings.sidebar_visible {
            sidebar::floating(app, ui, content);
        }
    }
    keep_room_for_panels(app, ctx);
    row_tools::remember_last_playlist(
        ctx,
        app.user.as_ref().map(|user| user.id.as_str()),
        &app.actions,
    );
    launch_flights(app, ctx);
    devices::popup(app, ctx);
    dialogs::show(app, ctx);
    update::show(app, ctx);
    widgets::drag_ghost(ctx, &app.palette, app.locale);
    kit::flight::paint(ctx, &app.palette);
    toasts(app, ctx, theme::PLAYER_BAR_HEIGHT + 16.0);
    caption_buttons(ui, &app.palette, app.locale);
    fastframe_titlebar::resize(ui);
}

/// Sends a cover flying from the pointer to where a song went this frame:
/// the playlist it was added to, or the Group row for a share.
fn launch_flights(app: &App, ctx: &Context) {
    let Some(from) = ctx.input(|input| input.pointer.interact_pos().or(input.pointer.hover_pos()))
    else {
        return;
    };
    for action in &app.actions {
        match action {
            Action::AddToPlaylist {
                playlist_id, items, ..
            } => {
                let art = items
                    .first()
                    .and_then(|item| item.image(64))
                    .map(str::to_owned);
                kit::flight::launch(ctx, art, from, playlist_id);
            }
            Action::Friends(crate::friends::Action::Share(post)) => {
                kit::flight::launch(ctx, post.artwork_url.clone(), from, kit::flight::GROUP);
            }
            _ => {}
        }
    }
}

/// The main window's narrowest width with these panels open: their least
/// widths and the page's.
fn main_min_width(page: f32, sidebar: bool, right_panel: bool) -> f32 {
    let sidebar = if sidebar { SIDEBAR_MIN_WIDTH } else { 0.0 };
    let right = if right_panel {
        theme::SIDE_PANEL_MIN_WIDTH
    } else {
        0.0
    };
    (sidebar + right + page).max(crate::window::MAIN_MIN_SIZE[0])
}

/// The sidebar's narrowest width.
pub(crate) const SIDEBAR_MIN_WIDTH: f32 = 210.0;

/// Raise the window's minimum width while the Queue or Lyrics panel is open,
/// so even at its narrowest the page beside the panels keeps the room its
/// top bar needs (#624), and lower it again once they close. Window managers
/// that tile ignore the minimum; the panels still give way there.
///
/// Full-screen lyrics draw no sidebar or side panel (an open queue becomes a
/// column inside them), so they ask for no room for them. A screen narrower
/// than the panels' minimum, such as a portrait monitor, then holds the
/// full-screen window without a minimum wider than itself (#783).
fn keep_room_for_panels(app: &App, ctx: &Context) {
    if crate::window::fixed_size() {
        return;
    }
    let panels = app.lyrics_fullscreen.is_none();
    let width = main_min_width(
        topbar::least_width(ctx),
        panels && app.settings.sidebar_visible,
        panels && (app.show_queue_panel || app.show_lyrics_panel || app.show_friends_panel),
    )
    .round();
    let id = Id::new("main-min-width");
    let sent = ctx.data(|data| data.get_temp::<f32>(id));
    // The window starts at MAIN_MIN_SIZE, so there is nothing to send until
    // the panels need more.
    if sent.unwrap_or(crate::window::MAIN_MIN_SIZE[0]) != width {
        ctx.data_mut(|data| data.insert_temp(id, width));
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(vec2(
            width,
            crate::window::MAIN_MIN_SIZE[1],
        )));
    }
}

/// How a resizable side panel fits beside the page.
pub(crate) struct PanelFit {
    /// The widths the panel may take this frame.
    pub range: std::ops::RangeInclusive<f32>,
    /// Whether the room the page needs holds the panel under the width the
    /// person chose for it.
    pub yielding: bool,
}

/// Fit a side panel between `range` and the page beside it. The panel gives
/// up width, down to its minimum, so the page keeps `room_for_page`, the
/// least its top bar needs (#624). The width chosen for the panel stays in
/// the settings and comes back once the window widens again.
///
/// `room` is what the panel and everything after it share.
pub(crate) fn yielding_panel(
    ctx: &Context,
    id: &str,
    range: std::ops::RangeInclusive<f32>,
    chosen: f32,
    room: f32,
) -> PanelFit {
    let (min, max) = range.into_inner();
    let max = max.min(room).max(min);
    let width = chosen.clamp(min, max);
    // egui remembers the width the panel last drew at. While nobody drags
    // its handle, hand it back the chosen width, either way: a resize made
    // elsewhere (the fullscreen queue column shares the queue width) shows
    // here too, and the panel never writes a stale width back over it.
    let id = Id::new(id);
    if !ctx.is_being_dragged(id.with("__resize"))
        && let Some(mut state) = egui::containers::panel::PanelState::load(ctx, id)
        && (state.outer_rect.width() - width).abs() > 0.5
    {
        state.outer_rect.set_width(width);
        ctx.data_mut(|data| data.insert_persisted(id, state));
    }
    PanelFit {
        range: min..=max,
        yielding: max < chosen - 0.5,
    }
}

/// Whether the person is dragging this panel's resize handle right now.
pub(crate) fn panel_dragged(ctx: &Context, id: &str) -> bool {
    ctx.is_being_dragged(Id::new(id).with("__resize"))
}

/// Whether a panel's new width is one the person chose: it moved while not
/// held back by the page, or by their own drag.
pub(crate) fn panel_width_chosen(ctx: &Context, id: &str, fit: &PanelFit) -> bool {
    !fit.yielding || panel_dragged(ctx, id)
}

/// Keeps the most recent loading preview of each metadata type available to
/// the loaded hero as an artwork fallback. The fixed typed slot bounds this to
/// one playlist, album, artist, and show instead of scanning known metadata on
/// every loaded frame.
fn loading_preview<T>(
    ctx: &Context,
    key: &str,
    details: &Loadable<T>,
    known: impl FnOnce() -> Option<T>,
) -> Option<Arc<T>>
where
    T: Send + Sync + 'static,
{
    let memory_id = Id::new("collection-loading-preview");
    let key = Id::new(key);
    if details.get().is_some() {
        ctx.data(|data| data.get_temp::<(Id, Arc<T>)>(memory_id))
            .filter(|(stored_key, _)| *stored_key == key)
            .map(|(_, preview)| preview)
    } else {
        let preview = known().map(Arc::new);
        ctx.data_mut(|data| match &preview {
            Some(preview) => {
                data.insert_temp(memory_id, (key, Arc::clone(preview)));
            }
            None => data.remove::<(Id, Arc<T>)>(memory_id),
        });
        preview
    }
}

/// The gap between the window's edge and its cards (the sidebar, the
/// page, the side panels), and between neighbouring cards.
pub(crate) const CARD_GAP: f32 = 8.0;
/// The corners of the page and side panel cards, the sidebar's too.
pub(crate) const CARD_RADIUS: f32 = 14.0;

/// A side panel's frame: the window's colour around a card inset by
/// [`CARD_GAP`], with 12 points between the card's edge and the contents.
/// Paint the card with [`side_card`] first thing inside the panel.
pub(crate) fn side_card_frame(_palette: &theme::Palette) -> Frame {
    let gap = CARD_GAP as i8;
    Frame::new().inner_margin(Margin {
        left: gap + 12,
        right: gap + 12,
        top: gap + 12,
        bottom: gap + 12,
    })
}

/// Paints a side panel's card behind its contents. `ui` is the panel's.
pub(crate) fn side_card(ui: &egui::Ui, palette: &theme::Palette) {
    let card = ui.max_rect().expand(12.0);
    ui.painter().rect(
        card,
        CARD_RADIUS,
        glass_fill(palette, palette.panel),
        Stroke::new(1.0, palette.outline),
        egui::StrokeKind::Inside,
    );
    glass_sheen(ui, card, CARD_RADIUS);
}

/// A card's fill as glass: its colour, partly see-through, so the
/// playing song's glow behind the window shows through it.
pub(crate) fn glass_fill(palette: &theme::Palette, fill: Color32) -> Color32 {
    let alpha = if palette.dark { 120 } else { 150 };
    Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), alpha)
}

/// The light catching a glass card's top edge: a bright hairline and a
/// faint sheen just under it.
pub(crate) fn glass_sheen(ui: &egui::Ui, rect: Rect, radius: f32) {
    let painter = ui.painter();
    let inset = radius * 0.6;
    let width = (rect.width() - inset * 2.0).max(0.0);
    painter.rect_filled(
        Rect::from_min_size(rect.min + vec2(inset, 0.0), vec2(width, 1.0)),
        0.0,
        Color32::from_white_alpha(28),
    );
    // A few faint bands under it, fading down: plain rects, so the sheen
    // costs no mesh.
    for band in 0..4u8 {
        painter.rect_filled(
            Rect::from_min_size(
                rect.min + vec2(inset, 1.0 + f32::from(band) * 6.0),
                vec2(width, 6.0),
            ),
            0.0,
            Color32::from_white_alpha(8 - band * 2),
        );
    }
}

/// The colour a cover lends the glow. Pale and greyish cool tones read as
/// steel rather than colour, so a tint with any real hue is lifted to at
/// least moderate saturation and brightness; true greys stay grey.
fn glow_color(tint: Color32) -> Color32 {
    let mut hsva = egui::ecolor::Hsva::from(tint);
    if hsva.s >= 0.12 && hsva.s < 0.5 {
        // A washed-out colour gets its hue back, and is kept deep enough
        // that the extra saturation reads as colour, not pastel.
        hsva.s = 0.5;
        hsva.v = hsva.v.clamp(0.45, 0.62);
    }
    hsva.a = 1.0;
    Color32::from(hsva)
}

/// How long the glow takes to become the next song's colour.
const GLOW_CHANGE: f32 = 0.9;

/// The window behind the cards: black (or the light theme's grey) with
/// the playing song's colour rising from the bottom edge, around the
/// player island, fading out toward the top. When the song changes, the
/// glow turns into its colour; with nothing playing it fades away.
fn backdrop(app: &mut App, ctx: &Context) {
    let palette = app.palette;
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::background());
    painter.rect_filled(screen, 0.0, palette.window);
    let art = if app.settings.accent_from_art {
        app.now_playing()
            .and_then(|now| now.art_small.or(now.art_url))
    } else {
        None
    };
    let target = app
        .tint_for(art.as_deref())
        .map(glow_color)
        .unwrap_or(Color32::TRANSPARENT);
    let id = Id::new("backdrop-glow");
    let (from, to) = ctx
        .data(|data| data.get_temp::<(Color32, Color32)>(id))
        .unwrap_or((target, target));
    let (from, to) = if to == target {
        (from, to)
    } else {
        // Turn from what is shown now toward the new song's colour.
        let shown = kit::motion::running(ctx, id.with("change")).map_or(to, |t| mix(from, to, t));
        kit::motion::begin(ctx, id.with("change"), GLOW_CHANGE);
        (shown, target)
    };
    ctx.data_mut(|data| data.insert_temp(id, (from, to)));
    let glow = kit::motion::running(ctx, id.with("change")).map_or(to, |t| mix(from, to, t));
    if glow.a() == 0 {
        return;
    }
    let [r, g, b, alpha] = glow.to_srgba_unmultiplied();
    let solid = Color32::from_rgb(r, g, b);
    let lift = |strength: f32| {
        let color = blend(palette.window, solid, strength);
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
    };
    let strong = lift(if palette.dark { 0.95 } else { 0.7 });
    let middle = lift(if palette.dark { 0.45 } else { 0.3 });
    let height = screen.height();
    let mut mesh = egui::Mesh::default();
    for (y, color) in [
        (screen.top() + height * 0.2, Color32::TRANSPARENT),
        (screen.top() + height * 0.65, middle),
        (screen.bottom(), strong),
    ] {
        mesh.colored_vertex(egui::pos2(screen.left(), y), color);
        mesh.colored_vertex(egui::pos2(screen.right(), y), color);
    }
    for band in 0..2u32 {
        let i = band * 2;
        mesh.add_triangle(i, i + 1, i + 3);
        mesh.add_triangle(i, i + 3, i + 2);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// `a` turned `t` of the way to `b`, alpha included, so a glow can fade
/// from nothing.
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let mut from = a.to_srgba_unmultiplied();
    let mut to = b.to_srgba_unmultiplied();
    // A transparent end takes the other end's colour, so it only fades.
    if from[3] == 0 {
        from = [to[0], to[1], to[2], 0];
    }
    if to[3] == 0 {
        to = [from[0], from[1], from[2], 0];
    }
    let channel =
        |i: usize| (f32::from(from[i]) + (f32::from(to[i]) - f32::from(from[i])) * t).round() as u8;
    Color32::from_rgba_unmultiplied(channel(0), channel(1), channel(2), channel(3))
}

/// The page is a card like the sidebar: every edge beside the window or
/// another card is rounded, and a cover's wash fills it to its corners
/// instead of ending in a straight cut beside the sidebar.
fn central(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let right_panel = app.show_queue_panel || app.show_lyrics_panel || app.show_friends_panel;
    let gap = CARD_GAP as i8;
    egui::CentralPanel::default()
        .frame(Frame::new().inner_margin(Margin {
            left: gap,
            right: if right_panel { 0 } else { gap },
            top: gap,
            bottom: gap,
        }))
        .show(ui, |ui| {
            let rect = ui.max_rect();
            ui.painter()
                .rect_filled(rect, CARD_RADIUS, glass_fill(&palette, palette.window));
            glass_sheen(ui, rect, CARD_RADIUS);
            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
            let page = app.page().clone();
            // The wash belongs to the page's hero, so it scrolls away with
            // it. The page's offset is known once it has been laid out, so
            // the previous frame's serves.
            let scrolled_id = Id::new(("page-scrolled", page.encode()));
            let scrolled = ui
                .ctx()
                .data(|data| data.get_temp::<f32>(scrolled_id))
                .unwrap_or(0.0);
            if let Some((top, bottom)) = header_wash(&palette) {
                widgets::paint_vertical_gradient(ui, header_rect(rect, scrolled), top, bottom);
            }
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
            // egui fades a scrolled page's edge into the panel's plain
            // colour, which shows as a pale band over a cover's tint; the
            // page casts a shadow under the header instead.
            ui.spacing_mut().scroll.fade.strength = 0.0;
            topbar::show(app, ui);
            // One banner at a time: a song that will not play matters more
            // than a slow app, which waits its turn.
            if app.dialog.is_none() && page != Page::Settings {
                if app.audio_key_banner {
                    audio_key_banner(app, ui);
                } else if app.slow_banner {
                    slow_banner(app, ui);
                }
            }
            // A new page rises into place; going back or forward slides it
            // in from the side travelled toward.
            let (enter, from_side) = page_entrance(ui.ctx(), &page, app.history_index);
            let offset = vec2((1.0 - enter) * 24.0 * from_side, (1.0 - enter) * 10.0);
            let scroll = crate::autoscroll::show(
                ui,
                egui::ScrollArea::vertical()
                    .id_salt(("page", page.encode()))
                    .auto_shrink([false, false]),
                egui::Vec2b::new(false, true),
                |ui| {
                    if enter < 1.0 {
                        ui.multiply_opacity(0.2 + 0.8 * enter);
                    }
                    ui.with_visual_transform(
                        egui::emath::TSTransform::from_translation(offset),
                        |ui| {
                            Frame::new()
                                .inner_margin(Margin {
                                    left: widgets::PAGE_PADDING as i8,
                                    right: widgets::PAGE_PADDING as i8,
                                    top: 4,
                                    bottom: 48,
                                })
                                .show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    match page {
                                        Page::Home => home::show(app, ui),
                                        Page::TopSongs => collection::top_songs(app, ui),
                                        Page::NewReleases => new_releases::page(app, ui),
                                        Page::Search => search::show(app, ui),
                                        Page::LikedSongs => collection::liked(app, ui),
                                        Page::SoundCloudLikes => soundcloud::show(app, ui, page),
                                        Page::Albums
                                        | Page::Artists
                                        | Page::Podcasts
                                        | Page::Episodes => library::show(app, ui, page),
                                        Page::Playlist(id) => collection::playlist(app, ui, &id),
                                        Page::MixedPlaylist(id) => {
                                            collection::mixed_playlist(app, ui, &id)
                                        }
                                        Page::SoundCloud(uri) => {
                                            collection::soundcloud(app, ui, &uri)
                                        }
                                        Page::Album(id) => collection::album(app, ui, &id),
                                        Page::Artist(id) => artist::show(app, ui, &id),
                                        Page::Show(id) => show::show(app, ui, &id),
                                        Page::Radio(seed) => radio::radio(app, ui, &seed),
                                        Page::Queue => queue::page(app, ui),
                                        Page::Settings => settings::show(app, ui),
                                        Page::Group => group::show(app, ui),
                                    }
                                });
                        },
                    );
                    if app.dialog.is_none() && !app.show_devices {
                        keys::scroll_page(ui);
                    }
                },
            );
            ui.ctx()
                .data_mut(|data| data.insert_temp(scrolled_id, scroll.state.offset.y));
            header_shadow(ui, scroll.inner_rect, scroll.state.offset.y, palette.dark);
            ui.painter().rect_stroke(
                rect,
                CARD_RADIUS,
                Stroke::new(1.0, palette.outline),
                egui::StrokeKind::Inside,
            );
        });
}

/// How long a page takes to come in.
const PAGE_ENTER: f32 = 0.32;

/// The progress of the open page's entrance, and the side it comes from:
/// 0 when it rises from below (a page opened anew), 1 from the right
/// (forward), -1 from the left (back).
fn page_entrance(ctx: &Context, page: &Page, history_index: usize) -> (f32, f32) {
    let id = Id::new("page-entrance");
    let key = page.encode();
    let last = ctx.data(|data| data.get_temp::<(String, usize, f32)>(id));
    let side = match &last {
        Some((last_key, _, side)) if *last_key == key => *side,
        Some((_, last_index, _)) => {
            let side = match history_index.cmp(last_index) {
                std::cmp::Ordering::Less => -1.0,
                std::cmp::Ordering::Greater => 0.0,
                std::cmp::Ordering::Equal => 0.0,
            };
            kit::motion::begin(ctx, id.with("transition"), PAGE_ENTER);
            side
        }
        None => 0.0,
    };
    ctx.data_mut(|data| data.insert_temp(id, (key, history_index, side)));
    let t = kit::motion::running(ctx, id.with("transition")).unwrap_or(1.0);
    (t, side)
}

/// Where the "Why?" link of the slow-app banner leads.
const SLOW_BANNER_GUIDE: &str = "https://spotifast.rocks/troubleshooting/#rate-limits";
/// Where the "Why?" link of the audio-key banner leads.
pub const AUDIO_KEY_GUIDE: &str = "https://spotifast.rocks/troubleshooting/#audio-key";

/// A strip under the top bar suggesting a personal app, after Spotify
/// told the shared app, which only listeners signed in before 0.14.2 still
/// use, that it sent too many requests. It stays put while the
/// page scrolls, and goes when it is closed.
fn slow_banner(app: &mut App, ui: &mut egui::Ui) {
    use crate::i18n::gettext;
    let locale = app.locale;
    banner(
        app,
        ui,
        &gettext(locale, "Spotify is being slow on the shared app."),
        &gettext(
            locale,
            "Siphon no longer offers the shared app, and Spotify can stop accepting it at any time. Set up your own free Spotify app and you won't have to wait.",
        ),
        [
            (
                gettext(locale, "Set up personal app"),
                Action::OpenPersonalAppSetup,
            ),
            (
                // Translators: a button that opens a page explaining why
                // the banner it sits in appeared.
                gettext(locale, "Why?"),
                Action::OpenUrl(SLOW_BANNER_GUIDE.into()),
            ),
        ],
        Action::CloseSlowBanner,
    );
}

/// A strip under the top bar after Spotify refused the key that decrypts a
/// song on this computer, offering another Spotify device, which fetches
/// its own audio, and the guide's explanation.
fn audio_key_banner(app: &mut App, ui: &mut egui::Ui) {
    use crate::i18n::gettext;
    let locale = app.locale;
    banner(
        app,
        ui,
        &gettext(
            locale,
            "Spotify won't let this computer play this song right now.",
        ),
        &gettext(
            locale,
            "Spotify refused the key that unlocks the audio. Siphon can't fix that, but it sometimes clears up on its own. You can still play on your phone or another Spotify device.",
        ),
        [
            (
                gettext(locale, "Play on another device"),
                Action::PlayOnAnotherDevice,
            ),
            (
                gettext(locale, "Why?"),
                Action::OpenUrl(AUDIO_KEY_GUIDE.into()),
            ),
        ],
        Action::CloseAudioKeyBanner,
    );
}

/// A notice under the top bar: a bold line, a sentence or two, a main and
/// a secondary button, and a close button.
fn banner(
    app: &mut App,
    ui: &mut egui::Ui,
    title: &str,
    text: &str,
    [(primary, primary_action), (secondary, secondary_action)]: [(std::borrow::Cow<'static, str>, Action);
        2],
    close: Action,
) {
    use crate::i18n::gettext;
    let palette = app.palette;
    let locale = app.locale;
    Frame::new()
        .outer_margin(Margin {
            left: widgets::PAGE_PADDING as i8,
            right: widgets::PAGE_PADDING as i8,
            top: 0,
            bottom: 8,
        })
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin {
            left: 16,
            right: 6,
            top: 12,
            bottom: 12,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if theme::icon_button(
                    ui,
                    Icon::X,
                    16.0,
                    palette.secondary,
                    palette.text,
                    &gettext(locale, "Close banner"),
                )
                .clicked()
                {
                    app.actions.push(close);
                }
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(title)
                                .font(theme::bold(14.0))
                                .color(palette.text),
                        )
                        .wrap(),
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(text)
                                .font(theme::regular(13.0))
                                .color(palette.secondary),
                        )
                        .wrap(),
                    );
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        if theme::pill_button(ui, &palette, &primary, true).clicked() {
                            app.actions.push(primary_action);
                        }
                        if theme::pill_button(ui, &palette, &secondary, false).clicked() {
                            app.actions.push(secondary_action);
                        }
                    });
                });
            });
        });
}

/// The gradient behind a page's header, top to bottom: a custom palette's
/// `tint` fading out, whose alpha is its strength. `None` leaves the plain
/// card. Cover colour no longer washes the header: the playing song's
/// colour rises from the bottom of the window instead ([`backdrop`]).
fn header_wash(palette: &theme::Palette) -> Option<(Color32, Color32)> {
    (palette.tint.a() > 0).then_some((palette.tint, Color32::TRANSPARENT))
}

/// Where the header's wash is painted: the top of the panel, moved up by
/// how far the page has scrolled, so it leaves with the hero.
fn header_rect(panel: Rect, scrolled: f32) -> Rect {
    Rect::from_min_size(
        panel.min - vec2(0.0, scrolled.max(0.0)),
        vec2(panel.width(), 340.0),
    )
}

/// The shadow the header casts on a page scrolled under it, deepening over
/// the first few points of scrolling. Dark in both themes, lighter over a
/// light page.
fn header_shadow(ui: &egui::Ui, page: Rect, scrolled: f32, dark: bool) {
    let depth = (scrolled / 24.0).clamp(0.0, 1.0);
    if depth <= 0.0 {
        return;
    }
    let strength = if dark { 110.0 } else { 36.0 };
    let rect = Rect::from_min_size(page.min, vec2(page.width(), 14.0));
    widgets::paint_vertical_gradient(
        ui,
        rect,
        egui::Color32::from_black_alpha((strength * depth) as u8),
        egui::Color32::TRANSPARENT,
    );
}

/// Without a native title bar, the top of every panel drags the window, not
/// only the page's top bar.
const fn titlebar_spans_window(on_macos: bool, custom_titlebar: bool) -> bool {
    on_macos || custom_titlebar
}

/// The room the top bar's own controls need beside the caption buttons: the
/// 760-point minimum with the default 250-point sidebar leaves 510 points.
const TOPBAR_MIN_CONTENT: f32 = 510.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct WindowControlsReservation {
    pub topbar_width: f32,
    pub topbar_top: f32,
    pub queue_top: f32,
    pub lyrics_top: f32,
}

/// The caption buttons on the right belong to the outermost header: the
/// queue's or the lyrics' when either panel is open, otherwise the top bar,
/// beside its controls or above them when the page is too narrow.
fn controls_reservation(
    footprint: fastframe_titlebar::Footprint,
    queue: bool,
    lyrics: bool,
    topbar_width: f32,
) -> WindowControlsReservation {
    let mut space = WindowControlsReservation {
        topbar_width: 0.0,
        topbar_top: 0.0,
        queue_top: 0.0,
        lyrics_top: 0.0,
    };
    if footprint.right <= 0.0 {
        return space;
    }
    if queue {
        space.queue_top = footprint.height;
    } else if lyrics {
        space.lyrics_top = footprint.height;
    } else {
        let reserve = footprint.reserve(topbar_width, TOPBAR_MIN_CONTENT);
        space.topbar_width = reserve.inline_width;
        space.topbar_top = reserve.push_down;
    }
    space
}

/// Left-hand Linux controls sit above the sidebar's navigation rows.
pub(super) fn linux_left_controls_inset(ctx: &egui::Context) -> f32 {
    if !cfg!(target_os = "linux") {
        return 0.0;
    }
    let footprint = fastframe_titlebar::footprint(ctx, theme::CUSTOM_TOP_BAR_HEIGHT);
    if footprint.left > 0.0 {
        footprint.height
    } else {
        0.0
    }
}

pub(super) fn window_controls_reservation(
    ctx: &egui::Context,
    queue: bool,
    lyrics: bool,
    topbar_width: f32,
) -> WindowControlsReservation {
    controls_reservation(
        fastframe_titlebar::footprint(ctx, theme::CUSTOM_TOP_BAR_HEIGHT),
        queue,
        lyrics,
        topbar_width,
    )
}

/// Draws the caption buttons over the window's top corners in the palette's
/// colours: Windows' glyphs in the secondary text colour, GNOME's round
/// buttons in the text colour on the hover surface.
fn caption_buttons(ui: &mut egui::Ui, palette: &theme::Palette, locale: crate::i18n::Locale) {
    use crate::i18n::gettext;
    let style = fastframe_titlebar::Style {
        icon: if cfg!(target_os = "linux") {
            palette.text
        } else {
            palette.secondary
        },
        button: palette.surface_hover,
        header_height: theme::CUSTOM_TOP_BAR_HEIGHT,
    };
    // Literal calls, so xgettext extracts each label.
    let minimize = gettext(locale, "Minimize");
    let maximize = gettext(locale, "Maximize");
    let restore = gettext(locale, "Restore");
    let close = gettext(locale, "Close");
    let labels = fastframe_titlebar::Labels {
        minimize: &minimize,
        maximize: &maximize,
        restore: &restore,
        close: &close,
    };
    fastframe_titlebar::controls(ui, &style, &labels);
}

pub fn blend(base: Color32, tint: Color32, amount: f32) -> Color32 {
    let a = egui::Rgba::from(base);
    let b = egui::Rgba::from(tint);
    let mixed = a * (1.0 - amount) + b * amount;
    let mut color = Color32::from(mixed);
    color[3] = 255;
    color
}

/// How long a new toast takes to rise into place.
const TOAST_RISE: f32 = 0.26;

/// How far below its place a toast `age` seconds old still sits, rising
/// 12 points over `duration` seconds.
fn toast_lift(age: f32, duration: f32) -> f32 {
    if duration <= 0.0 || age >= duration {
        0.0
    } else {
        (1.0 - kit::motion::ease(age / duration)) * 12.0
    }
}

fn toasts(app: &mut App, ctx: &egui::Context, bottom_offset: f32) {
    if app.toasts.is_empty() && app.downloads.active.is_empty() {
        return;
    }
    let palette = app.palette;
    // The newest toast rises out of the island's edge as it fades in.
    let newest = app
        .toasts
        .iter()
        .map(|toast| toast.created.elapsed().as_secs_f32())
        .fold(f32::INFINITY, f32::min);
    let lift = toast_lift(newest, crate::motion::seconds(ctx, TOAST_RISE));
    if lift > 0.0 {
        ctx.request_repaint();
    }
    egui::Area::new(egui::Id::new("toasts"))
        .anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -bottom_offset + lift))
        .order(egui::Order::Tooltip)
        .interactable(
            !app.downloads.active.is_empty() || app.toasts.iter().any(|t| t.action.is_some()),
        )
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            downloads::show(app, ui);
            let mut chosen = None;
            for toast in &app.toasts {
                let age = toast.created.elapsed().as_secs_f32();
                let life = toast.lifetime().as_secs_f32();
                let alpha = if age < 0.15 {
                    age / 0.15
                } else if age > life - 0.4 {
                    ((life - age) / 0.4).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                ui.set_opacity(alpha);
                // A toast leaving slides out to the right as it fades.
                let leaving = if age > life - 0.4 {
                    (age - (life - 0.4)) / 0.4
                } else {
                    0.0
                };
                ui.with_visual_transform(
                    egui::emath::TSTransform::from_translation(vec2(
                        24.0 * kit::motion::ease(leaving.clamp(0.0, 1.0)),
                        0.0,
                    )),
                    |ui| {
                        Frame::new()
                            .fill(palette.overlay)
                            .stroke(Stroke::new(1.0, palette.outline))
                            .corner_radius(CornerRadius::same(theme::RADIUS))
                            .inner_margin(Margin::symmetric(14, 10))
                            .shadow(egui::epaint::Shadow {
                                offset: [0, 4],
                                blur: 16,
                                spread: 0,
                                color: palette.shadow,
                            })
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let (icon, color) = match toast.kind {
                                        ToastKind::Info => (Icon::CircleCheck, palette.accent),
                                        ToastKind::Error => (Icon::CircleAlert, palette.danger),
                                    };
                                    theme::icon(ui, icon, 16.0, color);
                                    // Laid out at its own width. The area
                                    // remembers its size, so after a short toast a
                                    // label left to wrap at the area's width broke
                                    // long messages on every word.
                                    let galley = ui.painter().layout(
                                        match toast.kind {
                                            ToastKind::Error => widgets::sanitize_error_message(
                                                &toast.message,
                                                app.locale,
                                            )
                                            .into_owned(),
                                            ToastKind::Info => toast.message.clone(),
                                        },
                                        theme::medium(13.5),
                                        palette.text,
                                        280.0,
                                    );
                                    ui.add(egui::Label::new(galley));
                                    if let Some((label, action)) = &toast.action {
                                        ui.add_space(4.0);
                                        if theme::link(
                                            ui,
                                            label.as_str(),
                                            theme::medium(13.5),
                                            palette.text,
                                        )
                                        .clicked()
                                        {
                                            chosen = Some(action.clone());
                                        }
                                    }
                                });
                            });
                    },
                );
            }
            if let Some(action) = chosen {
                app.actions.push(action);
            }
        });
}

#[cfg(test)]
mod toast_tests {
    use super::*;

    /// A toast starts 12 points low, rises into place, and never moves
    /// under reduced motion or once it has arrived.
    #[test]
    fn a_new_toast_rises_into_place() {
        assert_eq!(toast_lift(0.0, TOAST_RISE), 12.0);
        assert!(toast_lift(TOAST_RISE / 2.0, TOAST_RISE) < 12.0);
        assert_eq!(toast_lift(TOAST_RISE, TOAST_RISE), 0.0);
        assert_eq!(toast_lift(f32::INFINITY, TOAST_RISE), 0.0);
        assert_eq!(toast_lift(0.0, 0.0), 0.0);
    }
}

#[cfg(test)]
mod window_chrome_tests {
    use super::*;
    use fastframe_titlebar::Footprint;

    /// Windows' three buttons in its 48-point header.
    const BUTTONS: Footprint = Footprint {
        left: 0.0,
        right: 3.0 * 36.0 + 5.0,
        height: theme::CUSTOM_TOP_BAR_HEIGHT,
    };

    #[test]
    fn the_side_panels_drag_a_window_without_a_native_title_bar() {
        assert!(titlebar_spans_window(true, false));
        assert!(titlebar_spans_window(false, true));
        assert!(!titlebar_spans_window(false, false));
    }

    #[test]
    fn caption_space_belongs_to_the_outermost_header() {
        let values = |footprint, queue, lyrics| {
            let space = controls_reservation(footprint, queue, lyrics, f32::INFINITY);
            [
                space.topbar_width,
                space.topbar_top,
                space.queue_top,
                space.lyrics_top,
            ]
        };
        let [width, height] = [BUTTONS.right, BUTTONS.height];
        assert_eq!(values(BUTTONS, false, false), [width, 0.0, 0.0, 0.0]);
        assert_eq!(values(BUTTONS, true, false), [0.0, 0.0, height, 0.0]);
        assert_eq!(values(BUTTONS, false, true), [0.0, 0.0, 0.0, height]);
        assert_eq!(values(BUTTONS, true, true), [0.0, 0.0, height, 0.0]);
        // No buttons on the right: full screen, the native frame, macOS's
        // traffic lights, or a GNOME layout with them all on the left.
        let none = Footprint {
            left: 50.0,
            ..Footprint::default()
        };
        for (queue, lyrics) in [(false, false), (true, false), (false, true)] {
            assert_eq!(values(none, queue, lyrics), [0.0; 4]);
        }
    }

    #[test]
    fn minimum_windows_window_stacks_caption_space_above_the_topbar() {
        let available = 760.0 - 250.0;
        let space = controls_reservation(BUTTONS, false, false, available);
        assert_eq!(space.topbar_width, 0.0);
        assert_eq!(space.topbar_top, BUTTONS.height);

        let inline =
            controls_reservation(BUTTONS, false, false, TOPBAR_MIN_CONTENT + BUTTONS.right);
        assert_eq!(inline.topbar_width, BUTTONS.right);
        assert_eq!(inline.topbar_top, 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A custom palette's wash fades in at the top of a page; the plain
    /// palettes leave the card clear (the song's colour rises from the
    /// bottom of the window instead).
    #[test]
    fn the_header_fades_only_from_the_palettes_wash() {
        let mut palette = theme::Palette::dark();
        assert_eq!(header_wash(&palette), None);
        palette.tint = Color32::from_black_alpha(24);
        assert_eq!(
            header_wash(&palette),
            Some((palette.tint, Color32::TRANSPARENT))
        );
    }

    /// Pale cool covers glow as colour, not steel; greys stay grey and
    /// already vivid colours keep their hue.
    #[test]
    fn the_glow_lifts_pale_colours_and_leaves_greys() {
        let steel = Color32::from_rgb(0x8a, 0x98, 0xa8);
        let lifted = egui::ecolor::Hsva::from(glow_color(steel));
        assert!(lifted.s >= 0.5 - 1e-3, "{lifted:?}");
        let grey = Color32::from_rgb(0x80, 0x80, 0x80);
        assert_eq!(glow_color(grey), grey);
        let ember = Color32::from_rgb(0xc0, 0x50, 0x10);
        let kept = egui::ecolor::Hsva::from(glow_color(ember));
        let original = egui::ecolor::Hsva::from(ember);
        assert!((kept.h - original.h).abs() < 0.01);
    }

    #[test]
    fn a_glow_fades_in_from_nothing_in_its_own_colour() {
        let blue = Color32::from_rgb(0x20, 0x40, 0xc0);
        let [r, g, b, a] = mix(Color32::TRANSPARENT, blue, 0.5).to_srgba_unmultiplied();
        // Colours are stored premultiplied, so a half-faded one may round
        // a step off.
        for (got, want) in [(r, 0x20u8), (g, 0x40), (b, 0xc0)] {
            assert!(got.abs_diff(want) <= 1, "{got} vs {want}");
        }
        assert!(a > 100 && a < 140);
        assert_eq!(mix(blue, Color32::TRANSPARENT, 1.0).a(), 0);
    }

    /// The wash sits at the top of an unscrolled page and leaves with the
    /// hero as the page scrolls, instead of staying behind the page.
    #[test]
    fn the_header_wash_scrolls_away_with_the_page() {
        let panel = Rect::from_min_size(egui::pos2(0.0, 80.0), vec2(800.0, 600.0));
        assert_eq!(header_rect(panel, 0.0).min, panel.min);
        assert_eq!(header_rect(panel, 0.0).width(), 800.0);
        assert_eq!(header_rect(panel, 120.0).min.y, panel.min.y - 120.0);
        assert_eq!(header_rect(panel, 120.0).max.y, panel.min.y + 220.0);
        assert!(
            header_rect(panel, 400.0).max.y < panel.min.y,
            "gone once scrolled past"
        );
        assert_eq!(
            header_rect(panel, -5.0).min,
            panel.min,
            "no overscroll bounce"
        );
    }

    /// The header casts a shadow only on a page scrolled under it, and it
    /// is black in both themes, never the page's own colour.
    #[test]
    fn the_header_shadow_appears_once_the_page_scrolls() {
        let ctx = egui::Context::default();
        let page = Rect::from_min_size(egui::pos2(0.0, 80.0), vec2(800.0, 600.0));
        let shadows = |scrolled: f32, dark: bool| {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                header_shadow(ui, page, scrolled, dark);
            });
            output.textures_delta.clear();
            output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) => Some(mesh.vertices.clone()),
                    _ => None,
                })
                .flatten()
                .collect::<Vec<_>>()
        };
        assert!(
            shadows(0.0, true).is_empty(),
            "nothing at the top of the page"
        );
        for dark in [true, false] {
            let vertices = shadows(40.0, dark);
            let top = vertices
                .iter()
                .find(|vertex| vertex.pos.y == page.top())
                .expect("a shadow along the page's top edge");
            assert!(top.color.a() > 0);
            assert_eq!((top.color.r(), top.color.g(), top.color.b()), (0, 0, 0));
        }
    }
}
