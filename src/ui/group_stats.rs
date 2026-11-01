//! The Group page's statistics: yours for any period, and for this month and
//! all time, everyone who shares theirs, side by side.

use egui::{Id, Rect, RichText, Sense, Ui, Vec2, pos2, vec2};

use crate::app::App;
use crate::friends::Profile;
use crate::friends::live::MemberStats;
use crate::i18n::gettext;
use crate::model::Action;
use crate::stats::{Action as Stats, Period, Report, Summary, Top, shared_artists, taste_match};
use crate::theme::{self, Icon, Palette};

use super::group::{card, chip};

const SHOWN: usize = 10;

fn hours_text(minutes: u64) -> String {
    let hours = minutes / 60;
    if hours == 0 {
        format!("{minutes} min")
    } else {
        format!("{hours} h {} min", minutes % 60)
    }
}

fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn heading(ui: &mut Ui, palette: &Palette, text: &str) {
    ui.add_space(20.0);
    theme::text(ui, text, theme::medium(16.0), palette.text);
    ui.add_space(8.0);
}

fn wrapped(ui: &mut Ui, text: impl Into<String>, palette: &Palette) {
    ui.add(
        egui::Label::new(
            RichText::new(text.into())
                .font(theme::regular(12.5))
                .color(palette.dim),
        )
        .wrap()
        .selectable(false),
    );
}

fn tiles(ui: &mut Ui, palette: &Palette, items: &[(String, String)]) {
    let gap = 8.0;
    let count = items.len().max(1) as f32;
    let width = ((ui.available_width() - gap * (count - 1.0)) / count).max(70.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (value, label) in items {
            let (rect, _) = ui.allocate_exact_size(vec2(width, 68.0), Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(rect, 12.0, palette.surface);
            painter.text(
                rect.left_top() + vec2(12.0, 12.0),
                egui::Align2::LEFT_TOP,
                value,
                theme::medium(19.0),
                palette.text,
            );
            painter.text(
                rect.left_bottom() + vec2(12.0, -12.0),
                egui::Align2::LEFT_BOTTOM,
                label,
                theme::regular(12.0),
                palette.dim,
            );
        }
    });
}

/// Upright bars with a label under some of them.
fn columns(ui: &mut Ui, palette: &Palette, values: &[u64], labels: &[String], id: &str) {
    let height = 96.0;
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, height + 18.0), Sense::hover());
    let largest = values.iter().copied().max().unwrap_or(0).max(1) as f32;
    let step = rect.width() / values.len().max(1) as f32;
    let every = (40.0 / step).ceil().max(1.0) as usize;
    for (index, value) in values.iter().enumerate() {
        let left = rect.left() + step * index as f32;
        let bar = (*value as f32 / largest * height).max(if *value > 0 { 2.0 } else { 0.0 });
        let bar_rect = Rect::from_min_max(
            pos2(left + step * 0.15, rect.top() + height - bar),
            pos2(left + step * 0.85, rect.top() + height),
        );
        let hit = Rect::from_min_max(
            pos2(left, rect.top()),
            pos2(left + step, rect.top() + height),
        );
        let response = ui
            .interact(hit, Id::new((id, index)), Sense::hover())
            .on_hover_text(format!(
                "{}: {}",
                labels.get(index).map_or("", String::as_str),
                hours_text(*value)
            ));
        let color = if response.hovered() {
            palette.text
        } else {
            palette.secondary
        };
        ui.painter().rect_filled(bar_rect, 2.0, color);
        if index % every == 0
            && let Some(label) = labels.get(index)
        {
            ui.painter().text(
                pos2(left + step / 2.0, rect.top() + height + 4.0),
                egui::Align2::CENTER_TOP,
                label,
                theme::regular(10.5),
                palette.dim,
            );
        }
    }
}

/// A sideways bar behind a name, as long as its share of the top row.
fn bar_row(ui: &mut Ui, palette: &Palette, name: &str, detail: &str, share: f32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
    let fill = Rect::from_min_size(
        rect.min,
        vec2(rect.width() * share.clamp(0.02, 1.0), rect.height()),
    );
    ui.painter().rect_filled(fill, 6.0, palette.surface_hover);
    let clip = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    clip.text(
        rect.left_center() + vec2(10.0, 0.0),
        egui::Align2::LEFT_CENTER,
        name,
        theme::regular(13.0),
        palette.text,
    );
    clip.text(
        rect.right_center() - vec2(10.0, 0.0),
        egui::Align2::RIGHT_CENTER,
        detail,
        theme::regular(12.0),
        palette.dim,
    );
    ui.add_space(4.0);
}

/// A numbered row of a top list, with its cover where there is one. A song
/// plays when clicked.
fn top_row(
    ui: &mut Ui,
    palette: &Palette,
    rank: usize,
    row: &Top,
    with_art: bool,
    id: Id,
    actions: &mut Vec<Action>,
) {
    let width = ui.available_width();
    let height = if with_art { 48.0 } else { 40.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let playable = !row.uri.is_empty();
    let response = ui.interact(
        rect,
        id,
        if playable {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    if response.hovered() && playable {
        ui.painter().rect_filled(rect, 8.0, palette.surface_hover);
    }
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.text(
        rect.left_center() + vec2(14.0, 0.0),
        egui::Align2::CENTER_CENTER,
        rank.to_string(),
        theme::medium(13.0),
        palette.dim,
    );
    let mut left = rect.left() + 30.0;
    if with_art {
        let cover = Rect::from_min_size(pos2(left, rect.top() + 4.0), Vec2::splat(40.0));
        super::widgets::paint_cover(
            ui,
            palette,
            row.art.as_deref(),
            cover,
            6.0,
            Icon::Music,
            None,
        );
        left += 50.0;
    }
    let (value, unit) = (
        format!("{} plays", thousands(row.plays)),
        hours_text(row.minutes),
    );
    let right = rect.right() - 10.0;
    let value_width = 96.0;
    let text_clip = Rect::from_min_max(
        pos2(left, rect.top()),
        pos2(right - value_width - 8.0, rect.bottom()),
    );
    let text = ui
        .painter()
        .with_clip_rect(text_clip.intersect(ui.clip_rect()));
    let middle = rect.center().y;
    if row.detail.is_empty() {
        text.text(
            pos2(left, middle),
            egui::Align2::LEFT_CENTER,
            &row.name,
            theme::medium(13.5),
            palette.text,
        );
    } else {
        text.text(
            pos2(left, middle - 9.0),
            egui::Align2::LEFT_CENTER,
            &row.name,
            theme::medium(13.5),
            palette.text,
        );
        text.text(
            pos2(left, middle + 9.0),
            egui::Align2::LEFT_CENTER,
            &row.detail,
            theme::regular(12.0),
            palette.dim,
        );
    }
    painter.text(
        pos2(right, middle - 8.0),
        egui::Align2::RIGHT_CENTER,
        value,
        theme::regular(12.5),
        palette.secondary,
    );
    painter.text(
        pos2(right, middle + 8.0),
        egui::Align2::RIGHT_CENTER,
        unit,
        theme::regular(11.5),
        palette.dim,
    );
    if response.clicked() {
        actions.push(if row.uri.starts_with("spotify:") {
            Action::PlayUris {
                uris: vec![row.uri.clone()],
                index: 0,
            }
        } else {
            Action::OpenLink(row.uri.clone())
        });
    }
}

/// A list that shows its first rows, and the rest on asking.
fn top_list(
    app: &App,
    ui: &mut Ui,
    title: &str,
    rows: &[Top],
    with_art: bool,
    actions: &mut Vec<Action>,
) {
    if rows.is_empty() {
        return;
    }
    let palette = app.palette;
    heading(ui, &palette, title);
    let id = Id::new(("group-stats-more", title));
    let more = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(false);
    let shown = if more {
        rows.len()
    } else {
        SHOWN.min(rows.len())
    };
    for (index, row) in rows.iter().take(shown).enumerate() {
        top_row(
            ui,
            &palette,
            index + 1,
            row,
            with_art,
            id.with(index),
            actions,
        );
    }
    if rows.len() > SHOWN {
        ui.add_space(4.0);
        let label = if more {
            gettext(app.locale, "Show fewer")
        } else {
            gettext(app.locale, "Show more")
        };
        if theme::soft_button(ui, &palette, None, &label, false).clicked() {
            ui.data_mut(|data| data.insert_temp(id, !more));
        }
    }
}

fn clock(ui: &mut Ui, palette: &Palette, hours: &[u64], id: &str) {
    if hours.iter().all(|hour| *hour == 0) {
        return;
    }
    let labels: Vec<String> = (0..hours.len()).map(|hour| format!("{hour}:00")).collect();
    columns(ui, palette, hours, &labels, id);
}

fn own_report(app: &App, ui: &mut Ui, report: &Report, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    tiles(
        ui,
        &palette,
        &[
            (
                thousands(report.minutes),
                gettext(locale, "minutes listened").into_owned(),
            ),
            (
                thousands(report.plays),
                gettext(locale, "plays").into_owned(),
            ),
            (
                thousands(report.distinct_artists),
                gettext(locale, "artists").into_owned(),
            ),
            (
                report.streak_days.to_string(),
                gettext(locale, "day streak").into_owned(),
            ),
        ],
    );
    ui.add_space(8.0);
    let mut extras = vec![format!(
        "{} {}",
        thousands(report.distinct_songs),
        gettext(locale, "different songs")
    )];
    if report.new_artists > 0 {
        extras.push(format!(
            "{} {}",
            thousands(report.new_artists),
            gettext(locale, "new artists")
        ));
    }
    extras.push(format!(
        "{}: {} {}",
        gettext(locale, "longest streak"),
        report.longest_streak_days,
        gettext(locale, "days")
    ));
    wrapped(ui, extras.join(" · "), &palette);
    if report.estimated_percent > 0 {
        wrapped(
            ui,
            gettext(
                locale,
                "{percent}% of this time is estimated: Spotify lists what played on your other devices, but not for how long. Importing your history makes it exact.",
            )
            .replace("{percent}", &report.estimated_percent.to_string()),
            &palette,
        );
    }
    if report.months.len() > 1 {
        heading(ui, &palette, &gettext(locale, "Minutes each month"));
        let values: Vec<u64> = report.months.iter().map(|(_, minutes)| *minutes).collect();
        let labels: Vec<String> = report
            .months
            .iter()
            .map(|(month, _)| month.clone())
            .collect();
        columns(ui, &palette, &values, &labels, "group-stats-months");
    }
    top_list(
        app,
        ui,
        &gettext(locale, "Top songs"),
        &report.songs,
        true,
        actions,
    );
    top_list(
        app,
        ui,
        &gettext(locale, "Top artists"),
        &report.artists,
        false,
        actions,
    );
    top_list(
        app,
        ui,
        &gettext(locale, "Top albums"),
        &report.albums,
        false,
        actions,
    );
    if !report.genres.is_empty() {
        heading(ui, &palette, &gettext(locale, "Top genres"));
        let largest = report.genres.first().map_or(1, |g| g.minutes.max(1)) as f32;
        for genre in report.genres.iter().take(SHOWN) {
            bar_row(
                ui,
                &palette,
                &genre.name,
                &hours_text(genre.minutes),
                genre.minutes as f32 / largest,
            );
        }
        if report.genres_known_percent < 100 {
            wrapped(
                ui,
                gettext(
                    locale,
                    "Spotify and SoundCloud name genres for {percent}% of this time so far.",
                )
                .replace("{percent}", &report.genres_known_percent.to_string()),
                &palette,
            );
        }
    }
    if report.hours.iter().any(|hour| *hour > 0) {
        heading(ui, &palette, &gettext(locale, "When you listen"));
        clock(ui, &palette, &report.hours, "group-stats-hours");
        ui.add_space(12.0);
        let days = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        let labels: Vec<String> = days
            .iter()
            .map(|day| gettext(locale, day).into_owned())
            .collect();
        columns(ui, &palette, &report.weekdays, &labels, "group-stats-days");
    }
}

/// Everyone sharing, most listening first, and how close each is to you.
fn leaderboard(app: &App, ui: &mut Ui, me: &Profile, shared: &[MemberStats], mine: &Summary) {
    let palette = app.palette;
    let locale = app.locale;
    if shared.is_empty() {
        return;
    }
    heading(ui, &palette, &gettext(locale, "The group"));
    let mut rows: Vec<&MemberStats> = shared.iter().collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.summary.minutes));
    let largest = rows.first().map_or(1, |r| r.summary.minutes.max(1)) as f32;
    for row in rows {
        let name = if row.user_id == me.user_id {
            gettext(locale, "You").into_owned()
        } else {
            app.friends_feed
                .members
                .iter()
                .find(|member| member.user_id == row.user_id)
                .map_or_else(
                    || gettext(locale, "Someone").into_owned(),
                    |member| member.display_name.clone(),
                )
        };
        let mut detail = hours_text(row.summary.minutes);
        if row.user_id != me.user_id {
            detail = format!(
                "{detail} · {}% {}",
                taste_match(mine, &row.summary),
                gettext(locale, "match")
            );
        }
        bar_row(
            ui,
            &palette,
            &name,
            &detail,
            row.summary.minutes as f32 / largest,
        );
    }
}

fn their_summary(
    app: &App,
    ui: &mut Ui,
    name: &str,
    theirs: &Summary,
    mine: &Summary,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    let locale = app.locale;
    tiles(
        ui,
        &palette,
        &[
            (
                thousands(theirs.minutes),
                gettext(locale, "minutes listened").into_owned(),
            ),
            (
                thousands(theirs.plays),
                gettext(locale, "plays").into_owned(),
            ),
            (
                format!("{}%", taste_match(mine, theirs)),
                gettext(locale, "taste match").into_owned(),
            ),
        ],
    );
    let common = shared_artists(mine, theirs);
    if !common.is_empty() {
        ui.add_space(8.0);
        wrapped(
            ui,
            format!(
                "{} {name}: {}",
                gettext(locale, "You and"),
                common.into_iter().take(6).collect::<Vec<_>>().join(", ")
            ),
            &palette,
        );
    }
    top_list(
        app,
        ui,
        &gettext(locale, "Top songs"),
        &theirs.songs,
        true,
        actions,
    );
    top_list(
        app,
        ui,
        &gettext(locale, "Top artists"),
        &theirs.artists,
        false,
        actions,
    );
    if !theirs.genres.is_empty() {
        heading(ui, &palette, &gettext(locale, "Top genres"));
        let largest = theirs.genres.first().map_or(1, |g| g.minutes.max(1)) as f32;
        for genre in &theirs.genres {
            bar_row(
                ui,
                &palette,
                &genre.name,
                &hours_text(genre.minutes),
                genre.minutes as f32 / largest,
            );
        }
    }
    if theirs.hours.iter().any(|hour| *hour > 0) {
        heading(ui, &palette, &gettext(locale, "When they listen"));
        clock(ui, &palette, &theirs.hours, "group-stats-their-hours");
    }
}

fn import_card(app: &App, ui: &mut Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    ui.add_space(24.0);
    card(ui, &palette, |ui| {
        theme::text(
            ui,
            gettext(locale, "Add your whole Spotify history"),
            theme::medium(14.0),
            palette.text,
        );
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                RichText::new(gettext(
                    locale,
                    "Spotify's API only remembers your last 50 plays. For every play since your account began, ask for your Extended streaming history on Spotify's privacy page. It can take up to 30 days to arrive. Then choose the ZIP here. It stays on this computer.",
                ))
                .font(theme::regular(13.0))
                .color(palette.secondary),
            )
            .wrap()
            .selectable(false),
        );
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let importing = app.stats.importing;
            let label = if importing {
                gettext(locale, "Adding your history…")
            } else {
                gettext(locale, "Choose the file")
            };
            if ui
                .add_enabled_ui(!importing, |ui| {
                    theme::soft_button(ui, &palette, None, &label, true)
                })
                .inner
                .clicked()
            {
                actions.push(Action::Stats(Stats::Import));
            }
            if theme::soft_button(
                ui,
                &palette,
                None,
                &gettext(locale, "Open Spotify's privacy page"),
                false,
            )
            .clicked()
            {
                actions.push(Action::OpenUrl(
                    "https://www.spotify.com/account/privacy/".to_owned(),
                ));
            }
        });
    });
}

pub fn show(app: &mut App, ui: &mut Ui, me: &Profile, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let locale = app.locale;
    ui.horizontal_wrapped(|ui| {
        for period in Period::ALL {
            let label = gettext(locale, period.label());
            let id = Id::new(("group-stats-period", period.label()));
            if chip(ui, &palette, &label, app.stats.period == period, id).clicked() {
                actions.push(Action::Stats(Stats::Period(period)));
            }
        }
    });
    ui.add_space(8.0);

    let shared: Vec<MemberStats> = app
        .stats
        .period
        .shared_key()
        .and_then(|key| app.live.group_stats.get(key))
        .cloned()
        .unwrap_or_default();
    let others: Vec<&MemberStats> = shared
        .iter()
        .filter(|row| row.user_id != me.user_id && row.summary.plays > 0)
        .collect();
    let viewing_id = Id::new("group-stats-person");
    let mut viewing = ui
        .data(|data| data.get_temp::<String>(viewing_id))
        .unwrap_or_default();
    if !others.iter().any(|row| row.user_id == viewing) {
        viewing.clear();
    }
    if !others.is_empty() {
        ui.horizontal_wrapped(|ui| {
            if chip(
                ui,
                &palette,
                &gettext(locale, "You"),
                viewing.is_empty(),
                viewing_id.with("you"),
            )
            .clicked()
            {
                viewing.clear();
            }
            let someone = gettext(locale, "Someone");
            for row in &others {
                let name = app
                    .friends_feed
                    .members
                    .iter()
                    .find(|member| member.user_id == row.user_id)
                    .map_or(someone.as_ref(), |member| member.display_name.as_str());
                if chip(
                    ui,
                    &palette,
                    name,
                    viewing == row.user_id,
                    viewing_id.with(&row.user_id),
                )
                .clicked()
                {
                    viewing.clone_from(&row.user_id);
                }
            }
        });
        ui.add_space(8.0);
    }
    ui.data_mut(|data| data.insert_temp(viewing_id, viewing.clone()));
    ui.add_space(8.0);

    let Some(report) = app.stats.reports.get(&app.stats.period).cloned() else {
        theme::text(
            ui,
            gettext(locale, "Counting…"),
            theme::regular(13.0),
            palette.dim,
        );
        return;
    };
    let mine = report.summary();
    if let Some(row) = others.iter().find(|row| row.user_id == viewing) {
        let name = app
            .friends_feed
            .members
            .iter()
            .find(|member| member.user_id == row.user_id)
            .map_or_else(String::new, |member| member.display_name.clone());
        their_summary(app, ui, &name, &row.summary, &mine, actions);
    } else if report.plays == 0 {
        theme::text(
            ui,
            gettext(
                locale,
                "Nothing counted for this period yet. What you play here from now on is counted, and Spotify's last 50 plays are added as they are read.",
            ),
            theme::regular(13.0),
            palette.dim,
        );
    } else {
        own_report(app, ui, &report, actions);
    }
    leaderboard(app, ui, me, &shared, &mine);
    if app.settings.group_sharing != Some(true) {
        ui.add_space(12.0);
        wrapped(
            ui,
            gettext(
                locale,
                "The group can't see these while you keep your listening private.",
            ),
            &palette,
        );
    }
    import_card(app, ui, actions);
}
