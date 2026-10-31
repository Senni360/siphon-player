//! A cover flying to where a song went: added to a playlist, it arcs to
//! that playlist's row in the sidebar; shared, to the Group row. The row
//! it lands on pulses once. It shows where the song went without a word.
//!
//! Places a flight can land register their rect each frame they are drawn
//! ([`target`]). A flight whose place is not on screen lands at the left
//! edge, where the hidden sidebar's handle is.

use egui::{Context, Id, Pos2, Rect, pos2, vec2};

use crate::theme::{Icon, Palette};

/// How long a cover takes to fly.
pub const FLIGHT: f32 = 0.62;
/// How long the row it lands on pulses.
pub const LANDING: f32 = 0.45;

/// The key flights to the Group row use.
pub const GROUP: &str = "group";

#[derive(Clone, Debug)]
struct Flight {
    art: Option<String>,
    from: Pos2,
    target: String,
    started: f64,
}

fn flights_id() -> Id {
    Id::new("kit-flights")
}

fn target_id(key: &str) -> Id {
    Id::new(("kit-flight-target", key))
}

fn landing_id(key: &str) -> Id {
    Id::new(("kit-flight-landing", key))
}

/// Registers where `key` (a playlist's URI or id, or [`GROUP`]) is drawn
/// this frame.
pub fn target(ctx: &Context, key: &str, rect: Rect) {
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|data| data.insert_temp(target_id(key), (rect, frame)));
}

/// Sends a cover from `from` to `key`. Under less motion nothing flies;
/// the row still pulses.
pub fn launch(ctx: &Context, art: Option<String>, from: Pos2, key: &str) {
    if crate::motion::reduced(ctx) {
        super::motion::begin(ctx, landing_id(key), LANDING);
        return;
    }
    let started = ctx.input(|input| input.time);
    ctx.data_mut(|data| {
        let flights = data.get_temp_mut_or_default::<Vec<Flight>>(flights_id());
        flights.push(Flight {
            art,
            from,
            target: key.to_owned(),
            started,
        });
        // A burst of adds keeps only the newest few in the air.
        let excess = flights.len().saturating_sub(4);
        flights.drain(..excess);
    });
}

/// The progress of `key`'s landing pulse, while it runs.
pub fn landing(ctx: &Context, key: &str) -> Option<f32> {
    super::motion::running(ctx, landing_id(key))
}

/// Paints the pulse of a flight landing on `rect`: a ring that swells out
/// of the row and fades.
pub fn paint_landing(ui: &egui::Ui, palette: &Palette, key: &str, rect: Rect, radius: f32) {
    if let Some(t) = landing(ui.ctx(), key) {
        let grow = super::motion::ease(t);
        ui.painter().rect_stroke(
            rect.expand(4.0 * grow),
            radius + 4.0 * grow,
            egui::Stroke::new(2.0 * (1.0 - grow), palette.text.gamma_multiply(1.0 - grow)),
            egui::StrokeKind::Outside,
        );
        ui.painter().rect_filled(
            rect,
            radius,
            palette.text.gamma_multiply(0.08 * (1.0 - grow)),
        );
    }
}

/// Where a flight to `key` lands now.
fn destination(ctx: &Context, key: &str) -> Pos2 {
    let frame = ctx.cumulative_frame_nr();
    ctx.data(|data| data.get_temp::<(Rect, u64)>(target_id(key)))
        .filter(|(_, seen)| frame.saturating_sub(*seen) <= 1)
        .map(|(rect, _)| rect.left_center() + vec2(28.0, 0.0))
        .unwrap_or_else(|| {
            let screen = ctx.content_rect();
            pos2(screen.left() + 12.0, screen.center().y)
        })
}

/// The point on the arc from `from` to `to` at eased progress `t`: it
/// lifts above the straight line, like something tossed.
fn arc(from: Pos2, to: Pos2, t: f32) -> Pos2 {
    let lift = ((to - from).length() * 0.35).clamp(60.0, 220.0);
    let control = pos2((from.x + to.x) / 2.0, from.y.min(to.y) - lift);
    let a = from.lerp(control, t);
    let b = control.lerp(to, t);
    a.lerp(b, t)
}

/// Draws every cover in the air, above everything, and starts the landing
/// pulse of each that arrives.
pub fn paint(ctx: &Context, palette: &Palette) {
    let flights = ctx.data(|data| data.get_temp::<Vec<Flight>>(flights_id()));
    let Some(flights) = flights.filter(|flights| !flights.is_empty()) else {
        return;
    };
    let now = ctx.input(|input| input.time);
    let mut flying = Vec::new();
    for (index, flight) in flights.into_iter().enumerate() {
        let raw = ((now - flight.started) as f32 / FLIGHT).clamp(0.0, 1.0);
        if raw >= 1.0 {
            super::motion::begin(ctx, landing_id(&flight.target), LANDING);
            continue;
        }
        // Eased in and out, so it leaves and arrives gently.
        let t = raw * raw * (3.0 - 2.0 * raw);
        let to = destination(ctx, &flight.target);
        let at = arc(flight.from, to, t);
        let side = 44.0 - 22.0 * t;
        let alpha = if raw > 0.85 { (1.0 - raw) / 0.15 } else { 1.0 };
        let rect = Rect::from_center_size(at, vec2(side, side));
        egui::Area::new(Id::new(("kit-flight", index)))
            .order(egui::Order::Tooltip)
            .interactable(false)
            .fixed_pos(rect.min)
            .show(ctx, |ui| {
                ui.set_opacity(alpha);
                let (slot, _) = ui.allocate_exact_size(rect.size(), egui::Sense::hover());
                super::super::widgets::paint_shadow(ui, palette, slot, 6.0);
                super::super::widgets::paint_cover(
                    ui,
                    palette,
                    flight.art.as_deref(),
                    slot,
                    6.0,
                    Icon::Music,
                    None,
                );
            });
        flying.push(flight);
    }
    if !flying.is_empty() {
        ctx.request_repaint();
    }
    ctx.data_mut(|data| data.insert_temp(flights_id(), flying));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arc_starts_and_ends_where_asked_and_lifts_between() {
        let from = pos2(800.0, 600.0);
        let to = pos2(100.0, 300.0);
        assert!((arc(from, to, 0.0) - from).length() < 0.01);
        assert!((arc(from, to, 1.0) - to).length() < 0.01);
        assert!(
            arc(from, to, 0.5).y < to.y,
            "the middle rises above both ends"
        );
    }

    #[test]
    fn with_less_motion_nothing_flies_but_the_row_still_answers() {
        let ctx = Context::default();
        crate::motion::apply(&ctx, true);
        launch(&ctx, None, pos2(10.0, 10.0), "spotify:playlist:x");
        assert!(
            ctx.data(|data| data.get_temp::<Vec<Flight>>(flights_id()))
                .is_none_or(|flights| flights.is_empty())
        );
    }
}
