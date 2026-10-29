//! Modal dialogs: playlist details, confirmations, shortcuts.

use egui::{Align, CornerRadius, Frame, Layout, Margin, Stroke};

use crate::app::App;
use crate::i18n::{Locale, gettext, ngettext};
use crate::model::{Action, Dialog};
use crate::theme;

fn soundcloud_connect(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let locale = app.locale;
    let connected_name = app
        .soundcloud_account
        .user()
        .map(|user| user.username.clone());
    let Some(Dialog::SoundCloudConnect(draft)) = &mut app.dialog else {
        return;
    };
    theme::text(
        ui,
        gettext(locale, "Connect SoundCloud"),
        theme::bold(20.0),
        palette.text,
    );
    ui.add_space(12.0);
    if let Some(name) = connected_name {
        ui.label(gettext(locale, "Connected as {name}").replace("{name}", &name));
        if theme::pill_button(
            ui,
            &palette,
            &gettext(locale, "Disconnect SoundCloud"),
            false,
        )
        .clicked()
        {
            app.actions.push(Action::DisconnectSoundCloud);
        }
        ui.add_space(12.0);
    }
    ui.add_enabled_ui(!draft.busy, |ui| {
        theme::text(ui, gettext(locale, "Profile URL or username"), theme::medium(13.0), palette.secondary);
        let profile = text_field(ui, &palette, locale, "soundcloud-profile", &mut draft.profile, "https://soundcloud.com/username", !draft.advanced);
        ui.label(egui::RichText::new(gettext(locale, "Add public likes, playlists, reposts and followings. No sign-in needed.")).color(palette.secondary));
        ui.add_space(12.0);
        let advanced = egui::CollapsingHeader::new(gettext(locale, "Advanced: use a session token")).id_salt("soundcloud-advanced").open(Some(draft.advanced)).show(ui, |ui| {
            ui.label(gettext(locale, "1. Sign in at soundcloud.com in your browser."));
            ui.label(gettext(locale, "2. Open DevTools (F12), then Application > Cookies > https://soundcloud.com. In Firefox, use Storage > Cookies."));
            ui.label(gettext(locale, "3. Copy the value of oauth_token and paste it below."));
            ui.label(egui::RichText::new(gettext(locale, "This token can act as your SoundCloud account. Treat it like a password. It is saved in your computer's protected credential store.")).color(palette.secondary));
            let token = Frame::new().fill(palette.surface).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                super::widgets::text_edit(ui, locale, egui::TextEdit::singleline(&mut draft.token).id(crate::soundcloud::connect::token_field_id()).password(true).font(theme::regular(14.0)).frame(egui::Frame::NONE).hint_text(egui::RichText::new(gettext(locale, "Session token")).color(palette.dim)).desired_width(f32::INFINITY))
            }).inner;
            if ui.memory(|memory| memory.focused().is_none()) { token.request_focus(); }
            token
        });
        if advanced.header_response.clicked() {
            draft.advanced = !draft.advanced;
            ui.memory_mut(|memory| {
                if draft.advanced { memory.request_focus(crate::soundcloud::connect::token_field_id()); }
                else { memory.surrender_focus(crate::soundcloud::connect::token_field_id()); }
            });
        }
        if let Some(error) = &draft.error {
            ui.label(error);
        }
        ui.add_space(16.0);
        let submit = (profile.lost_focus() || advanced.body_returned.as_ref().is_some_and(|response| response.lost_focus())) && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let label = if draft.advanced { gettext(locale, "Connect session") } else { gettext(locale, "Connect public profile") };
        if theme::pill_button(ui, &palette, &label, true).clicked() || submit {
            app.actions.push(Action::ConnectSoundCloud);
        }
    });
    if draft.busy {
        super::widgets::loading_row(ui, &palette, locale);
    }
    ui.add_space(8.0);
    if theme::pill_button(
        ui,
        &palette,
        &gettext(locale, "Continue without connecting"),
        false,
    )
    .clicked()
    {
        app.actions.push(Action::CloseDialog);
    }
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(dialog) = app.dialog.clone() else {
        return;
    };
    if let Dialog::CoverArt { url, shown, title } = &dialog {
        cover_art(app, ctx, url, shown.as_deref(), title);
        return;
    }
    let palette = app.palette;
    let locale = app.locale;
    let mut playlist_link_selected = false;
    let frame = Frame::new()
        .fill(palette.overlay)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(theme::RADIUS + 4))
        .inner_margin(Margin::same(24))
        .shadow(egui::epaint::Shadow {
            offset: [0, 10],
            blur: 40,
            spread: 0,
            color: palette.shadow,
        });
    let response = egui::Modal::new(egui::Id::new("dialog"))
        .frame(frame)
        .backdrop_color(egui::Color32::from_black_alpha(if palette.dark {
            150
        } else {
            80
        }))
        .show(ctx, |ui| {
            ui.set_width(420.0);
            match dialog {
                Dialog::AddToPlaylist { request, name, items, source, mut query, loading } => {
                    theme::text(ui, gettext(locale, "Add to playlist"), theme::bold(20.0), palette.text);
                    ui.add_space(8.0);
                    ui.label(&name);
                    ui.add_space(8.0);
                    if loading {
                        super::widgets::loading_row(ui, &palette, locale);
                    } else {
                        let before = app.actions.len();
                        let field = super::widgets::playlist_picker_source(ui, app, &items, &mut query, source.as_deref());
                        if ui.memory(|memory| memory.focused().is_none()) {
                            field.request_focus();
                        }
                        if app.actions.len() > before {
                            playlist_link_selected = true;
                            // Apply closure before the selection. Duplicate confirmation
                            // or a create dialog can then replace this picker normally.
                            let adding = matches!(app.actions[before], Action::AddToPlaylist { .. } | Action::AddSoundCloudPlaylist { .. });
                            app.actions.insert(before, if adding { Action::PlaylistLinkChosen } else { Action::CloseDialog });
                        } else if let Some(Dialog::AddToPlaylist { request: current, query: stored, .. }) = &mut app.dialog
                            && *current == request
                        {
                            *stored = query;
                        }
                    }
                    ui.add_space(8.0);
                    if theme::pill_button(ui, &palette, &gettext(locale, "Cancel"), false).clicked() {
                        app.actions.push(Action::CloseDialog);
                    }
                }
                // Drawn by cover_art above.
                Dialog::CoverArt { .. } => {}
                Dialog::SoundCloudConnect(_) => {
                    let height = (ctx.viewport_rect().height() - 96.0).max(120.0);
                    // A modal's initial area is small. Let this scroller use
                    // the window height before it decides to clip its buttons.
                    ui.set_max_height(height);
                    egui::ScrollArea::vertical().max_height(height)
                        .show(ui, |ui| soundcloud_connect(app, ui));
                }
                Dialog::PersonalAppIntro => {
                    theme::text(ui, gettext(locale, "Move to your own Spotify app"), theme::bold(20.0), palette.text);
                    ui.add_space(12.0);
                    ui.add(egui::Label::new(egui::RichText::new(gettext(locale, "You're signed in through the shared app Siphon used before. Siphon no longer offers it, and Spotify can stop accepting it at any time. Set up your own free Spotify app so nothing stops your music. It takes a few minutes.")).font(theme::regular(14.0)).color(palette.secondary)).wrap());
                    ui.add_space(10.0);
                    ui.add_space(8.0);
                    buttons(ui, |ui| {
                        if theme::pill_button(ui, &palette, &gettext(locale, "Set up personal app"), true).clicked() {
                            app.actions.push(Action::OpenPersonalAppSetup);
                        }
                        if theme::pill_button(ui, &palette, &gettext(locale, "Not now"), false).clicked() {
                            app.actions.push(Action::CloseDialog);
                        }
                    });
                }
                Dialog::CreatePlaylist { .. } => create_playlist(app, ui),
                Dialog::MixedPlaylist { .. } => mixed_playlist(app, ui),
                Dialog::EditPlaylist { .. } => edit_playlist(app, ui),
                Dialog::ConfirmDeletePlaylist { id, name, owned } => {
                    theme::text(
                        ui,
                        if owned {
                            gettext(locale, "Delete playlist?")
                        } else {
                            gettext(locale, "Remove from Your Library?")
                        },
                        theme::bold(20.0),
                        palette.text,
                    );
                    ui.add_space(8.0);
                    let body = if id.starts_with(crate::mixed_playlists::PLAYLIST_PREFIX) {
                        gettext(locale, "Delete “{name}” from this computer? This cannot be undone.")
                    } else if owned {
                        // Translators: {name} is a playlist name.
                        gettext(locale, "Delete “{name}”? You can recover it from Spotify for 90 days.")
                    } else {
                        // Translators: {name} is a playlist name.
                        gettext(locale, "“{name}” will no longer appear in Your Library.")
                    }
                    .replace("{name}", &name);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(body)
                                .font(theme::regular(14.0))
                                .color(palette.secondary),
                        )
                        .wrap(),
                    );
                    ui.add_space(20.0);
                    buttons(ui, |ui| {
                        if theme::pill_button(
                            ui,
                            &palette,
                            &if owned {
                                gettext(locale, "Delete")
                            } else {
                                gettext(locale, "Remove")
                            },
                            true,
                        )
                        .clicked()
                        {
                            app.actions.push(Action::DeletePlaylist(id.clone()));
                        }
                        if theme::pill_button(ui, &palette, &gettext(locale, "Cancel"), false).clicked() {
                            app.actions.push(Action::CloseDialog);
                        }
                    });
                }
                Dialog::ConfirmPlaylistDuplicates {
                    playlist_id,
                    playlist_name,
                    items,
                    position,
                    duplicate_uris,
                } => {
                    let multiple = items.len() > 1;
                    theme::text(
                        ui,
                        if multiple {
                            gettext(locale, "Songs already in this playlist")
                        } else {
                            gettext(locale, "Song already in this playlist")
                        },
                        theme::bold(20.0),
                        palette.text,
                    );
                    ui.add_space(8.0);
                    let body = duplicate_message(locale, &playlist_name, &items, &duplicate_uris);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(body)
                                .font(theme::regular(14.0))
                                .color(palette.secondary),
                        )
                        .wrap(),
                    );
                    ui.add_space(20.0);
                    buttons(ui, |ui| {
                        if theme::pill_button(ui, &palette, &gettext(locale, "Add anyway"), true).clicked() {
                            app.actions.push(Action::ConfirmAddToPlaylist {
                                playlist_id: playlist_id.clone(),
                                playlist_name: playlist_name.clone(),
                                items: items.clone(),
                                position,
                            });
                        }
                        if theme::pill_button(ui, &palette, &gettext(locale, "Cancel"), false).clicked() {
                            app.actions.push(Action::CloseDialog);
                        }
                    });
                }
                Dialog::Shortcuts => {
                    theme::text(ui, gettext(locale, "Keyboard shortcuts"), theme::bold(20.0), palette.text);
                    ui.add_space(12.0);
                    // `theme::text` truncates, which in a grid makes each cell
                    // claim almost no width and turns "Ctrl+Shift+A" into
                    // "Ctrl…". A shortcut is unusable when abbreviated, so
                    // these cells are sized to their content.
                    let cell = |ui: &mut egui::Ui, text: &str, font: egui::FontId, color| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(text).font(font).color(color))
                                .extend()
                                .selectable(false),
                        );
                    };
                    // The list is longer than a small screen is tall, so it
                    // scrolls inside the dialog rather than running off the
                    // bottom with the Done button beyond reach.
                    let room = ui.ctx().content_rect().height() - 190.0;
                    crate::autoscroll::show(
                        ui,
                        egui::ScrollArea::vertical()
                        .max_height(room.max(120.0))
                        .auto_shrink([false, true]),
                        egui::Vec2b::new(false, true),
                        |ui| {
                            egui::Grid::new("shortcuts")
                                .num_columns(2)
                                .spacing([24.0, 8.0])
                                .show(ui, |ui| {
                                    for (keys, description) in super::keys::shortcuts(locale) {
                                        cell(ui, &keys, theme::semibold(13.0), palette.text);
                                        cell(
                                            ui,
                                            &description,
                                            theme::regular(13.5),
                                            palette.secondary,
                                        );
                                        ui.end_row();
                                    }
                                });
                        },
                    );
                    ui.add_space(16.0);
                    buttons(ui, |ui| {
                        if theme::pill_button(ui, &palette, &gettext(locale, "Done"), true).clicked() {
                            app.actions.push(Action::CloseDialog);
                        }
                    });
                }
                Dialog::PremiumNeeded => {
                    theme::text(
                        ui,
                        gettext(locale, "This account cannot play music here"),
                        theme::bold(20.0),
                        palette.text,
                    );
                    ui.add_space(8.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(gettext(
                                locale,
                                "Playback needs Spotify Premium. Free accounts can browse and search, but cannot play music through Siphon.",
                            ))
                            .font(theme::regular(14.0))
                            .color(palette.secondary),
                        )
                        .wrap(),
                    );
                    ui.add_space(20.0);
                    buttons(ui, |ui| {
                        if theme::pill_button(ui, &palette, &gettext(locale, "OK"), true).clicked() {
                            app.actions.push(Action::CloseDialog);
                        }
                    });
                }
            }
        });
    app.dialog_rect = Some(response.response.rect);
    if response.should_close() && !playlist_link_selected {
        app.actions.push(Action::CloseDialog);
    }
}

// Keep button rows content-sized when the shared modal area last held a taller dialog.
fn buttons(ui: &mut egui::Ui, add_buttons: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), ui.spacing().interact_size.y),
        Layout::right_to_left(Align::Center),
        add_buttons,
    );
}

fn duplicate_message(
    locale: Locale,
    playlist_name: &str,
    items: &[crate::api::models::PlayableItem],
    duplicate_uris: &[String],
) -> String {
    let mut seen = std::collections::HashSet::new();
    let names: Vec<&str> = items
        .iter()
        .filter(|item| duplicate_uris.iter().any(|uri| uri == item.uri()))
        .map(crate::api::models::PlayableItem::name)
        .filter(|name| seen.insert(*name))
        .collect();
    let one_item = items.len() == 1;
    let message = match names.as_slice() {
        [] if one_item => {
            // Translators: {playlist} is a playlist name.
            gettext(
                locale,
                "This song is already in “{playlist}”. Add it again?",
            )
        }
        [] => {
            // Translators: {playlist} is a playlist name.
            gettext(
                locale,
                "This song is already in “{playlist}”. Add them anyway?",
            )
        }
        [name] if one_item => {
            // Translators: {name} is a song name and {playlist} a playlist name.
            gettext(locale, "“{name}” is already in “{playlist}”. Add it again?")
                .replace("{name}", name)
                .into()
        }
        [name] => {
            // Translators: {name} is a song name and {playlist} a playlist name.
            gettext(
                locale,
                "“{name}” is already in “{playlist}”. Add them anyway?",
            )
            .replace("{name}", name)
            .into()
        }
        [first, second] => {
            // Translators: {first} and {second} are song names, {playlist} a playlist name.
            gettext(
                locale,
                "“{first}” and “{second}” are already in “{playlist}”. Add them anyway?",
            )
            .replace("{first}", first)
            .replace("{second}", second)
            .into()
        }
        [first, second, rest @ ..] => ngettext(
            locale,
            // Translators: {first} and {second} are song names, {count} is how many more songs are already in the playlist, and {playlist} is a playlist name.
            "“{first}”, “{second}”, and {count} more are already in “{playlist}”. Add them anyway?",
            "“{first}”, “{second}”, and {count} more are already in “{playlist}”. Add them anyway?",
            rest.len() as u32,
        )
        .replace("{first}", first)
        .replace("{second}", second)
        .replace("{count}", &rest.len().to_string())
        .into(),
    };
    message.replace("{playlist}", playlist_name)
}

fn text_field(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    locale: crate::i18n::Locale,
    id: &str,
    text: &mut String,
    hint: &str,
    focus: bool,
) -> egui::Response {
    let response = Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            super::widgets::text_edit(
                ui,
                locale,
                egui::TextEdit::singleline(text)
                    .id(egui::Id::new(id))
                    .hint_text(egui::RichText::new(hint).color(palette.dim))
                    .font(theme::regular(14.0))
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY),
            )
        })
        .inner;
    if focus && ui.memory(|memory| memory.focused().is_none()) {
        response.request_focus();
    }
    response
}

fn create_playlist(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let locale = app.locale;
    let busy = app.playlist_busy;
    let Some(Dialog::CreatePlaylist {
        name,
        public,
        add_uris,
    }) = &mut app.dialog
    else {
        return;
    };
    theme::text(
        ui,
        gettext(locale, "New playlist"),
        theme::bold(20.0),
        palette.text,
    );
    ui.add_space(12.0);
    theme::text(
        ui,
        gettext(locale, "Name"),
        theme::medium(13.0),
        palette.secondary,
    );
    let field = text_field(
        ui,
        &palette,
        locale,
        "playlist-name",
        name,
        &gettext(locale, "My playlist"),
        true,
    );
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        super::widgets::switch(ui, &palette, &gettext(locale, "Public playlist"), public);
        theme::text(
            ui,
            gettext(locale, "Public playlist"),
            theme::regular(14.0),
            palette.text,
        );
    });
    if !add_uris.is_empty() {
        ui.add_space(6.0);
        let count = add_uris.len();
        theme::text(
            ui,
            ngettext(
                locale,
                // Translators: {count} is the number of songs.
                "{count} song will be added.",
                "{count} songs will be added.",
                count as u32,
            )
            .replace("{count}", &count.to_string()),
            theme::regular(13.0),
            palette.secondary,
        );
    }
    ui.add_space(20.0);
    let submit = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
    let name_value = name.trim().to_string();
    let public_value = *public;
    let uris = add_uris.clone();
    buttons(ui, |ui| {
        if busy {
            theme::spinner(ui, 18.0, palette.accent);
        } else {
            let create = theme::pill_button(ui, &palette, &gettext(locale, "Create"), true)
                .clicked()
                || submit;
            if create && !name_value.is_empty() {
                app.actions.push(Action::CreatePlaylist {
                    name: name_value.clone(),
                    public: public_value,
                    add_uris: uris.clone(),
                });
            }
            if theme::pill_button(ui, &palette, &gettext(locale, "Cancel"), false).clicked() {
                app.actions.push(Action::CloseDialog);
            }
        }
    });
}

fn edit_playlist(app: &mut App, ui: &mut egui::Ui) {
    let existing_image = if let Some(Dialog::EditPlaylist { id, .. }) = &app.dialog {
        app.library
            .playlists
            .get()
            .and_then(|playlists| playlists.iter().find(|playlist| &playlist.id == id))
            .and_then(|playlist| crate::api::models::pick_image(&playlist.images, 100))
            .map(str::to_owned)
    } else {
        None
    };
    let palette = app.palette;
    let locale = app.locale;
    let busy = app.playlist_busy;
    let Some(Dialog::EditPlaylist {
        cover,
        id,
        name,
        description,
        public,
    }) = &mut app.dialog
    else {
        return;
    };
    theme::text(
        ui,
        gettext(locale, "Edit details"),
        theme::bold(20.0),
        palette.text,
    );
    ui.add_space(12.0);
    egui::ScrollArea::vertical()
        .max_height((ui.ctx().content_rect().height() - 210.0).max(100.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(selected) = &cover.selection {
                    ui.ctx()
                        .include_bytes(selected.uri.clone(), selected.jpeg.clone());
                    ui.add(
                        egui::Image::new(selected.uri.clone())
                            .texture_options(crate::images::COVER_TEXTURE)
                            .fit_to_exact_size(egui::vec2(72.0, 72.0))
                            .alt_text(gettext(locale, "Selected playlist cover")),
                    );
                } else if let Some(url) = &existing_image {
                    ui.add(
                        egui::Image::new(url)
                            .texture_options(crate::images::COVER_TEXTURE)
                            .fit_to_exact_size(egui::vec2(72.0, 72.0))
                            .alt_text(gettext(locale, "Current playlist cover")),
                    );
                }
                ui.vertical(|ui| {
                    if cover.request.is_some() || cover.uploading.is_some() {
                        theme::spinner(ui, 18.0, palette.accent);
                        theme::text(
                            ui,
                            if cover.uploading.is_some() {
                                gettext(locale, "Uploading cover…")
                            } else {
                                gettext(locale, "Choosing cover…")
                            },
                            theme::regular(13.0),
                            palette.secondary,
                        );
                    } else {
                        if theme::pill_button(ui, &palette, &gettext(locale, "Change cover"), false)
                            .clicked()
                        {
                            app.actions.push(Action::ChoosePlaylistCover(id.clone()));
                        }
                        if cover.selection.is_some()
                            && theme::pill_button(
                                ui,
                                &palette,
                                &gettext(locale, "Upload cover"),
                                true,
                            )
                            .clicked()
                        {
                            app.actions.push(Action::UploadPlaylistCover(id.clone()));
                        }
                    }
                });
            });
            if let Some(error) = &cover.error {
                ui.add(egui::Label::new(egui::RichText::new(error).color(palette.text)).wrap());
            }
            ui.add_space(10.0);
            theme::text(
                ui,
                gettext(locale, "Name"),
                theme::medium(13.0),
                palette.secondary,
            );
            text_field(
                ui,
                &palette,
                locale,
                "edit-name",
                name,
                &gettext(locale, "Playlist name"),
                true,
            );
            ui.add_space(10.0);
            theme::text(
                ui,
                gettext(locale, "Description"),
                theme::medium(13.0),
                palette.secondary,
            );
            Frame::new()
                .fill(palette.surface)
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    super::widgets::text_edit(
                        ui,
                        locale,
                        egui::TextEdit::multiline(description)
                            .id(egui::Id::new("edit-description"))
                            .hint_text(
                                egui::RichText::new(gettext(locale, "Optional description"))
                                    .color(palette.dim),
                            )
                            .font(theme::regular(14.0))
                            .frame(egui::Frame::NONE)
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    );
                });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                // Unknown shows as off; only a change of the switch is sent, so
                // a playlist nothing has described keeps whatever it was.
                let mut shown = public.unwrap_or(false);
                if super::widgets::switch(
                    ui,
                    &palette,
                    &gettext(locale, "Public playlist"),
                    &mut shown,
                )
                .changed()
                {
                    *public = Some(shown);
                }
                theme::text(
                    ui,
                    gettext(locale, "Public playlist"),
                    theme::regular(14.0),
                    palette.text,
                );
            });
        });
    ui.add_space(20.0);
    let id = id.clone();
    let busy = busy || cover.uploading.is_some() || cover.request.is_some();
    let name_value = name.trim().to_string();
    let description_value = description.trim().to_string();
    let public_value = *public;
    buttons(ui, |ui| {
        if busy {
            theme::spinner(ui, 18.0, palette.accent);
        } else {
            if theme::pill_button(ui, &palette, &gettext(locale, "Save"), true).clicked()
                && !name_value.is_empty()
            {
                app.actions.push(Action::UpdatePlaylist {
                    id: id.clone(),
                    name: name_value.clone(),
                    description: description_value.clone(),
                    public: public_value,
                });
            }
            if theme::pill_button(ui, &palette, &gettext(locale, "Cancel"), false).clicked() {
                app.actions.push(Action::CloseDialog);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::duplicate_message;
    use crate::api::models::{PlayableItem, Track};
    use crate::i18n::Locale;

    fn song(uri: &str, name: &str) -> PlayableItem {
        PlayableItem::Track(Track {
            uri: uri.into(),
            name: name.into(),
            ..Default::default()
        })
    }

    #[test]
    fn external_picker_selection_closes_before_adding_without_a_late_close() {
        use crate::app::{App, AppOptions};
        use crate::backend::Waker;
        use crate::model::{Action, Dialog};
        use crate::paths::AppDirs;
        use crate::settings::Settings;
        for dark in [false, true] {
            for size in [[800.0, 650.0], [1280.0, 800.0]] {
                let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join(".cache")
                    .join(format!(
                        "bridge-picker-{}",
                        crate::mixed_playlists::new_id()
                    ));
                let mut app = App::new(
                    &Waker::default(),
                    AppDirs {
                        config: root.join("config"),
                        state: root.join("state"),
                        cache: root.join("cache"),
                    },
                    Settings::default(),
                    AppOptions {
                        media_controls: false,
                        restore_sign_in: false,
                        tray: false,
                        autostart: false,
                    },
                );
                crate::demo::populate(&mut app);
                app.palette = if dark {
                    crate::theme::Palette::dark()
                } else {
                    crate::theme::Palette::light()
                };
                let target = app
                    .mixed_playlists
                    .create("Bridge destination".into(), String::new());
                app.dialog = Some(Dialog::AddToPlaylist {
                    request: 1,
                    name: "Song".into(),
                    items: vec![song("soundcloud:track:7", "Song")],
                    source: None,
                    query: String::new(),
                    loading: false,
                });
                app.actions.clear();
                let ctx = egui::Context::default();
                ctx.enable_accesskit();
                crate::theme::install(&ctx);
                let draw = |app: &mut App, events| {
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(size[0], size[1]),
                            )),
                            events,
                            ..Default::default()
                        },
                        |_| super::show(app, &ctx),
                    );
                    output.textures_delta.clear();
                    output.platform_output.accesskit_update.unwrap()
                };
                draw(&mut app, vec![]);
                let tree = draw(&mut app, vec![]);
                let (node, _) = tree
                    .nodes
                    .iter()
                    .find(|(_, node)| node.label() == Some("Bridge destination (Mixed)"))
                    .expect("ordinary playlist picker destination");
                draw(
                    &mut app,
                    vec![egui::Event::AccessKitActionRequest(
                        egui::accesskit::ActionRequest {
                            target_tree: egui::accesskit::TreeId::ROOT,
                            target_node: *node,
                            action: egui::accesskit::Action::Click,
                            data: None,
                        },
                    )],
                );
                assert!(
                    matches!(app.actions.as_slice(), [Action::PlaylistLinkChosen, Action::AddToPlaylist { playlist_id, .. }] if playlist_id == &format!("spotifast:playlist:{target}"))
                );
                for action in std::mem::take(&mut app.actions) {
                    app.apply(action, &ctx);
                }
                assert!(app.dialog.is_none());
                assert_eq!(app.mixed_playlists.get(&target).unwrap().entries.len(), 1);
                assert!(
                    app.toasts
                        .iter()
                        .any(|toast| toast.message == "Adding to playlist…")
                );
                app.backend.shutdown();
                drop(app);
                if root.exists() {
                    std::fs::remove_dir_all(root).unwrap();
                }
            }
        }
    }

    #[test]
    fn duplicate_dialog_names_the_song() {
        let items = vec![song("spotify:track:honey", "Honey")];
        let message = duplicate_message(
            Locale::English,
            "The best music ever",
            &items,
            &["spotify:track:honey".into()],
        );

        assert_eq!(
            message,
            "“Honey” is already in “The best music ever”. Add it again?"
        );
    }

    #[test]
    fn duplicate_dialog_names_only_the_duplicates_in_a_selection() {
        let items = vec![
            song("spotify:track:honey", "Honey"),
            song("spotify:track:new", "New song"),
        ];
        let message = duplicate_message(
            Locale::English,
            "The best music ever",
            &items,
            &["spotify:track:honey".into()],
        );

        assert_eq!(
            message,
            "“Honey” is already in “The best music ever”. Add them anyway?"
        );
    }
}

/// A page's header cover, as large as the window allows, over the dimmed
/// page. A click anywhere, on the cover or around it, or Esc closes it.
fn cover_art(app: &mut App, ctx: &egui::Context, url: &str, shown: Option<&str>, title: &str) {
    let palette = app.palette;
    let side = cover_art_side(ctx.content_rect().size());
    let response = egui::Modal::new(egui::Id::new("cover-art"))
        .frame(Frame::new().shadow(egui::epaint::Shadow {
            offset: [0, 10],
            blur: 40,
            spread: 0,
            color: palette.shadow,
        }))
        .backdrop_color(egui::Color32::from_black_alpha(if palette.dark {
            190
        } else {
            150
        }))
        .show(ctx, |ui| {
            let (rect, cover) =
                ui.allocate_exact_size(egui::Vec2::splat(side), egui::Sense::click());
            cover.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Image, true, title));
            super::widgets::paint_cover_with_thumbnail(
                ui,
                &palette,
                super::widgets::CoverSources {
                    requested: Some(url),
                    previous: shown,
                    ..Default::default()
                },
                rect,
                8.0,
                theme::Icon::Music,
                Some(app.backend.art()),
            );
            cover.clicked()
        });
    app.dialog_rect = Some(response.response.rect);
    if response.inner || response.should_close() {
        app.actions.push(Action::CloseDialog);
    }
}

/// The enlarged cover's side: as much of the window as leaves a margin
/// around it, and no larger than Spotify's largest artwork, 640 pixels wide.
fn cover_art_side(window: egui::Vec2) -> f32 {
    (window.x.min(window.y) - 96.0).clamp(120.0, 640.0)
}

#[cfg(test)]
mod cover_art_tests {
    use super::cover_art_side;

    #[test]
    fn the_enlarged_cover_fits_the_window_up_to_spotifys_largest_artwork() {
        assert_eq!(cover_art_side(egui::vec2(1240.0, 800.0)), 640.0);
        assert_eq!(cover_art_side(egui::vec2(760.0, 620.0)), 524.0);
        assert_eq!(cover_art_side(egui::vec2(100.0, 100.0)), 120.0);
    }
}

fn mixed_playlist(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let locale = app.locale;
    let Some(Dialog::MixedPlaylist {
        id,
        name,
        description,
        tracks,
    }) = &mut app.dialog
    else {
        return;
    };
    theme::text(
        ui,
        if id.is_some() {
            "Edit mixed playlist"
        } else {
            "New mixed playlist"
        },
        theme::bold(20.0),
        palette.text,
    );
    ui.add_space(12.0);
    theme::text(ui, "Name", theme::medium(13.0), palette.secondary);
    let field = text_field(
        ui,
        &palette,
        locale,
        "mixed-name",
        name,
        "My playlist",
        true,
    );
    ui.add_space(10.0);
    theme::text(ui, "Description", theme::medium(13.0), palette.secondary);
    text_field(
        ui,
        &palette,
        locale,
        "mixed-description",
        description,
        "Optional description",
        false,
    );
    ui.add_space(10.0);
    theme::text(
        ui,
        "Spotify and SoundCloud songs. Saved on this computer.",
        theme::regular(13.0),
        palette.secondary,
    );
    let submit = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
    let action = Action::SaveMixedPlaylist {
        id: id.clone(),
        name: name.trim().into(),
        description: description.clone(),
        tracks: tracks.clone(),
    };
    let valid = !name.trim().is_empty();
    ui.add_space(20.0);
    buttons(ui, |ui| {
        if (theme::pill_button(ui, &palette, &gettext(locale, "Save"), true).clicked() || submit)
            && valid
        {
            app.actions.push(action);
        }
        if theme::pill_button(ui, &palette, &gettext(locale, "Cancel"), false).clicked() {
            app.actions.push(Action::CloseDialog);
        }
    });
}
