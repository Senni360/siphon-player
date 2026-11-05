//! One cover, two places: a card or a library row pressed to open a page hands
//! its cover over, and the page's big cover grows out of it. The eye follows the
//! thing it pressed across the change of page.

use egui::{Context, Id, Rect};

use super::motion;
use crate::theme::{Icon, Palette};

/// How long a pressed cover waits for its page's big cover to ask for it.
const FRESH: f64 = 0.4;
const FLIGHT: f32 = 0.46;

#[derive(Clone)]
struct Launch {
    art: Option<String>,
    from: Rect,
    radius: f32,
    at: f64,
}

#[derive(Clone)]
struct Landing {
    launch: Launch,
    transition: motion::Transition,
}

fn launch_id() -> Id {
    Id::new("kit-shared-launch")
}

fn landing_id() -> Id {
    Id::new("kit-shared-landing")
}

/// Hands over the cover at `from`, for the page being opened.
pub fn launch(ctx: &Context, art: Option<&str>, from: Rect, radius: f32) {
    let at = ctx.input(|input| input.time);
    let launch = Launch {
        art: art.map(str::to_string),
        from,
        radius,
        at,
    };
    ctx.data_mut(|data| data.insert_temp(launch_id(), launch));
}

/// Where a cover is `t` of the way from `from` to `to`.
fn between(from: Rect, to: Rect, t: f32) -> Rect {
    Rect::from_min_max(
        from.min + (to.min - from.min) * t,
        from.max + (to.max - from.max) * t,
    )
}

/// Called by a page's big cover at `to` before it paints. While a handed-over
/// cover is growing into place it is painted over the page, and this returns
/// true so that the big cover waits underneath.
pub fn land(ui: &egui::Ui, palette: &Palette, to: Rect, radius: f32) -> bool {
    let ctx = ui.ctx();
    let now = ctx.input(|input| input.time);
    let launched = ctx.data(|data| data.get_temp::<Launch>(launch_id()));
    if launched.is_some() {
        ctx.data_mut(|data| data.remove::<Launch>(launch_id()));
    }
    if let Some(launch) = launched.filter(|launch| now - launch.at < FRESH) {
        let landing = Landing {
            launch,
            transition: motion::Transition::start(ctx, FLIGHT),
        };
        ctx.data_mut(|data| data.insert_temp(landing_id(), landing));
    }
    let Some(landing) = ctx.data(|data| data.get_temp::<Landing>(landing_id())) else {
        return false;
    };
    if landing.transition.finished_at(now) {
        ctx.data_mut(|data| data.remove::<Landing>(landing_id()));
        return false;
    }
    let t = landing.transition.progress(ctx);
    let rect = between(landing.launch.from, to, t);
    let corner = landing.launch.radius + (radius - landing.launch.radius) * t;
    egui::Area::new(Id::new("kit-shared-cover"))
        .order(egui::Order::Foreground)
        .interactable(false)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            let (slot, _) = ui.allocate_exact_size(rect.size(), egui::Sense::hover());
            crate::ui::widgets::paint_shadow(ui, palette, slot, corner);
            crate::ui::widgets::paint_cover(
                ui,
                palette,
                landing.launch.art.as_deref(),
                slot,
                corner,
                Icon::Music,
                None,
            );
        });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cover_travels_from_where_it_was_pressed_to_its_place() {
        let from = Rect::from_min_size(egui::pos2(10.0, 400.0), egui::vec2(40.0, 40.0));
        let to = Rect::from_min_size(egui::pos2(300.0, 80.0), egui::vec2(220.0, 220.0));
        assert_eq!(between(from, to, 0.0), from);
        assert_eq!(between(from, to, 1.0), to);
        let middle = between(from, to, 0.5);
        assert!(middle.width() > from.width() && middle.width() < to.width());
    }
}
