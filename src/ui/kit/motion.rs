//! Motion for the redesign: one easing curve, a few lengths, and
//! transitions that something the user did starts. A transition asks for
//! frames only while it runs, so the app is idle again the moment it ends.
//! When less motion is asked for (`crate::motion`), every transition shows
//! where it ends at once.

use egui::{Context, Id};

/// Hover, press and toggles.
pub const FEEDBACK: f32 = 0.15;
/// Menus, popovers and tooltips.
pub const POPOVER: f32 = 0.19;
/// Entrances, song changes and the island growing into now playing.
pub const ENTER: f32 = 0.42;
/// The gap between items that enter one after another.
pub const STAGGER: f32 = 0.04;
/// Items past this many enter together with the last staggered one.
pub const MAX_STAGGERED: usize = 6;

/// Ease-out quint, the one curve every transition uses: fast to start,
/// long to settle. CSS writes it `cubic-bezier(0.22, 1, 0.36, 1)`.
pub fn ease(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(5)
}

/// How long the `index`th item of a group waits before it enters.
pub fn stagger(index: usize) -> f32 {
    index.min(MAX_STAGGERED - 1) as f32 * STAGGER
}

/// A transition with a start time, an optional delay and a length, in
/// egui's clock (`InputState::time`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transition {
    started: f64,
    delay: f32,
    duration: f32,
}

impl Transition {
    /// Starts now, lasting `duration` seconds, or no time at all when less
    /// motion is asked for.
    pub fn start(ctx: &Context, duration: f32) -> Self {
        Self::at(
            ctx.input(|input| input.time),
            crate::motion::seconds(ctx, duration),
        )
    }

    /// Starts at `started` on egui's clock.
    pub fn at(started: f64, duration: f32) -> Self {
        Self {
            started,
            delay: 0.0,
            duration,
        }
    }

    /// The same transition, waiting `delay` seconds before it moves. A
    /// transition that takes no time takes no delay either.
    pub fn delayed(self, delay: f32) -> Self {
        Self {
            delay: if self.duration > 0.0 { delay } else { 0.0 },
            ..self
        }
    }

    /// Eased progress from 0 to 1 at `now`.
    pub fn progress_at(&self, now: f64) -> f32 {
        if self.duration <= 0.0 {
            return 1.0;
        }
        let elapsed = (now - self.started) as f32 - self.delay;
        ease(elapsed / self.duration)
    }

    pub fn finished_at(&self, now: f64) -> bool {
        self.duration <= 0.0 || (now - self.started) as f32 >= self.delay + self.duration
    }

    /// Eased progress now, asking for the next frame while it still moves.
    pub fn progress(&self, ctx: &Context) -> f32 {
        let now = ctx.input(|input| input.time);
        if !self.finished_at(now) {
            ctx.request_repaint();
        }
        self.progress_at(now)
    }
}

/// Starts the transition kept under `id`, replacing one still running.
pub fn begin(ctx: &Context, id: Id, duration: f32) -> Transition {
    let transition = Transition::start(ctx, duration);
    ctx.data_mut(|data| data.insert_temp(id, transition));
    transition
}

/// The progress of the transition kept under `id`, or `None` once it has
/// ended, when it is forgotten so the view draws its resting state.
pub fn running(ctx: &Context, id: Id) -> Option<f32> {
    let transition = ctx.data(|data| data.get_temp::<Transition>(id))?;
    let now = ctx.input(|input| input.time);
    if transition.finished_at(now) {
        ctx.data_mut(|data| data.remove::<Transition>(id));
        return None;
    }
    Some(transition.progress(ctx))
}

/// Eased progress, from 0 to 1, of something drawn for the first time
/// under `id`: it starts the first frame it is asked for, waits `delay`
/// seconds, then takes `duration`. Under less motion it is 1 at once.
pub fn appear(ctx: &Context, id: Id, duration: f32, delay: f32) -> f32 {
    let now = ctx.input(|input| input.time);
    let started = ctx.data_mut(|data| *data.get_temp_mut_or_insert_with(id, || now));
    Transition::at(started, crate::motion::seconds(ctx, duration))
        .delayed(delay)
        .progress(ctx)
}

/// Eased progress, from 0 to 1, of something drawn again after not being
/// drawn (a panel opened, a section shown): it enters over `duration`
/// every time it comes back.
pub fn opening(ctx: &Context, id: Id, duration: f32) -> f32 {
    let frame = ctx.cumulative_frame_nr();
    let last = ctx.data(|data| data.get_temp::<u64>(id.with("drawn")));
    ctx.data_mut(|data| data.insert_temp(id.with("drawn"), frame));
    if last.is_none_or(|last| frame.saturating_sub(last) > 1) {
        begin(ctx, id.with("opening"), duration);
    }
    running(ctx, id.with("opening")).unwrap_or(1.0)
}

/// Where the next shape painted in `layer` will go, to move what is
/// painted after it with [`shift_since`].
pub fn mark(ctx: &Context, layer: egui::LayerId) -> egui::layers::ShapeIdx {
    ctx.graphics(|graphics| {
        graphics
            .get(layer)
            .map_or(egui::layers::ShapeIdx(0), |list| list.next_idx())
    })
}

/// Moves everything painted in `layer` since `start` by `offset`, for
/// something sliding into place. Only what is seen moves, not where it is
/// clicked, which the slide is too short to matter for.
pub fn shift_since(
    ctx: &Context,
    layer: egui::LayerId,
    start: egui::layers::ShapeIdx,
    offset: egui::Vec2,
) {
    if offset == egui::Vec2::ZERO {
        return;
    }
    ctx.graphics_mut(|graphics| {
        let list = graphics.entry(layer);
        let end = list.next_idx();
        list.transform_range(
            start,
            end,
            egui::emath::TSTransform::from_translation(offset),
        );
    });
}

/// Eased progress toward `on`, from 0 (off) to 1 (on), over `duration`.
/// The first frame shows the state as it is, without moving.
pub fn toward(ctx: &Context, id: Id, on: bool, duration: f32) -> f32 {
    ctx.animate_bool_with_time_and_easing(id, on, crate::motion::seconds(ctx, duration), ease)
}

/// An eased number that glides to `target` over `duration` seconds.
pub fn glide(ctx: &Context, id: Id, target: f32, duration: f32) -> f32 {
    ctx.animate_value_with_time(id, target, crate::motion::seconds(ctx, duration))
}

/// How long a changed count takes to roll to its new value.
pub const ROLL: f32 = 0.32;

/// A count that rolls when it changes: the old figure slides out upward
/// (downward when it fell) and the new one slides in behind it. Paints
/// `value` with its left edge at `pos`'s x and its vertical centre at
/// `pos`'s y, and returns the width it took.
pub fn rolling_count(
    ui: &egui::Ui,
    id: Id,
    pos: egui::Pos2,
    value: u64,
    font: egui::FontId,
    color: egui::Color32,
) -> f32 {
    #[derive(Clone, Copy)]
    struct Roll {
        from: u64,
        to: u64,
        transition: Transition,
    }
    let ctx = ui.ctx();
    let roll = ctx.data_mut(|data| {
        let roll = data.get_temp_mut_or_insert_with(id, || Roll {
            from: value,
            to: value,
            transition: Transition::at(f64::NEG_INFINITY, 0.0),
        });
        if roll.to != value {
            *roll = Roll {
                from: roll.to,
                to: value,
                transition: Transition::start(ctx, ROLL),
            };
        }
        *roll
    });
    let painter = ui.painter();
    let galley = painter.layout_no_wrap(value.to_string(), font.clone(), color);
    let height = galley.size().y;
    let t = roll.transition.progress(ctx);
    let width = galley.size().x;
    if t >= 1.0 || roll.from == roll.to {
        painter.galley(pos - egui::vec2(0.0, height / 2.0), galley, color);
        return width;
    }
    let up = if roll.to > roll.from { 1.0 } else { -1.0 };
    let distance = height * 0.7;
    let clip = egui::Rect::from_min_size(
        pos - egui::vec2(0.0, height / 2.0 + 1.0),
        egui::vec2(width.max(24.0) + 8.0, height + 2.0),
    );
    let painter = painter.with_clip_rect(clip.intersect(ui.clip_rect()));
    let old = painter.layout_no_wrap(roll.from.to_string(), font, color.gamma_multiply(1.0 - t));
    painter.galley(
        pos - egui::vec2(0.0, height / 2.0 + up * distance * t),
        old,
        color,
    );
    painter.galley(
        pos - egui::vec2(0.0, height / 2.0 - up * distance * (1.0 - t)),
        galley.clone(),
        color.gamma_multiply(t),
    );
    width
}

/// How long a sent thing takes to fly off.
pub const FLY: f32 = 0.5;

/// Where a "sent" icon is at progress `t`: it leaves up and to the right,
/// shrinking and fading, then comes back in from below. Returns the
/// offset, the scale and the opacity.
pub fn fly_off(t: f32) -> (egui::Vec2, f32, f32) {
    let t = t.clamp(0.0, 1.0);
    if t < 0.55 {
        let out = ease(t / 0.55);
        (
            egui::vec2(18.0 * out, -18.0 * out),
            1.0 - 0.4 * out,
            1.0 - out,
        )
    } else {
        let back = ease((t - 0.55) / 0.45);
        (
            egui::vec2(0.0, 8.0 * (1.0 - back)),
            0.85 + 0.15 * back,
            back,
        )
    }
}

/// How long a refused press nudges.
pub const NUDGE: f32 = 0.24;

/// The sideways offset of a control nudging at progress `t`: out 4 points
/// in `direction` and back, once.
pub fn nudge(t: f32, direction: f32) -> f32 {
    4.0 * direction * (t.clamp(0.0, 1.0) * std::f32::consts::PI).sin()
}

/// How long a pressed control takes to settle back.
pub const PRESS: f32 = 0.12;

/// The scale of a control while pressed: 0.97, easing back on release.
pub fn press_scale(ctx: &Context, response: &egui::Response) -> f32 {
    1.0 - 0.03
        * toward(
            ctx,
            response.id.with("press"),
            response.is_pointer_button_down_on(),
            PRESS,
        )
}

/// Six dots bursting out of `center` as something is given, at the pop's
/// progress `t`: they fly out and shrink away. Nothing is drawn at rest.
pub fn burst(painter: &egui::Painter, center: egui::Pos2, t: f32, color: egui::Color32) {
    if t <= 0.0 || t >= 1.0 {
        return;
    }
    let spread = ease(t);
    for dot in 0..6 {
        let angle = dot as f32 / 6.0 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let at = center + egui::Vec2::angled(angle) * (8.0 + 10.0 * spread);
        painter.circle_filled(at, 1.6 * (1.0 - spread), color);
    }
}

/// How long a heart pops when it is given.
pub const POP: f32 = 0.3;

/// The scale of a heart popping at progress `t`: it swells by up to 30%
/// and settles back to its size.
pub fn pop(t: f32) -> f32 {
    1.0 + (t.clamp(0.0, 1.0) * std::f32::consts::PI).sin() * 0.3
}

/// How long the light takes to cross a row that started playing.
pub const SWEEP: f32 = 0.55;

/// How long a refused action shakes.
pub const REFUSE: f32 = 0.36;

/// The sideways offset, in points, of something refusing at progress `t`:
/// three swings that die away, never more than 4 points.
pub fn refuse(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    4.0 * (1.0 - t) * (t * std::f32::consts::PI * 6.0).sin()
}

/// How a song change moves on the player island.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Swipe {
    /// A song played from a list: the old song and its queue leave to the
    /// left, the new ones arrive from the right.
    Replace,
    /// The first queue tile grows into the cover; the old cover leaves left.
    Next,
    /// The cover shrinks back into the first tile; the previous song
    /// arrives from the left.
    Previous,
}

/// Where a song's cover and text sit during a swipe, relative to their
/// resting place: an offset, a scale about the cover's left edge, and an
/// opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub dx: f32,
    pub scale: f32,
    pub alpha: f32,
}

impl Placement {
    pub const REST: Self = Self {
        dx: 0.0,
        scale: 1.0,
        alpha: 1.0,
    };

    fn lerp(from: Self, to: Self, t: f32) -> Self {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        Self {
            dx: mix(from.dx, to.dx),
            scale: mix(from.scale, to.scale),
            alpha: mix(from.alpha, to.alpha),
        }
    }
}

/// How far a song slides when it arrives or leaves.
const ARRIVE: f32 = 44.0;
const LEAVE: f32 = 56.0;
/// A queue tile's size against the island's cover.
const TILE_SCALE: f32 = 0.56;

impl Swipe {
    /// The incoming song at eased progress `t`. `tile_dx` is how far the
    /// first queue tile sits right of the cover.
    pub fn incoming(self, t: f32, tile_dx: f32) -> Placement {
        let from = match self {
            Self::Replace => Placement {
                dx: ARRIVE,
                scale: 1.0,
                alpha: 0.0,
            },
            Self::Next => Placement {
                dx: tile_dx,
                scale: TILE_SCALE,
                alpha: 0.5,
            },
            Self::Previous => Placement {
                dx: -ARRIVE,
                scale: 1.0,
                alpha: 0.0,
            },
        };
        Placement::lerp(from, Placement::REST, t)
    }

    /// The outgoing song at eased progress `t`. It is gone a little before
    /// the incoming one settles, so the two never read as one cover.
    pub fn outgoing(self, t: f32, tile_dx: f32) -> Placement {
        let to = match self {
            Self::Replace | Self::Next => Placement {
                dx: -LEAVE,
                scale: 1.0,
                alpha: 0.0,
            },
            Self::Previous => Placement {
                dx: tile_dx,
                scale: TILE_SCALE,
                alpha: 0.0,
            },
        };
        Placement::lerp(Placement::REST, to, (t * 1.25).min(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pop_swells_and_settles() {
        assert_eq!(pop(0.0), 1.0);
        assert!((pop(1.0) - 1.0).abs() < 1e-5);
        assert!((pop(0.5) - 1.3).abs() < 1e-5);
    }

    /// A refusal swings both ways, stays within 4 points and comes to rest
    /// where it started.
    #[test]
    fn a_refusal_shakes_and_settles() {
        let samples: Vec<f32> = (0..=100).map(|i| refuse(i as f32 / 100.0)).collect();
        assert_eq!(refuse(0.0), 0.0);
        assert!(refuse(1.0).abs() < 1e-4);
        assert!(samples.iter().all(|dx| dx.abs() <= 4.0));
        assert!(samples.iter().any(|dx| *dx > 1.0));
        assert!(samples.iter().any(|dx| *dx < -1.0));
    }

    #[test]
    fn the_curve_starts_fast_and_settles() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        assert!(ease(0.2) > 0.6, "most of the way in the first fifth");
        let mut last = 0.0;
        for step in 1..=20 {
            let value = ease(step as f32 / 20.0);
            assert!(value >= last);
            last = value;
        }
        assert_eq!(ease(-1.0), 0.0);
        assert_eq!(ease(2.0), 1.0);
    }

    #[test]
    fn staggering_stops_after_six_items() {
        assert_eq!(stagger(0), 0.0);
        assert_eq!(stagger(1), STAGGER);
        assert_eq!(stagger(5), stagger(40));
    }

    #[test]
    fn a_transition_waits_out_its_delay_then_ends() {
        let transition = Transition::at(10.0, 0.4).delayed(0.1);
        assert_eq!(transition.progress_at(10.05), 0.0);
        assert!(transition.progress_at(10.3) > 0.5);
        assert!(!transition.finished_at(10.49));
        assert!(transition.finished_at(10.5));
        assert_eq!(transition.progress_at(11.0), 1.0);
    }

    /// With less motion a transition takes no time, ignores its delay, and
    /// never asks for another frame.
    #[test]
    fn less_motion_shows_the_end_at_once() {
        let ctx = egui::Context::default();
        crate::motion::apply(&ctx, true);
        let transition = Transition::start(&ctx, ENTER).delayed(0.2);
        assert!(transition.finished_at(0.0));
        assert_eq!(transition.progress_at(0.0), 1.0);
    }

    #[test]
    fn a_kept_transition_is_forgotten_when_it_ends() {
        let ctx = egui::Context::default();
        let id = Id::new("kit-test");
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            begin(ui.ctx(), id, 0.0);
            assert_eq!(running(ui.ctx(), id), None);
        });
        output.textures_delta.clear();
        assert!(ctx.data(|data| data.get_temp::<Transition>(id)).is_none());
    }

    #[test]
    fn every_swipe_starts_where_the_song_was_and_ends_at_rest() {
        for swipe in [Swipe::Replace, Swipe::Next, Swipe::Previous] {
            assert_eq!(swipe.incoming(1.0, 290.0), Placement::REST);
            assert_eq!(swipe.outgoing(0.0, 290.0), Placement::REST);
            assert_eq!(swipe.outgoing(1.0, 290.0).alpha, 0.0);
            assert!(swipe.incoming(0.0, 290.0).alpha < 1.0);
        }
    }

    /// Next grows the first queue tile into the cover; Previous shrinks
    /// the cover back into it, so the two are mirror images.
    #[test]
    fn next_and_previous_travel_through_the_first_tile() {
        let next = Swipe::Next.incoming(0.0, 290.0);
        assert_eq!((next.dx, next.scale), (290.0, TILE_SCALE));
        let back = Swipe::Previous.outgoing(1.0, 290.0);
        assert_eq!((back.dx, back.scale), (290.0, TILE_SCALE));
        assert!(Swipe::Replace.outgoing(1.0, 290.0).dx < 0.0);
        assert!(Swipe::Previous.incoming(0.0, 290.0).dx < 0.0);
    }
}
