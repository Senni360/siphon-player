//! Everything about belonging: connecting Spotify, choosing between groups,
//! and the People tab, where whoever runs a group sees who is in it and decides
//! what they may do.

use egui::{Id, RichText, Ui, vec2};

use crate::app::App;
use crate::friends::Action as Group;
use crate::friends::account::{self, Account, Member, Op, Rights, Role, SoundCloudLink};
use crate::i18n::gettext;
use crate::model::Action;
use crate::theme::{self, Icon};

use super::group::{card, field};

/// A code to type on a phone, which makes it another device of this account.
pub(super) fn link_a_phone(app: &App, ui: &mut Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    // A connected phone says so, but another can always be linked: a second
    // phone, or another browser on the same one.
    if app.phone.watched {
        theme::text(
            ui,
            gettext(locale, "A phone is connected."),
            theme::medium(13.0),
            palette.text,
        );
        ui.add_space(6.0);
    }
    let left = match &app.phone.code {
        Some(Ok((code, made))) => crate::app::remote::CODE_LASTS
            .checked_sub(made.elapsed())
            .map(|left| (code.clone(), left)),
        _ => None,
    };
    if let Some((code, left)) = left {
        paragraph(
            ui,
            gettext(
                locale,
                "On your phone, open encore.senni.dev and type this code. Your phone then has this account, its groups and its library, and it can play on this computer.",
            ),
            &palette,
        );
        ui.add_space(6.0);
        let spaced = format!(
            "{} {}",
            &code[..4.min(code.len())],
            &code[4.min(code.len())..]
        );
        theme::text(ui, spaced, theme::semibold(26.0), palette.text);
        theme::text(
            ui,
            gettext(locale, "Good for {minutes} more minutes")
                .replace("{minutes}", &(left.as_secs() / 60 + 1).to_string()),
            theme::regular(12.0),
            palette.dim,
        );
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
        ui.add_space(6.0);
        // A code works once, so the next phone or browser needs its own.
        let asking = app.phone.asking_code;
        if theme::soft_button(ui, &palette, None, &gettext(locale, "New code"), asking).clicked()
            && !asking
        {
            actions.push(Action::LinkPhone);
        }
        return;
    }
    // A phone joins this computer's group account, so there has to be one.
    if app.friends_feed.account.is_none() {
        theme::text(
            ui,
            gettext(
                locale,
                "Connect to your group first. Your phone joins the same account.",
            ),
            theme::regular(13.0),
            palette.secondary,
        );
        ui.add_space(6.0);
        if theme::soft_button(ui, &palette, None, &gettext(locale, "Open Group"), false).clicked() {
            actions.push(Action::Open(crate::model::Page::Group));
        }
        return;
    }
    if let Some(Err(problem)) = &app.phone.code {
        theme::text(ui, problem, theme::regular(13.0), palette.warning);
        ui.add_space(4.0);
    }
    let asking = app.phone.asking_code;
    if theme::soft_button(
        ui,
        &palette,
        Some(Icon::Plus),
        &if app.phone.watched {
            gettext(locale, "Link another phone")
        } else {
            gettext(locale, "Link a phone")
        },
        asking,
    )
    .clicked()
        && !asking
    {
        actions.push(Action::LinkPhone);
    }
}

/// The other devices on this account, to sign out one at a time.
pub(super) fn linked_devices(app: &App, ui: &mut Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    let Some(account) = &app.friends_feed.account else {
        return;
    };
    for device in account.devices.iter().filter(|device| !device.current) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            theme::text(
                ui,
                app.private_text(&device.name),
                theme::regular(13.0),
                palette.secondary,
            );
            if theme::soft_button(ui, &palette, None, &gettext(locale, "Sign out"), false).clicked()
            {
                actions.push(account_op(Op::RemoveDevice(device.id.clone())));
            }
        });
    }
}

fn account_op(op: Op) -> Action {
    Action::Friends(Group::Account(op))
}

fn paragraph(ui: &mut Ui, text: impl Into<String>, palette: &crate::theme::Palette) {
    ui.add(
        egui::Label::new(
            RichText::new(text.into())
                .font(theme::regular(13.5))
                .color(palette.secondary),
        )
        .wrap()
        .selectable(false),
    );
}

fn heading(ui: &mut Ui, text: impl Into<String>, palette: &crate::theme::Palette) {
    theme::text(ui, text.into(), theme::medium(15.0), palette.text);
}

fn error_line(app: &App, ui: &mut Ui) {
    if let Some(error) = &app.friends_feed.error {
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            theme::icon(ui, Icon::CircleAlert, 16.0, app.palette.danger);
            theme::text(ui, error, theme::regular(13.0), app.palette.secondary);
        });
    }
}

/// Before this computer knows who you are. There is no name to invent and no
/// code to find first: your Spotify account is your place in the group.
pub fn connect(app: &App, ui: &mut Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    let feed = &app.friends_feed;
    card(ui, &palette, |ui| {
        heading(
            ui,
            gettext(locale, "Connect your Spotify account"),
            &palette,
        );
        ui.add_space(4.0);
        paragraph(
            ui,
            gettext(
                locale,
                "Your groups follow your Spotify account: sign in on another computer and you're in the same groups, with the same songs. Encore shows your Spotify account to the group service once to prove it's you, and the service doesn't keep anything it could use to reach your account.",
            ),
            &palette,
        );
        ui.add_space(14.0);
        let label = if feed.busy {
            gettext(locale, "Connecting…")
        } else {
            gettext(locale, "Connect Spotify")
        };
        ui.add_enabled_ui(!feed.busy && app.user.is_some(), |ui| {
            if theme::pill_button(ui, &palette, &label, true).clicked() {
                actions.push(Action::Friends(Group::ConnectSpotify));
            }
        });
        error_line(app, ui);
    });
}

/// Someone from before accounts: everything works, and connecting makes it follow them.
pub fn connect_nudge(app: &App, ui: &mut Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    card(ui, &palette, |ui| {
        heading(ui, gettext(locale, "Make your group follow you"), &palette);
        ui.add_space(4.0);
        paragraph(
            ui,
            gettext(
                locale,
                "Connect Spotify and signing in on any computer brings back your groups. You keep your name and everything you've shared.",
            ),
            &palette,
        );
        ui.add_space(10.0);
        ui.add_enabled_ui(!app.friends_feed.busy, |ui| {
            if theme::pill_button(ui, &palette, &gettext(locale, "Connect Spotify"), true).clicked()
            {
                actions.push(Action::Friends(Group::ConnectSpotify));
            }
        });
    });
}

/// Signed in, in no group yet: start one or use a code.
pub fn no_group(app: &App, ui: &mut Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    card(ui, &palette, |ui| {
        heading(ui, gettext(locale, "You're not in a group yet"), &palette);
        ui.add_space(4.0);
        paragraph(
            ui,
            gettext(
                locale,
                "Start one and invite your friends, or enter the code someone gave you.",
            ),
            &palette,
        );
        ui.add_space(12.0);
        new_or_join(app, ui, actions);
        error_line(app, ui);
    });
}

fn new_or_join(app: &App, ui: &mut Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    let id = Id::new("group-new-or-join");
    let (mut name, mut code) = ui
        .data(|data| data.get_temp::<(String, String)>(id))
        .unwrap_or_default();
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 130.0).max(120.0);
        ui.allocate_ui(vec2(width, 36.0), |ui| {
            field(
                ui,
                &palette,
                &mut name,
                &gettext(locale, "New group's name"),
                false,
                60,
            );
        });
        ui.add_enabled_ui(!name.trim().is_empty(), |ui| {
            if theme::soft_button(
                ui,
                &palette,
                Some(Icon::Plus),
                &gettext(locale, "Create"),
                true,
            )
            .clicked()
            {
                actions.push(account_op(Op::NewGroup(
                    std::mem::take(&mut name).trim().to_owned(),
                )));
            }
        });
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 130.0).max(120.0);
        ui.allocate_ui(vec2(width, 36.0), |ui| {
            field(
                ui,
                &palette,
                &mut code,
                &gettext(locale, "Invite code"),
                true,
                256,
            );
        });
        ui.add_enabled_ui(!code.trim().is_empty(), |ui| {
            if theme::soft_button(ui, &palette, None, &gettext(locale, "Join"), false).clicked() {
                actions.push(account_op(Op::JoinWithCode(
                    std::mem::take(&mut code).trim().to_owned(),
                )));
            }
        });
    });
    ui.data_mut(|data| data.insert_temp(id, (name, code)));
}

/// The group being looked at, and the others to move to. Hidden when there is
/// only one.
pub fn switcher(app: &App, ui: &mut Ui, account: &Account, actions: &mut Vec<Action>) {
    if account.groups.len() < 2 {
        return;
    }
    let palette = app.palette;
    let chosen = account::current_group();
    let current = account
        .chosen_group(chosen.as_deref())
        .map_or("", |group| group.name.as_str());
    let response = ui.add(
        egui::Button::image_and_text(
            Icon::ChevronDown.image(palette.text, 15.0),
            egui::RichText::new(current).font(theme::medium(13.0)),
        )
        .fill(palette.surface)
        .corner_radius(12)
        .min_size(egui::vec2(0.0, 28.0)),
    );
    egui::Popup::menu(&response)
        .frame(super::widgets::menu_frame(&palette))
        .show(|ui| {
            ui.set_min_width(180.0);
            for group in &account.groups {
                let selected = chosen.as_deref() == Some(group.id.as_str());
                let icon = selected.then_some(Icon::Check);
                if super::widgets::menu_item(ui, &palette, icon, &group.name) && !selected {
                    actions.push(Action::Friends(Group::SelectGroup(group.id.clone())));
                }
            }
        });
}

fn role_label(app: &App, role: Role) -> String {
    gettext(
        app.locale,
        match role {
            Role::Owner => "Owner",
            Role::Admin => "Admin",
            Role::Member => "Member",
        },
    )
    .into_owned()
}

/// The People tab.
pub fn people(app: &mut App, ui: &mut Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    let Some(account) = app.friends_feed.account.clone() else {
        // A service from before accounts has no people to manage.
        return;
    };
    let chosen = account::current_group();
    if let Some(group) = account.chosen_group(chosen.as_deref()) {
        card(ui, &palette, |ui| {
            heading(ui, group.name.clone(), &palette);
            ui.add_space(2.0);
            theme::text(
                ui,
                format!(
                    "{} · {} {}",
                    role_label(app, group.role),
                    group.members.len(),
                    gettext(locale, "members")
                ),
                theme::regular(12.5),
                palette.dim,
            );
            ui.add_space(10.0);
            for member in &group.members {
                member_row(app, ui, group, member, &account.user_id, actions);
                ui.add_space(4.0);
            }
            for spotify in &group.pending {
                ui.horizontal(|ui| {
                    theme::text(
                        ui,
                        format!("{spotify} · {}", gettext(locale, "joins when they sign in")),
                        theme::regular(13.0),
                        palette.dim,
                    );
                    if theme::icon_button(
                        ui,
                        Icon::X,
                        14.0,
                        palette.dim,
                        palette.text,
                        &gettext(locale, "Cancel"),
                    )
                    .clicked()
                    {
                        actions.push(account_op(Op::CancelAdd {
                            group: group.id.clone(),
                            spotify: spotify.clone(),
                        }));
                    }
                });
            }
            if group.role.manages() {
                ui.add_space(8.0);
                add_member(app, ui, &group.id, actions);
            }
        });
        ui.add_space(12.0);
        if group.may_invite() {
            invite_card(app, ui, group, actions);
            ui.add_space(12.0);
        }
        group_danger(app, ui, group, actions);
        ui.add_space(12.0);
    }
    account_card(app, ui, &account, actions);
    ui.add_space(12.0);
    card(ui, &palette, |ui| {
        heading(ui, gettext(locale, "Another group"), &palette);
        ui.add_space(8.0);
        new_or_join(app, ui, actions);
    });
    error_line(app, ui);
}

fn member_row(
    app: &App,
    ui: &mut Ui,
    group: &account::Group,
    member: &Member,
    me: &str,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    let locale = app.locale;
    let open_id = Id::new(("group-member-open", &group.id, &member.user_id));
    let open = ui
        .data(|data| data.get_temp::<bool>(open_id))
        .unwrap_or(false);
    let itself = member.user_id == me;
    // An admin may manage members; only the owner may touch an admin.
    let editable = !itself
        && member.role != Role::Owner
        && (group.role == Role::Owner
            || (group.role == Role::Admin && member.role == Role::Member));
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(28.0), egui::Sense::hover());
        super::group::avatar(
            ui,
            &palette,
            rect.center(),
            28.0,
            &member.display_name,
            false,
        );
        ui.vertical(|ui| {
            theme::text(
                ui,
                if itself {
                    format!("{} ({})", member.display_name, gettext(locale, "you"))
                } else {
                    member.display_name.clone()
                },
                theme::medium(13.5),
                palette.text,
            );
            let mut detail = role_label(app, member.role);
            if let Some(soundcloud) = &member.soundcloud {
                detail.push_str(&format!(" · SoundCloud: {soundcloud}"));
            }
            theme::text(ui, detail, theme::regular(12.0), palette.dim);
        });
        if editable {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::soft_button(
                    ui,
                    &palette,
                    None,
                    &if open {
                        gettext(locale, "Done")
                    } else {
                        gettext(locale, "Manage")
                    },
                    false,
                )
                .clicked()
                {
                    ui.data_mut(|data| data.insert_temp(open_id, !open));
                }
            });
        }
    });
    if !(open && editable) {
        return;
    }
    let mut rights: Rights = member.rights;
    ui.indent(open_id.with("rights"), |ui| {
        let mut changed = false;
        changed |= super::widgets::switch_line(ui, &palette, &gettext(locale, "Can share songs and playlists"), &mut rights.share).changed();
        changed |= super::widgets::switch_line(ui, &palette, &gettext(locale, "Can host parties"), &mut rights.host).changed();
        changed |= super::widgets::switch_line(ui, &palette, &gettext(locale, "Can add to a party's queue"), &mut rights.queue).changed();
        changed |= super::widgets::switch_line(ui, &palette, &gettext(locale, "Can make invite codes"), &mut rights.invite).changed();
        if changed {
            actions.push(account_op(Op::SetRights {
                group: group.id.clone(),
                user: member.user_id.clone(),
                rights,
            }));
        }
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if group.role == Role::Owner {
                let admin = member.role == Role::Admin;
                let label = if admin { gettext(locale, "Remove admin") } else { gettext(locale, "Make admin") };
                if theme::soft_button(ui, &palette, None, &label, false).clicked() {
                    actions.push(account_op(Op::SetAdmin {
                        group: group.id.clone(),
                        user: member.user_id.clone(),
                        admin: !admin,
                    }));
                }
                if theme::soft_button(ui, &palette, None, &gettext(locale, "Make owner"), false)
                    .on_hover_text(gettext(locale, "They run the group from now on, and you become an admin"))
                    .clicked()
                {
                    actions.push(Action::ShowDialog(crate::model::Dialog::Confirm {
                        title: gettext(locale, "Hand over this group?").into_owned(),
                        message: format!(
                            "{} {}",
                            member.display_name,
                            gettext(locale, "will run the group from now on, and you become an admin.")
                        ),
                        confirm: gettext(locale, "Make owner").into_owned(),
                        action: Box::new(account_op(Op::HandOver {
                            group: group.id.clone(),
                            user: member.user_id.clone(),
                        })),
                    }));
                }
            }
            if theme::soft_button(ui, &palette, None, &gettext(locale, "Remove from group"), false).clicked() {
                actions.push(Action::ShowDialog(crate::model::Dialog::Confirm {
                    title: gettext(locale, "Remove from the group?").into_owned(),
                    message: format!(
                        "{} {}",
                        member.display_name,
                        gettext(locale, "loses access now, and old invite codes won't bring them back. What they shared stays.")
                    ),
                    confirm: gettext(locale, "Remove").into_owned(),
                    action: Box::new(account_op(Op::RemoveMember {
                        group: group.id.clone(),
                        user: member.user_id.clone(),
                    })),
                }));
            }
        });
    });
}

fn add_member(app: &App, ui: &mut Ui, group: &str, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    let id = Id::new(("group-add-member", group));
    let mut text = ui
        .data(|data| data.get_temp::<String>(id))
        .unwrap_or_default();
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 90.0).max(120.0);
        ui.allocate_ui(vec2(width, 36.0), |ui| {
            field(
                ui,
                &palette,
                &mut text,
                &gettext(locale, "Add by Spotify profile link or username"),
                false,
                300,
            );
        });
        ui.add_enabled_ui(!text.trim().is_empty(), |ui| {
            if theme::soft_button(
                ui,
                &palette,
                Some(Icon::Plus),
                &gettext(locale, "Add"),
                false,
            )
            .clicked()
            {
                actions.push(account_op(Op::AddMember {
                    group: group.to_owned(),
                    spotify: std::mem::take(&mut text).trim().to_owned(),
                }));
            }
        });
    });
    ui.data_mut(|data| data.insert_temp(id, text));
}

fn invite_card(app: &App, ui: &mut Ui, group: &account::Group, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    card(ui, &palette, |ui| {
        heading(ui, gettext(locale, "Invite code"), &palette);
        ui.add_space(6.0);
        match &group.invite {
            Some(code) => {
                ui.horizontal(|ui| {
                    theme::text(
                        ui,
                        app.private_text(code),
                        theme::medium(14.0),
                        palette.text,
                    );
                    if theme::soft_button(
                        ui,
                        &palette,
                        Some(Icon::Copy),
                        &gettext(locale, "Copy"),
                        false,
                    )
                    .clicked()
                    {
                        ui.ctx().copy_text(code.clone());
                        actions.push(Action::ShowToast(gettext(locale, "Copied").into_owned()));
                    }
                });
            }
            None => {
                theme::text(
                    ui,
                    gettext(locale, "Nobody can join with a code right now"),
                    theme::regular(13.0),
                    palette.dim,
                );
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if theme::soft_button(ui, &palette, None, &gettext(locale, "New code"), false)
                .on_hover_text(gettext(locale, "The old code stops working"))
                .clicked()
            {
                actions.push(account_op(Op::NewInvite(group.id.clone())));
            }
            if group.role.manages()
                && group.invite.is_some()
                && theme::soft_button(ui, &palette, None, &gettext(locale, "Turn off"), false)
                    .clicked()
            {
                actions.push(account_op(Op::TurnOffInvite(group.id.clone())));
            }
        });
    });
}

fn group_danger(app: &App, ui: &mut Ui, group: &account::Group, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    let id = Id::new(("group-rename-group", &group.id));
    let mut name = ui
        .data(|data| data.get_temp::<String>(id))
        .unwrap_or_else(|| group.name.clone());
    card(ui, &palette, |ui| {
        if group.role.manages() {
            ui.horizontal(|ui| {
                ui.allocate_ui(vec2(260.0, 36.0), |ui| {
                    field(
                        ui,
                        &palette,
                        &mut name,
                        &gettext(locale, "Group name"),
                        false,
                        60,
                    );
                });
                let changed = !name.trim().is_empty() && name.trim() != group.name;
                ui.add_enabled_ui(changed, |ui| {
                    if theme::soft_button(ui, &palette, None, &gettext(locale, "Rename"), false)
                        .clicked()
                    {
                        actions.push(account_op(Op::RenameGroup {
                            group: group.id.clone(),
                            name: name.trim().to_owned(),
                        }));
                    }
                });
            });
            ui.add_space(8.0);
        }
        ui.horizontal(|ui| {
            if theme::soft_button(
                ui,
                &palette,
                Some(Icon::LogOut),
                &gettext(locale, "Leave this group"),
                false,
            )
            .clicked()
            {
                actions.push(Action::ShowDialog(crate::model::Dialog::Confirm {
                    title: gettext(locale, "Leave this group?").into_owned(),
                    message: gettext(
                        locale,
                        "You'll need a new invite to come back. What you shared stays.",
                    )
                    .into_owned(),
                    confirm: gettext(locale, "Leave").into_owned(),
                    action: Box::new(account_op(Op::LeaveGroup(group.id.clone()))),
                }));
            }
            if group.role == Role::Owner
                && theme::soft_button(
                    ui,
                    &palette,
                    Some(Icon::Trash),
                    &gettext(locale, "Delete group"),
                    false,
                )
                .clicked()
            {
                actions.push(Action::ShowDialog(crate::model::Dialog::Confirm {
                    title: gettext(locale, "Delete this group?").into_owned(),
                    message: gettext(
                        locale,
                        "Everything shared in it goes too, for everyone. This can't be undone.",
                    )
                    .into_owned(),
                    confirm: gettext(locale, "Delete").into_owned(),
                    action: Box::new(account_op(Op::DeleteGroup(group.id.clone()))),
                }));
            }
        });
    });
    ui.data_mut(|data| data.insert_temp(id, name));
}

/// You: your name, linked accounts, what is shared, and the computers signed in.
fn account_card(app: &App, ui: &mut Ui, account: &Account, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    let name_id = Id::new("group-rename");
    let mut name = ui
        .data(|data| data.get_temp::<String>(name_id))
        .unwrap_or_else(|| account.display_name.clone());
    card(ui, &palette, |ui| {
        heading(ui, gettext(locale, "Your account"), &palette);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.allocate_ui(vec2(260.0, 36.0), |ui| {
                field(
                    ui,
                    &palette,
                    &mut name,
                    &gettext(locale, "Your name"),
                    app.settings.streamer_mode,
                    40,
                );
            });
            let changed = !name.trim().is_empty() && name.trim() != account.display_name;
            ui.add_enabled_ui(changed, |ui| {
                if theme::soft_button(ui, &palette, None, &gettext(locale, "Rename"), false)
                    .clicked()
                {
                    actions.push(Action::Friends(Group::Rename(name.trim().to_owned())));
                }
            });
        });
        ui.add_space(10.0);
        // Spotify.
        ui.horizontal(|ui| match &account.spotify_id {
            Some(id) => {
                theme::text(
                    ui,
                    format!("Spotify: {}", app.private_text(id)),
                    theme::regular(13.0),
                    palette.secondary,
                );
                if theme::soft_button(ui, &palette, None, &gettext(locale, "Unlink"), false)
                    .on_hover_text(gettext(
                        locale,
                        "New computers won't find your groups until you connect again",
                    ))
                    .clicked()
                {
                    actions.push(account_op(Op::UnlinkSpotify));
                }
            }
            None => {
                theme::text(
                    ui,
                    gettext(locale, "Spotify isn't linked"),
                    theme::regular(13.0),
                    palette.dim,
                );
                if theme::soft_button(ui, &palette, None, &gettext(locale, "Connect"), false)
                    .clicked()
                {
                    actions.push(Action::Friends(Group::ConnectSpotify));
                }
            }
        });
        ui.add_space(4.0);
        // SoundCloud: only a label for friends and for this person's other
        // computers. SoundCloud's own sign-in never leaves the app.
        let here = app.soundcloud_account.user();
        ui.horizontal(|ui| match (&account.soundcloud, here) {
            (Some(link), _) => {
                theme::text(
                    ui,
                    format!("SoundCloud: {}", app.private_text(&link.username)),
                    theme::regular(13.0),
                    palette.secondary,
                );
                if theme::soft_button(ui, &palette, None, &gettext(locale, "Unlink"), false)
                    .clicked()
                {
                    actions.push(account_op(Op::UnlinkSoundCloud));
                }
            }
            (None, Some(user)) => {
                theme::text(
                    ui,
                    gettext(locale, "SoundCloud isn't linked"),
                    theme::regular(13.0),
                    palette.dim,
                );
                if theme::soft_button(
                    ui,
                    &palette,
                    None,
                    &gettext(locale, "Link SoundCloud"),
                    false,
                )
                .clicked()
                {
                    actions.push(account_op(Op::LinkSoundCloud(SoundCloudLink {
                        id: user.user_id.to_string(),
                        username: user.username.clone(),
                    })));
                }
            }
            (None, None) => {
                theme::text(
                    ui,
                    gettext(locale, "Connect SoundCloud in Settings to link it"),
                    theme::regular(13.0),
                    palette.dim,
                );
            }
        });
        ui.add_space(10.0);
        let mut sharing = app.settings.group_sharing == Some(true);
        if super::widgets::switch_line(
            ui,
            &palette,
            &gettext(locale, "Share what I'm playing with my groups"),
            &mut sharing,
        )
        .changed()
        {
            actions.push(Action::Live(crate::friends::live::Action::SetSharing(
                sharing,
            )));
        }
        ui.add_space(10.0);
        if theme::soft_button(
            ui,
            &palette,
            None,
            &gettext(locale, "Forget this computer"),
            false,
        )
        .clicked()
        {
            actions.push(Action::Friends(Group::Leave));
        }
    });
    ui.data_mut(|data| data.insert_temp(name_id, name));
}
