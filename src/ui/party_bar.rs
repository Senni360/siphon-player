//! The strip across the top of every page while listening with others. Being
//! in a party is never something to find out by accident: it says whose music
//! this is, who is here and what is coming, and Leave is always one press away.

use egui::{Align, Frame, Layout, Margin, RichText, Sense, Ui, vec2};

use crate::app::App;
use crate::friends::live::{Action as Live, Channel, Session};
use crate::i18n::gettext;
use crate::model::{Action, Page};
use crate::theme::{self, Icon, Palette};

fn name_of(app: &App, user: &str) -> String {
    app.friends_feed
        .members
        .iter()
        .find(|member| member.user_id == user)
        .map_or_else(
            || gettext(app.locale, "Someone").into_owned(),
            |member| member.display_name.clone(),
        )
}

/// A dot that breathes, so the strip reads as live at a glance.
fn live_dot(ui: &mut Ui, palette: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
    let time = ui.input(|i| i.time) as f32;
    let breath = 0.65 + 0.35 * (time * 2.2).sin().abs();
    ui.painter()
        .circle_filled(rect.center(), 4.0, palette.accent.gamma_multiply(breath));
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(120));
}

fn line(ui: &mut Ui, text: String, font: egui::FontId, color: egui::Color32) {
    ui.add(
        egui::Label::new(RichText::new(text).font(font).color(color))
            .truncate()
            .selectable(false),
    );
}

/// How the party reads in one line: "Friday · Hosted by Ana · 3 listening".
pub(crate) fn party_line(app: &App, channel: &Channel, me: &str) -> String {
    let host = if channel.host_id == me {
        gettext(app.locale, "You're the host").into_owned()
    } else {
        format!(
            "{} {}",
            gettext(app.locale, "Hosted by"),
            name_of(app, &channel.host_id)
        )
    };
    format!(
        "{} · {host} · {} {}",
        channel.name,
        channel.members.len(),
        gettext(app.locale, "listening")
    )
}

/// What leaving means right now, and what to call it, while with others: the
/// last one in a party ends it. `None` while alone.
pub(crate) fn leave_action(app: &App) -> Option<(Action, String)> {
    if app.live.session == Session::Alone {
        return None;
    }
    let me = app.friends_feed.profile.as_ref()?.user_id.as_str();
    let ends = app
        .live
        .my_channel()
        .is_some_and(|c| c.locked && c.host_id == me && c.members.len() <= 1);
    Some(if ends {
        (
            Action::Live(Live::End),
            gettext(app.locale, "End party").into_owned(),
        )
    } else {
        (
            Action::Live(Live::Leave),
            gettext(app.locale, "Leave party").into_owned(),
        )
    })
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let Some(me) = app.friends_feed.profile.as_ref().map(|p| p.user_id.clone()) else {
        return;
    };
    let palette = app.palette;
    let locale = app.locale;
    let followers = app.live.followers_of(&me).len();
    let session = app.live.session.clone();
    let channel = app.live.my_channel().cloned();
    if session == Session::Alone && followers == 0 {
        return;
    }
    let mut actions = Vec::new();
    Frame::new()
        .fill(palette.accent.gamma_multiply(0.14))
        .stroke(egui::Stroke::new(1.0, palette.accent.gamma_multiply(0.35)))
        .corner_radius(10.0)
        .inner_margin(Margin::symmetric(12, 7))
        .outer_margin(Margin::symmetric(12, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                live_dot(ui, &palette);
                let (title, detail) = match (&session, &channel) {
                    (Session::Channel(_), Some(channel)) => {
                        let locked = !channel.controlled_by(&me);
                        (
                            party_line(app, channel, &me),
                            if locked {
                                gettext(locale, "Anything you play joins the queue")
                            } else {
                                gettext(locale, "You're at the controls")
                            }
                            .into_owned(),
                        )
                    }
                    (Session::Following(user), _) => (
                        format!(
                            "{} {}",
                            gettext(locale, "Listening along with"),
                            name_of(app, user)
                        ),
                        gettext(locale, "They choose the music").into_owned(),
                    ),
                    _ => (
                        if followers == 1 {
                            gettext(locale, "1 person is listening along with you").into_owned()
                        } else {
                            format!(
                                "{followers} {}",
                                gettext(locale, "people are listening along with you")
                            )
                        },
                        String::new(),
                    ),
                };
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if session != Session::Alone {
                        let ends = channel
                            .as_ref()
                            .is_some_and(|c| c.locked && c.host_id == me && c.members.len() <= 1);
                        let label = if ends {
                            gettext(locale, "End party")
                        } else {
                            gettext(locale, "Leave")
                        };
                        if theme::soft_button(ui, &palette, Some(Icon::LogOut), &label, false)
                            .clicked()
                        {
                            actions.push(Action::Live(if ends { Live::End } else { Live::Leave }));
                        }
                        if let (Session::Channel(_), Some(channel), Some(group)) =
                            (&session, &channel, crate::friends::account::current_group())
                            && theme::soft_button(
                                ui,
                                &palette,
                                Some(Icon::Copy),
                                &gettext(locale, "Copy invite"),
                                false,
                            )
                            .on_hover_text(gettext(
                                locale,
                                "Copies a link. Anyone in the group who opens it joins this party.",
                            ))
                            .clicked()
                        {
                            ui.ctx().copy_text(format!(
                                "{}{group}/{}",
                                crate::link::PARTY_PAGE,
                                channel.id
                            ));
                            actions.push(Action::ShowToast(
                                gettext(locale, "Invite link copied").into_owned(),
                            ));
                        }
                        if app.page() != &Page::Group
                            && theme::soft_button(
                                ui,
                                &palette,
                                None,
                                &gettext(locale, "Open"),
                                false,
                            )
                            .clicked()
                        {
                            actions.push(Action::Open(Page::Group));
                        }
                    }
                    // A guest's wish reaches the one who can grant it, in plain sight.
                    if let Some(channel) = channel.as_ref().filter(|c| c.controlled_by(&me))
                        && !channel.skip_requests.is_empty()
                    {
                        let count = channel.skip_requests.len();
                        let text = if count == 1 {
                            format!(
                                "{} {}",
                                name_of(app, &channel.skip_requests[0]),
                                gettext(locale, "asked to skip")
                            )
                        } else {
                            format!("{count} {}", gettext(locale, "asked to skip"))
                        };
                        if theme::soft_button(
                            ui,
                            &palette,
                            Some(Icon::SkipForwardFilled),
                            &text,
                            true,
                        )
                        .clicked()
                        {
                            actions.push(Action::Next);
                        }
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.vertical(|ui| {
                            line(ui, title, theme::medium(13.0), palette.text);
                            if !detail.is_empty() {
                                line(ui, detail, theme::regular(11.5), palette.secondary);
                            }
                        });
                    });
                });
            });
        });
    app.actions.extend(actions);
}
