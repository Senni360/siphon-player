//! Minimal controls inside the existing Friends panel. The redesign can reuse
//! the model and actions without retaining this view.

use super::{Action, Model, NewPost, Source};
use crate::model::Action as AppAction;

#[derive(Clone, Default)]
struct Draft {
    invite: String,
    name: String,
    url: String,
    title: String,
    note: String,
    source: usize,
}

pub fn draw(ui: &mut egui::Ui, model: &Model) -> Vec<AppAction> {
    let mut actions = Vec::new();
    let id = ui.make_persistent_id("siphon-friends-draft");
    let mut draft = ui.data_mut(|d| d.get_temp::<Draft>(id)).unwrap_or_default();
    ui.heading("Group feed");
    if let Some(error) = &model.error {
        ui.label(error);
    }
    ui.add_enabled_ui(!model.busy, |ui| {
        if let Some(profile) = &model.profile {
            ui.label(format!("Joined as {}", profile.display_name));
            ui.collapsing("Your group identity", |ui| {
                ui.label(format!("ID: {}", profile.user_id));
                if draft.name.is_empty() {
                    draft.name.clone_from(&profile.display_name);
                }
                ui.add(
                    egui::TextEdit::singleline(&mut draft.name)
                        .hint_text("Display name")
                        .char_limit(40),
                );
                if ui.button("Rename").clicked() {
                    actions.push(AppAction::Friends(Action::Rename(draft.name.trim().into())));
                }
                ui.label("Leave removes this device's saved identity. Keep your secret first.");
                if ui.button("Leave group on this device").clicked() {
                    actions.push(AppAction::Friends(Action::Leave));
                    draft = Draft::default();
                }
            });
            if let Some(secret) = &model.recovery_secret {
                ui.label("Your secret is shown once. Keep it private and save it somewhere safe.");
                let mut shown = secret.clone();
                ui.add(
                    egui::TextEdit::singleline(&mut shown)
                        .password(true)
                        .interactive(false),
                );
                if ui.button("Copy secret").clicked() {
                    ui.ctx().copy_text(secret.clone());
                }
                if ui.button("I saved my secret").clicked() {
                    actions.push(AppAction::Friends(Action::DismissSecret));
                }
            }
            ui.collapsing("Share a link", |ui| {
                egui::ComboBox::from_id_salt("friends-source")
                    .selected_text(["Spotify", "SoundCloud", "YouTube"][draft.source])
                    .show_ui(ui, |ui| {
                        for (index, label) in
                            ["Spotify", "SoundCloud", "YouTube"].iter().enumerate()
                        {
                            ui.selectable_value(&mut draft.source, index, *label);
                        }
                    });
                ui.add(
                    egui::TextEdit::singleline(&mut draft.url)
                        .hint_text("Song, album, playlist or set URL"),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut draft.title)
                        .hint_text("Title")
                        .char_limit(300),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut draft.note)
                        .hint_text("Note (optional)")
                        .char_limit(280),
                );
                if ui
                    .add_enabled(
                        !draft.url.trim().is_empty() && !draft.title.trim().is_empty(),
                        egui::Button::new("Share with friends"),
                    )
                    .clicked()
                {
                    let source =
                        [Source::Spotify, Source::Soundcloud, Source::Youtube][draft.source];
                    let mut post = NewPost::from_url(
                        draft.url.trim().into(),
                        source,
                        draft.title.trim().into(),
                    );
                    post.note = draft.note.trim().into();
                    actions.push(AppAction::Friends(Action::Share(post)));
                    draft.url.clear();
                    draft.title.clear();
                    draft.note.clear();
                }
            });
            if ui.button("Refresh group feed").clicked() {
                actions.push(AppAction::Friends(Action::Refresh));
            }
            if model.posts.is_empty() {
                ui.label("Share the first find with your friends.");
            }
            for post in &model.posts {
                ui.separator();
                ui.label(format!("{} shared", post.author_name));
                if ui.link(&post.item.title).clicked() {
                    // Link parsing stays centralized. YouTube will be handled by
                    // the source worker; until then its allowlisted web link opens.
                    let action = crate::link::parse(&post.item.url).map_or_else(
                        || AppAction::OpenUrl(post.item.url.clone()),
                        AppAction::OpenLink,
                    );
                    actions.push(action);
                }
                if !post.item.artists.is_empty() {
                    ui.label(post.item.artists.join(", "));
                }
                if !post.item.note.is_empty() {
                    ui.label(&post.item.note);
                }
                ui.horizontal(|ui| {
                    let heart = if post.liked { "♥" } else { "♡" };
                    if ui
                        .add_enabled(
                            post.id != "pending-share",
                            egui::Button::new(format!("{heart} {}", post.like_count)),
                        )
                        .clicked()
                    {
                        actions.push(AppAction::Friends(Action::Like {
                            id: post.id.clone(),
                            liked: !post.liked,
                        }));
                    }
                    if post.author_id == profile.user_id
                        && ui
                            .add_enabled(post.id != "pending-share", egui::Button::new("Delete"))
                            .clicked()
                    {
                        actions.push(AppAction::Friends(Action::Delete(post.id.clone())));
                    }
                });
            }
            if model.next_cursor.is_some() && ui.button("Older posts").clicked() {
                actions.push(AppAction::Friends(Action::More));
            }
        } else {
            ui.label("Join your friends' private group to share music from all three sources.");
            ui.add(
                egui::TextEdit::singleline(&mut draft.invite)
                    .hint_text("Invite code")
                    .password(true)
                    .char_limit(256),
            );
            ui.add(
                egui::TextEdit::singleline(&mut draft.name)
                    .hint_text("Display name")
                    .char_limit(40),
            );
            if ui
                .add_enabled(
                    !draft.invite.is_empty() && !draft.name.trim().is_empty(),
                    egui::Button::new("Join group"),
                )
                .clicked()
            {
                actions.push(AppAction::Friends(Action::Join {
                    invite_code: std::mem::take(&mut draft.invite),
                    display_name: draft.name.trim().into(),
                }));
            }
            if model.error.is_some() {
                if ui.button("Retry saved identity").clicked() {
                    actions.push(AppAction::Friends(Action::Open));
                }
                if ui.button("Forget saved identity").clicked() {
                    actions.push(AppAction::Friends(Action::Leave));
                }
            }
        }
    });
    if model.busy {
        ui.spinner();
    }
    ui.data_mut(|d| d.insert_temp(id, draft));
    actions
}
