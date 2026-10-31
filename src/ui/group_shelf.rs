//! "From the group" at the top of Home: the newest songs friends shared in
//! the Siphon group, before anything a service recommends. Each share shows
//! its cover, its title in its source's colour, who shared it and when,
//! their note, a play control and a heart.

use egui::{Align2, Color32, Rect, Sense, Ui, pos2, vec2};

use crate::app::App;
use crate::friends::{Post, Source as PostSource};
use crate::i18n::gettext;
use crate::model::Action;
use crate::theme::{self, Icon};

use super::kit::Source;

/// How many shares Home shows; the rest are in the group panel.
const SHOWN: usize = 3;
const CARD_HEIGHT: f32 = 132.0;
const GAP: f32 = 12.0;

pub fn show(app: &mut App, ui: &mut Ui) {
    let palette = app.palette;
    let locale = app.locale;
    ui.horizontal(|ui| {
        theme::section_title(ui, &palette, &gettext(locale, "From the group"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::link(
                ui,
                gettext(locale, "Open group"),
                theme::regular(13.0),
                palette.secondary,
            )
            .clicked()
            {
                app.actions.push(Action::Open(crate::model::Page::Group));
            }
        });
    });
    ui.add_space(4.0);

    let feed = &app.friends_feed;
    if feed.profile.is_none() {
        let join = message(
            ui,
            app,
            &gettext(
                locale,
                "Join your group to see the songs your friends share here first.",
            ),
            Some(gettext(locale, "Join").into_owned()),
        );
        if join {
            app.actions.push(Action::Open(crate::model::Page::Group));
        }
        return;
    }
    if let Some(error) = feed.error.clone().filter(|_| feed.posts.is_empty()) {
        let retry = message(
            ui,
            app,
            &format!(
                "{} {error}",
                gettext(locale, "The group feed couldn't be loaded.")
            ),
            Some(gettext(locale, "Try again").into_owned()),
        );
        if retry {
            app.actions
                .push(Action::Friends(crate::friends::Action::Refresh));
        }
        return;
    }
    if feed.posts.is_empty() {
        let text = if feed.busy {
            gettext(locale, "Loading the group's shares…")
        } else {
            gettext(
                locale,
                "Nothing shared yet. Share what's playing from Now playing, and it lands here for everyone.",
            )
        };
        message(ui, app, &text, None);
        return;
    }

    let posts: Vec<Post> = feed.posts.iter().take(SHOWN).cloned().collect();
    let width = ui.available_width();
    let columns = if width > 900.0 { SHOWN } else { 2 }.min(posts.len());
    let card_width = (width - GAP * (columns as f32 - 1.0)) / columns as f32;
    let (area, _) = ui.allocate_exact_size(vec2(width, CARD_HEIGHT), Sense::hover());
    for (index, post) in posts.iter().take(columns).enumerate() {
        let rect = Rect::from_min_size(
            pos2(area.left() + index as f32 * (card_width + GAP), area.top()),
            vec2(card_width, CARD_HEIGHT),
        );
        // Shares arrive one after another the first time Home shows them.
        let arrive = ui.ctx().animate_bool_with_time(
            egui::Id::new(("group-share-arrive", &post.id)),
            true,
            crate::motion::seconds(ui.ctx(), super::kit::motion::ENTER + index as f32 * 0.06),
        );
        let rect = rect.translate(vec2(0.0, (1.0 - super::kit::motion::ease(arrive)) * 10.0));
        card(app, ui, rect, post, arrive);
    }
    ui.add_space(20.0);
}

/// A quiet line for the empty, joining and failed states, with one action.
/// Returns whether its button was clicked.
fn message(ui: &mut Ui, app: &App, text: &str, button: Option<String>) -> bool {
    let palette = app.palette;
    let mut clicked = false;
    egui::Frame::new()
        .fill(palette.surface)
        .corner_radius(12)
        .inner_margin(egui::Margin::symmetric(16, 14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                // The line wraps beside its button instead of running under
                // it when the page is narrow.
                let reserve = if button.is_some() { 110.0 } else { 0.0 };
                let width = (ui.available_width() - reserve).max(80.0);
                ui.allocate_ui(vec2(width, 0.0), |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(text)
                                .font(theme::regular(13.5))
                                .color(palette.secondary),
                        )
                        .wrap(),
                    );
                });
                if let Some(label) = button {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        clicked = ui.button(label).clicked();
                    });
                }
            });
        });
    ui.add_space(20.0);
    clicked
}

fn card(app: &mut App, ui: &mut Ui, rect: Rect, post: &Post, alpha: f32) {
    let palette = app.palette;
    let locale = app.locale;
    let response = ui.interact(
        rect,
        ui.id().with(("group-share", &post.id)),
        Sense::hover(),
    );
    let fill = if response.hovered() {
        palette.surface_hover
    } else {
        palette.surface
    };
    let painter = ui.painter().clone();
    painter.rect_filled(rect, 14.0, fill.gamma_multiply(alpha));

    let cover = Rect::from_min_size(rect.min + vec2(14.0, 14.0), vec2(64.0, 64.0));
    super::widgets::paint_cover(
        ui,
        &palette,
        post.item.artwork_url.as_deref(),
        cover,
        9.0,
        Icon::Music,
        None,
    );
    let left = cover.right() + 12.0;
    let text_width = (rect.right() - left - 14.0).max(40.0);
    let who = format!("{} · {}", post.author_name, ago(locale, post.created_at));
    painter.text(
        pos2(left, cover.top() + 2.0),
        Align2::LEFT_TOP,
        who,
        theme::regular(12.0),
        palette.dim.gamma_multiply(alpha),
    );
    let source = match post.item.source {
        PostSource::Spotify => Source::Spotify,
        PostSource::Soundcloud => Source::SoundCloud,
        PostSource::Youtube => Source::YouTube,
    };
    let title = super::widgets::ellipsized(
        ui,
        &post.item.title,
        theme::medium(14.0),
        source.color(&palette).gamma_multiply(alpha),
        text_width,
        1,
    );
    painter.galley(pos2(left, cover.top() + 20.0), title, Color32::WHITE);
    let artists = super::widgets::ellipsized(
        ui,
        &post.item.artists.join(", "),
        theme::regular(12.5),
        palette.secondary.gamma_multiply(alpha),
        text_width,
        1,
    );
    painter.galley(pos2(left, cover.top() + 40.0), artists, Color32::WHITE);
    if !post.item.note.is_empty() {
        let note = super::widgets::ellipsized(
            ui,
            &format!("“{}”", post.item.note),
            theme::regular(13.0),
            palette.text.gamma_multiply(alpha),
            rect.width() - 28.0 - 80.0,
            1,
        );
        painter.galley(
            pos2(rect.left() + 14.0, cover.bottom() + 12.0),
            note,
            Color32::WHITE,
        );
    }

    // Play and heart, bottom right.
    let play = Rect::from_center_size(
        pos2(rect.right() - 30.0, rect.bottom() - 26.0),
        vec2(32.0, 32.0),
    );
    let play_response = ui
        .interact(
            play,
            ui.id().with(("group-share-play", &post.id)),
            Sense::click(),
        )
        .on_hover_text(gettext(locale, "Play"));
    // Named for its song, so a screen reader tells the cards apart.
    let play_label = format!("{} {}", gettext(locale, "Play"), post.item.title);
    play_response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, play_label.clone())
    });
    painter.circle_filled(play.center(), 16.0, palette.accent.gamma_multiply(alpha));
    theme::paint_icon(ui, Icon::PlayFilled, play, 14.0, palette.on_accent);
    theme::focus_ring(ui, &play_response);
    if play_response.clicked() {
        app.actions.push(Action::OpenLink(post.item.url.clone()));
    }

    let heart = Rect::from_center_size(pos2(play.left() - 26.0, play.center().y), vec2(28.0, 28.0));
    let liked = post.liked;
    let label = if liked {
        gettext(locale, "Remove heart")
    } else {
        gettext(locale, "Heart")
    };
    let heart_response = ui
        .interact(
            heart,
            ui.id().with(("group-share-heart", &post.id)),
            Sense::click(),
        )
        .on_hover_text(label.as_ref());
    let heart_label = format!("{label} {}", post.item.title);
    heart_response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, liked, heart_label.clone())
    });
    // A heart pops when it is given.
    let pop = ui.ctx().animate_bool_with_time(
        egui::Id::new(("group-share-heart-pop", &post.id)),
        liked,
        crate::motion::seconds(ui.ctx(), super::kit::motion::POP),
    );
    let swell = if liked {
        super::kit::motion::pop(pop)
    } else {
        1.0
    };
    let icon = if liked {
        Icon::HeartFilled
    } else {
        Icon::Heart
    };
    let color = if liked || heart_response.hovered() {
        palette.text
    } else {
        palette.secondary
    };
    theme::paint_icon(ui, icon, heart, 16.0 * swell, color.gamma_multiply(alpha));
    if post.like_count > 0 {
        painter.text(
            pos2(heart.left() - 2.0, heart.center().y),
            Align2::RIGHT_CENTER,
            post.like_count.to_string(),
            theme::regular(12.0),
            palette.dim.gamma_multiply(alpha),
        );
    }
    theme::focus_ring(ui, &heart_response);
    if heart_response.clicked() {
        app.actions
            .push(Action::Friends(crate::friends::Action::Like {
                id: post.id.clone(),
                liked: !liked,
            }));
    }
}

/// "12 min ago" and friends, from the service's millisecond timestamp.
pub(super) fn ago(locale: crate::i18n::Locale, created_at_ms: u64) -> String {
    let now = jiff::Timestamp::now().as_millisecond().max(0) as u64;
    let minutes = now.saturating_sub(created_at_ms) / 60_000;
    if minutes < 1 {
        gettext(locale, "just now").into_owned()
    } else if minutes < 60 {
        gettext(locale, "{n} min ago").replace("{n}", &minutes.to_string())
    } else if minutes < 60 * 24 {
        gettext(locale, "{n} h ago").replace("{n}", &(minutes / 60).to_string())
    } else {
        gettext(locale, "{n} d ago").replace("{n}", &(minutes / 60 / 24).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_times_read_as_minutes_hours_and_days() {
        let now = jiff::Timestamp::now().as_millisecond() as u64;
        let locale = crate::i18n::Locale::English;
        assert_eq!(ago(locale, now), "just now");
        assert_eq!(ago(locale, now - 12 * 60_000), "12 min ago");
        assert_eq!(ago(locale, now - 3 * 3_600_000), "3 h ago");
        assert_eq!(ago(locale, now - 2 * 86_400_000), "2 d ago");
        assert_eq!(
            ago(locale, now + 60_000),
            "just now",
            "a clock ahead of ours"
        );
    }
}
