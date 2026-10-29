//! A compact progress card beside the existing completion/error toasts.

use crate::app::App;
use crate::i18n::gettext;
use crate::model::Action;
use crate::theme::{self, Icon};

pub(super) fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    for (id, progress) in &app.downloads.active {
        egui::Frame::new()
            .fill(palette.overlay)
            .stroke(egui::Stroke::new(1.0, palette.outline))
            .corner_radius(egui::CornerRadius::same(theme::RADIUS))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(280.0);
                ui.horizontal(|ui| {
                    theme::icon(ui, Icon::Download, 16.0, palette.accent);
                    let title = crate::bidi::layout(
                        ui.painter(),
                        &progress.title,
                        theme::medium(13.5),
                        palette.text,
                        250.0,
                        1,
                        Some(crate::bidi::ELLIPSIS),
                    );
                    ui.add(egui::Label::new(title));
                });
                let stage = if progress.cancelling {
                    "Cancelling"
                } else {
                    match progress.stage.as_str() {
                        "queued" => "Queued",
                        "probing" => "Finding audio",
                        "converting" => "Converting",
                        "tagging" => "Writing tags",
                        "analyzing" => "Analyzing BPM and key",
                        _ => "Downloading",
                    }
                };
                let label = if progress.total > 1 {
                    format!("{stage} · {} / {}", progress.item, progress.total)
                } else {
                    stage.to_string()
                };
                ui.horizontal(|ui| {
                    if let Some(fraction) = progress.fraction {
                        ui.add(
                            egui::ProgressBar::new(fraction)
                                .desired_width(80.0)
                                .show_percentage(),
                        );
                    } else {
                        ui.add(egui::Spinner::new().size(14.0));
                    }
                    theme::text(ui, &label, theme::regular(12.0), palette.secondary);
                });
                ui.add_enabled_ui(!progress.cancelling, |ui| {
                    if theme::soft_button(ui, &palette, None, &gettext(app.locale, "Cancel"), false)
                        .clicked()
                    {
                        app.actions.push(Action::CancelDownload(*id));
                    }
                });
            });
    }
}
