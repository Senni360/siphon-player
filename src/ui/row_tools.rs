//! The four square tools on every song row (`kit::tools`), wired to what the
//! app can do today: Download through the helper, Credits from the backend's
//! per-song reads, and Add to the playlist used last this session. BPM and
//! key wait for the helper's analysis protocol, so that tool says so instead
//! of answering with numbers nobody measured.

use egui::{Id, Rect, Ui, UiBuilder, pos2, vec2};

use crate::api::models::PlayableItem;
use crate::app::App;
use crate::i18n::gettext;
use crate::model::{Action, Loadable};
use crate::theme;

use super::kit::motion;
use super::kit::tools::{self, Pick, State, Tool};

/// The column the tools take in a song list, their squares and a margin.
pub const WIDTH: f32 = tools::ROW_WIDTH + 12.0;

/// The playlist a song was last added to this session: the destination of
/// Add to last-used playlist. Nothing stores it between launches yet. It
/// belongs to the account that added the song, so another account never
/// sees its name or adds to it.
#[derive(Clone, Debug, PartialEq)]
struct LastPlaylist {
    owner: String,
    id: String,
    name: String,
}

fn last_id() -> Id {
    Id::new("row-tools-last-playlist")
}

/// Remembers the destination of any song `owner` added to a playlist this
/// frame, from a menu, a drag or a tool, so Add to last-used playlist
/// follows it. Call once per frame after the views have drawn.
pub fn remember_last_playlist(ctx: &egui::Context, owner: Option<&str>, actions: &[Action]) {
    let Some(owner) = owner else {
        return;
    };
    let last = actions.iter().rev().find_map(|action| match action {
        Action::AddToPlaylist {
            playlist_id,
            playlist_name,
            ..
        } => Some(LastPlaylist {
            owner: owner.to_owned(),
            id: playlist_id.clone(),
            name: playlist_name.clone(),
        }),
        _ => None,
    });
    if let Some(last) = last {
        ctx.data_mut(|data| data.insert_temp(last_id(), last));
    }
}

/// The last playlist `owner` added to, if any.
fn last_playlist(ctx: &egui::Context, owner: Option<&str>) -> Option<LastPlaylist> {
    ctx.data(|data| data.get_temp::<LastPlaylist>(last_id()))
        .filter(|last| Some(last.owner.as_str()) == owner)
}

/// Forgets the last-used playlist, for sign-out.
pub fn forget(ctx: &egui::Context) {
    ctx.data_mut(|data| data.remove::<LastPlaylist>(last_id()));
}

/// Draws the tools for `item` in `cell`. `shown` is whether the row asks for
/// them now (hovered, focused, playing); they slide in and out with it.
pub fn show(
    ui: &mut Ui,
    app: &mut App,
    item: &PlayableItem,
    cell: Rect,
    shown: bool,
    unavailable: bool,
) {
    let ctx = ui.ctx().clone();
    let uri = item.uri().to_string();
    let id = ui.id().with(("row-tools", &uri));
    let open = ctx.data(|data| data.get_temp::<Tool>(id.with("open")));
    let reveal = ctx.animate_bool_with_time(
        id.with("reveal"),
        shown || open.is_some(),
        crate::motion::seconds(&ctx, motion::FEEDBACK * 1.5),
    );
    if reveal <= 0.0 {
        return;
    }
    let states = states(app, &ctx, item, unavailable);
    let band = Rect::from_center_size(
        pos2(cell.left() + tools::ROW_WIDTH / 2.0 + 4.0, cell.center().y),
        vec2(tools::ROW_WIDTH, tools::SIZE),
    );
    let mut child = ui.new_child(UiBuilder::new().max_rect(band).id_salt(("tools", &uri)));
    let row = tools::row(&mut child, &app.palette, &states, motion::ease(reveal));
    if let Some(pick) = row.pick {
        apply(app, &ctx, id, item, pick, open);
    }
    match ctx.data(|data| data.get_temp::<Tool>(id.with("open"))) {
        Some(Tool::Credits) => credits_popover(app, &ctx, id, &uri, row.rects[2]),
        Some(Tool::AddToLast) => playlist_popover(app, &ctx, id, item, row.rects[3]),
        _ => {}
    }
}

/// The signed-in account, which owns the remembered playlist.
fn owner(app: &App) -> Option<&str> {
    app.user.as_ref().map(|user| user.id.as_str())
}

fn states(app: &App, ctx: &egui::Context, item: &PlayableItem, unavailable: bool) -> [State; 4] {
    let locale = app.locale;
    let off_here = || State::Off(gettext(locale, "This song isn't available here").into_owned());
    let download = if unavailable {
        off_here()
    } else if !app.downloads.available {
        State::Off(gettext(locale, "Download needs the Siphon helper").into_owned())
    } else if app.download_source(item).is_none() {
        State::Off(gettext(locale, "This song can't be downloaded").into_owned())
    } else {
        State::Ready(gettext(locale, "Download").into_owned())
    };
    let bpm = State::Off(
        gettext(
            locale,
            "BPM and key arrive with the Siphon helper's analysis",
        )
        .into_owned(),
    );
    let credits = if unavailable {
        off_here()
    } else {
        match app.credits.get(item.uri()) {
            Some(Loadable::Loading) => State::Running {
                fraction: None,
                tip: gettext(locale, "Loading credits…").into_owned(),
            },
            _ => State::Ready(gettext(locale, "Credits").into_owned()),
        }
    };
    let last = last_playlist(ctx, owner(app));
    let add = if unavailable || !item.is_track() {
        off_here()
    } else if let Some(last) = last {
        State::Ready(
            gettext(
                locale,
                "Add to {playlist}. Right-click for another playlist",
            )
            .replace("{playlist}", &last.name),
        )
    } else {
        State::Ready(gettext(locale, "Add to a playlist").into_owned())
    };
    [download, bpm, credits, add]
}

fn apply(
    app: &mut App,
    ctx: &egui::Context,
    id: Id,
    item: &PlayableItem,
    pick: Pick,
    open: Option<Tool>,
) {
    let set_open = |tool: Option<Tool>| {
        ctx.data_mut(|data| match tool {
            Some(tool) => {
                data.insert_temp(id.with("open"), tool);
            }
            None => data.remove::<Tool>(id.with("open")),
        });
    };
    match pick.tool {
        Tool::Download => app.actions.push(Action::Download(vec![item.clone()])),
        // Off until the helper can analyse; a click does nothing.
        Tool::BpmKey => {}
        Tool::Credits => {
            if open == Some(Tool::Credits) {
                close_credits(app, item.uri());
                set_open(None);
            } else {
                app.actions
                    .push(Action::RequestCredits(item.uri().to_string()));
                set_open(Some(Tool::Credits));
            }
        }
        Tool::AddToLast => {
            let last = last_playlist(ctx, owner(app));
            match last.filter(|_| !pick.secondary) {
                Some(last) => app.actions.push(Action::AddToPlaylist {
                    playlist_id: last.id,
                    playlist_name: last.name,
                    items: vec![item.clone()],
                }),
                None => set_open(Some(Tool::AddToLast)),
            }
        }
    }
}

/// Stops a credits read nobody is waiting for.
fn close_credits(app: &mut App, uri: &str) {
    if matches!(app.credits.get(uri), Some(Loadable::Loading)) {
        app.actions.push(Action::CancelCredits(uri.to_string()));
    }
}

/// A popover under `anchor`. Returns false once a click outside closes it.
fn popover(
    palette: &crate::theme::Palette,
    ctx: &egui::Context,
    id: Id,
    anchor: Rect,
    add_contents: impl FnOnce(&mut Ui),
) -> bool {
    let width = 280.0;
    let position = pos2(anchor.right() - width, anchor.bottom() + 6.0);
    let rise = ctx.animate_bool_with_time(
        id.with("popover-rise"),
        true,
        crate::motion::seconds(ctx, motion::POPOVER),
    );
    let response = egui::Area::new(id.with("popover"))
        .order(egui::Order::Foreground)
        .fixed_pos(position + vec2(0.0, (1.0 - motion::ease(rise)) * -6.0))
        .show(ctx, |ui| {
            ui.multiply_opacity(rise);
            super::widgets::menu_frame(palette).show(ui, |ui| {
                ui.set_width(width - 16.0);
                add_contents(ui);
            });
        })
        .response;
    let clicked_outside = ctx.input(|input| input.pointer.any_pressed())
        && !response.contains_pointer()
        && !ctx
            .pointer_hover_pos()
            .is_some_and(|at| anchor.contains(at));
    let escape = ctx.input(|input| input.key_pressed(egui::Key::Escape));
    !(clicked_outside || escape)
}

fn credits_popover(app: &mut App, ctx: &egui::Context, id: Id, uri: &str, anchor: Rect) {
    let palette = app.palette;
    let locale = app.locale;
    let state = app.credits.get(uri).cloned();
    let mut retry = false;
    let keep = popover(&palette, ctx, id, anchor, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        theme::text(
            ui,
            gettext(locale, "Credits"),
            theme::medium(13.0),
            palette.text,
        );
        retry = credits_body(
            ui,
            locale,
            state.as_ref(),
            [palette.text, palette.secondary, palette.dim],
        );
    });
    if retry {
        app.actions.push(Action::RequestCredits(uri.to_string()));
    }
    if !keep {
        close_credits(app, uri);
        ctx.data_mut(|data| data.remove::<Tool>(id.with("open")));
    }
}

/// A song's credits as a list of roles and names, or the reason there are
/// none. `colors` are the text, secondary and dim colours of the surface it
/// sits on. Returns whether Try again was clicked after a failure.
pub(crate) fn credits_body(
    ui: &mut Ui,
    locale: crate::i18n::Locale,
    state: Option<&Loadable<Option<crate::credits::Credits>>>,
    colors: [egui::Color32; 3],
) -> bool {
    let [text, secondary, dim] = colors;
    let mut retry = false;
    match state {
        None | Some(Loadable::NotLoaded) | Some(Loadable::Loading) => {
            theme::text(
                ui,
                gettext(locale, "Loading credits…"),
                theme::regular(12.5),
                secondary,
            );
        }
        Some(Loadable::Loaded(Some(credits))) => {
            for credit in credits.contributors.iter().take(12) {
                ui.horizontal(|ui| {
                    let role = credit
                        .role
                        .as_deref()
                        .or(credit.group.as_deref())
                        .unwrap_or("");
                    ui.allocate_ui_with_layout(
                        vec2(96.0, 16.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| theme::text(ui, role, theme::regular(12.0), dim),
                    );
                    theme::text(ui, &credit.name, theme::regular(12.5), text);
                });
            }
            if !credits.sources.is_empty() {
                theme::text(
                    ui,
                    gettext(locale, "From {sources}")
                        .replace("{sources}", &credits.sources.join(", ")),
                    theme::regular(11.5),
                    dim,
                );
            }
        }
        Some(Loadable::Loaded(None)) => {
            theme::text(
                ui,
                gettext(locale, "No credits for this song."),
                theme::regular(12.5),
                secondary,
            );
        }
        Some(Loadable::Failed(_)) => {
            theme::text(
                ui,
                gettext(locale, "Credits couldn't be loaded."),
                theme::regular(12.5),
                secondary,
            );
            retry = ui.button(gettext(locale, "Try again")).clicked();
        }
    }
    retry
}

fn playlist_popover(app: &mut App, ctx: &egui::Context, id: Id, item: &PlayableItem, anchor: Rect) {
    let query_id = id.with("playlist-query");
    let before = app.actions.len();
    let palette = app.palette;
    let mut keep = popover(&palette, ctx, id, anchor, |ui| {
        let stored = ui.data(|data| data.get_temp::<String>(query_id));
        let fresh = stored.is_none();
        let mut query = stored.unwrap_or_default();
        let field =
            super::widgets::playlist_picker(ui, app, std::slice::from_ref(item), &mut query);
        if fresh {
            field.request_focus();
        }
        ui.data_mut(|data| data.insert_temp(query_id, query));
    });
    if app.actions.len() > before {
        keep = false;
    }
    if !keep {
        ctx.data_mut(|data| {
            data.remove::<Tool>(id.with("open"));
            data.remove::<String>(query_id);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn added(id: &str, name: &str) -> Action {
        Action::AddToPlaylist {
            playlist_id: id.into(),
            playlist_name: name.into(),
            items: Vec::new(),
        }
    }

    /// Whatever added a song last, a menu, a drag or the tool itself, is
    /// where Add to last-used playlist goes next; other actions leave it.
    #[test]
    fn the_last_playlist_follows_the_latest_add() {
        let ctx = egui::Context::default();
        let last = || last_playlist(&ctx, Some("alice"));
        assert_eq!(last(), None);
        remember_last_playlist(
            &ctx,
            Some("alice"),
            &[added("a", "Night drive"), added("b", "Beats to flip")],
        );
        assert_eq!(last().unwrap().name, "Beats to flip");
        remember_last_playlist(&ctx, Some("alice"), &[Action::ToggleSidebar]);
        assert_eq!(last().unwrap().id, "b");
    }

    /// Account A's destination is never offered to account B, whether B
    /// arrives after a sign-out (which forgets it) or directly.
    #[test]
    fn another_account_never_sees_the_last_playlist() {
        let ctx = egui::Context::default();
        remember_last_playlist(&ctx, Some("alice"), &[added("a", "Night drive")]);
        assert_eq!(last_playlist(&ctx, Some("bob")), None);
        assert_eq!(last_playlist(&ctx, None), None);
        forget(&ctx);
        assert_eq!(last_playlist(&ctx, Some("alice")), None);
        remember_last_playlist(&ctx, None, &[added("a", "Night drive")]);
        assert_eq!(last_playlist(&ctx, Some("alice")), None, "nobody signed in");
    }
}
