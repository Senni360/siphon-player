//! Now playing: the island grown to fill the content area. The playing
//! song's cover large, its title over the cover's colour, the row tools as
//! labelled pills, and Up next, Lyrics and Credits beside it. The island
//! stays below as the transport. Clicking the island's cover opens it; Esc,
//! the chevron or the cover again shrinks it back into the island.

use egui::{Align2, Color32, Id, Rect, Sense, Ui, UiBuilder, pos2, vec2};

use crate::api::models::PlayableItem;
use crate::app::App;
use crate::i18n::gettext;
use crate::model::{Action, Loadable};
use crate::theme::{self, Icon};

use super::kit::{Source, motion};

/// The view's side panel, and the narrowest content area that keeps it.
const PANEL_WIDTH: f32 = 340.0;
const PANEL_MIN_CONTENT: f32 = 820.0;
const GROW: f32 = 0.46;

fn open_id() -> Id {
    Id::new("now-playing-open")
}

pub fn is_open(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp::<bool>(open_id()))
        .unwrap_or(false)
}

/// Closes Now playing at once, for sign-out.
pub fn close(ctx: &egui::Context) {
    ctx.data_mut(|data| data.insert_temp(open_id(), false));
}

pub fn toggle(ctx: &egui::Context) {
    let open = !is_open(ctx);
    ctx.data_mut(|data| data.insert_temp(open_id(), open));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    UpNext,
    Lyrics,
    Credits,
}

impl Tab {
    const ALL: [Self; 3] = [Self::UpNext, Self::Lyrics, Self::Credits];
}

/// Draws the view over `content`, grown out of `island`.
pub fn show(app: &mut App, ui: &mut Ui, content: Rect, island: Rect) {
    let ctx = ui.ctx().clone();
    if is_open(&ctx) && ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        toggle(&ctx);
    }
    let open = is_open(&ctx);
    let grown = ctx.animate_bool_with_time(
        Id::new("now-playing-grow"),
        open,
        crate::motion::seconds(&ctx, GROW),
    );
    if grown <= 0.0 {
        return;
    }
    let t = motion::ease(grown);
    // The top bar stays: search and back keep working over Now playing.
    let content = Rect::from_min_max(
        pos2(
            content.left(),
            content.top() + theme::top_bar_height() + theme::titlebar_inset(&ctx),
        ),
        content.max,
    );
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    let frame = Rect::from_min_max(
        pos2(
            lerp(island.left(), content.left() + 8.0),
            lerp(island.top(), content.top() + 8.0),
        ),
        pos2(
            lerp(island.right(), content.right() - 8.0),
            lerp(island.bottom(), content.bottom()),
        ),
    );
    let radius = lerp(22.0, 16.0);
    let palette = app.palette;
    // Without a cover colour, a dark neutral: the view stays dark in both
    // themes, so its white words keep their contrast.
    let tint = app
        .now_playing_tint()
        .unwrap_or(Color32::from_rgb(0x28, 0x28, 0x2d));
    // Always dark behind its words, whatever the theme: the cover's colour,
    // deepened.
    let ground = super::blend(Color32::from_rgb(10, 10, 12), tint, 0.38);

    egui::Area::new(Id::new("now-playing"))
        .order(egui::Order::Middle)
        .fixed_pos(content.min)
        .interactable(open)
        .show(&ctx, |ui| {
            // The page behind fades to the window as the view grows over it,
            // so none of it shows around the rounded corners.
            ui.painter()
                .rect_filled(content, 0.0, palette.window.gamma_multiply(t));
            ui.set_clip_rect(frame.expand(1.0));
            ui.painter().rect(
                frame,
                radius,
                ground,
                egui::Stroke::new(1.0, Color32::from_white_alpha(18)),
                egui::StrokeKind::Inside,
            );
            // Its own words rise in once the frame has mostly grown.
            ui.multiply_opacity(((grown - 0.35) / 0.65).clamp(0.0, 1.0));
            let inner =
                Rect::from_min_max(content.min + vec2(8.0, 8.0), content.max - vec2(8.0, 0.0));
            body(app, ui, inner, t);
        });
}

fn body(app: &mut App, ui: &mut Ui, inner: Rect, t: f32) {
    let ctx = ui.ctx().clone();
    let close = Rect::from_min_size(inner.min + vec2(14.0, 14.0), vec2(32.0, 32.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(close));
    if theme::icon_button(
        &mut child,
        Icon::ChevronDown,
        18.0,
        Color32::from_white_alpha(190),
        Color32::WHITE,
        &gettext(app.locale, "Back into the player (Esc)"),
    )
    .clicked()
    {
        toggle(&ctx);
    }

    let Some(now) = app.now_playing() else {
        ui.painter().text(
            inner.center(),
            Align2::CENTER_CENTER,
            gettext(app.locale, "Nothing playing"),
            theme::medium(16.0),
            Color32::from_white_alpha(200),
        );
        return;
    };
    let panel = inner.width() >= PANEL_MIN_CONTENT;
    let left = if panel {
        Rect::from_min_max(
            inner.min,
            pos2(inner.right() - PANEL_WIDTH - 16.0, inner.bottom()),
        )
    } else {
        inner
    };

    // Cover.
    let side = (left.height() * 0.48)
        .min(left.width() - 112.0)
        .clamp(140.0, 340.0);
    let block_height = side + 24.0 + 70.0 + 24.0 + 34.0;
    let top = left.center().y - block_height / 2.0;
    let cover = Rect::from_min_size(
        pos2(left.left() + 56.0, top + (1.0 - t) * 18.0),
        vec2(side, side),
    );
    super::widgets::paint_shadow(ui, &app.palette, cover, 18.0);
    super::widgets::paint_cover(
        ui,
        &app.palette,
        now.art_url.as_deref().or(now.art_small.as_deref()),
        cover,
        18.0,
        Icon::Music,
        Some(app.backend.art()),
    );

    // Title, wiped in from the left when the song or the view is new.
    let text_left = cover.left();
    let text_width = left.right() - text_left - 40.0;
    let title_galley = super::widgets::ellipsized(
        ui,
        &now.title,
        theme::semibold(26.0),
        Source::of(&now.uri).on_art(),
        text_width,
        2,
    );
    let title_pos = pos2(text_left, cover.bottom() + 24.0);
    let reveal_id = Id::new(("now-playing-title", &now.uri));
    let shown = ctx.animate_bool_with_time(reveal_id, true, crate::motion::seconds(&ctx, 0.5));
    let reveal = motion::ease(shown.min(t));
    let wipe = Rect::from_min_size(
        title_pos,
        vec2(
            title_galley.size().x * reveal + 24.0 * reveal,
            title_galley.size().y,
        ),
    );
    let title_height = title_galley.size().y;
    ui.painter()
        .with_clip_rect(wipe.intersect(ui.clip_rect()))
        .galley(title_pos, title_galley, Color32::WHITE);
    let meta = if now.album_name.is_empty() {
        now.subtitle.clone()
    } else {
        format!("{} · {}", now.subtitle, now.album_name)
    };
    ui.painter().text(
        pos2(text_left, title_pos.y + title_height + 6.0),
        Align2::LEFT_TOP,
        meta,
        theme::regular(15.0),
        Color32::from_white_alpha(185),
    );

    // Tools as pills.
    let pills_top = title_pos.y + title_height + 36.0;
    let item = app.now_playing_item();
    pills(app, ui, pos2(text_left, pills_top), item.as_ref(), &now.uri);

    if panel {
        let rect = Rect::from_min_max(
            pos2(inner.right() - PANEL_WIDTH - 8.0, inner.top() + 16.0),
            pos2(inner.right() - 16.0, inner.bottom() - 16.0),
        );
        let slide = (1.0 - t) * 40.0;
        side_panel(
            app,
            ui,
            rect.translate(vec2(slide, 0.0)),
            &now.uri,
            now.position_ms,
        );
    }
}

/// A labelled tool: icon and words on a quiet glass pill.
fn pill(
    ui: &mut Ui,
    at: egui::Pos2,
    icon: Icon,
    label: &str,
    tip: &str,
    enabled: bool,
) -> (egui::Response, f32) {
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), theme::regular(13.0), Color32::WHITE);
    let rect = Rect::from_min_size(at, vec2(galley.size().x + 44.0, 34.0));
    let response = ui.interact(
        rect,
        ui.id().with(("pill", label)),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let fill = if enabled && response.hovered() {
        Color32::from_white_alpha(40)
    } else {
        Color32::from_white_alpha(24)
    };
    ui.painter().rect(
        rect,
        10.0,
        fill,
        egui::Stroke::new(1.0, Color32::from_white_alpha(30)),
        egui::StrokeKind::Inside,
    );
    let alpha = if enabled { 235 } else { 110 };
    theme::paint_icon(
        ui,
        icon,
        Rect::from_center_size(pos2(rect.left() + 20.0, rect.center().y), vec2(16.0, 16.0)),
        15.0,
        Color32::from_white_alpha(alpha),
    );
    ui.painter().galley(
        pos2(rect.left() + 34.0, rect.center().y - galley.size().y / 2.0),
        galley,
        Color32::from_white_alpha(alpha),
    );
    theme::focus_ring(ui, &response);
    (response.on_hover_text(tip), rect.width())
}

fn pills(app: &mut App, ui: &mut Ui, at: egui::Pos2, item: Option<&PlayableItem>, uri: &str) {
    let locale = app.locale;
    let mut x = at.x;
    let downloadable =
        app.downloads.available && item.is_some_and(|item| app.download_source(item).is_some());
    let download_tip = if !app.downloads.available {
        gettext(locale, "Download needs the Siphon helper")
    } else {
        gettext(locale, "Download")
    };
    let (response, width) = pill(
        ui,
        pos2(x, at.y),
        Icon::Download,
        &gettext(locale, "Download"),
        &download_tip,
        downloadable,
    );
    if response.clicked()
        && let Some(item) = item
    {
        app.actions.push(Action::Download(vec![item.clone()]));
    }
    x += width + 8.0;
    let (_, width) = pill(
        ui,
        pos2(x, at.y),
        Icon::Activity,
        &gettext(locale, "BPM and key"),
        &gettext(
            locale,
            "BPM and key arrive with the Siphon helper's analysis",
        ),
        false,
    );
    x += width + 8.0;
    let (response, width) = pill(
        ui,
        pos2(x, at.y),
        Icon::FileText,
        &gettext(locale, "Credits"),
        &gettext(locale, "Credits"),
        true,
    );
    if response.clicked() {
        ui.ctx()
            .data_mut(|data| data.insert_temp(Id::new("now-playing-tab"), Tab::Credits));
        if app.credits.get(uri).is_none() {
            app.actions.push(Action::RequestCredits(uri.to_owned()));
        }
    }
    x += width + 8.0;
    let (response, _) = pill(
        ui,
        pos2(x, at.y),
        Icon::Users,
        &gettext(locale, "Share with group"),
        &gettext(locale, "Share with group"),
        item.is_some(),
    );
    if response.clicked()
        && let Some(item) = item
        && let Ok(post) = crate::friends::NewPost::from_item(item)
    {
        app.actions
            .push(Action::Friends(crate::friends::Action::Share(post)));
    }
}

fn side_panel(app: &mut App, ui: &mut Ui, rect: Rect, uri: &str, position_ms: u32) {
    let ctx = ui.ctx().clone();
    let locale = app.locale;
    ui.painter()
        .rect_filled(rect, 18.0, Color32::from_black_alpha(70));
    let tab_id = Id::new("now-playing-tab");
    let tab = ctx
        .data(|data| data.get_temp::<Tab>(tab_id))
        .unwrap_or(Tab::UpNext);
    // Segmented tabs with a sliding selection.
    let bar = Rect::from_min_size(rect.min + vec2(10.0, 10.0), vec2(rect.width() - 20.0, 34.0));
    ui.painter()
        .rect_filled(bar, 17.0, Color32::from_white_alpha(14));
    let segment = (bar.width() - 6.0) / 3.0;
    let index = Tab::ALL
        .iter()
        .position(|candidate| *candidate == tab)
        .unwrap_or(0) as f32;
    let slid = ctx.animate_value_with_time(
        tab_id.with("slide"),
        index,
        crate::motion::seconds(&ctx, motion::FEEDBACK * 2.0),
    );
    let selected = Rect::from_min_size(
        pos2(bar.left() + 3.0 + slid * segment, bar.top() + 3.0),
        vec2(segment, bar.height() - 6.0),
    );
    ui.painter()
        .rect_filled(selected, 14.0, Color32::from_white_alpha(38));
    for (position, candidate) in Tab::ALL.iter().enumerate() {
        let cell = Rect::from_min_size(
            pos2(bar.left() + 3.0 + position as f32 * segment, bar.top()),
            vec2(segment, bar.height()),
        );
        let label = match candidate {
            Tab::UpNext => gettext(locale, "Up next"),
            Tab::Lyrics => gettext(locale, "Lyrics"),
            Tab::Credits => gettext(locale, "Credits"),
        };
        let response = ui.interact(cell, tab_id.with(position), Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::Button,
                true,
                *candidate == tab,
                label.as_ref(),
            )
        });
        let alpha = if *candidate == tab || response.hovered() {
            255
        } else {
            165
        };
        ui.painter().text(
            cell.center(),
            Align2::CENTER_CENTER,
            &label,
            theme::medium(12.5),
            Color32::from_white_alpha(alpha),
        );
        if response.clicked() {
            ctx.data_mut(|data| data.insert_temp(tab_id, *candidate));
            if *candidate == Tab::Credits && app.credits.get(uri).is_none() {
                app.actions.push(Action::RequestCredits(uri.to_owned()));
            }
        }
    }

    let body = Rect::from_min_max(
        pos2(rect.left() + 16.0, bar.bottom() + 12.0),
        rect.max - vec2(16.0, 12.0),
    );
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(body)
            .id_salt(("now-playing-body", tab as u8)),
    );
    child.set_clip_rect(body.intersect(ui.clip_rect()));
    // Each tab arrives from the side it sits on.
    let arrive = ctx.animate_bool_with_time(
        tab_id.with(("arrive", tab as u8)),
        true,
        crate::motion::seconds(&ctx, motion::ENTER * 0.8),
    );
    child.multiply_opacity(arrive);
    match tab {
        Tab::UpNext => up_next(app, &mut child),
        Tab::Lyrics => lyrics(app, &mut child, uri, position_ms),
        Tab::Credits => {
            let state = app.credits.get(uri).cloned();
            let retry = super::row_tools::credits_body(
                &mut child,
                locale,
                state.as_ref(),
                [
                    Color32::from_white_alpha(240),
                    Color32::from_white_alpha(185),
                    Color32::from_white_alpha(140),
                ],
            );
            if retry {
                app.actions.push(Action::RequestCredits(uri.to_owned()));
            }
        }
    }
}

/// What Up next last showed at its head, and how it last changed.
#[derive(Clone, Debug)]
struct UpNext {
    head: Option<String>,
    change: Option<(motion::Swipe, motion::Transition)>,
}

/// How the queue's head moving from `before` to `head` reads, with `playing`
/// now playing: the old head playing is Next, anything else replaced the
/// queue. `None` when the head did not move.
fn queue_change(
    before: Option<&str>,
    head: Option<&str>,
    playing: Option<&str>,
) -> Option<motion::Swipe> {
    if before == head {
        None
    } else if before.is_some() && before == playing {
        Some(motion::Swipe::Next)
    } else {
        Some(motion::Swipe::Replace)
    }
}

fn up_next(app: &mut App, ui: &mut Ui) {
    let upcoming: Vec<PlayableItem> = app
        .queue
        .get()
        .map(|queue| queue.queue.iter().take(8).cloned().collect())
        .unwrap_or_default();
    if upcoming.is_empty() {
        theme::text(
            ui,
            gettext(
                app.locale,
                "Nothing queued. Songs from the list you play from line up here.",
            ),
            theme::regular(12.5),
            Color32::from_white_alpha(170),
        );
        return;
    }
    // The list moves with the queue: up one place on Next, in afresh when
    // the queue was replaced.
    let ctx = ui.ctx().clone();
    let head = upcoming.first().map(|item| item.uri().to_owned());
    let playing = app.now_playing().map(|now| now.uri);
    let state_id = Id::new("now-playing-up-next");
    let before = ctx.data(|data| data.get_temp::<UpNext>(state_id));
    if before.as_ref().map(|before| &before.head) != Some(&head) {
        let change = before.and_then(|before| {
            queue_change(before.head.as_deref(), head.as_deref(), playing.as_deref())
        });
        let state = UpNext {
            head: head.clone(),
            change: change.map(|change| (change, motion::Transition::start(&ctx, motion::ENTER))),
        };
        ctx.data_mut(|data| data.insert_temp(state_id, state));
    }
    let change = ctx
        .data(|data| data.get_temp::<UpNext>(state_id))
        .and_then(|state| state.change);
    let now = ctx.input(|input| input.time);
    if change.is_some_and(|(_, transition)| !transition.finished_at(now)) {
        ctx.request_repaint();
    }
    for (index, item) in upcoming.iter().enumerate() {
        let (slot, _) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::hover());
        let (dy, alpha) = match change {
            Some((motion::Swipe::Next, transition)) => {
                ((1.0 - transition.progress_at(now)) * slot.height(), 1.0)
            }
            Some((_, transition)) => {
                let t = transition.delayed(motion::stagger(index)).progress_at(now);
                ((1.0 - t) * 10.0, t)
            }
            None => (0.0, 1.0),
        };
        let row = slot.translate(vec2(0.0, dy));
        let mut cell = ui.new_child(UiBuilder::new().max_rect(row));
        cell.multiply_opacity(alpha);
        let ui = &mut cell;
        let cover = Rect::from_min_size(pos2(row.left(), row.center().y - 20.0), vec2(40.0, 40.0));
        super::widgets::paint_cover(
            ui,
            &app.palette,
            app.item_image(item, 64).as_deref(),
            cover,
            7.0,
            Icon::Music,
            None,
        );
        let left = cover.right() + 10.0;
        let width = row.right() - left - 44.0;
        let title = super::widgets::ellipsized(
            ui,
            item.name(),
            theme::medium(13.5),
            Source::of(item.uri()).on_art(),
            width,
            1,
        );
        ui.painter()
            .galley(pos2(left, row.center().y - 17.0), title, Color32::WHITE);
        let artist = super::widgets::ellipsized(
            ui,
            &item.subtitle(),
            theme::regular(12.0),
            Color32::from_white_alpha(160),
            width,
            1,
        );
        ui.painter()
            .galley(pos2(left, row.center().y + 2.0), artist, Color32::WHITE);
        ui.painter().text(
            pos2(row.right(), row.center().y),
            Align2::RIGHT_CENTER,
            crate::util::format_duration_ms(item.duration_ms()),
            theme::regular(12.0),
            Color32::from_white_alpha(150),
        );
    }
}

fn lyrics(app: &mut App, ui: &mut Ui, uri: &str, position_ms: u32) {
    let locale = app.locale;
    let ours = app.lyrics_uri.as_deref() == Some(uri);
    match (&app.lyrics, ours) {
        (Loadable::Loaded(Some(lyrics)), true) if lyrics.instrumental => {
            theme::text(
                ui,
                gettext(locale, "Instrumental"),
                theme::regular(13.0),
                Color32::from_white_alpha(185),
            );
        }
        (Loadable::Loaded(Some(lyrics)), true) => {
            let current = lyrics.active_line(position_ms);
            for (index, line) in lyrics.lines.iter().take(40).enumerate() {
                let lit = current == Some(index) || !lyrics.synced;
                let color = Color32::from_white_alpha(if lit { 250 } else { 110 });
                let galley = super::widgets::ellipsized(
                    ui,
                    &line.text,
                    theme::semibold(17.0),
                    color,
                    ui.available_width(),
                    3,
                );
                let (rect, _) = ui.allocate_exact_size(
                    vec2(ui.available_width(), galley.size().y + 6.0),
                    Sense::hover(),
                );
                ui.painter().galley(rect.min, galley, color);
            }
        }
        (Loadable::Loading, true) => {
            theme::text(
                ui,
                gettext(locale, "Loading lyrics…"),
                theme::regular(13.0),
                Color32::from_white_alpha(185),
            );
        }
        (Loadable::Loaded(None), true) => {
            theme::text(
                ui,
                gettext(locale, "No lyrics for this song."),
                theme::regular(13.0),
                Color32::from_white_alpha(185),
            );
        }
        _ => {
            theme::text(
                ui,
                gettext(locale, "Lyrics load with the lyrics panel."),
                theme::regular(12.5),
                Color32::from_white_alpha(170),
            );
            if !app.show_lyrics_panel && ui.button(gettext(locale, "Show lyrics")).clicked() {
                app.actions.push(Action::ToggleLyricsPanel);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The queue's head starting to play moves Up next up one place; a new
    /// head from anywhere else brings the list in afresh.
    #[test]
    fn up_next_reads_a_head_change_as_next_or_replace() {
        assert_eq!(queue_change(Some("a"), Some("a"), Some("z")), None);
        assert_eq!(
            queue_change(Some("a"), Some("b"), Some("a")),
            Some(motion::Swipe::Next)
        );
        assert_eq!(
            queue_change(Some("a"), Some("x"), Some("q")),
            Some(motion::Swipe::Replace)
        );
        assert_eq!(
            queue_change(None, Some("x"), None),
            Some(motion::Swipe::Replace)
        );
    }

    /// The island's cover opens Now playing and a second click closes it;
    /// it starts closed in a new window.
    #[test]
    fn the_cover_opens_and_closes_now_playing() {
        let ctx = egui::Context::default();
        assert!(!is_open(&ctx));
        toggle(&ctx);
        assert!(is_open(&ctx));
        toggle(&ctx);
        assert!(!is_open(&ctx));
    }
}
