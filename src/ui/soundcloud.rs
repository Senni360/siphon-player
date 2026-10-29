//! SoundCloud collections use the same rows, selection, sort and filter as Spotify.
use std::sync::Arc;

use super::collection::{self, Actions, Hero, HeroImages, Table};
use crate::{
    api::models::PlayableItem,
    app::App,
    i18n::gettext,
    model::{Page, RowContext, TableItem},
    theme::Icon,
};

pub fn show(app: &mut App, ui: &mut egui::Ui, page: Page) {
    let locale = app.locale;
    if page == Page::SoundCloudLikes && app.soundcloud_account.user().is_none() {
        ui.heading(gettext(locale, "SoundCloud Likes"));
        ui.label(gettext(
            locale,
            "Connect a SoundCloud profile to see its likes.",
        ));
        if crate::theme::pill_button(
            ui,
            &app.palette,
            &gettext(locale, "Connect SoundCloud"),
            true,
        )
        .clicked()
        {
            app.actions.push(crate::model::Action::ShowDialog(
                crate::model::Dialog::SoundCloudConnect(Default::default()),
            ));
        }
        return;
    }
    let library = &app.soundcloud_library;
    let (title, loading, error, more, revision) = (
        gettext(locale, "Likes").into_owned(),
        library.loading,
        library.error.clone(),
        library.snapshot.next_likes.is_some(),
        library.revision,
    );
    let names = app.user_names_revision;
    let items =
        if let Some(items) = collection::table_items_hit(app, &page, revision, revision, names) {
            items
        } else {
            let tracks = &app.soundcloud_library.snapshot.likes;
            let rows: Vec<TableItem> = tracks
                .iter()
                .cloned()
                .map(|track| (PlayableItem::Track(track), None, None))
                .collect();
            collection::remember_table_items(app, page.clone(), revision, revision, names, rows)
        };
    let uri = format!(
        "soundcloud:likes:{}",
        app.soundcloud_account.user().map_or(0, |user| user.user_id)
    );
    collection::hero(
        app,
        ui,
        Hero {
            images: HeroImages::default(),
            liked: page == Page::SoundCloudLikes,
            kind: gettext(locale, "SoundCloud"),
            title: &title,
            description: None,
            byline: vec![(
                app.soundcloud_account
                    .user()
                    .map(|user| user.username.clone())
                    .unwrap_or_default(),
                None,
            )],
            avatar: None,
            round: false,
        },
    );
    let filter_id = ui.id().with(("soundcloud-filter", &page));
    let mut filter = ui
        .data(|data| data.get_temp::<String>(filter_id))
        .unwrap_or_default();
    let sort = app.table_sorts.get(&page).copied();
    let plays_shown_rows = page != Page::SoundCloudLikes;
    let view = collection::prepare_table_view(
        ui,
        app,
        &page,
        &items,
        collection::ViewOptions {
            needle: &filter.trim().to_lowercase(),
            sort,
            plays_shown_rows,
        },
        revision,
    );
    let uris: Arc<[String]> = view.view_uris.clone().unwrap_or_else(|| {
        items
            .iter()
            .filter(|row| app.row_playable(&row.0))
            .map(|row| row.0.uri().to_owned())
            .collect::<Vec<_>>()
            .into()
    });
    collection::actions_row(
        app,
        ui,
        Actions {
            play_uri: Some(uri.clone()),
            view: if plays_shown_rows
                || sort.is_some()
                || !filter.trim().is_empty()
                || uris.is_empty()
            {
                Some(Arc::clone(&uris))
            } else {
                None
            },
            saved: None,
            saved_icons: (Icon::Heart, Icon::HeartFilled),
            saved_tooltips: Default::default(),
            owned_playlist: None,
            reload: Some((page.clone(), loading)),
            name: &title,
            save_radio: None,
            plays_shown_rows,
        },
        Some(&mut filter),
    );
    ui.data_mut(|data| data.insert_temp(filter_id, filter.clone()));
    collection::table(
        app,
        ui,
        Table {
            items: &items,
            row_offset: 0,
            pagination: None,
            context: RowContext::Context {
                uri,
                editable_playlist: None,
            },
            show_album: false,
            show_cover: true,
            show_added: false,
            show_added_by: false,
            show_plays: false,
            page,
            loading,
            error: error.as_deref(),
            can_load_more: more && error.is_none(),
            filter: &filter,
            items_revision: revision,
            plays_shown_rows,
        },
    );
}
